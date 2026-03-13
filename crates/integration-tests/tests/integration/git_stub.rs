// Copyright 2026 Oxide Computer Company

//! Tests for Git stub storage of blessed API versions.
//!
//! When Git stub storage is enabled, older (non-latest) blessed API versions are
//! stored as `.gitstub` files containing a Git stub reference (`commit:path`) instead
//! of full JSON files. The content is retrieved via `git cat-file blob` at
//! runtime.

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use dropshot_api_manager::{
    ManagedApis,
    test_util::{
        CheckResult, ProblemKind, ProblemSummary, check_apis_up_to_date,
        check_apis_with_summaries,
    },
};
use integration_tests::{
    ExpectedConflictKind, ExpectedConflicts, all_conflict_paths,
    jj_conflict_paths, *,
};

/// Test that Git stub conversion happens when adding a new version, and that
/// the content is preserved correctly.
///
/// When a new version is added to an API with Git stub storage enabled, the
/// older blessed versions should be converted from full JSON files to Git stub
/// files. The Git stubs should point to the first commit where each version was
/// introduced.
#[test]
fn test_conversion() -> Result<()> {
    let env = TestEnvironment::new_git()?;
    conversion_impl(&env)
}

/// Test git stub conversion with a pure jj backend.
#[test]
fn test_pure_jj_conversion() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }
    let env = TestEnvironment::new_jj()?;
    conversion_impl(&env)
}

fn conversion_impl(env: &TestEnvironment) -> Result<()> {
    let apis = versioned_health_git_stub_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;
    let first_commit = env.get_current_commit_hash()?;
    let original_v1 =
        env.read_versioned_document("versioned-health", "1.0.0")?;

    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should exist as JSON"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "2.0.0")?,
        "v2 should exist as JSON"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should exist as JSON"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should not yet be a Git stub"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should not yet be a Git stub"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should not yet be a Git stub"
    );

    env.make_unrelated_commit("unrelated change 1")?;
    env.make_unrelated_commit("unrelated change 2")?;
    let current_commit = env.get_current_commit_hash()?;
    assert_ne!(
        first_commit, current_commit,
        "current commit should have advanced past the first commit"
    );

    // v3 (the previous latest) should be converted to a Git stub in the same
    // operation that creates v4.
    let extended_apis = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&extended_apis)?;

    assert!(
        !env.is_file_committed("versioned-health/versioned-health-4.0.0-")?,
        "v4 should not be committed yet"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should now be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should now be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should now be a Git stub"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 JSON should be removed"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "2.0.0")?,
        "v2 JSON should be removed"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 JSON should be removed"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "4.0.0")?,
        "v4 should not be a Git stub"
    );

    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        let git_stub =
            env.read_versioned_git_stub("versioned-health", version)?;
        assert_eq!(
            git_stub.commit().to_string(),
            first_commit,
            "Git stub for v{} should point to the first commit ({}) \
             (current commit: {})",
            version,
            first_commit,
            current_commit
        );
    }

    let git_stub = env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert!(
        git_stub
            .path()
            .as_str()
            .starts_with("documents/versioned-health/versioned-health-1.0.0-"),
        "path {} should start with `documents/versioned-health/versioned-health-1.0.0-`",
        git_stub.path().as_str(),
    );
    assert_eq!(
        git_stub.path().extension(),
        Some("json"),
        "path {} should have extension `json`",
        git_stub.path().as_str(),
    );

    let git_stub_v1_content =
        env.read_git_stub_content("versioned-health", "1.0.0")?;
    assert_eq!(
        original_v1, git_stub_v1_content,
        "Git stub content should match original"
    );

    let result = check_apis_up_to_date(env.environment(), &extended_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that the latest version and versions sharing its first commit are not
/// converted to Git stubs.
///
/// When multiple versions share the same first commit as the latest, no
/// conversion should happen. We don't want check to fail immediately after
/// multiple versions were added in a single commit -- that is a poor user
/// experience.
#[test]
fn test_same_first_commit_no_conversion() -> Result<()> {
    let env = TestEnvironment::new_git()?;
    let apis = versioned_health_git_stub_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(
        result,
        CheckResult::Success,
        "check should pass when all versions share the same first commit"
    );

    env.generate_documents(&apis)?;

    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should still exist as JSON"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should not be a Git stub"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should remain as JSON (same first commit as latest)"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "2.0.0")?,
        "v2 should remain as JSON (same first commit as latest)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should not be a Git stub"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should not be a Git stub"
    );

    Ok(())
}

/// Test that lockstep APIs are never converted to Git stubs.
#[test]
fn test_lockstep_never_converted_to_git_stub() -> Result<()> {
    let env = TestEnvironment::new_git()?;
    let apis = lockstep_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;
    env.generate_documents(&apis)?;

    assert!(
        env.lockstep_document_exists("health"),
        "lockstep document should exist"
    );
    assert!(
        !env.lockstep_git_stub_exists("health"),
        "lockstep should never be a Git stub"
    );

    Ok(())
}

/// Test that only versions with different first commits are converted.
///
/// When the latest version is blessed, only versions from earlier commits
/// should be converted. Versions sharing the same first commit as latest should
/// remain as JSON.
#[test]
fn test_mixed_first_commits_selective_conversion() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2_no_git_stub = versioned_health_reduced_apis()?;
    env.generate_documents(&v1_v2_no_git_stub)?;
    env.commit_documents()?;
    let first_commit = env.get_current_commit_hash()?;

    let v1_v2_v3_no_git_stub = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3_no_git_stub)?;
    env.commit_documents()?;
    let second_commit = env.get_current_commit_hash()?;
    assert_ne!(first_commit, second_commit);

    assert!(env.versioned_local_document_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);

    // v3 is latest (from second_commit) while v1, v2 are from first_commit.
    let v1_v2_v3_git_stub = versioned_health_git_stub_apis()?;
    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v1_v2_v3_git_stub)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should suggest converting v1, v2 (different first commit)"
    );
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "1.0.0",
                ProblemKind::BlessedVersionShouldBeGitStub,
            ),
            ProblemSummary::new(
                "versioned-health",
                "2.0.0",
                ProblemKind::BlessedVersionShouldBeGitStub,
            ),
        ],
    );

    env.generate_documents(&v1_v2_v3_git_stub)?;

    assert!(env.versioned_git_stub_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_git_stub_exists("versioned-health", "2.0.0")?);

    let v1_stub = env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(v1_stub.commit().to_string(), first_commit);

    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);
    assert!(!env.versioned_git_stub_exists("versioned-health", "3.0.0")?);

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_git_stub)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that Git stubs work correctly after reloading from a different state.
///
/// This also tests that versions introduced in different commits have Git stubs
/// pointing to their respective first commits.
#[test]
fn test_git_stub_check_after_conversion() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    env.make_unrelated_commit("between v2 and v3")?;

    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;
    let v3_commit = env.get_current_commit_hash()?;
    assert_ne!(v1_v2_commit, v3_commit, "v1/v2 and v3 in different commits");

    env.make_unrelated_commit("after v3")?;

    let extended_apis = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&extended_apis)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &extended_apis)?;
    assert_eq!(result, CheckResult::Success);

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should be a Git stub"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should be JSON"
    );

    let v1_git_stub =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    let v1_commit = v1_git_stub.commit().to_string();
    assert_eq!(
        v1_commit, v1_v2_commit,
        "v1 Git stub should point to the commit where v1 was first introduced"
    );

    let v2_git_stub =
        env.read_versioned_git_stub("versioned-health", "2.0.0")?;
    let v2_commit = v2_git_stub.commit().to_string();
    assert_eq!(
        v2_commit, v1_v2_commit,
        "v2 Git stub should point to the commit where v2 was first introduced"
    );

    let v3_git_stub =
        env.read_versioned_git_stub("versioned-health", "3.0.0")?;
    let v3_commit_from_git_stub = v3_git_stub.commit().to_string();
    assert_eq!(
        v3_commit_from_git_stub, v3_commit,
        "v3 Git stub should point to the commit where v3 was first introduced"
    );

    assert_ne!(
        v1_commit, v3_commit_from_git_stub,
        "v1 and v3 should point to different commits"
    );

    Ok(())
}

/// Test that without Git stub storage enabled, no conversion happens.
#[test]
fn test_no_conversion_without_git_stub_enabled() -> Result<()> {
    let env = TestEnvironment::new_git()?;
    let apis = versioned_health_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;
    env.generate_documents(&apis)?;

    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should be JSON"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "2.0.0")?,
        "v2 should be JSON"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should be JSON"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should not be a Git stub"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should not be a Git stub"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should not be a Git stub"
    );

    Ok(())
}

/// Test that Git stubs are converted back to JSON when Git stub storage is
/// disabled, with content preservation.
///
/// This is the reverse of `test_conversion`. When a user disables Git stub
/// storage, existing Git stubs should be converted back to full JSON files.
#[test]
fn test_convert_to_json_when_disabled() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let apis_with_git_stub = versioned_health_git_stub_apis()?;
    env.generate_documents(&apis_with_git_stub)?;
    env.commit_documents()?;

    let original_v1 =
        env.read_versioned_document("versioned-health", "1.0.0")?;
    let original_v2 =
        env.read_versioned_document("versioned-health", "2.0.0")?;

    let extended_with_git_stub = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&extended_with_git_stub)?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should be a Git stub"
    );

    let extended_without_git_stub = versioned_health_with_v4_apis()?;
    let (result, summaries) = check_apis_with_summaries(
        env.environment(),
        &extended_without_git_stub,
    )?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update when Git stubs exist but Git stub \
         storage is disabled"
    );
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "1.0.0",
                ProblemKind::GitStubShouldBeJson,
            ),
            ProblemSummary::new(
                "versioned-health",
                "2.0.0",
                ProblemKind::GitStubShouldBeJson,
            ),
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::GitStubShouldBeJson,
            ),
        ],
    );

    env.generate_documents(&extended_without_git_stub)?;

    assert!(
        !env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 Git stub should be removed"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 Git stub should be removed"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 Git stub should be removed"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should be JSON"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "2.0.0")?,
        "v2 should be JSON"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should be JSON"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should be JSON"
    );

    let restored_v1 =
        env.read_versioned_document("versioned-health", "1.0.0")?;
    let restored_v2 =
        env.read_versioned_document("versioned-health", "2.0.0")?;
    assert_eq!(
        original_v1, restored_v1,
        "v1 content should match original after Git stub-to-JSON conversion"
    );
    assert_eq!(
        original_v2, restored_v2,
        "v2 content should match original after Git stub-to-JSON conversion"
    );

    Ok(())
}

