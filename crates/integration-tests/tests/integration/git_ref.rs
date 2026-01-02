// Copyright 2025 Oxide Computer Company

//! Tests for git ref storage of blessed API versions.
//!
//! When git ref storage is enabled, older (non-latest) blessed API versions are
//! stored as `.gitref` files containing a git reference (`commit:path`) instead
//! of full JSON files. The content is retrieved via `git cat-file blob` at
//! runtime.

use anyhow::Result;
use dropshot_api_manager::{
    GitRef,
    test_util::{CheckResult, check_apis_up_to_date},
};
use integration_tests::*;

/// Test that git ref conversion happens when adding a new version, and that
/// the content is preserved correctly.
///
/// When a new version is added to an API with git ref storage enabled, the
/// older blessed versions should be converted from full JSON files to git ref
/// files. The git refs should point to the first commit where each version was
/// introduced.
#[test]
fn test_conversion() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = versioned_health_git_ref_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;
    let first_commit = env.get_current_commit_hash_full()?;
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
        !env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should not yet be a git ref"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should not yet be a git ref"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should not yet be a git ref"
    );

    env.make_unrelated_commit("unrelated change 1")?;
    env.make_unrelated_commit("unrelated change 2")?;
    let current_commit = env.get_current_commit_hash_full()?;
    assert_ne!(
        first_commit, current_commit,
        "current commit should have advanced past the first commit"
    );

    // v3 (the previous latest) should be converted to a git ref in the same
    // operation that creates v4.
    let extended_apis = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&extended_apis)?;

    assert!(
        !env.is_file_committed("versioned-health/versioned-health-4.0.0-")?,
        "v4 should not be committed yet"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should now be a git ref"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should now be a git ref"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should now be a git ref"
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
        !env.versioned_git_ref_exists("versioned-health", "4.0.0")?,
        "v4 should not be a git ref"
    );

    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        let git_ref_content =
            env.read_versioned_git_ref("versioned-health", version)?;
        let commit = git_ref_content.trim().split(':').next().unwrap();
        assert_eq!(
            commit, first_commit,
            "git ref for v{} should point to the first commit ({}) \
             (current commit: {})",
            version, first_commit, current_commit
        );
    }

    let git_ref_content =
        env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    assert!(
        git_ref_content.contains(':'),
        "git ref should contain a colon separator"
    );
    let git_ref = git_ref_content
        .parse::<GitRef>()
        .expect("git ref should parse correctly");
    assert!(
        git_ref
            .path
            .as_str()
            .starts_with("documents/versioned-health/versioned-health-1.0.0-"),
        "path {} should start with `documents/versioned-health/versioned-health-1.0.0-`",
        git_ref.path.as_str(),
    );
    assert_eq!(
        git_ref.path.extension(),
        Some("json"),
        "path {} should have extension `json`",
        git_ref.path.as_str(),
    );

    let git_ref_v1_content =
        env.read_git_ref_content("versioned-health", "1.0.0")?;
    assert_eq!(
        original_v1, git_ref_v1_content,
        "git ref content should match original"
    );

    let result = check_apis_up_to_date(env.environment(), &extended_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that the latest version and versions sharing its first commit are not
/// converted to git refs.
///
/// When multiple versions share the same first commit as the latest, no
/// conversion should happen. We don't want check to fail immediately after
/// multiple versions were added in a single commit -- that is a poor user
/// experience.
#[test]
fn test_same_first_commit_no_conversion() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = versioned_health_git_ref_apis()?;

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
        !env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should not be a git ref"
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
        !env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should not be a git ref"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should not be a git ref"
    );

    Ok(())
}

