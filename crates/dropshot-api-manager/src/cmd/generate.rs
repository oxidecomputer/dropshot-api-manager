// Copyright 2026 Oxide Computer Company

use crate::{
    FAILURE_EXIT_CODE,
    apis::ManagedApis,
    environment::{BlessedSource, GeneratedSource, ResolvedEnv},
    output::{
        CheckResult, OutputOpts, Styles, display_api_spec_version,
        display_load_problems, display_non_version_problems,
        display_resolution, display_version_problems,
        headers::{self, *},
        plural,
    },
    resolved::{Fix, NonVersionProblem, Resolved, VersionProblem},
};
use anyhow::{Result, anyhow, bail};
use owo_colors::OwoColorize;
use std::{io, process::ExitCode};

#[derive(Clone, Copy, Debug)]
pub(crate) enum GenerateResult {
    Success,
    Failures,
}

impl GenerateResult {
    pub(crate) fn to_exit_code(self) -> ExitCode {
        match self {
            GenerateResult::Success => ExitCode::SUCCESS,
            GenerateResult::Failures => FAILURE_EXIT_CODE.into(),
        }
    }
}

pub(crate) fn generate_impl(
    apis: &ManagedApis,
    env: &ResolvedEnv,
    blessed_source: &BlessedSource,
    generated_source: &GeneratedSource,
    output: &OutputOpts,
) -> Result<GenerateResult> {
    let styles = output.styles(supports_color::Stream::Stderr);
    let mut stderr = std::io::stderr().lock();
    generate_impl_inner(
        &mut stderr,
        apis,
        env,
        blessed_source,
        generated_source,
        &styles,
    )
}

fn generate_impl_inner(
    writer: &mut dyn io::Write,
    apis: &ManagedApis,
    env: &ResolvedEnv,
    blessed_source: &BlessedSource,
    generated_source: &GeneratedSource,
    styles: &Styles,
) -> Result<GenerateResult> {
    let (generated, errors) = generated_source.load(
        writer,
        apis,
        styles,
        &env.repo_root,
        &env.vcs,
    )?;
    display_load_problems(writer, &errors, styles)?;

    let (local_files, errors) = env.local_source.load(
        writer,
        apis,
        styles,
        &env.repo_root,
        &env.vcs,
    )?;
    display_load_problems(writer, &errors, styles)?;

    let (blessed, errors) =
        blessed_source.load(writer, &env.repo_root, apis, styles, &env.vcs)?;
    display_load_problems(writer, &errors, styles)?;

    let resolved = Resolved::new(env, apis, &blessed, &generated, &local_files);
    writeln!(writer, "{:>HEADER_WIDTH$}", SEPARATOR)?;

    let total = resolved.nexpected_documents();
    writeln!(
        writer,
        "{:>HEADER_WIDTH$} {} OpenAPI {}...",
        "Updating".style(styles.success_header),
        total.style(styles.bold),
        plural::documents(total),
    )?;

    if resolved.has_unfixable_problems() {
        return match display_resolution(writer, env, apis, &resolved, styles)? {
            CheckResult::Failures => Ok(GenerateResult::Failures),
            unexpected => {
                Err(anyhow!("unexpectedly got {unexpected:?} from summarize()"))
            }
        };
    }

    let mut num_updated = 0;
    let mut num_unchanged = 0;
    let mut num_errors = 0;

    // Apply fixes for problems with supported API versions.
    for api in apis.iter_apis() {
        let ident = api.ident();
        for version in api.iter_versions_semver() {
            // unwrap(): there should be a resolution for every managed API
            let resolution =
                resolved.resolution_for_api_version(ident, version).unwrap();
            assert!(
                !resolution.has_errors(),
                "found unfixable problems, but that should have been \
                 checked above"
            );

            let problems: Vec<_> = resolution.problems().collect();
            if problems.is_empty() {
                writeln!(
                    writer,
                    "{:>HEADER_WIDTH$} {}",
                    UNCHANGED.style(styles.unchanged_header),
                    display_api_spec_version(api, version, styles, resolution),
                )?;
                num_unchanged += 1;
            } else {
                writeln!(
                    writer,
                    "{:>HEADER_WIDTH$} {}",
                    STALE.style(styles.warning_header),
                    display_api_spec_version(api, version, styles, resolution),
                )?;

                apply_fixes(
                    writer,
                    env,
                    problems.into_iter().map(expect_version_fix),
                    styles,
                    &mut num_updated,
                    &mut num_errors,
                )?;
            }
        }

        if let Some(symlink_problem) = resolved.symlink_problem(ident) {
            writeln!(
                writer,
                "{:>HEADER_WIDTH$} {} \"latest\" symlink",
                STALE.style(styles.warning_header),
                ident.style(styles.filename),
            )?;

            apply_fixes(
                writer,
                env,
                std::iter::once(expect_non_version_fix(symlink_problem)),
                styles,
                &mut num_updated,
                &mut num_errors,
            )?;
        } else if api.is_versioned() {
            writeln!(
                writer,
                "{:>HEADER_WIDTH$} {} \"latest\" symlink",
                UNCHANGED.style(styles.unchanged_header),
                ident.style(styles.filename),
            )?;
        }
    }

    // Fix problems not associated with any supported version, if any.
    apply_fixes(
        writer,
        env,
        resolved.non_version_problems().map(expect_non_version_fix),
        styles,
        &mut num_updated,
        &mut num_errors,
    )?;

    // Done with the first resolution. Release borrows so the source
    // collections can be dropped in parallel later.
    drop(resolved);

    if num_errors > 0 {
        print_final_status(
            writer,
            styles,
            total,
            num_updated,
            num_unchanged,
            num_errors,
        )?;
        return Ok(GenerateResult::Failures);
    }

    // Finally, check again for any problems. Since we expect this should have
    // fixed everything, be quiet unless we find something amiss.
    let mut nproblems = 0;
    let (local_files_recheck, errors) = env.local_source.load(
        writer,
        apis,
        styles,
        &env.repo_root,
        &env.vcs,
    )?;
    writeln!(
        writer,
        "{:>HEADER_WIDTH$} all local files",
        "Rechecking".style(styles.success_header),
    )?;
    display_load_problems(writer, &errors, styles)?;
    let resolved =
        Resolved::new(env, apis, &blessed, &generated, &local_files_recheck);
    let non_version_problems: Vec<_> =
        resolved.non_version_problems().collect();
    nproblems += non_version_problems.len();
    if !non_version_problems.is_empty() {
        display_non_version_problems(writer, non_version_problems, styles)?;
    }
    for api in apis.iter_apis() {
        let ident = api.ident();
        for version in api.iter_versions_semver() {
            // unwrap(): there should be a resolution for every managed API
            let resolution =
                resolved.resolution_for_api_version(ident, version).unwrap();
            let problems: Vec<_> = resolution.problems().collect();
            nproblems += problems.len();
            if !problems.is_empty() {
                writeln!(
                    writer,
                    "found unexpected problem with API {} version {} \
                     (this is a bug)",
                    ident, version
                )?;
                display_version_problems(writer, env, problems, styles)?;
            }
        }

        if let Some(symlink_problem) = resolved.symlink_problem(ident) {
            nproblems += 1;
            writeln!(
                writer,
                "found unexpected problem with API {} symlink (this is a bug)",
                ident
            )?;
            display_non_version_problems(
                writer,
                std::iter::once(symlink_problem),
                styles,
            )?;
        }
    }

    // Release borrows held by `resolved`, then drop all source
    // collections in parallel. Each contains many parsed OpenAPI
    // documents whose sequential drops are costly.
    drop(resolved);
    std::thread::scope(|s| {
        s.spawn(|| drop(blessed));
        s.spawn(|| drop(generated));
        s.spawn(|| drop(local_files));
        s.spawn(|| drop(local_files_recheck));
    });

    if nproblems > 0 {
        bail!(
            "ERROR: found problems after successfully fixing everything \
             (this is a BUG!)"
        );
    } else {
        print_final_status(
            writer,
            styles,
            total,
            num_updated,
            num_unchanged,
            num_errors,
        )?;
        Ok(GenerateResult::Success)
    }
}