/// Test that duplicate Git stub and JSON files are handled correctly.
///
/// When both Git stub and JSON exist for the same version, the system should:
///
/// - With Git stub enabled: delete the JSON (Git stub preferred for non-latest)
/// - With Git stub disabled: delete the Git stub (JSON preferred)
///
/// This can happen from interrupted conversions, manual file manipulation,
/// or merge conflicts.
#[test]
fn test_duplicates() -> Result<()> {
    let env = TestEnvironment::new_git()?;
    let apis = versioned_health_git_stub_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;

    let extended = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&extended)?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should not have a JSON file"
    );

    // Manually create a duplicate JSON file for v1.
    let json_content =
        env.read_git_stub_content("versioned-health", "1.0.0")?;
    let git_stub_path = env
        .find_versioned_git_stub_path("versioned-health", "1.0.0")?
        .expect("Git stub should exist");
    let json_path = git_stub_path.with_extension("");
    env.create_file(&json_path, &json_content)?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "Git stub should still exist"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "duplicate JSON should exist"
    );

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &extended)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update when duplicate files exist"
    );
    assert_eq!(
        summaries,
        [ProblemSummary::new(
            "versioned-health",
            "1.0.0",
            ProblemKind::DuplicateLocalFile,
        )],
    );

    env.generate_documents(&extended)?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "Git stub should still exist after generate"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "duplicate JSON should be deleted"
    );

    // Now test with Git stubs disabled.
    env.create_file(&json_path, &json_content)?;
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "Git stub should exist"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "JSON should exist"
    );

    let extended_no_git_stub = versioned_health_with_v4_apis()?;
    env.generate_documents(&extended_no_git_stub)?;

    assert!(
        !env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        ".gitstub should be deleted"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "JSON should remain"
    );

    Ok(())
}

/// Test that Git stub points to the most recent addition when a version is
/// removed and re-added.
///
/// Consider the situation where:
///
/// - commit 1 adds API v1
/// - commit 2 removes API v1 (by dropping it from the list of supported versions)
/// - commit 3 re-adds API v1
///
/// When Git stub storage is enabled and v1 is converted to a Git stub, the stub
/// should point to commit 3 (the most recent addition), not commit 1 (the
/// original addition). This ensures the Git stub points to the current version
/// of the file.
#[test]
fn test_remove_readd() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2_v3 = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;
    let commit_1 = env.get_current_commit_hash()?;

    let v1_path_before = env
        .find_versioned_document_path("versioned-health", "1.0.0")?
        .expect("v1 should exist after commit 1");

    let v2_v3_only = versioned_health_no_v1_apis()?;
    env.generate_documents(&v2_v3_only)?;
    env.commit_documents()?;
    let commit_2 = env.get_current_commit_hash()?;

    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should be removed after commit 2"
    );

    let v1_v2_v3_again = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3_again)?;
    env.commit_documents()?;
    let commit_3 = env.get_current_commit_hash()?;

    let v1_path_after = env
        .find_versioned_document_path("versioned-health", "1.0.0")?
        .expect("v1 should exist after commit 3");
    assert_eq!(
        v1_path_before, v1_path_after,
        "v1 path should be the same (same content hash)"
    );

    assert_ne!(commit_1, commit_2, "commits 1 and 2 should differ");
    assert_ne!(commit_2, commit_3, "commits 2 and 3 should differ");
    assert_ne!(commit_1, commit_3, "commits 1 and 3 should differ");

    let v4_with_git_stub = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v4_with_git_stub)?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be converted to a Git stub"
    );

    // The Git stub for v1 should point to commit 3 (the re-addition), not
    // commit 1 (the original addition).
    let v1_git_stub =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    let v1_commit = v1_git_stub.commit().to_string();

    assert_eq!(
        v1_commit, commit_3,
        "v1 Git stub should point to the re-addition commit (commit 3: {}), \
         not the original addition (commit 1: {})",
        commit_3, commit_1
    );

    let v2_git_stub =
        env.read_versioned_git_stub("versioned-health", "2.0.0")?;
    let v2_commit = v2_git_stub.commit().to_string();
    assert_eq!(
        v2_commit, commit_1,
        "v2 Git stub should point to commit 1 (never removed)"
    );

    let v3_git_stub =
        env.read_versioned_git_stub("versioned-health", "3.0.0")?;
    let v3_commit = v3_git_stub.commit().to_string();
    assert_eq!(
        v3_commit, commit_1,
        "v3 Git stub should point to commit 1 (never removed)"
    );

    Ok(())
}

/// Test that when the latest version is removed, the previous-latest version
/// is converted from a Git stub back to a JSON file.
#[test]
fn test_latest_removed() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2 = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    env.make_unrelated_commit("between v2 and v3")?;

    let v1_v2_v3 = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3)?;

    assert!(env.versioned_git_stub_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_git_stub_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);

    env.commit_documents()?;
    let v3_commit = env.get_current_commit_hash()?;
    assert_ne!(v1_v2_commit, v3_commit);

    env.make_unrelated_commit("between v3 and v4")?;

    let v1_v2_v3_v4 = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4)?;

    assert!(env.versioned_git_stub_exists("versioned-health", "3.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "4.0.0")?);

    env.commit_documents()?;

    // Remove v4 by going back to v1-v3.
    env.generate_documents(&v1_v2_v3)?;

    // v3 should be converted back to JSON because it's now the latest.
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should be JSON (new latest after v4 removal)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should not be a Git stub anymore"
    );

    // v1, v2 should remain as Git stubs (different first commit from v3).
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should remain as a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should remain as a Git stub"
    );

    let v1_git_stub =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(v1_git_stub.commit().to_string(), v1_v2_commit);

    let v2_git_stub =
        env.read_versioned_git_stub("versioned-health", "2.0.0")?;
    assert_eq!(v2_git_stub.commit().to_string(), v1_v2_commit);

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that when the latest version is removed, versions that share the same
/// first commit as the new latest are also converted from Git stubs to JSON.
#[test]
fn test_latest_removed_same_commit() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_only = versioned_health_v1_only_apis()?;
    env.generate_documents(&v1_only)?;
    env.commit_documents()?;
    let v1_commit = env.get_current_commit_hash()?;

    env.make_unrelated_commit("between v1 and v2/v3")?;

    // Add v2, v3 in the same generate call (they share the same first commit).
    let v1_v2_v3 = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3)?;

    assert!(env.versioned_git_stub_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);

    env.commit_documents()?;
    let v2_v3_commit = env.get_current_commit_hash()?;
    assert_ne!(v1_commit, v2_v3_commit);

    env.make_unrelated_commit("between v3 and v4")?;

    let v1_v2_v3_v4 = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4)?;

    assert!(env.versioned_git_stub_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_git_stub_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_git_stub_exists("versioned-health", "3.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "4.0.0")?);

    env.commit_documents()?;

    // Remove v4 by going back to v1-v3.
    env.generate_documents(&v1_v2_v3)?;

    // v3 should be converted back to JSON because it's now the latest.
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should be JSON (new latest after v4 removal)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should not be a Git stub anymore"
    );

    // v2 was introduced in the same commit as v3 (the new latest), so it should
    // also be converted back to JSON.
    assert!(
        env.versioned_local_document_exists("versioned-health", "2.0.0")?,
        "v2 should be JSON (same first commit as new latest v3)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should not be a Git stub anymore"
    );

    // v1 should remain as a Git stub (different first commit from v3).
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should remain as a Git stub"
    );

    let v1_git_stub =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(v1_git_stub.commit().to_string(), v1_commit);

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that git errors during first commit lookup are reported as problems.
///
/// This test uses a fake git binary that fails on `--diff-filter=A` commands
/// to simulate a git failure during first commit lookup.
#[test]
fn test_git_error_reports_problem() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&apis)?;
    env.commit_documents()?;

    let fake_git = std::env::var("NEXTEST_BIN_EXE_fake_git")
        .expect("NEXTEST_BIN_EXE_fake_git should be set by nextest");
    let original_git = std::env::var("GIT").ok();

    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe {
        std::env::set_var("GIT", &fake_git);
        // Tell fake_git where the real git is.
        std::env::set_var("REAL_GIT", original_git.as_deref().unwrap_or("git"));
        std::env::set_var("FAKE_GIT_FAIL", "diff_filter_a");
    }

    let v4_apis = versioned_health_with_v4_git_stub_apis()?;
    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v4_apis)?;

    // Should report a failure due to the unfixable GitStubFirstCommitUnknown
    // problem.
    assert_eq!(result, CheckResult::Failures);
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "1.0.0",
                ProblemKind::GitStubFirstCommitUnknown,
            ),
            ProblemSummary::new(
                "versioned-health",
                "2.0.0",
                ProblemKind::GitStubFirstCommitUnknown,
            ),
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::GitStubFirstCommitUnknown,
            ),
            ProblemSummary::new(
                "versioned-health",
                "4.0.0",
                ProblemKind::LocalVersionMissingLocal,
            ),
            ProblemSummary::for_api(
                "versioned-health",
                ProblemKind::LatestLinkStale,
            ),
        ],
    );

    Ok(())
}

/// Test behavior when running in a shallow clone where Git stubs point to
/// commits whose objects are not available.
///
/// This simulates the scenario where:
///
/// 1. Git stub storage is set up and committed to main.
/// 2. CI does a shallow clone (`git clone --depth 1`).
/// 3. CI runs `check` and the Git stubs can't be resolved because the commits
///    they reference are outside the shallow boundary.
#[test]
fn test_shallow_clone_with_git_stubs() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2_v3 = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;

    env.make_unrelated_commit("intermediate")?;

    let v4 = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v4)?;
    env.commit_documents()?;

    assert!(env.versioned_git_stub_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_git_stub_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_git_stub_exists("versioned-health", "3.0.0")?);

    let shallow_env = env.shallow_clone(1)?;

    assert!(
        shallow_env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "Git stub should exist in shallow clone"
    );

    // Check should fail early.
    let result = check_apis_up_to_date(shallow_env.environment(), &v4);
    result.expect_err("check should fail in shallow clone with Git stubs");

    Ok(())
}