/// Test that lockstep APIs are never converted to git ref files.
#[test]
fn test_lockstep_never_converted_to_git_ref() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = lockstep_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;
    env.generate_documents(&apis)?;

    assert!(
        env.lockstep_document_exists("health"),
        "lockstep document should exist"
    );
    assert!(
        !env.lockstep_git_ref_exists("health"),
        "lockstep should never be a git ref"
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
    let env = TestEnvironment::new()?;

    let v1_v2_no_git_ref = versioned_health_reduced_apis()?;
    env.generate_documents(&v1_v2_no_git_ref)?;
    env.commit_documents()?;
    let first_commit = env.get_current_commit_hash_full()?;

    let v1_v2_v3_no_git_ref = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3_no_git_ref)?;
    env.commit_documents()?;
    let second_commit = env.get_current_commit_hash_full()?;
    assert_ne!(first_commit, second_commit);

    assert!(env.versioned_local_document_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);

    // v3 is latest (from second_commit) while v1, v2 are from first_commit.
    let v1_v2_v3_git_ref = versioned_health_git_ref_apis()?;
    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_git_ref)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should suggest converting v1, v2 (different first commit)"
    );

    env.generate_documents(&v1_v2_v3_git_ref)?;

    assert!(env.versioned_git_ref_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_git_ref_exists("versioned-health", "2.0.0")?);

    let v1_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_commit = v1_ref.trim().split(':').next().unwrap();
    assert_eq!(v1_commit, first_commit);

    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);
    assert!(!env.versioned_git_ref_exists("versioned-health", "3.0.0")?);

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_git_ref)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that git refs work correctly after reloading from a different state.
///
/// This also tests that versions introduced in different commits have git refs
/// pointing to their respective first commits.
#[test]
fn test_git_ref_check_after_conversion() -> Result<()> {
    let env = TestEnvironment::new()?;

    let v1_v2_apis = versioned_health_reduced_git_ref_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash_full()?;

    env.make_unrelated_commit("between v2 and v3")?;

    let v1_v2_v3_apis = versioned_health_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;
    let v3_commit = env.get_current_commit_hash_full()?;
    assert_ne!(v1_v2_commit, v3_commit, "v1/v2 and v3 in different commits");

    env.make_unrelated_commit("after v3")?;

    let extended_apis = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&extended_apis)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &extended_apis)?;
    assert_eq!(result, CheckResult::Success);

    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should be a git ref"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should be a git ref"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should be a git ref"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should be JSON"
    );

    let v1_git_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_commit = v1_git_ref.trim().split(':').next().unwrap();
    assert_eq!(
        v1_commit, v1_v2_commit,
        "v1 git ref should point to the commit where v1 was first introduced"
    );

    let v2_git_ref = env.read_versioned_git_ref("versioned-health", "2.0.0")?;
    let v2_commit = v2_git_ref.trim().split(':').next().unwrap();
    assert_eq!(
        v2_commit, v1_v2_commit,
        "v2 git ref should point to the commit where v2 was first introduced"
    );

    let v3_git_ref = env.read_versioned_git_ref("versioned-health", "3.0.0")?;
    let v3_commit_from_git_ref = v3_git_ref.trim().split(':').next().unwrap();
    assert_eq!(
        v3_commit_from_git_ref, v3_commit,
        "v3 git ref should point to the commit where v3 was first introduced"
    );

    assert_ne!(
        v1_commit, v3_commit_from_git_ref,
        "v1 and v3 should point to different commits"
    );

    Ok(())
}

/// Test that without git ref storage enabled, no conversion happens.
#[test]
fn test_no_conversion_without_git_ref_enabled() -> Result<()> {
    let env = TestEnvironment::new()?;
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
        !env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should not be a git ref"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should not be a git ref"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should not be a git ref"
    );

    Ok(())
}

/// Test that git ref files are converted back to JSON when git ref storage is
/// disabled, with content preservation.
///
/// This is the reverse of `test_conversion`. When a user
/// disables git ref storage (by removing `use_git_ref_storage()` from their API
/// config), existing git ref files should be converted back to full JSON files.
#[test]
fn test_convert_to_json_when_disabled() -> Result<()> {
    let env = TestEnvironment::new()?;

    let apis_with_git_ref = versioned_health_git_ref_apis()?;
    env.generate_documents(&apis_with_git_ref)?;
    env.commit_documents()?;

    let original_v1 =
        env.read_versioned_document("versioned-health", "1.0.0")?;
    let original_v2 =
        env.read_versioned_document("versioned-health", "2.0.0")?;

    let extended_with_git_ref = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&extended_with_git_ref)?;

    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should be a git ref"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should be a git ref"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should be a git ref"
    );

    let extended_without_git_ref = versioned_health_with_v4_apis()?;
    let result =
        check_apis_up_to_date(env.environment(), &extended_without_git_ref)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update when git refs exist but git ref \
         storage is disabled"
    );

    env.generate_documents(&extended_without_git_ref)?;

    assert!(
        !env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 git ref should be removed"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 git ref should be removed"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 git ref should be removed"
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
        "v1 content should match original after git ref-to-JSON conversion"
    );
    assert_eq!(
        original_v2, restored_v2,
        "v2 content should match original after git ref-to-JSON conversion"
    );

    Ok(())
}

