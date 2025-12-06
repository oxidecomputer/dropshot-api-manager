// Copyright 2025 Oxide Computer Company

//! Test utilities for the Dropshot API manager.

pub use crate::output::CheckResult;
use crate::{
    apis::ManagedApis,
    cmd::{check::check_impl, diff::diff_impl, dispatch::BlessedSourceArgs},
    environment::{Environment, GeneratedSource},
    output::OutputOpts,
};

/// Check that a set of APIs is up-to-date.
///
/// This is meant to be called within a test.
pub fn check_apis_up_to_date(
    env: &Environment,
    apis: &ManagedApis,
) -> Result<CheckResult, anyhow::Error> {
    // env.resolve(None) assumes that env.default_openapi_dir is where the
    // OpenAPI documents live and doesn't need a further override. (If a custom
    // directory is desired, it can always be passed in via `env`.)
    let env = env.resolve(None)?;

    let blessed_source =
        BlessedSourceArgs { blessed_from_git: None, blessed_from_dir: None }
            .to_blessed_source(&env)?;
    let generated_source = GeneratedSource::Generated;
    let output = OutputOpts { color: clap::ColorChoice::Auto };

    check_impl(apis, &env, &blessed_source, &generated_source, &output)
}

/// Generate the diff output as a string. Used for testing.
///
/// Returns the diff output that would be written to stdout by the diff command.
pub fn get_diff_output(
    env: &Environment,
    apis: &ManagedApis,
) -> Result<String, anyhow::Error> {
    let env = env.resolve(None)?;

    let blessed_source =
        BlessedSourceArgs { blessed_from_git: None, blessed_from_dir: None }
            .to_blessed_source(&env)?;
    let output = OutputOpts { color: clap::ColorChoice::Auto };

    let mut buffer = Vec::new();
    diff_impl(apis, &env, &blessed_source, &output, &mut buffer)?;

    // Normalize path separators for cross-platform consistency in tests.
    let result = String::from_utf8(buffer).map_err(|e| {
        anyhow::anyhow!("diff output is not valid UTF-8: {}", e)
    })?;
    Ok(result.replace('\\', "/"))
}
