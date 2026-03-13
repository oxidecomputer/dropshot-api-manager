// Copyright 2026 Oxide Computer Company

//! Test utilities for the Dropshot API manager.

pub use crate::output::CheckResult;
#[doc(hidden)]
pub use crate::resolved::{ProblemKind, ProblemSummary};
use crate::{
    apis::ManagedApis,
    cmd::{
        check::check_impl_with_summaries,
        dispatch::{BlessedSourceArgs, GeneratedSourceArgs},
    },
    environment::{Environment, GeneratedSource},
    output::OutputOpts,
    resolved,
};
use camino::Utf8PathBuf;

/// Check that a set of APIs is up-to-date.
///
/// This is meant to be called within a test.
pub fn check_apis_up_to_date(
    env: &Environment,
    apis: &ManagedApis,
) -> Result<CheckResult, anyhow::Error> {
    let (result, _summaries) = check_apis_with_summaries(env, apis)?;
    Ok(result)
}

/// Check that a set of APIs is up-to-date, loading generated documents from
/// the given directory instead of generating them from the API definitions.
pub fn check_apis_with_generated_from_dir(
    env: &Environment,
    apis: &ManagedApis,
    generated_from_dir: Utf8PathBuf,
) -> Result<CheckResult, anyhow::Error> {
    let (result, _summaries) =
        check_apis_with_generated_from_dir_and_summaries(
            env,
            apis,
            generated_from_dir,
        )?;
    Ok(result)
}

/// Like [`check_apis_up_to_date`], but also returns the list of problem
/// summaries for detailed assertions in tests.
#[doc(hidden)]
pub fn check_apis_with_summaries(
    env: &Environment,
    apis: &ManagedApis,
) -> Result<(CheckResult, Vec<resolved::ProblemSummary>), anyhow::Error> {
    let env = resolve_env(env)?;
    let (blessed_source, generated_source, output) =
        default_sources(&env, None)?;
    check_impl_with_summaries(
        apis,
        &env,
        &blessed_source,
        &generated_source,
        &output,
    )
}

/// Like [`check_apis_with_generated_from_dir`], but also returns the list
/// of problem summaries for detailed assertions in tests.
#[doc(hidden)]
pub fn check_apis_with_generated_from_dir_and_summaries(
    env: &Environment,
    apis: &ManagedApis,
    generated_from_dir: Utf8PathBuf,
) -> Result<(CheckResult, Vec<resolved::ProblemSummary>), anyhow::Error> {
    let env = resolve_env(env)?;
    let (blessed_source, generated_source, output) =
        default_sources(&env, Some(generated_from_dir))?;
    check_impl_with_summaries(
        apis,
        &env,
        &blessed_source,
        &generated_source,
        &output,
    )
}

fn resolve_env(
    env: &Environment,
) -> Result<crate::environment::ResolvedEnv, anyhow::Error> {
    // env.resolve(None) assumes that env.default_openapi_dir is where the
    // OpenAPI documents live and doesn't need a further override. (If a custom
    // directory is desired, it can always be passed in via `env`.)
    env.resolve(None)
}

fn default_sources(
    env: &crate::environment::ResolvedEnv,
    generated_from_dir: Option<Utf8PathBuf>,
) -> Result<
    (crate::environment::BlessedSource, GeneratedSource, OutputOpts),
    anyhow::Error,
> {
    let blessed_source = BlessedSourceArgs {
        blessed_from_vcs: None,
        blessed_from_vcs_path: None,
        blessed_from_dir: None,
    }
    .to_blessed_source(env)?;
    let generated_source =
        GeneratedSource::from(GeneratedSourceArgs { generated_from_dir });
    let output = OutputOpts { color: clap::ColorChoice::Auto };
    Ok((blessed_source, generated_source, output))
}
