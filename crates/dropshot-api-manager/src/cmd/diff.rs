// Copyright 2025 Oxide Computer Company

use crate::{
    apis::ManagedApis,
    environment::{BlessedSource, ResolvedEnv},
    output::{OutputOpts, Styles, display_load_problems, write_diff},
    spec_files_blessed::BlessedApiSpecFile,
    spec_files_generic::ApiFiles,
    spec_files_local::LocalApiSpecFile,
};
use anyhow::Context;
use camino::Utf8Path;
use dropshot_api_manager_types::ApiIdent;
use owo_colors::OwoColorize;
use similar::TextDiff;
use std::process::ExitCode;

/// Compare local OpenAPI documents against blessed (upstream) versions.
///
/// For each API with differences, shows the diff between what's on disk locally
/// and what's blessed in the upstream branch (typically origin/main).
pub(crate) fn diff_impl(
    apis: &ManagedApis,
    env: &ResolvedEnv,
    blessed_source: &BlessedSource,
    output: &OutputOpts,
) -> anyhow::Result<ExitCode> {
    let mut styles = Styles::default();
    if output.use_color(supports_color::Stream::Stdout) {
        styles.colorize();
    }

    let (local_files, errors) = env.local_source.load(apis, &styles)?;
    display_load_problems(&errors, &styles)?;

    let (blessed_files, errors) =
        blessed_source.load(&env.repo_root, apis, &styles)?;
    display_load_problems(&errors, &styles)?;

    let mut any_diff = false;

    for api in apis.iter_apis() {
        let ident = api.ident();
        let local_api = local_files.get(ident);
        let blessed_api = blessed_files.get(ident);

        let has_diff = diff_api(ident, local_api, blessed_api, &styles)?;
        any_diff = any_diff || has_diff;
    }

    if !any_diff {
        eprintln!("No differences from blessed.");
    }

    Ok(ExitCode::SUCCESS)
}

fn diff_api(
    ident: &ApiIdent,
    local_api: Option<&ApiFiles<Vec<LocalApiSpecFile>>>,
    blessed_api: Option<&ApiFiles<BlessedApiSpecFile>>,
    styles: &Styles,
) -> anyhow::Result<bool> {
    // Collect all versions from both sources
    let mut all_versions: Vec<semver::Version> = Vec::new();
    if let Some(local) = local_api {
        all_versions.extend(local.versions().keys().cloned());
    }
    if let Some(blessed) = blessed_api {
        for v in blessed.versions().keys() {
            if !all_versions.contains(v) {
                all_versions.push(v.clone());
            }
        }
    }
    all_versions.sort();

    if all_versions.is_empty() {
        return Ok(false);
    }

    let mut has_diff = false;

    for version in &all_versions {
        let local_file = local_api
            .and_then(|a| a.versions().get(version))
            .and_then(|files| files.first());
        let blessed_file = blessed_api.and_then(|a| a.versions().get(version));

        match (blessed_file, local_file) {
            (None, Some(local)) => {
                // New version added locally. Diff against the previous blessed
                // version to show what actually changed in the schema.
                let prev_blessed = blessed_api.and_then(|api| {
                    api.versions()
                        .iter()
                        .filter(|(v, _)| *v < version)
                        .max_by_key(|(v, _)| *v)
                        .map(|(_, file)| file)
                });

                let local_content = std::str::from_utf8(local.contents())
                    .context("local file is not valid UTF-8")?;
                let local_path = local.spec_file_name().path();

                if let Some(prev) = prev_blessed {
                    let base_content = std::str::from_utf8(prev.contents())
                        .context("blessed file is not valid UTF-8")?;
                    let base_path = prev.spec_file_name().path();

                    // Skip if no actual diff (shouldn't happen, but be safe).
                    if base_content == local_content {
                        continue;
                    }

                    eprintln!(
                        "\n{} v{}: {} (new locally)",
                        ident.style(styles.filename),
                        version,
                        "added".style(styles.success_header),
                    );
                    let diff =
                        TextDiff::from_lines(base_content, local_content);
                    write_diff(
                        &diff,
                        base_path.as_ref(),  // old path
                        local_path.as_ref(), // new path
                        styles,
                        3,    // context lines
                        true, // show missing newline hint
                        &mut std::io::stdout(),
                    )?;
                } else {
                    // No previous version to compare against - show full file.
                    eprintln!(
                        "\n{} v{}: {} (new locally)",
                        ident.style(styles.filename),
                        version,
                        "added".style(styles.success_header),
                    );
                    let diff = TextDiff::from_lines("", local_content);
                    write_diff(
                        &diff,
                        Utf8Path::new("/dev/null"), // old path
                        local_path.as_ref(),        // new path
                        styles,
                        3,    // context lines
                        true, // show missing newline hint
                        &mut std::io::stdout(),
                    )?;
                }
                has_diff = true;
            }
            (Some(blessed), None) => {
                // Version removed locally
                eprintln!(
                    "\n{} v{}: {} (removed locally)",
                    ident.style(styles.filename),
                    version,
                    "removed".style(styles.failure_header),
                );
                let blessed_content =
                    std::str::from_utf8(blessed.contents())
                        .context("blessed file is not valid UTF-8")?;
                let diff = TextDiff::from_lines(blessed_content, "");
                let blessed_path = blessed.spec_file_name().path();
                write_diff(
                    &diff,
                    blessed_path.as_ref(),      // old path
                    Utf8Path::new("/dev/null"), // new path
                    styles,
                    3,    // context lines
                    true, // show missing newline hint
                    &mut std::io::stdout(),
                )?;
                has_diff = true;
            }
            (Some(blessed), Some(local)) => {
                let blessed_content =
                    std::str::from_utf8(blessed.contents())
                        .context("blessed file is not valid UTF-8")?;
                let local_content = std::str::from_utf8(local.contents())
                    .context("local file is not valid UTF-8")?;

                if blessed_content != local_content {
                    eprintln!(
                        "\n{} v{}: {}",
                        ident.style(styles.filename),
                        version,
                        "modified".style(styles.warning_header),
                    );
                    let diff =
                        TextDiff::from_lines(blessed_content, local_content);
                    let blessed_path = blessed.spec_file_name().path();
                    let local_path = local.spec_file_name().path();
                    write_diff(
                        &diff,
                        blessed_path.as_ref(), // old path
                        local_path.as_ref(),   // new path
                        styles,
                        3,    // context lines
                        true, // show missing newline hint
                        &mut std::io::stdout(),
                    )?;
                    has_diff = true;
                }
            }
            (None, None) => {
                // Shouldn't happen since we collected versions from both
                unreachable!()
            }
        }
    }

    Ok(has_diff)
}
