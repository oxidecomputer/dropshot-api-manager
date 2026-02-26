// Copyright 2026 Oxide Computer Company

//! Integration tests for the `--generated-from-dir` flag.

use anyhow::Result;
use atomicwrites::{AtomicFile, OverwriteBehavior};
use dropshot_api_manager::test_util::{
    CheckResult, check_apis_up_to_date, check_apis_with_generated_from_dir,
};
use integration_tests::*;
use std::io::Write;

/// Write content to a file atomically, matching this project's convention
/// of using `atomicwrites` instead of `std::fs::write`.
fn atomic_write(path: &camino::Utf8Path, content: &str) -> Result<()> {
    AtomicFile::new(path, OverwriteBehavior::AllowOverwrite)
        .write(|f| f.write_all(content.as_bytes()))?;
    Ok(())
}

/// When `--generated-from-dir` points to an empty directory, the tool should
/// report a clear error rather than panicking.
#[test]
fn test_generated_from_empty_dir_does_not_panic() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = versioned_health_apis()?;
    env.generate_documents(&apis)?;

    // Create an empty directory for "generated" source.
    let empty_dir = env.workspace_root().join("empty-gen");
    std::fs::create_dir_all(&empty_dir)?;

    // The missing API should be reported as an unfixable problem, not a
    // panic.
    let result = check_apis_with_generated_from_dir(
        env.environment(),
        &apis,
        empty_dir,
    )?;
    assert_eq!(result, CheckResult::Failures);
    Ok(())
}

/// When `--generated-from-dir` has documents for only some APIs (in a
/// mixed lockstep + versioned config), the tool should report a clear error
/// rather than panicking.
#[test]
fn test_generated_from_partial_dir_does_not_panic() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_mixed_test_apis()?;
    env.generate_documents(&apis)?;

    // Create a partial generated dir: only the lockstep file, no versioned.
    let partial_dir = env.workspace_root().join("partial-gen");
    std::fs::create_dir_all(&partial_dir)?;
    let lockstep_content = env.read_lockstep_document("health")?;
    atomic_write(&partial_dir.join("health.json"), &lockstep_content)?;

    // The versioned APIs have no generated source, so they should be
    // reported as failures.
    let result = check_apis_with_generated_from_dir(
        env.environment(),
        &apis,
        partial_dir,
    )?;
    assert_eq!(result, CheckResult::Failures);
    Ok(())
}

/// When `--generated-from-dir` provides only some versions of a versioned
/// API (e.g. the dir has v3 and v4 but not v1 or v2 which are stored as
/// git refs locally), the tool should report failures for the missing
/// versions rather than panicking.
#[test]
fn test_generated_from_dir_partial_versions() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = versioned_health_git_ref_apis()?;
    env.generate_documents(&apis)?;
    env.commit_documents()?;

    // Advance HEAD and add v4, triggering git ref conversion for older
    // blessed versions.
    env.make_unrelated_commit("advance")?;
    let apis_v4 = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&apis_v4)?;

    // Build a generated dir containing only the versions that still have
    // full JSON files locally (v3, v4). Versions stored as git refs (v1,
    // v2) won't be found by find_versioned_document_path, so the generated
    // dir will be incomplete.
    let gen_dir = env.workspace_root().join("gen-partial-versions");
    std::fs::create_dir_all(gen_dir.join("versioned-health"))?;
    for v in &["1.0.0", "2.0.0", "3.0.0", "4.0.0"] {
        if let Ok(Some(path)) =
            env.find_versioned_document_path("versioned-health", v)
        {
            let content = env.read_file(&path)?;
            let filename = camino::Utf8Path::new(&path).file_name().unwrap();
            atomic_write(
                &gen_dir.join("versioned-health").join(filename),
                &content,
            )?;
        }
    }

    // Should not panic. Some versions are missing, so the result should
    // be Failures.
    let result = check_apis_with_generated_from_dir(
        env.environment(),
        &apis_v4,
        gen_dir,
    )?;
    assert_eq!(result, CheckResult::Failures);
    Ok(())
}

/// When `--generated-from-dir` has all required documents, check should
/// succeed.
#[test]
fn test_generated_from_complete_dir_succeeds() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = lockstep_health_apis()?;
    env.generate_documents(&apis)?;

    // Create a generated dir with the correct lockstep file.
    let gen_dir = env.workspace_root().join("gen-complete");
    std::fs::create_dir_all(&gen_dir)?;
    let content = env.read_lockstep_document("health")?;
    atomic_write(&gen_dir.join("health.json"), &content)?;

    let result =
        check_apis_with_generated_from_dir(env.environment(), &apis, gen_dir)?;
    assert_eq!(result, CheckResult::Success);

    // Also verify that the normal check (generating from code) agrees.
    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::Success);
    Ok(())
}