/// Test that shallow clones work fine when Git stub storage is not enabled.
#[test]
fn test_shallow_clone_without_git_stubs() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    // Use APIs without Git stub storage.
    let v1_v2_v3 = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;

    env.make_unrelated_commit("intermediate")?;

    let shallow_env = env.shallow_clone(1)?;

    assert!(
        shallow_env
            .versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 document should exist in shallow clone"
    );

    // Check should succeed since we're not using Git stub storage.
    let result = check_apis_up_to_date(shallow_env.environment(), &v1_v2_v3)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that Git stubs don't cause merge conflicts when two branches with
/// different merge bases both convert the same API version to a Git stub.
///
/// See [`no_conflict_setup`] for the test scenario.
#[test]
fn test_no_merge_conflict() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let v1_v2_commit = no_conflict_setup(&mut env)?;

    env.merge_branch_without_renames("branch_a")?;

    no_conflict_verify(&env, &v1_v2_commit)
}

/// Test that rebase succeeds without conflict when both branches add the same
/// version.
///
/// This is the rebase equivalent of [`test_no_merge_conflict`]. See
/// [`no_conflict_setup`] for the test scenario.
#[test]
fn test_no_rebase_conflict() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let v1_v2_commit = no_conflict_setup(&mut env)?;

    env.checkout_branch("branch_a")?;
    let rebase_result = env.try_rebase_onto("branch_b")?;
    assert_eq!(
        rebase_result,
        RebaseResult::Clean,
        "rebase should succeed without conflict"
    );

    no_conflict_verify(&env, &v1_v2_commit)
}

/// Test that jj merge succeeds without conflict when both branches add the
/// same version.
///
/// This is the jj equivalent of [`test_no_merge_conflict`].
#[test]
fn test_jj_no_merge_conflict() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    let v1_v2_commit = no_conflict_setup(&mut env)?;

    let merge_result = env.jj_try_merge("branch_a", "branch_b", "merge")?;
    assert_eq!(
        merge_result,
        JjMergeResult::Clean,
        "jj merge should succeed without conflict"
    );

    no_conflict_verify(&env, &v1_v2_commit)
}

/// Test that jj rebase succeeds without conflict when both branches add the
/// same version.
///
/// This is the jj equivalent of [`test_no_rebase_conflict`].
#[test]
fn test_jj_no_rebase_conflict() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    let v1_v2_commit = no_conflict_setup(&mut env)?;

    let rebase_result = env.jj_try_rebase("branch_a", "branch_b")?;
    assert_eq!(
        rebase_result,
        JjRebaseResult::Clean,
        "jj rebase should succeed without conflict"
    );

    no_conflict_verify(&env, &v1_v2_commit)
}

/// Setup for [`test_no_merge_conflict`] and [`test_no_rebase_conflict`].
///
/// Git stubs store `<commit-hash>:<path>` where `<commit-hash>` is "the
/// first commit when the file was most recently introduced." This is a
/// deterministic property of the file's history, independent of which branch
/// you're on.
///
/// ```text
/// History:
///     main: [initial] -- [v1,v2 added] -- [unrelated A] -- [unrelated B]
///                               |                 |
///                               |                 +-- branch_b: [add v3]
///                               |                         (v1,v2 become Git stubs)
///                               |
///                               +-- branch_a: [add v3]
///                                       (v1,v2 become Git stubs)
/// ```
///
/// Both branches add the same v3, so:
/// - The v1 and v2 Git stubs should have identical content on both branches.
/// - The merge/rebase should succeed without conflict.
///
/// Returns the commit hash where v1/v2 were added. Leaves the environment on
/// branch_b.
fn no_conflict_setup(env: &mut TestEnvironment) -> Result<String> {
    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    env.make_unrelated_commit("unrelated A")?;
    env.create_branch("branch_a")?;

    // Give branch_a and branch_b different merge bases.
    env.make_unrelated_commit("unrelated B")?;
    env.create_branch("branch_b")?;

    env.checkout_branch("branch_a")?;
    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub on branch_a"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub on branch_a"
    );

    let v1_stub_branch_a =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    let v2_stub_branch_a =
        env.read_versioned_git_stub("versioned-health", "2.0.0")?;

    env.checkout_branch("branch_b")?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub on branch_b"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub on branch_b"
    );

    let v1_stub_branch_b =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    let v2_stub_branch_b =
        env.read_versioned_git_stub("versioned-health", "2.0.0")?;

    // Git stubs should be identical: both point to v1_v2_commit.
    assert_eq!(
        v1_stub_branch_a, v1_stub_branch_b,
        "v1 Git stubs should be identical on both branches"
    );
    assert_eq!(
        v2_stub_branch_a, v2_stub_branch_b,
        "v2 Git stubs should be identical on both branches"
    );

    assert_eq!(
        v1_stub_branch_a.commit().to_string(),
        v1_v2_commit,
        "Git stub should point to the original commit"
    );

    Ok(v1_v2_commit)
}

/// Verify the result of [`test_no_merge_conflict`] or
/// [`test_no_rebase_conflict`].
fn no_conflict_verify(env: &TestEnvironment, v1_v2_commit: &str) -> Result<()> {
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 Git stub should exist"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 Git stub should exist"
    );

    let v1_stub = env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(
        v1_stub.commit().to_string(),
        v1_v2_commit,
        "v1 Git stub should point to the original commit"
    );

    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should exist"
    );

    Ok(())
}

/// Test that the generate command resolves rename/rename conflicts that occur
/// when two branches both add different new API versions.
///
/// See [`rename_conflict_v3_v4_setup`] for the test scenario.
#[test]
fn test_rename_conflict_resolved_by_generate() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_v3_v4_setup(&mut env)?;

    let merge_result = env.try_merge_branch("branch_a")?;
    let MergeResult::Conflict(conflicted_files) = merge_result else {
        panic!("merge should have conflicts due to rename/rename detection");
    };
    assert_eq!(
        conflicted_files,
        all_conflict_paths(&expected_conflicts),
        "conflicted files should match expected"
    );

    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4_apis)?;
    env.complete_merge()?;

    rename_conflict_v3_v4_verify(&env, &v1_v2_commit, &v1_v2_v3_v4_apis)
}

/// Test that the generate command resolves rename/rename conflicts during
/// rebase.
///
/// This is the rebase equivalent of [`test_rename_conflict_resolved_by_generate`].
/// See [`rename_conflict_v3_v4_setup`] for the test scenario.
#[test]
fn test_rebase_rename_conflict_resolved_by_generate() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_v3_v4_setup(&mut env)?;

    env.checkout_branch("branch_a")?;
    let rebase_result = env.try_rebase_onto("branch_b")?;
    let RebaseResult::Conflict(conflicted_files) = rebase_result else {
        panic!("rebase should have conflicts due to rename/rename detection");
    };
    assert_eq!(
        conflicted_files,
        all_conflict_paths(&expected_conflicts),
        "conflicted files should match expected"
    );

    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4_apis)?;
    env.continue_rebase()?;

    rename_conflict_v3_v4_verify(&env, &v1_v2_commit, &v1_v2_v3_v4_apis)
}

/// Test that jj merge only conflicts on the symlink when branches add
/// different versions (v3 vs v4).
///
/// Unlike git, jj does NOT have rename detection. Git sees the deletion of
/// v2.json and creation of v3.json/v4.json as a rename conflict, but jj treats
/// them as independent operations. The only conflict is on the latest symlink
/// which both branches update to different targets.
///
/// This is the jj equivalent of [`test_rename_conflict_resolved_by_generate`].
#[test]
fn test_jj_symlink_conflict_v3_v4_merge() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_v3_v4_setup(&mut env)?;

    let merge_result = env.jj_try_merge("branch_a", "branch_b", "merge")?;
    let JjMergeResult::Conflict(conflicted_files) = merge_result else {
        panic!("jj merge should have symlink conflict; got clean merge");
    };
    assert_eq!(
        conflicted_files,
        jj_conflict_paths(&expected_conflicts),
        "jj should only conflict on symlink, not rename-related files"
    );

    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_stub_apis()?;
    env.jj_resolve_conflicts(&v1_v2_v3_v4_apis)?;

    rename_conflict_v3_v4_verify(&env, &v1_v2_commit, &v1_v2_v3_v4_apis)
}

/// Test that jj rebase only conflicts on the symlink when branches add
/// different versions (v3 vs v4).
///
/// This is the rebase equivalent of [`test_jj_symlink_conflict_v3_v4_merge`].
#[test]
fn test_jj_symlink_conflict_v3_v4_rebase() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_v3_v4_setup(&mut env)?;

    let rebase_result = env.jj_try_rebase("branch_a", "branch_b")?;
    let JjRebaseResult::Conflict(conflicted_files) = rebase_result else {
        panic!("jj rebase should have symlink conflict; got clean rebase");
    };
    assert_eq!(
        conflicted_files,
        jj_conflict_paths(&expected_conflicts),
        "jj should only conflict on symlink, not rename-related files"
    );

    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_stub_apis()?;
    env.jj_resolve_conflicts(&v1_v2_v3_v4_apis)?;

    rename_conflict_v3_v4_verify(&env, &v1_v2_commit, &v1_v2_v3_v4_apis)
}

/// Setup for [`test_rename_conflict_resolved_by_generate`] and
/// [`test_rebase_rename_conflict_resolved_by_generate`].
///
/// When Git's rename detection is active, it can misinterpret the deletion of
/// an old version (converted to Git stub) and creation of a new version as a
/// "rename". If two branches both do this with different new versions, Git
/// reports a rename/rename conflict.
///
/// ```text
/// History:
///     main: [initial] -- [v1,v2 added] -- [unrelated A] -- [unrelated B]
///                               |                 |
///                               |                 +-- branch_b: [add v4]
///                               |                         (v1,v2 become Git stubs)
///                               |
///                               +-- branch_a: [add v3]
///                                       (v1,v2 become Git stubs)
/// ```
///
/// Git sees both branches "renaming" v2.json to different files (v3.json vs
/// v4.json), causing a rename/rename conflict.
///
/// Returns the commit hash where v1/v2 were added and the expected conflicted
/// files with their conflict kinds. Leaves the environment on branch_b.
fn rename_conflict_v3_v4_setup(
    env: &mut TestEnvironment,
) -> Result<(String, ExpectedConflicts)> {
    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    env.make_unrelated_commit("unrelated A")?;
    env.create_branch("branch_a")?;

    env.make_unrelated_commit("unrelated B")?;
    env.create_branch("branch_b")?;

    // Capture paths before Git stub conversion for conflict tracking.
    let v2_json_path = env
        .find_versioned_document_path("versioned-health", "2.0.0")?
        .expect("v2 should exist as JSON before branching");

    env.checkout_branch("branch_a")?;
    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    let v3_json_path = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 should exist as JSON on branch_a");
    env.commit_documents()?;

    env.checkout_branch("branch_b")?;
    let v1_v2_v4_apis = versioned_health_v1_v2_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v4_apis)?;
    let v4_json_path = env
        .find_versioned_document_path("versioned-health", "4.0.0")?
        .expect("v4 should exist as JSON on branch_b");
    env.commit_documents()?;

    // Git's rename detection sees v2.json "renamed" to v3/v4 on different
    // branches. jj has no rename detection, so only the symlink conflicts.
    let latest_symlink: Utf8PathBuf =
        "documents/versioned-health/versioned-health-latest.json".into();
    let expected_conflicts: ExpectedConflicts = [
        (v2_json_path, ExpectedConflictKind::Rename),
        (v3_json_path, ExpectedConflictKind::Rename),
        (v4_json_path, ExpectedConflictKind::Rename),
        (latest_symlink, ExpectedConflictKind::Symlink),
    ]
    .into_iter()
    .collect();

    Ok((v1_v2_commit, expected_conflicts))
}

