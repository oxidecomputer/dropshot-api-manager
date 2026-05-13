// Copyright 2026 Oxide Computer Company

use crate::{
    apis::ManagedApis,
    environment::{BlessedSource, GeneratedSource, ResolvedEnv},
    output::{
        CheckResult, Styles, display_load_problems, display_resolution,
        headers::*,
    },
    resolved::{ProblemSummary, Resolved},
};
use std::io;

pub(crate) fn check_impl(
    writer: &mut dyn io::Write,
    apis: &ManagedApis,
    env: &ResolvedEnv,
    blessed_source: &BlessedSource,
    generated_source: &GeneratedSource,
    styles: &Styles,
) -> anyhow::Result<CheckResult> {
    let (result, _summaries) = check_impl_with_summaries(
        writer,
        apis,
        env,
        blessed_source,
        generated_source,
        styles,
    )?;
    Ok(result)
}

/// Run the check pipeline, rendering all user-visible output to `writer`.
///
/// Production callers (the CLI dispatch path) pass `&mut
/// std::io::stderr().lock()` along with [`OutputOpts::styles`]; tests pass a
/// `Vec<u8>` and `Styles::default()` to capture the same output into a
/// `String`.
pub(crate) fn check_impl_with_summaries(
    writer: &mut dyn io::Write,
    apis: &ManagedApis,
    env: &ResolvedEnv,
    blessed_source: &BlessedSource,
    generated_source: &GeneratedSource,
    styles: &Styles,
) -> anyhow::Result<(CheckResult, Vec<ProblemSummary>)> {
    writeln!(writer, "{:>HEADER_WIDTH$}", SEPARATOR)?;

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
    let result = display_resolution(writer, env, apis, &resolved, styles)?;

    // Extract owned summaries before dropping the borrowed resolved state.
    let summaries = resolved.problem_summaries();

    // Release borrows held by `resolved`, then drop the source
    // collections in parallel. Each contains many parsed OpenAPI
    // documents whose sequential drops are costly.
    drop(resolved);
    std::thread::scope(|s| {
        s.spawn(|| drop(blessed));
        s.spawn(|| drop(generated));
        s.spawn(|| drop(local_files));
    });

    Ok((result, summaries))
}