fn print_final_status(
    writer: &mut dyn io::Write,
    styles: &Styles,
    ndocuments: usize,
    num_updated: usize,
    num_unchanged: usize,
    num_errors: usize,
) -> io::Result<()> {
    writeln!(writer, "{:>HEADER_WIDTH$}", SEPARATOR)?;
    let status_header = if num_errors == 0 {
        headers::SUCCESS.style(styles.success_header)
    } else {
        headers::FAILURE.style(styles.failure_header)
    };
    writeln!(
        writer,
        "{:>HEADER_WIDTH$} {} {}: {} {} made, {} unchanged, {} failed",
        status_header,
        ndocuments.style(styles.bold),
        plural::documents(ndocuments),
        num_updated.style(styles.bold),
        plural::changes(num_updated),
        num_unchanged.style(styles.bold),
        num_errors.style(styles.bold),
    )?;
    Ok(())
}

fn expect_version_fix<'a>(p: &'a VersionProblem<'a>) -> Fix<'a> {
    p.fix().expect("attempting to fix unfixable problem")
}

fn expect_non_version_fix<'a>(p: &'a NonVersionProblem<'a>) -> Fix<'a> {
    p.fix().expect("attempting to fix unfixable problem")
}

fn apply_fixes<'a>(
    writer: &mut dyn io::Write,
    env: &ResolvedEnv,
    fixes: impl IntoIterator<Item = Fix<'a>>,
    styles: &Styles,
    num_updated: &mut usize,
    num_errors: &mut usize,
) -> io::Result<()> {
    for fix in fixes {
        match fix.execute(env) {
            Ok(steps) => {
                *num_updated += 1;
                for s in steps {
                    writeln!(
                        writer,
                        "{:>HEADER_WIDTH$} {}",
                        "Fixed".style(styles.success_header),
                        s,
                    )?;
                }
            }
            Err(error) => {
                *num_errors += 1;
                writeln!(
                    writer,
                    "{:>HEADER_WIDTH$} fix {:?}: {:#}",
                    "FIX FAILED".style(styles.failure_header),
                    fix.to_string(),
                    error
                )?;
            }
        }
    }
    Ok(())
}