/// Verify the result of [`test_rename_conflict_resolved_by_generate`] or
/// [`test_rebase_rename_conflict_resolved_by_generate`].
fn rename_conflict_v3_v4_verify(
    env: &TestEnvironment,
    v1_v2_commit: &str,
    v1_v2_v3_v4_apis: &ManagedApis,
) -> Result<()> {
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub"
    );

    let v1_stub = env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(
        v1_stub.commit().to_string(),
        v1_v2_commit,
        "v1 Git stub should point to the original commit"
    );

    // v3 is locally added (not blessed), so it should be JSON.
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should exist as JSON (locally added)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should not be a Git stub (locally added, not blessed)"
    );

    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON (latest)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "4.0.0")?,
        "v4 should not be a Git stub"
    );

    let result = check_apis_up_to_date(env.environment(), v1_v2_v3_v4_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that the generate command resolves rename/rename conflicts when both
/// branches add v3 with different content.
///
/// See [`rename_conflict_blessed_setup`] for the test scenario.
#[test]
fn test_rename_conflict_blessed_versions() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&mut env)?;

    let merge_result = env.try_merge_branch("main")?;
    let MergeResult::Conflict(conflicted_files) = merge_result else {
        panic!(
            "merge should have conflicts due to different v3 contents; \
             got clean merge"
        );
    };
    assert_eq!(
        conflicted_files,
        all_conflict_paths(&expected_conflicts),
        "conflicted files should match expected"
    );

    // Resolution: make branch's alternate v3 into v4, keep main's v3.
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4alt_apis)?;
    env.complete_merge()?;

    rename_conflict_blessed_verify(&env, &v1_v2_commit, &v1_v2_v3_v4alt_apis)
}

/// Test that the generate command resolves rename/rename conflicts during
/// rebase when both branches add v3 with different content.
///
/// This is the rebase equivalent of [`test_rename_conflict_blessed_versions`].
/// See [`rename_conflict_blessed_setup`] for the test scenario.
#[test]
fn test_rebase_rename_conflict_blessed_versions() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&mut env)?;

    let rebase_result = env.try_rebase_onto("main")?;
    let RebaseResult::Conflict(conflicted_files) = rebase_result else {
        panic!(
            "rebase should have conflicts due to different v3 contents; \
             got clean rebase"
        );
    };
    assert_eq!(
        conflicted_files,
        all_conflict_paths(&expected_conflicts),
        "conflicted files should match expected"
    );

    // Resolution: make branch's alternate v3 into v4, keep main's v3.
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4alt_apis)?;
    env.continue_rebase()?;

    rename_conflict_blessed_verify(&env, &v1_v2_commit, &v1_v2_v3_v4alt_apis)
}

/// Test that jj merge only conflicts on the symlink when both branches add v3
/// with different content.
///
/// Although both branches add v3 with different content, the v3 files have
/// different hashes (and thus different filenames), so jj treats them as
/// different files. The only conflict is on the latest symlink.
///
/// This is the jj equivalent of [`test_rename_conflict_blessed_versions`].
#[test]
fn test_jj_symlink_conflict_blessed_merge() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&mut env)?;

    let merge_result = env.jj_try_merge("main", "branch_b", "merge")?;
    let JjMergeResult::Conflict(conflicted_files) = merge_result else {
        panic!("jj merge should have symlink conflict; got clean merge");
    };
    assert_eq!(
        conflicted_files,
        jj_conflict_paths(&expected_conflicts),
        "jj should only conflict on symlink, not rename-related files"
    );

    // Resolution: make branch's alternate v3 into v4, keep main's v3.
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_stub_apis()?;
    env.jj_resolve_conflicts(&v1_v2_v3_v4alt_apis)?;

    rename_conflict_blessed_verify(&env, &v1_v2_commit, &v1_v2_v3_v4alt_apis)
}

/// Test that jj rebase only conflicts on the symlink when both branches add v3
/// with different content.
///
/// This is the rebase equivalent of [`test_jj_symlink_conflict_blessed_merge`].
#[test]
fn test_jj_symlink_conflict_blessed_rebase() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&mut env)?;

    let rebase_result = env.jj_try_rebase("branch_b", "main")?;
    let JjRebaseResult::Conflict(conflicted_files) = rebase_result else {
        panic!("jj rebase should have symlink conflict; got clean rebase");
    };
    assert_eq!(
        conflicted_files,
        jj_conflict_paths(&expected_conflicts),
        "jj should only conflict on symlink, not rename-related files"
    );

    // Resolution: make branch's alternate v3 into v4, keep main's v3.
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_stub_apis()?;
    env.jj_resolve_conflicts(&v1_v2_v3_v4alt_apis)?;

    rename_conflict_blessed_verify(&env, &v1_v2_commit, &v1_v2_v3_v4alt_apis)
}

/// Test merge with main as first parent (reverse direction).
///
/// This tests the same scenario as [`test_rename_conflict_blessed_versions`],
/// but merges branch_b into main instead of main into branch_b. The user
/// suspects git may have asymmetric behavior depending on which branch is the
/// first parent.
///
/// See [`rename_conflict_blessed_setup`] for the test scenario.
#[test]
fn test_rename_conflict_blessed_versions_main_first() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&mut env)?;

    // Setup leaves us on branch_b. Switch to main and merge branch_b into it.
    env.checkout_branch("main")?;
    let merge_result = env.try_merge_branch("branch_b")?;
    let MergeResult::Conflict(conflicted_files) = merge_result else {
        panic!(
            "merge should have conflicts due to different v3 contents; \
             got clean merge"
        );
    };
    assert_eq!(
        conflicted_files,
        all_conflict_paths(&expected_conflicts),
        "conflicted files should match expected"
    );

    // Resolution: make branch's alternate v3 into v4, keep main's v3.
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4alt_apis)?;
    env.complete_merge()?;

    rename_conflict_blessed_verify(&env, &v1_v2_commit, &v1_v2_v3_v4alt_apis)
}

// Note: We cannot do a test like this for jj at the moment (where p1 is main
// and p2 is the branch), because our code is not currently aware of jj and HEAD
// isn't updated until the merge is committed. If and when the Dropshot API
// manager gains jj awareness, we can test this scenario.

/// Setup for [`test_rename_conflict_blessed_versions`] and
/// [`test_rebase_rename_conflict_blessed_versions`].
///
/// This simulates a realistic scenario where:
/// 1. main adds v3 (standard version)
/// 2. branch adds v3 with different content (alternate version)
/// 3. Both branches convert v2 to Git stub when adding v3
/// 4. Merge/rebase causes a conflict on v3 files (different hashes due to
///    different content)
/// 5. Resolution: the branch's v3-alternate becomes v4, main's v3 stays as v3
///
/// ```text
/// History:
///     main: [v1,v2] -- [add v3 standard]
///              |
///              +-- branch_b: [add v3 alternate]
/// ```
///
/// Both branches add v3, but with different content. Git sees:
/// - main: v2.json deleted, v3-<hash1>.json created
/// - branch_b: v2.json deleted, v3-<hash2>.json created
///
/// This causes a rename/rename conflict.
///
/// Returns the commit hash where v1/v2 were added and the expected conflicted
/// files with their conflict kinds. Leaves the environment on branch_b.
fn rename_conflict_blessed_setup(
    env: &mut TestEnvironment,
) -> Result<(String, ExpectedConflicts)> {
    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    env.create_branch("branch_b")?;

    // Capture paths before Git stub conversion for conflict tracking.
    let v2_json_path = env
        .find_versioned_document_path("versioned-health", "2.0.0")?
        .expect("v2 should exist as JSON before generating v3");

    // On main: add v3 (standard).
    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    let v3_json_path_main = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 should exist as JSON on main");
    env.commit_documents()?;

    // On branch_b: add v3-alternate (different content, different hash).
    env.checkout_branch("branch_b")?;
    let v3_alt_apis = versioned_health_v3_alternate_git_stub_apis()?;
    env.generate_documents(&v3_alt_apis)?;
    let v3_json_path_b = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 should exist as JSON on branch_b");
    env.commit_documents()?;

    // Git's rename detection sees v2.json "renamed" to v3 on both branches
    // (different destinations). jj has no rename detection.
    let latest_symlink: Utf8PathBuf =
        "documents/versioned-health/versioned-health-latest.json".into();
    let expected_conflicts: ExpectedConflicts = [
        (v2_json_path, ExpectedConflictKind::Rename),
        (v3_json_path_main, ExpectedConflictKind::Rename),
        (v3_json_path_b, ExpectedConflictKind::Rename),
        (latest_symlink, ExpectedConflictKind::Symlink),
    ]
    .into_iter()
    .collect();

    Ok((v1_v2_commit, expected_conflicts))
}

