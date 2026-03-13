// Copyright 2026 Oxide Computer Company

use crate::{
    apis::ManagedApis,
    environment::{BlessedSource, GeneratedSource, ResolvedEnv},
    output::{
        CheckResult, OutputOpts, display_load_problems, display_resolution,
        headers::*,
    },
    resolved::{ProblemSummary, Resolved},
};

pub(crate) fn check_impl(
    apis: &ManagedApis,
    env: &ResolvedEnv,
    blessed_source: &BlessedSource,
    generated_source: &GeneratedSource,
    output: &OutputOpts,
) -> anyhow::Result<CheckResult> {
    let (result, _summaries) = check_impl_with_summaries(
        apis,
        env,
        blessed_source,
        generated_source,
        output,
    )?;
    Ok(result)
}

pub(crate) fn check_impl_with_summaries(
    apis: &ManagedApis,
    env: &ResolvedEnv,
    blessed_source: &BlessedSource,
    generated_source: &GeneratedSource,
    output: &OutputOpts,
) -> anyhow::Result<(CheckResult, Vec<ProblemSummary>)> {
    let styles = output.styles(supports_color::Stream::Stderr);

    eprintln!("{:>HEADER_WIDTH$}", SEPARATOR);

    let (generated, errors) =
        generated_source.load(apis, &styles, &env.repo_root, &env.vcs)?;
    display_load_problems(&errors, &styles)?;

    let (local_files, errors) =
        env.local_source.load(apis, &styles, &env.repo_root, &env.vcs)?;
    display_load_problems(&errors, &styles)?;

    let (blessed, errors) =
        blessed_source.load(&env.repo_root, apis, &styles, &env.vcs)?;
    display_load_problems(&errors, &styles)?;

    let resolved = Resolved::new(env, apis, &blessed, &generated, &local_files);

    eprintln!("{:>HEADER_WIDTH$}", SEPARATOR);
    let result = display_resolution(env, apis, &resolved, &styles)?;

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
