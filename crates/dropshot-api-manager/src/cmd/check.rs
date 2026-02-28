// Copyright 2026 Oxide Computer Company

use crate::{
    apis::ManagedApis,
    environment::{BlessedSource, GeneratedSource, ResolvedEnv},
    output::{
        CheckResult, OutputOpts, Styles, display_load_problems,
        display_resolution, headers::*,
    },
    resolved::Resolved,
};

pub(crate) fn check_impl(
    apis: &ManagedApis,
    env: &ResolvedEnv,
    blessed_source: &BlessedSource,
    generated_source: &GeneratedSource,
    output: &OutputOpts,
) -> anyhow::Result<CheckResult> {
    let mut styles = Styles::default();
    if output.use_color(supports_color::Stream::Stderr) {
        styles.colorize();
    }

    eprintln!("{:>HEADER_WIDTH$}", SEPARATOR);

    let (generated, errors) =
        generated_source.load(apis, &styles, &env.repo_root)?;
    display_load_problems(&errors, &styles)?;

    let (local_files, errors) =
        env.local_source.load(apis, &styles, &env.repo_root)?;
    display_load_problems(&errors, &styles)?;

    let (blessed, errors) =
        blessed_source.load(&env.repo_root, apis, &styles)?;
    display_load_problems(&errors, &styles)?;

    let resolved = Resolved::new(env, apis, &blessed, &generated, &local_files);

    eprintln!("{:>HEADER_WIDTH$}", SEPARATOR);
    let result = display_resolution(env, apis, &resolved, &styles)?;

    // Release borrows held by `resolved`, then drop the source
    // collections in parallel. Each contains many parsed OpenAPI
    // documents whose sequential drops are costly.
    drop(resolved);
    std::thread::scope(|s| {
        s.spawn(|| drop(blessed));
        s.spawn(|| drop(generated));
        s.spawn(|| drop(local_files));
    });

    Ok(result)
}