/// Verify the result of [`test_rename_conflict_blessed_versions`] or
/// [`test_rebase_rename_conflict_blessed_versions`].
fn rename_conflict_blessed_verify(
    env: &TestEnvironment,
    v1_v2_commit: &str,
    v1_v2_v3_v4alt_apis: &ManagedApis,
) -> Result<()> {
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub"
    );

    let v1_stub = env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(
        v1_stub.commit().to_string(),
        v1_v2_commit,
        "v1 Git stub should point to the original commit"
    );

    // v3 is blessed (from main) and not latest.
    assert!(
        env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should be a Git stub (blessed from main, not latest)"
    );

    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON (latest)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "4.0.0")?,
        "v4 should not be a Git stub"
    );

    let result = check_apis_up_to_date(env.environment(), v1_v2_v3_v4alt_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that unparseable Git stubs (e.g., with conflict markers) are
/// detected and regenerated.
#[test]
fn test_unparseable_git_stub_regenerated() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    env.make_unrelated_commit("intermediate")?;

    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );

    let v1_git_stub_path = env
        .find_versioned_git_stub_path("versioned-health", "1.0.0")?
        .expect("v1 Git stub should exist");
    let corrupted_content = "<<<<<<< HEAD\n\
abc123:documents/versioned-health/old.json\n\
=======\n\
def456:documents/versioned-health/new.json\n\
>>>>>>> branch\n";
    env.create_file(&v1_git_stub_path, corrupted_content)?;

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update for unparseable Git stub"
    );
    assert_eq!(
        summaries,
        [ProblemSummary::new(
            "versioned-health",
            "1.0.0",
            ProblemKind::BlessedVersionCorruptedLocal,
        )],
    );

    env.generate_documents(&v1_v2_v3_apis)?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 Git stub should be regenerated"
    );

    let v1_git_stub =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(
        v1_git_stub.commit().to_string(),
        v1_v2_commit,
        "regenerated v1 Git stub should point to the original commit"
    );

    let v1_content = env.read_git_stub_content("versioned-health", "1.0.0")?;
    assert!(
        v1_content.contains("\"openapi\""),
        "regenerated Git stub content should be valid OpenAPI"
    );

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that Git stubs with non-canonical format (backslashes, missing
/// trailing newline) are regenerated.
#[test]
fn test_non_canonical_git_stub_regenerated() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    env.make_unrelated_commit("intermediate")?;

    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );

    let v1_git_stub_path = env
        .find_versioned_git_stub_path("versioned-health", "1.0.0")?
        .expect("v1 Git stub should exist");
    let original_content = env.read_file(&v1_git_stub_path)?;
    let original_content = original_content.trim();

    let (commit, path) = original_content
        .split_once(':')
        .expect("Git stub should have commit:path format");
    let non_canonical_path = path.replace('/', "\\");
    let non_canonical_content = format!("{}:{}", commit, non_canonical_path);
    env.create_file(&v1_git_stub_path, &non_canonical_content)?;

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update for non-canonical Git stub"
    );
    assert_eq!(
        summaries,
        [ProblemSummary::new(
            "versioned-health",
            "1.0.0",
            ProblemKind::BlessedVersionCorruptedLocal,
        )],
    );

    env.generate_documents(&v1_v2_v3_apis)?;

    let v1_git_stub_raw =
        env.read_versioned_git_stub_raw("versioned-health", "1.0.0")?;
    assert!(
        v1_git_stub_raw.ends_with('\n'),
        "regenerated Git stub should have trailing newline"
    );
    assert!(
        !v1_git_stub_raw.contains('\\'),
        "regenerated Git stub should use forward slashes"
    );

    let v1_git_stub =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(
        v1_git_stub.commit().to_string(),
        v1_v2_commit,
        "regenerated v1 Git stub should point to the original commit"
    );

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that an empty Git stub is detected and regenerated.
#[test]
fn test_empty_git_stub_regenerated() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;

    env.make_unrelated_commit("intermediate")?;

    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;

    let v1_git_stub_path = env
        .find_versioned_git_stub_path("versioned-health", "1.0.0")?
        .expect("v1 Git stub should exist");
    env.create_file(&v1_git_stub_path, "")?;

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update for empty Git stub"
    );
    assert_eq!(
        summaries,
        [ProblemSummary::new(
            "versioned-health",
            "1.0.0",
            ProblemKind::BlessedVersionCorruptedLocal,
        )],
    );

    env.generate_documents(&v1_v2_v3_apis)?;

    let v1_git_stub_raw =
        env.read_versioned_git_stub_raw("versioned-health", "1.0.0")?;
    assert!(
        v1_git_stub_raw.contains(':'),
        "regenerated Git stub should have commit:path format"
    );

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that a Git stub with an invalid commit hash is regenerated.
#[test]
fn test_invalid_commit_hash_git_stub_regenerated() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;

    env.make_unrelated_commit("intermediate")?;

    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;

    let v1_git_stub_path = env
        .find_versioned_git_stub_path("versioned-health", "1.0.0")?
        .expect("v1 Git stub should exist");
    let invalid_content =
        "not-a-valid-hash:documents/versioned-health/file.json\n";
    env.create_file(&v1_git_stub_path, invalid_content)?;

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update for invalid commit hash"
    );
    assert_eq!(
        summaries,
        [ProblemSummary::new(
            "versioned-health",
            "1.0.0",
            ProblemKind::BlessedVersionCorruptedLocal,
        )],
    );

    env.generate_documents(&v1_v2_v3_apis)?;

    let v1_git_stub =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    let v1_commit = v1_git_stub.commit().to_string();
    assert_eq!(
        v1_commit.len(),
        40,
        "regenerated Git stub should have valid SHA-1 commit hash"
    );

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that a Git stub pointing to a nonexistent commit/path is
/// regenerated. This covers the case where a gitstub is syntactically valid but
/// semantically invalid (the commit or path no longer exists in git, e.g.,
/// after a rebase or force-push).
#[test]
fn test_unresolvable_git_stub_regenerated() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    env.make_unrelated_commit("intermediate")?;

    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );

    // Write a syntactically valid but semantically invalid gitstub.
    let v1_git_stub_path = env
        .find_versioned_git_stub_path("versioned-health", "1.0.0")?
        .expect("v1 Git stub should exist");
    let nonexistent_commit = "aa".repeat(20);
    let unresolvable_content =
        format!("{nonexistent_commit}:documents/versioned-health/fake.json\n");
    env.create_file(&v1_git_stub_path, &unresolvable_content)?;

    // Check should report NeedsUpdate (not Failures).
    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update for unresolvable Git stub, \
         not a hard failure"
    );
    assert_eq!(
        summaries,
        [ProblemSummary::new(
            "versioned-health",
            "1.0.0",
            ProblemKind::BlessedVersionCorruptedLocal,
        )],
    );

    // Generate should fix the problem by deleting and recreating the gitstub.
    env.generate_documents(&v1_v2_v3_apis)?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 Git stub should be regenerated"
    );

    let v1_git_stub =
        env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(
        v1_git_stub.commit().to_string(),
        v1_v2_commit,
        "regenerated v1 Git stub should point to the original commit"
    );

    let v1_content = env.read_git_stub_content("versioned-health", "1.0.0")?;
    assert!(
        v1_content.contains("\"openapi\""),
        "regenerated Git stub content should be valid OpenAPI"
    );

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test the dependent branch workflow with Git stubs.
///
/// See [`dependent_branch_setup`] for the test scenario.
#[test]
fn test_dependent_branch_merge() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let v1_v2_commit = dependent_branch_setup(&mut env)?;

    // Merge branch_a into main first.
    env.checkout_branch("main")?;
    env.merge_branch_without_renames("branch_a")?;

    dependent_branch_after_a_merged_verify(&env, &v1_v2_commit)?;

    // Now merge main into branch_b. This is a clean merge because branch_b is
    // based on branch_a, and main now has exactly branch_a's changes.
    env.checkout_branch("branch_b")?;
    let merge_result = env.try_merge_branch("main")?;
    assert_eq!(
        merge_result,
        MergeResult::Clean,
        "merge should be clean (branch_b is ahead of main)"
    );

    // After the merge, v3 and v4 are both still JSON (no generate run yet).
    dependent_branch_after_merge_before_generate_verify(&env)?;

    // Run generate to convert v3 to a Git stub (now that v3 is blessed).
    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4_apis)?;

    dependent_branch_after_generate_verify(
        &env,
        &v1_v2_commit,
        &v1_v2_v3_v4_apis,
    )
}

/// Test the dependent branch workflow with git rebase.
///
/// This is the rebase equivalent of [`test_dependent_branch_merge`]. See
/// [`dependent_branch_setup`] for the test scenario.
#[test]
fn test_dependent_branch_rebase() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let v1_v2_commit = dependent_branch_setup(&mut env)?;

    // Merge branch_a into main first.
    env.checkout_branch("main")?;
    env.merge_branch_without_renames("branch_a")?;

    dependent_branch_after_a_merged_verify(&env, &v1_v2_commit)?;

    // Now rebase branch_b onto main. This is a clean rebase because branch_b's
    // base (branch_a) is now part of main.
    env.checkout_branch("branch_b")?;
    let rebase_result = env.try_rebase_onto("main")?;
    assert_eq!(
        rebase_result,
        RebaseResult::Clean,
        "rebase should be clean (branch_b's base is now on main)"
    );

    // After the rebase, v3 and v4 are both still JSON (no generate run yet).
    dependent_branch_after_merge_before_generate_verify(&env)?;

    // Run generate to convert v3 to a Git stub (now that v3 is blessed).
    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4_apis)?;

    dependent_branch_after_generate_verify(
        &env,
        &v1_v2_commit,
        &v1_v2_v3_v4_apis,
    )
}

/// Test the dependent branch workflow with jj merge.
///
/// This is the jj equivalent of [`test_dependent_branch_merge`]. See
/// [`dependent_branch_setup`] for the test scenario.
#[test]
fn test_jj_dependent_branch_merge() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    let v1_v2_commit = dependent_branch_setup(&mut env)?;

    // Create a merge of branch_a into main using jj.
    let main_a_merge_result =
        env.jj_try_merge("main", "branch_a", "merge branch_a into main")?;
    assert_eq!(
        main_a_merge_result,
        JjMergeResult::Clean,
        "merging branch_a into main should be clean"
    );

    // Update the main bookmark to point to the merge result. This is necessary
    // for the blessed version detection to work correctly.
    env.jj_set_bookmark("main", "@")?;

    // Now create a merge of this result with branch_b.
    let merge_result = env.jj_try_merge("@", "branch_b", "merge branch_b")?;
    assert_eq!(
        merge_result,
        JjMergeResult::Clean,
        "jj merge should be clean (branch_b is ahead)"
    );

    // After the merge, v3 and v4 are both still JSON (no generate run yet).
    dependent_branch_after_merge_before_generate_verify(&env)?;

    // Run generate to convert v3 to a Git stub (now that v3 is blessed).
    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4_apis)?;

    dependent_branch_after_generate_verify(
        &env,
        &v1_v2_commit,
        &v1_v2_v3_v4_apis,
    )
}