/// Test that duplicate git ref and JSON files are handled correctly.
///
/// When both git ref and JSON exist for the same version, the system should:
///
/// - With git ref enabled: delete the JSON (git ref preferred for non-latest)
/// - With git ref disabled: delete the git ref (JSON preferred)
///
/// This can happen from interrupted conversions, manual file manipulation,
/// or merge conflicts.
#[test]
fn test_duplicates() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = versioned_health_git_ref_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;

    let extended = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&extended)?;

    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should be a git ref"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should not have a JSON file"
    );

    // Manually create a duplicate JSON file for v1.
    let json_content = env.read_git_ref_content("versioned-health", "1.0.0")?;
    let git_ref_path = env
        .find_versioned_git_ref_path("versioned-health", "1.0.0")?
        .expect("git ref should exist");
    let json_path = git_ref_path.with_extension("");
    env.create_file(&json_path, &json_content)?;

    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "git ref should still exist"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "duplicate JSON should exist"
    );

    let result = check_apis_up_to_date(env.environment(), &extended)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update when duplicate files exist"
    );

    env.generate_documents(&extended)?;

    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "git ref should still exist after generate"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "duplicate JSON should be deleted"
    );

    // Now test with git refs disabled.
    env.create_file(&json_path, &json_content)?;
    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "git ref should exist"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "JSON should exist"
    );

    let extended_no_git_ref = versioned_health_with_v4_apis()?;
    env.generate_documents(&extended_no_git_ref)?;

    assert!(
        !env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        ".gitref should be deleted"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "JSON should remain"
    );

    Ok(())
}

/// Test that git ref points to the most recent addition when a version is
/// removed and re-added.
///
/// Consider the situation where:
///
/// - commit 1 adds API v1
/// - commit 2 removes API v1 (by dropping it from the list of supported versions)
/// - commit 3 re-adds API v1
///
/// When git ref storage is enabled and v1 is converted to a git ref, the ref
/// should point to commit 3 (the most recent addition), not commit 1 (the
/// original addition). This ensures the git ref points to the current version
/// of the file.
#[test]
fn test_remove_readd() -> Result<()> {
    let env = TestEnvironment::new()?;

    let v1_v2_v3 = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;
    let commit_1 = env.get_current_commit_hash_full()?;

    let v1_path_before = env
        .find_versioned_document_path("versioned-health", "1.0.0")?
        .expect("v1 should exist after commit 1");

    let v2_v3_only = versioned_health_no_v1_apis()?;
    env.generate_documents(&v2_v3_only)?;
    env.commit_documents()?;
    let commit_2 = env.get_current_commit_hash_full()?;

    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should be removed after commit 2"
    );

    let v1_v2_v3_again = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3_again)?;
    env.commit_documents()?;
    let commit_3 = env.get_current_commit_hash_full()?;

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

    let v4_with_git_ref = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&v4_with_git_ref)?;

    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should be converted to a git ref"
    );

    // The git ref for v1 should point to commit 3 (the re-addition), not
    // commit 1 (the original addition).
    let v1_git_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_commit = v1_git_ref.trim().split(':').next().unwrap();

    assert_eq!(
        v1_commit, commit_3,
        "v1 git ref should point to the re-addition commit (commit 3: {}), \
         not the original addition (commit 1: {})",
        commit_3, commit_1
    );

    let v2_git_ref = env.read_versioned_git_ref("versioned-health", "2.0.0")?;
    let v2_commit = v2_git_ref.trim().split(':').next().unwrap();
    assert_eq!(
        v2_commit, commit_1,
        "v2 git ref should point to commit 1 (never removed)"
    );

    let v3_git_ref = env.read_versioned_git_ref("versioned-health", "3.0.0")?;
    let v3_commit = v3_git_ref.trim().split(':').next().unwrap();
    assert_eq!(
        v3_commit, commit_1,
        "v3 git ref should point to commit 1 (never removed)"
    );

    Ok(())
}

/// Test that when the latest version is removed, the previous-latest version
/// is converted from a git ref back to a JSON file.
#[test]
fn test_latest_removed() -> Result<()> {
    let env = TestEnvironment::new()?;

    let v1_v2 = versioned_health_reduced_git_ref_apis()?;
    env.generate_documents(&v1_v2)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash_full()?;

    env.make_unrelated_commit("between v2 and v3")?;

    let v1_v2_v3 = versioned_health_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3)?;

    assert!(env.versioned_git_ref_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_git_ref_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);

    env.commit_documents()?;
    let v3_commit = env.get_current_commit_hash_full()?;
    assert_ne!(v1_v2_commit, v3_commit);

    env.make_unrelated_commit("between v3 and v4")?;

    let v1_v2_v3_v4 = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3_v4)?;

    assert!(env.versioned_git_ref_exists("versioned-health", "3.0.0")?);
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
        !env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should not be a git ref anymore"
    );

    // v1, v2 should remain as git refs (different first commit from v3).
    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should remain as a git ref"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should remain as a git ref"
    );

    let v1_git_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_commit_from_ref = v1_git_ref.trim().split(':').next().unwrap();
    assert_eq!(v1_commit_from_ref, v1_v2_commit);

    let v2_git_ref = env.read_versioned_git_ref("versioned-health", "2.0.0")?;
    let v2_commit_from_ref = v2_git_ref.trim().split(':').next().unwrap();
    assert_eq!(v2_commit_from_ref, v1_v2_commit);

    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that when the latest version is removed, versions that share the same
