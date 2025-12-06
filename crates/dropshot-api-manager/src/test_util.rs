// Copyright 2025 Oxide Computer Company

//! Test utilities for the Dropshot API manager.

pub use crate::output::CheckResult;
use crate::{
    apis::ManagedApis,
    cmd::{
        check::check_impl,
        diff::diff_api,
        dispatch::{BlessedSourceArgs, GeneratedSourceArgs},
    },
    environment::{Environment, GeneratedSource},
    output::{OutputOpts, Styles},
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
    let generated_source =
        GeneratedSource::from(GeneratedSourceArgs { generated_from_dir: None });
    let output = OutputOpts { color: clap::ColorChoice::Auto };

    check_impl(apis, &env, &blessed_source, &generated_source, &output)
}

/// Generate the diff output as a string. Used for testing.
///
/// Returns the diff output that would be written to stdout by the diff command.
/// Load errors are ignored (files with errors are simply not included).
pub fn get_diff_output(
    env: &Environment,
    apis: &ManagedApis,
) -> Result<String, anyhow::Error> {
    let env = env.resolve(None)?;

    let blessed_source =
        BlessedSourceArgs { blessed_from_git: None, blessed_from_dir: None }
            .to_blessed_source(&env)?;

    let styles = Styles::default();
    let (local_files, _errors) = env.local_source.load(apis, &styles)?;
    let (blessed_files, _errors) =
        blessed_source.load(&env.repo_root, apis, &styles)?;

    let mut output = Vec::new();
    for api in apis.iter_apis() {
        let ident = api.ident();
        let local_api = local_files.get(ident);
        let blessed_api = blessed_files.get(ident);
        diff_api(ident, local_api, blessed_api, &styles, &mut output)?;
    }

    Ok(String::from_utf8(output)?)
}