/// Test the dependent branch workflow with jj rebase.
///
/// This is the jj rebase equivalent of [`test_dependent_branch_merge`]. See
/// [`dependent_branch_setup`] for the test scenario.
#[test]
fn test_jj_dependent_branch_rebase() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    let v1_v2_commit = dependent_branch_setup(&mut env)?;

    // First, rebase branch_a onto main (this should be clean since branch_a is
    // already based on main).
    let rebase_a_result = env.jj_try_rebase("branch_a", "main")?;
    assert_eq!(
        rebase_a_result,
        JjRebaseResult::Clean,
        "rebasing branch_a onto main should be clean"
    );

    // Update the main bookmark to point to branch_a. This simulates the scenario
    // where branch_a has been merged into main.
    env.jj_set_bookmark("main", "branch_a")?;

    // Rebasing branch_b onto branch_a is not required, because jj automatically rebases descendants.

    // Create a new working copy at branch_b so we're on the right commit.
    env.jj_new("branch_b")?;

    // After the rebase, v3 and v4 are both still JSON (no generate run yet).
    dependent_branch_after_merge_before_generate_verify(&env)?;

    // Run generate to convert v3 to a Git stub (now that v3 is blessed).
    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4_apis)?;

    dependent_branch_after_generate_verify(
        &env,
        &v1_v2_commit,
        &v1_v2_v3_v4_apis,
    )
}

/// Setup for the dependent branch workflow tests.
///
/// This simulates a scenario where:
/// 1. main has v1 and v2 committed
/// 2. branch_a (off main) adds v3; v1 and v2 become Git stubs
/// 3. branch_b (off branch_a) adds v4; v3 should NOT become a Git stub because
///    v3 is not yet blessed (not on main)
///
/// ```text
/// History:
///     main: [initial] -- [v1,v2 added]
///                               |
///                               +-- branch_a: [add v3]
///                                       (v1,v2 become Git stubs)
///                                              |
///                                              +-- branch_b: [add v4]
///                                                      (v3 stays JSON - not blessed!)
/// ```
///
/// The key assertion is that on branch_b, v3 should NOT become a Git stub
/// because v3's first commit is not on main. This tests condition 2 from the
/// RFD: "The API is blessed (i.e. present in main)."
///
/// Returns the commit hash where v1/v2 were added. Leaves the environment on
/// branch_b.
fn dependent_branch_setup(env: &mut TestEnvironment) -> Result<String> {
    // Step 1: Create v1, v2 on main.
    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    // Step 2: Create branch_a off main and add v3.
    env.create_branch("branch_a")?;
    env.checkout_branch("branch_a")?;

    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;

    // Verify: v1 and v2 should now be Git stubs, v3 is JSON.
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub on branch_a"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub on branch_a"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should exist as JSON on branch_a"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should not be a Git stub on branch_a"
    );

    env.commit_documents()?;

    // Step 3: Create branch_b off branch_a and add v4.
    env.create_branch("branch_b")?;
    env.checkout_branch("branch_b")?;

    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4_apis)?;

    // v3 should not become a Git stub because v3 is not blessed yet.
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub on branch_b"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub on branch_b"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should exist as JSON on branch_b (not blessed yet)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should NOT be a Git stub on branch_b (not blessed yet)"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON on branch_b"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "4.0.0")?,
        "v4 should not be a Git stub (it's the latest)"
    );

    env.commit_documents()?;

    Ok(v1_v2_commit)
}

/// Verify the state after branch_a is merged into main.
fn dependent_branch_after_a_merged_verify(
    env: &TestEnvironment,
    v1_v2_commit: &str,
) -> Result<()> {
    // After merging branch_a into main:
    // - v1 and v2 should be Git stubs (they were converted on branch_a).
    // - v3 should be JSON (it's the latest on main now).
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub on main after merging branch_a"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub on main after merging branch_a"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should exist as JSON on main (it's the latest)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should not be a Git stub on main (it's the latest)"
    );

    // Verify Git stubs point to the correct commit.
    let v1_stub = env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(
        v1_stub.commit().to_string(),
        v1_v2_commit,
        "v1 Git stub should point to the original v1/v2 commit"
    );

    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Verify the state after merging main into branch_b, before running generate.
///
/// At this point:
/// - v1 and v2 are Git stubs (they were converted on branch_a).
/// - v3 and v4 are both JSON (v3 came from main, v4 is local).
fn dependent_branch_after_merge_before_generate_verify(
    env: &TestEnvironment,
) -> Result<()> {
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub after merge"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub after merge"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should exist as JSON after merge (blessed from main)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should not yet be a Git stub (generate not run yet)"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON after merge"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "4.0.0")?,
        "v4 should not be a Git stub (it's the latest)"
    );

    Ok(())
}

/// Verify the state after running generate on branch_b.
///
/// After generate:
/// - v1 and v2 are Git stubs.
/// - v3 should now be a Git stub (it's blessed from main and not latest).
/// - v4 should be JSON (it's the latest).
fn dependent_branch_after_generate_verify(
    env: &TestEnvironment,
    v1_v2_commit: &str,
    v1_v2_v3_v4_apis: &ManagedApis,
) -> Result<()> {
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should now be a Git stub (blessed from main, not latest)"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 JSON should be removed after conversion to Git stub"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON (latest)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "4.0.0")?,
        "v4 should not be a Git stub (it's the latest)"
    );

    // Verify Git stubs point to the correct commits.
    let v1_stub = env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(
        v1_stub.commit().to_string(),
        v1_v2_commit,
        "v1 Git stub should point to the original v1/v2 commit"
    );

    let result = check_apis_up_to_date(env.environment(), v1_v2_v3_v4_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test successive commits modifying the same non-blessed version with Git stub
/// storage.
///
/// See [`successive_changes_setup`] for the scenario.
#[test]
fn test_rebase_successive_changes_to_nonblessed_version() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    let (v1_v2_commit, expected_first_conflicts, expected_second_conflicts) =
        successive_changes_setup(&mut env)?;

    let rebase_result = env.try_rebase_onto("main")?;
    let RebaseResult::Conflict(conflicted_files) = rebase_result else {
        panic!("expected conflict on first rebase step; got clean rebase");
    };
    assert_eq!(conflicted_files, all_conflict_paths(&expected_first_conflicts));

    // Resolution: promote feature's alt-1 to v4, keep main's v3.
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4alt_apis)?;

    let continue_result = env.try_continue_rebase()?;
    let RebaseResult::Conflict(second_conflicted_files) = continue_result
    else {
        panic!("expected conflict on second rebase step; got clean rebase");
    };
    assert_eq!(
        second_conflicted_files,
        all_conflict_paths(&expected_second_conflicts),
    );

    // Resolution: update v4 to alt-2 content.
    let v1_v2_v3_v4alt2_apis =
        versioned_health_v1_v2_v3_v4alt2_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_v4alt2_apis)?;

    env.continue_rebase()?;

    successive_changes_verify(&env, &v1_v2_commit, &v1_v2_v3_v4alt2_apis)
}

/// jj variant of [`test_rebase_successive_changes_to_nonblessed_version`].
///
/// jj lacks rename detection, so only symlink conflicts occur (not rename/rename).
#[test]
fn test_jj_rebase_successive_changes_to_nonblessed_version() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    let (v1_v2_commit, expected_first_conflicts, expected_second_conflicts) =
        successive_changes_setup(&mut env)?;

    let rebase_result = env.jj_try_rebase("feature", "main")?;
    let JjRebaseResult::Conflict(_) = rebase_result else {
        panic!("expected conflict on jj rebase; got clean rebase");
    };

    let first_conflicts = env.jj_get_revision_conflicts("feature-")?;
    assert_eq!(first_conflicts, jj_conflict_paths(&expected_first_conflicts));

    // Resolution: promote feature's alt-1 to v4, keep main's v3.
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_stub_apis()?;
    env.jj_resolve_commit("feature-", &v1_v2_v3_v4alt_apis)?;

    let second_conflicts = env.jj_get_revision_conflicts("feature")?;
    assert_eq!(second_conflicts, jj_conflict_paths(&expected_second_conflicts));

    // Resolution: update v4 to alt-2 content.
    let v1_v2_v3_v4alt2_apis =
        versioned_health_v1_v2_v3_v4alt2_git_stub_apis()?;
    env.jj_resolve_commit("feature", &v1_v2_v3_v4alt2_apis)?;

    successive_changes_verify(&env, &v1_v2_commit, &v1_v2_v3_v4alt2_apis)
}

/// Setup for [`test_rebase_successive_changes_to_nonblessed_version`].
///
/// ```text
/// main: [v1,v2] -- [add v3 standard]
///            \         (v1,v2 -> gitstub)
///             feature: [add v3 alt-1] -- [v3 alt-1 -> alt-2]
///                          (v1,v2 -> gitstub)
/// ```
fn successive_changes_setup(
    env: &mut TestEnvironment,
) -> Result<(String, ExpectedConflicts, ExpectedConflicts)> {
    let v1_v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash()?;

    env.create_branch("feature")?;

    // Capture the v2 path before Git stub conversion.
    let v2_json_path = env
        .find_versioned_document_path("versioned-health", "2.0.0")?
        .expect("v2 should exist as JSON");

    let v1_v2_v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    let v3_standard_json_path = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 should exist as JSON");
    env.commit_documents()?;

    env.checkout_branch("feature")?;
    let v3_alt1_apis = versioned_health_v3_alternate_git_stub_apis()?;
    env.generate_documents(&v3_alt1_apis)?;
    let v3_alt1_json_path = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 alt-1 should exist");
    env.commit_documents()?;

    let v3_alt2_apis = versioned_health_v3_alternate2_git_stub_apis()?;
    env.generate_documents(&v3_alt2_apis)?;
    let v3_alt2_json_path = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 alt-2 should exist");
    env.commit_documents()?;

    // Pre-compute the v4 path: resolution will promote alt-1 to v4.
    let v4_json_path = {
        let temp_env = TestEnvironment::new_git()?;
        let v4_apis = versioned_health_v1_v2_v3_v4alt_git_stub_apis()?;
        temp_env.generate_documents(&v4_apis)?;
        temp_env
            .find_versioned_document_path("versioned-health", "4.0.0")?
            .expect("v4 should exist")
    };

    let latest_symlink: Utf8PathBuf =
        "documents/versioned-health/versioned-health-latest.json".into();

    // Git detects v2 -> v3 as rename (content similarity); both branches do
    // this to different v3 files, causing rename/rename conflict.
    let expected_first_conflicts: ExpectedConflicts = [
        (v2_json_path, ExpectedConflictKind::Rename),
        (v3_standard_json_path, ExpectedConflictKind::Rename),
        (v3_alt1_json_path.clone(), ExpectedConflictKind::Rename),
        (latest_symlink.clone(), ExpectedConflictKind::Symlink),
    ]
    .into_iter()
    .collect();

    // Git detects v3-alt1 -> v3-alt2 as rename; after resolution deletes alt1
    // (promoted to v4), this becomes rename/rename involving alt1, alt2, v4.
    let expected_second_conflicts: ExpectedConflicts = [
        (v3_alt1_json_path, ExpectedConflictKind::Rename),
        (v3_alt2_json_path, ExpectedConflictKind::Rename),
        (v4_json_path, ExpectedConflictKind::Rename),
        (latest_symlink, ExpectedConflictKind::Symlink),
    ]
    .into_iter()
    .collect();

    Ok((v1_v2_commit, expected_first_conflicts, expected_second_conflicts))
}