/// first commit as the new latest are also converted from git refs to JSON.
#[test]
fn test_latest_removed_same_commit() -> Result<()> {
    let env = TestEnvironment::new()?;

    let v1_only = versioned_health_v1_only_apis()?;
    env.generate_documents(&v1_only)?;
    env.commit_documents()?;
    let v1_commit = env.get_current_commit_hash_full()?;

    env.make_unrelated_commit("between v1 and v2/v3")?;

    // Add v2, v3 in the same generate call (they share the same first commit).
    let v1_v2_v3 = versioned_health_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3)?;

    assert!(env.versioned_git_ref_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);

    env.commit_documents()?;
    let v2_v3_commit = env.get_current_commit_hash_full()?;
    assert_ne!(v1_commit, v2_v3_commit);

    env.make_unrelated_commit("between v3 and v4")?;

    let v1_v2_v3_v4 = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3_v4)?;

    assert!(env.versioned_git_ref_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_git_ref_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_git_ref_exists("versioned-health", "3.0.0")?);
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
        !env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should not be a git ref anymore"
    );

    // v2 was introduced in the same commit as v3 (the new latest), so it should
    // also be converted back to JSON.
    assert!(
        env.versioned_local_document_exists("versioned-health", "2.0.0")?,
        "v2 should be JSON (same first commit as new latest v3)"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should not be a git ref anymore"
    );

    // v1 should remain as a git ref (different first commit from v3).
    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should remain as a git ref"
    );

    let v1_git_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_commit_from_ref = v1_git_ref.trim().split(':').next().unwrap();
    assert_eq!(v1_commit_from_ref, v1_commit);

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
    let env = TestEnvironment::new()?;

    let apis = versioned_health_git_ref_apis()?;
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
    }

    let v4_apis = versioned_health_with_v4_git_ref_apis()?;
    let result = check_apis_up_to_date(env.environment(), &v4_apis)?;

    // Should report a failure due to the unfixable GitRefFirstCommitUnknown
    // problem.
    assert_eq!(result, CheckResult::Failures);

    Ok(())
}

/// Test behavior when running in a shallow clone where the commit that added
/// an API version is not accessible.
///
/// In a shallow clone (e.g., `git clone --depth 1`), the commit history is
/// truncated. If a blessed API version was added in a commit before the
/// shallow boundary, `git log --diff-filter=A` can't find the actual commit
/// where the file was added.
///
/// Note that Git doesn't return an error in this case. Instead, it returns the
/// wrong commit (the shallow boundary commit), because git treats boundary
/// commits as having no parent, making any files present in the boundary appear
/// to have been "added" there.
///
/// This test documents this behavior and verifies that the tool:
///
/// 1. Successfully creates git refs (no error).
/// 2. But the git refs point to the shallow boundary, not the actual first
///    commit.
#[test]
fn test_shallow_clone_creates_incorrect_git_refs() -> Result<()> {
    let env = TestEnvironment::new()?;

    let v1_v2_v3 = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;
    let first_commit = env.get_current_commit_hash_full()?;

    env.make_unrelated_commit("intermediate commit")?;
    let second_commit = env.get_current_commit_hash_full()?;

    assert_ne!(first_commit, second_commit);

    // Simulate a shallow clone: first_commit is no longer accessible.
    env.make_shallow(&second_commit)?;

    let v4_git_ref = versioned_health_with_v4_git_ref_apis()?;
    let result = check_apis_up_to_date(env.environment(), &v4_git_ref)?;
    assert_eq!(result, CheckResult::NeedsUpdate);
    env.generate_documents(&v4_git_ref)?;

    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 git ref created"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 git ref created"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 git ref created"
    );

    // Git refs point to the wrong commit (the shallow boundary instead of
    // the actual first commit).
    let v1_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_ref_commit = v1_ref.trim().split(':').next().unwrap();

    assert_eq!(
        v1_ref_commit, second_commit,
        "In shallow clone, git ref points to shallow boundary"
    );

    Ok(())
}