/// Verifies final state: v1-v3 as Git stubs, v4 as JSON (latest).
fn successive_changes_verify(
    env: &TestEnvironment,
    v1_v2_commit: &str,
    final_apis: &ManagedApis,
) -> Result<()> {
    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        assert!(
            env.versioned_git_stub_exists("versioned-health", version)?,
            "{version} should be a Git stub"
        );
    }

    // v1/v2 Git stubs should point to the original commit.
    let v1_stub = env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(v1_stub.commit().to_string(), v1_v2_commit);

    // v4 is latest, so it's JSON not Git stub.
    assert!(env.versioned_local_document_exists("versioned-health", "4.0.0")?);
    assert!(!env.versioned_git_stub_exists("versioned-health", "4.0.0")?);

    let result = check_apis_up_to_date(env.environment(), final_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that stale Git stub commit hashes are detected and fixed.
///
/// After a rebase or force-push, the commit hash stored in a `.gitstub` file
/// may refer to a commit that is no longer an ancestor of the merge base.
/// The tool should detect this and update the Git stub to the correct commit.
///
/// This test simulates this by creating a divergent branch with a commit that
/// has the same files as main. The divergent commit exists in the object
/// store (so `git cat-file blob` works), but is not an ancestor of HEAD.
/// Overwriting the gitstub files to point to this divergent commit simulates
/// what happens after a rebase.
#[test]
fn test_stale_git_stub_commit() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    stale_git_stub_commit_impl(&mut env)
}

#[test]
fn test_pure_jj_stale_git_stub_commit() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }
    let mut env = TestEnvironment::new_jj()?;
    stale_git_stub_commit_impl(&mut env)
}

fn stale_git_stub_commit_impl(env: &mut TestEnvironment) -> Result<()> {
    // Step 1: generate v1-v3 and commit them.
    let v1_v2_v3 = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;
    let original_commit = env.get_current_commit_hash()?;

    // Step 2: create a divergent branch from the current commit. This
    // branch's commit will exist in git's object store but will not be an
    // ancestor of main's HEAD after main advances. The divergent commit's
    // tree includes the v1-v3 files, so `git cat-file blob` will work.
    env.create_branch("diverged")?;
    env.checkout_branch("diverged")?;
    env.make_unrelated_commit("divergent work")?;
    let divergent_commit = env.get_current_commit_hash()?;
    env.checkout_branch("main")?;

    // Step 3: advance main past the branch point. The divergent commit is
    // now NOT an ancestor of main's HEAD.
    env.make_unrelated_commit("advance main")?;

    // Step 4: add v4 so that v1-v3 get converted to Git stubs.
    let v4 = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v4)?;
    env.commit_documents()?;

    // Verify that v1-v3 are Git stubs pointing to the original commit.
    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        assert!(
            env.versioned_git_stub_exists("versioned-health", version)?,
            "v{version} should be a Git stub"
        );
        let git_stub =
            env.read_versioned_git_stub("versioned-health", version)?;
        assert_eq!(
            git_stub.commit().to_string(),
            original_commit,
            "v{version} Git stub should point to the original commit"
        );
    }

    let result = check_apis_up_to_date(env.environment(), &v4)?;
    assert_eq!(
        result,
        CheckResult::Success,
        "check should pass before tampering"
    );

    // Step 5: overwrite the Git stubs to point to the divergent commit.
    // This simulates what happens after a rebase: the commit hash is real
    // and the content is accessible, but the commit is not an ancestor of
    // the merge base.
    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        let git_stub_path = env
            .find_versioned_git_stub_path("versioned-health", version)?
            .expect("Git stub should exist");
        let git_stub_content =
            env.read_versioned_git_stub_raw("versioned-health", version)?;
        // Replace the commit hash but keep the path.
        let path_part = git_stub_content
            .trim()
            .split_once(':')
            .expect("Git stub should contain ':'")
            .1;
        let stale_content = format!("{}:{}\n", divergent_commit, path_part);
        env.create_file(&git_stub_path, &stale_content)?;
    }

    // Step 6: check should detect the stale commits.
    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v4)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should detect stale Git stub commits"
    );
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "1.0.0",
                ProblemKind::GitStubCommitStale,
            ),
            ProblemSummary::new(
                "versioned-health",
                "2.0.0",
                ProblemKind::GitStubCommitStale,
            ),
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::GitStubCommitStale,
            ),
        ],
    );

    // Step 7: generate should fix the stale Git stubs.
    env.generate_documents(&v4)?;

    // Verify the Git stubs now point to the correct commit.
    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        let git_stub =
            env.read_versioned_git_stub("versioned-health", version)?;
        assert_eq!(
            git_stub.commit().to_string(),
            original_commit,
            "v{version} Git stub should be updated to the correct commit"
        );
    }

    // Step 8: check should now pass.
    let result = check_apis_up_to_date(env.environment(), &v4)?;
    assert_eq!(
        result,
        CheckResult::Success,
        "check should pass after fixing stale Git stubs"
    );

    Ok(())
}

/// Test that stale Git stub commits are fixed even when duplicate files exist.
///
/// When both a `.json` and `.json.gitstub` exist for the same version (e.g.,
/// from an interrupted conversion), AND the gitstub's commit is stale, both
/// problems should be fixed in a single `generate` invocation.
#[test]
fn test_stale_git_stub_commit_with_duplicate() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;

    // Set up v1-v3, then create a divergent branch for the stale commit.
    let v1_v2_v3 = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;
    let original_commit = env.get_current_commit_hash()?;

    env.create_branch("diverged")?;
    env.checkout_branch("diverged")?;
    env.make_unrelated_commit("divergent work")?;
    let divergent_commit = env.get_current_commit_hash()?;
    env.checkout_branch("main")?;

    env.make_unrelated_commit("advance main")?;

    // Add v4 so v1-v3 become Git stubs.
    let v4 = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v4)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &v4)?;
    assert_eq!(result, CheckResult::Success);

    // Tamper with v1's gitstub to point to the divergent commit (stale).
    let git_stub_path = env
        .find_versioned_git_stub_path("versioned-health", "1.0.0")?
        .expect("v1 Git stub should exist");
    let git_stub_content =
        env.read_versioned_git_stub_raw("versioned-health", "1.0.0")?;
    let path_part = git_stub_content
        .trim()
        .split_once(':')
        .expect("Git stub should contain ':'")
        .1;
    let stale_content = format!("{}:{}\n", divergent_commit, path_part);
    env.create_file(&git_stub_path, &stale_content)?;

    // Also create a duplicate JSON file for v1 (simulating an interrupted
    // conversion).
    let json_content =
        env.read_git_stub_content("versioned-health", "1.0.0")?;
    let json_path = git_stub_path.with_extension("");
    env.create_file(&json_path, &json_content)?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 Git stub should exist"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 duplicate JSON should exist"
    );

    // Check should detect both issues.
    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v4)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should detect both duplicate and stale commit"
    );
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "1.0.0",
                ProblemKind::DuplicateLocalFile,
            ),
            ProblemSummary::new(
                "versioned-health",
                "1.0.0",
                ProblemKind::GitStubCommitStale,
            ),
        ],
    );

    // A single generate should fix both: remove the duplicate AND update the
    // stale commit.
    env.generate_documents(&v4)?;

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 Git stub should still exist"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 duplicate JSON should be removed"
    );

    // The Git stub should now point to the correct commit.
    let git_stub = env.read_versioned_git_stub("versioned-health", "1.0.0")?;
    assert_eq!(
        git_stub.commit().to_string(),
        original_commit,
        "v1 Git stub should be updated to the correct commit"
    );

    // Check should pass without needing a second generate.
    let result = check_apis_up_to_date(env.environment(), &v4)?;
    assert_eq!(
        result,
        CheckResult::Success,
        "check should pass after single generate"
    );

    Ok(())
}

/// Test that git errors during stale commit recomputation are handled
/// gracefully.
///
/// When `git merge-base --is-ancestor` fails (e.g., due to a corrupt or
/// inaccessible object store), `BlessedGitStub::to_git_stub` should propagate
/// the error as an unfixable `GitStubFirstCommitUnknown` problem rather than
/// panicking.
///
/// This exercises the `BlessedGitStub::Known` path where `git_is_ancestor`
/// errors, as opposed to the existing `test_git_error_reports_problem` which
/// exercises the `BlessedGitStub::Lazy` path where `git_first_commit_for_file`
/// errors.
#[test]
fn test_stale_git_stub_is_ancestor_error() -> Result<()> {
    let env = TestEnvironment::new_git()?;

    // Set up v1-v3, commit, then add v4 so v1-v3 become Git stubs (Known
    // variants).
    let v1_v2_v3 = versioned_health_git_stub_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;

    let v4 = versioned_health_with_v4_git_stub_apis()?;
    env.generate_documents(&v4)?;
    env.commit_documents()?;

    // Verify everything is clean before injecting the failure.
    let result = check_apis_up_to_date(env.environment(), &v4)?;
    assert_eq!(result, CheckResult::Success, "check should pass initially");

    // Now swap in fake-git that fails on --is-ancestor. This causes
    // git_is_ancestor to return an error, which propagates through
    // BlessedGitStub::to_git_stub as an Err.
    let fake_git = std::env::var("NEXTEST_BIN_EXE_fake_git")
        .expect("NEXTEST_BIN_EXE_fake_git should be set by nextest");
    let original_git = std::env::var("GIT").ok();

    // SAFETY:
    // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
    unsafe {
        std::env::set_var("GIT", &fake_git);
        std::env::set_var("REAL_GIT", original_git.as_deref().unwrap_or("git"));
        std::env::set_var("FAKE_GIT_FAIL", "is_ancestor");
    }

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v4)?;

    // Should report failures: the git_is_ancestor error propagates as an
    // unfixable GitStubFirstCommitUnknown problem for the non-latest blessed
    // versions (v1-v3) that have BlessedGitStub::Known variants.
    assert_eq!(
        result,
        CheckResult::Failures,
        "check should report failures when git_is_ancestor errors"
    );
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "1.0.0",
                ProblemKind::GitStubFirstCommitUnknown,
            ),
            ProblemSummary::new(
                "versioned-health",
                "2.0.0",
                ProblemKind::GitStubFirstCommitUnknown,
            ),
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::GitStubFirstCommitUnknown,
            ),
        ],
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Blessed version missing local
// ---------------------------------------------------------------------------

/// Verify that `BlessedVersionMissingLocal` is fixable with Git stub storage.
///
/// The restored v3 should be written as a Git stub (not JSON) because it's a
/// non-latest blessed version with Git stub storage enabled.
#[test]
fn test_blessed_version_missing_local_is_fixable_git_stub() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;

    // Step 1: Generate and commit v1 and v2 on main.
    let v2_apis = versioned_health_reduced_git_stub_apis()?;
    env.generate_documents(&v2_apis)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &v2_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Step 2: Add v3 (this triggers Git stub conversion for v1 and v2).
    let v3_apis = versioned_health_git_stub_apis()?;
    env.generate_documents(&v3_apis)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &v3_apis)?;
    assert_eq!(result, CheckResult::Success);

    // v3 is the latest, so it should be JSON. v1 and v2 should be Git stubs.
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should be JSON (latest)"
    );

    let v3_json_path = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 document should exist");

    // --- Part 1: Restore the latest blessed version as JSON. ---
    // With Git stub storage enabled, the latest version should still be
    // restored as JSON (not a Git stub).
    {
        env.create_branch("feature-latest")?;
        env.checkout_branch("feature-latest")?;

        std::fs::remove_file(env.workspace_root().join(&v3_json_path))
            .context("failed to delete blessed v3 file")?;

        let (result, summaries) =
            check_apis_with_summaries(env.environment(), &v3_apis)?;
        assert_eq!(result, CheckResult::NeedsUpdate);
        assert_eq!(
            summaries,
            [ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionMissingLocal,
            )],
        );

        env.generate_documents(&v3_apis)?;

        let result = check_apis_up_to_date(env.environment(), &v3_apis)?;
        assert_eq!(result, CheckResult::Success);

        // v3 is the latest: it should be restored as JSON, not a Git
        // stub, even though Git stub storage is enabled.
        assert!(
            env.versioned_local_document_exists("versioned-health", "3.0.0")?,
            "v3 should be restored as JSON (latest)"
        );
        assert!(
            !env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
            "v3 should not be a Git stub (it's the latest)"
        );

        env.checkout_branch("main")?;
    }

    // --- Part 2: Restore a non-latest blessed version as a Git stub. ---
    env.create_branch("feature")?;
    env.checkout_branch("feature")?;

    std::fs::remove_file(env.workspace_root().join(&v3_json_path))
        .context("failed to delete blessed v3 file")?;

    // This changes v3 in a trivial manner and introduces v4.
    let v4_trivial_apis =
        versioned_health_with_v4_trivial_v3_apis(Storage::GitStub)?;

    // Check should report NeedsUpdate (not Failure).
    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v4_trivial_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionMissingLocal,
            ),
            ProblemSummary::new(
                "versioned-health",
                "4.0.0",
                ProblemKind::LocalVersionMissingLocal,
            ),
            ProblemSummary::for_api(
                "versioned-health",
                ProblemKind::LatestLinkStale,
            ),
        ],
    );

    // Generate should restore v3 as a Git stub (not JSON, since v4 is now
    // latest and Git stub storage is enabled).
    env.generate_documents(&v4_trivial_apis)?;

    let result = check_apis_up_to_date(env.environment(), &v4_trivial_apis)?;
    assert_eq!(result, CheckResult::Success);

    // v3 should now be a Git stub, as a blessed, non-latest version.
    assert!(
        env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should be restored as a Git stub"
    );
    // v4 should be JSON as the latest version.
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON (latest)"
    );

    Ok(())
}

/// Shared setup for the blessed-version-missing-local git stub tests.
///
/// Creates this branch structure with Git stub storage:
/// ```text
/// main: [v1,v2] ── merge(feature1, --no-ff) = M
///          \
///           feature1: [v1,v2,v3] = B (v1,v2 become git stubs)
///                        \
///                         feature2: [v1,v2,v3-trivial,v4] = C
/// ```
///
/// Returns the environment positioned on `feature2`.
fn blessed_version_missing_local_git_stub_setup(
    env: &mut TestEnvironment,
) -> Result<()> {
    let v2_apis = versioned_health_reduced_git_stub_apis()?;
    let v3_apis = versioned_health_git_stub_apis()?;
    let v4_trivial_apis =
        versioned_health_with_v4_trivial_v3_apis(Storage::GitStub)?;

    // Step 1: main has v1 and v2 (both JSON, v2 is latest).
    env.generate_documents(&v2_apis)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &v2_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Step 2: feature1 adds v3. v1 and v2 become Git stubs.
    env.create_branch("feature1")?;
    env.checkout_branch("feature1")?;

    env.generate_documents(&v3_apis)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &v3_apis)?;
    assert_eq!(result, CheckResult::Success);

    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub on feature1"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub on feature1"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should be JSON on feature1 (latest)"
    );

    // Step 3: feature2 (from feature1) adds v4 with a trivially modified v3.
    env.create_branch("feature2")?;
    env.checkout_branch("feature2")?;

    env.generate_documents(&v4_trivial_apis)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &v4_trivial_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Step 4: merge feature1 into main, making v3 blessed.
    env.checkout_branch("main")?;
    env.merge_branch_without_renames("feature1")?;

    let result = check_apis_up_to_date(env.environment(), &v3_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Return to feature2.
    env.checkout_branch("feature2")?;

    Ok(())
}

fn blessed_version_missing_local_git_stub_verify(
    env: &TestEnvironment,
) -> Result<()> {
    let v4_trivial_apis =
        versioned_health_with_v4_trivial_v3_apis(Storage::GitStub)?;

    // v1, v2, v3 should all be Git stubs (blessed, not latest).
    assert!(
        env.versioned_git_stub_exists("versioned-health", "1.0.0")?,
        "v1 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "2.0.0")?,
        "v2 should be a Git stub"
    );
    assert!(
        env.versioned_git_stub_exists("versioned-health", "3.0.0")?,
        "v3 should be a Git stub (blessed, not latest)"
    );
    // v4 should be JSON (latest).
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON (latest)"
    );
    assert!(
        !env.versioned_git_stub_exists("versioned-health", "4.0.0")?,
        "v4 should not be a Git stub (it's the latest)"
    );

    let result = check_apis_up_to_date(env.environment(), &v4_trivial_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Rebase test: blessed version missing local with Git stub storage.
#[test]
fn test_rebase_blessed_version_missing_local_git_stub() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    blessed_version_missing_local_git_stub_setup(&mut env)?;

    let v4_trivial_apis =
        versioned_health_with_v4_trivial_v3_apis(Storage::GitStub)?;

    let rebase_result = env.try_rebase_onto("main")?;
    assert_eq!(rebase_result, RebaseResult::Clean);

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v4_trivial_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionMissingLocal,
            ),
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionExtraLocalSpec,
            ),
        ],
    );

    env.generate_documents(&v4_trivial_apis)?;

    blessed_version_missing_local_git_stub_verify(&env)
}

/// Merge test: blessed version missing local with Git stub storage.
#[test]
fn test_merge_blessed_version_missing_local_git_stub() -> Result<()> {
    let mut env = TestEnvironment::new_git()?;
    blessed_version_missing_local_git_stub_setup(&mut env)?;

    let v4_trivial_apis =
        versioned_health_with_v4_trivial_v3_apis(Storage::GitStub)?;

    let merge_result = env.try_merge_branch("main")?;
    assert_eq!(merge_result, MergeResult::Clean);

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v4_trivial_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionMissingLocal,
            ),
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionExtraLocalSpec,
            ),
        ],
    );

    env.generate_documents(&v4_trivial_apis)?;

    blessed_version_missing_local_git_stub_verify(&env)
}

/// jj rebase variant: blessed version missing local with Git stub storage.
#[test]
fn test_jj_rebase_blessed_version_missing_local_git_stub() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    blessed_version_missing_local_git_stub_setup(&mut env)?;

    let v4_trivial_apis =
        versioned_health_with_v4_trivial_v3_apis(Storage::GitStub)?;

    let rebase_result = env.jj_try_rebase("feature2", "main")?;
    assert_eq!(rebase_result, JjRebaseResult::Clean);

    env.jj_new("feature2")?;

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v4_trivial_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionMissingLocal,
            ),
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionExtraLocalSpec,
            ),
        ],
    );

    env.generate_documents(&v4_trivial_apis)?;

    blessed_version_missing_local_git_stub_verify(&env)
}

/// jj merge variant: blessed version missing local with Git stub storage.
#[test]
fn test_jj_merge_blessed_version_missing_local_git_stub() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let mut env = TestEnvironment::new_jj()?;
    blessed_version_missing_local_git_stub_setup(&mut env)?;

    let v4_trivial_apis =
        versioned_health_with_v4_trivial_v3_apis(Storage::GitStub)?;

    let merge_result =
        env.jj_try_merge("feature2", "main", "Merge main into feature2")?;
    assert_eq!(merge_result, JjMergeResult::Clean);

    let (result, summaries) =
        check_apis_with_summaries(env.environment(), &v4_trivial_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);
    assert_eq!(
        summaries,
        [
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionMissingLocal,
            ),
            ProblemSummary::new(
                "versioned-health",
                "3.0.0",
                ProblemKind::BlessedVersionExtraLocalSpec,
            ),
        ],
    );

    env.generate_documents(&v4_trivial_apis)?;

    blessed_version_missing_local_git_stub_verify(&env)
}
