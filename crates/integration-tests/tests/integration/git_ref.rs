// Copyright 2026 Oxide Computer Company

//! Tests for git ref storage of blessed API versions.
//!
//! When git ref storage is enabled, older (non-latest) blessed API versions are
//! stored as `.gitref` files containing a git reference (`commit:path`) instead
//! of full JSON files. The content is retrieved via `git cat-file blob` at
//! runtime.

use anyhow::Result;
use camino::Utf8PathBuf;
use dropshot_api_manager::{
    GitRef, ManagedApis,
    test_util::{CheckResult, check_apis_up_to_date},
};
use integration_tests::*;
use std::collections::{BTreeMap, BTreeSet};

/// The kind of conflict expected on a file during merge/rebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedConflictKind {
    /// A rename/rename conflict.
    ///
    /// Git's rename detection sees both branches "renaming" the same source
    /// file to different destinations. jj does not have rename detection, so
    /// this conflict kind only applies to git.
    Rename,
    /// A symlink conflict.
    ///
    /// Both branches update a symlink to point to different targets. Both git
    /// and jj detect this as a conflict.
    Symlink,
}

/// Extract all conflicted file paths from the expected conflicts map.
///
/// Used by git tests which detect all conflict types.
fn all_conflict_paths(
    conflicts: &BTreeMap<Utf8PathBuf, ExpectedConflictKind>,
) -> BTreeSet<Utf8PathBuf> {
    conflicts.keys().cloned().collect()
}

/// Extract only non-rename conflicted file paths from the expected conflicts
/// map.
///
/// Used by jj tests since jj does not have rename detection.
fn jj_conflict_paths(
    conflicts: &BTreeMap<Utf8PathBuf, ExpectedConflictKind>,
) -> BTreeSet<Utf8PathBuf> {
    conflicts
        .iter()
        .filter(|(_, kind)| **kind != ExpectedConflictKind::Rename)
        .map(|(path, _)| path.clone())
        .collect()
}

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
/// This is the reverse of `test_conversion`. When a user disables git ref
/// storage, existing git ref files should be converted back to full JSON files.
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

/// Test behavior when running in a shallow clone where git refs point to
/// commits whose objects are not available.
///
/// This simulates the scenario where:
///
/// 1. Git ref storage is set up and committed to main.
/// 2. CI does a shallow clone (`git clone --depth 1`).
/// 3. CI runs `check` and the git refs can't be resolved because the commits
///    they reference are outside the shallow boundary.
#[test]
fn test_shallow_clone_with_git_refs() -> Result<()> {
    let env = TestEnvironment::new()?;

    let v1_v2_v3 = versioned_health_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;

    env.make_unrelated_commit("intermediate")?;

    let v4 = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&v4)?;
    env.commit_documents()?;

    assert!(env.versioned_git_ref_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_git_ref_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_git_ref_exists("versioned-health", "3.0.0")?);

    let shallow_env = env.shallow_clone(1)?;

    assert!(
        shallow_env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "git ref file should exist in shallow clone"
    );

    // Check should fail early.
    let result = check_apis_up_to_date(shallow_env.environment(), &v4);
    result.expect_err("check should fail in shallow clone with git refs");

    Ok(())
}

/// Test that shallow clones work fine when git ref storage is not enabled.
#[test]
fn test_shallow_clone_without_git_refs() -> Result<()> {
    let env = TestEnvironment::new()?;

    // Use APIs without git ref storage.
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

    // Check should succeed since we're not using git ref storage.
    let result = check_apis_up_to_date(shallow_env.environment(), &v1_v2_v3)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that git ref files don't cause merge conflicts when two branches with
/// different merge bases both convert the same API version to a git ref.
///
/// See [`no_conflict_setup`] for the test scenario.
#[test]
fn test_no_merge_conflict() -> Result<()> {
    let env = TestEnvironment::new()?;
    let v1_v2_commit = no_conflict_setup(&env)?;

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
    let env = TestEnvironment::new()?;
    let v1_v2_commit = no_conflict_setup(&env)?;

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
/// This is the jj equivalent of [`test_no_merge_conflict`]. Uses git for setup,
/// jj for merge.
#[test]
fn test_jj_no_merge_conflict() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let env = TestEnvironment::new()?;
    let v1_v2_commit = no_conflict_setup(&env)?;
    env.jj_init()?;

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
/// This is the jj equivalent of [`test_no_rebase_conflict`]. Uses git for
/// setup, jj for rebase.
#[test]
fn test_jj_no_rebase_conflict() -> Result<()> {
    if !check_jj_available()? {
        return Ok(());
    }

    let env = TestEnvironment::new()?;
    let v1_v2_commit = no_conflict_setup(&env)?;
    env.jj_init()?;

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
/// Git ref files store `<commit-hash>:<path>` where `<commit-hash>` is "the
/// first commit when the file was most recently introduced." This is a
/// deterministic property of the file's history, independent of which branch
/// you're on.
///
/// ```text
/// History:
///     main: [initial] -- [v1,v2 added] -- [unrelated A] -- [unrelated B]
///                               |                 |
///                               |                 +-- branch_b: [add v3]
///                               |                         (v1,v2 become git refs)
///                               |
///                               +-- branch_a: [add v3]
///                                       (v1,v2 become git refs)
/// ```
///
/// Both branches add the same v3, so:
/// - The v1 and v2 git refs should have identical content on both branches.
/// - The merge/rebase should succeed without conflict.
///
/// Returns the commit hash where v1/v2 were added. Leaves the environment on
/// branch_b.
fn no_conflict_setup(env: &TestEnvironment) -> Result<String> {
    let v1_v2_apis = versioned_health_reduced_git_ref_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash_full()?;

    env.make_unrelated_commit("unrelated A")?;
    env.create_branch("branch_a")?;

    // Give branch_a and branch_b different merge bases.
    env.make_unrelated_commit("unrelated B")?;
    env.create_branch("branch_b")?;

    env.checkout_branch("branch_a")?;
    let v1_v2_v3_apis = versioned_health_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;

    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should be a git ref on branch_a"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should be a git ref on branch_a"
    );

    let v1_ref_branch_a =
        env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v2_ref_branch_a =
        env.read_versioned_git_ref("versioned-health", "2.0.0")?;

    env.checkout_branch("branch_b")?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;

    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should be a git ref on branch_b"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should be a git ref on branch_b"
    );

    let v1_ref_branch_b =
        env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v2_ref_branch_b =
        env.read_versioned_git_ref("versioned-health", "2.0.0")?;

    // Git refs should be identical: both point to v1_v2_commit.
    assert_eq!(
        v1_ref_branch_a, v1_ref_branch_b,
        "v1 git refs should be identical on both branches"
    );
    assert_eq!(
        v2_ref_branch_a, v2_ref_branch_b,
        "v2 git refs should be identical on both branches"
    );

    let v1_commit = v1_ref_branch_a.trim().split(':').next().unwrap();
    assert_eq!(
        v1_commit, v1_v2_commit,
        "git ref should point to the original commit"
    );

    Ok(v1_v2_commit)
}

/// Verify the result of [`test_no_merge_conflict`] or
/// [`test_no_rebase_conflict`].
fn no_conflict_verify(env: &TestEnvironment, v1_v2_commit: &str) -> Result<()> {
    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 git ref should exist"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 git ref should exist"
    );

    let v1_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_ref_commit = v1_ref.trim().split(':').next().unwrap();
    assert_eq!(
        v1_ref_commit, v1_v2_commit,
        "v1 git ref should point to the original commit"
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
    let env = TestEnvironment::new()?;
    let (v1_v2_commit, expected_conflicts) = rename_conflict_v3_v4_setup(&env)?;

    let merge_result = env.try_merge_branch("branch_a")?;
    let MergeResult::Conflict(conflicted_files) = merge_result else {
        panic!("merge should have conflicts due to rename/rename detection");
    };
    assert_eq!(
        conflicted_files,
        all_conflict_paths(&expected_conflicts),
        "conflicted files should match expected"
    );

    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_ref_apis()?;
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
    let env = TestEnvironment::new()?;
    let (v1_v2_commit, expected_conflicts) = rename_conflict_v3_v4_setup(&env)?;

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

    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_ref_apis()?;
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

    let env = TestEnvironment::new()?;
    let (v1_v2_commit, expected_conflicts) = rename_conflict_v3_v4_setup(&env)?;
    env.jj_init()?;

    let merge_result = env.jj_try_merge("branch_a", "branch_b", "merge")?;
    let JjMergeResult::Conflict(conflicted_files) = merge_result else {
        panic!("jj merge should have symlink conflict; got clean merge");
    };
    assert_eq!(
        conflicted_files,
        jj_conflict_paths(&expected_conflicts),
        "jj should only conflict on symlink, not rename-related files"
    );

    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_ref_apis()?;
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

    let env = TestEnvironment::new()?;
    let (v1_v2_commit, expected_conflicts) = rename_conflict_v3_v4_setup(&env)?;
    env.jj_init()?;

    let rebase_result = env.jj_try_rebase("branch_a", "branch_b")?;
    let JjRebaseResult::Conflict(conflicted_files) = rebase_result else {
        panic!("jj rebase should have symlink conflict; got clean rebase");
    };
    assert_eq!(
        conflicted_files,
        jj_conflict_paths(&expected_conflicts),
        "jj should only conflict on symlink, not rename-related files"
    );

    let v1_v2_v3_v4_apis = versioned_health_with_v4_git_ref_apis()?;
    env.jj_resolve_conflicts(&v1_v2_v3_v4_apis)?;

    rename_conflict_v3_v4_verify(&env, &v1_v2_commit, &v1_v2_v3_v4_apis)
}

/// Setup for [`test_rename_conflict_resolved_by_generate`] and
/// [`test_rebase_rename_conflict_resolved_by_generate`].
///
/// When Git's rename detection is active, it can misinterpret the deletion of
/// an old version (converted to git ref) and creation of a new version as a
/// "rename". If two branches both do this with different new versions, Git
/// reports a rename/rename conflict.
///
/// ```text
/// History:
///     main: [initial] -- [v1,v2 added] -- [unrelated A] -- [unrelated B]
///                               |                 |
///                               |                 +-- branch_b: [add v4]
///                               |                         (v1,v2 become git refs)
///                               |
///                               +-- branch_a: [add v3]
///                                       (v1,v2 become git refs)
/// ```
///
/// Git sees both branches "renaming" v2.json to different files (v3.json vs
/// v4.json), causing a rename/rename conflict.
///
/// Returns the commit hash where v1/v2 were added and the expected conflicted
/// files with their conflict kinds. Leaves the environment on branch_b.
fn rename_conflict_v3_v4_setup(
    env: &TestEnvironment,
) -> Result<(String, BTreeMap<Utf8PathBuf, ExpectedConflictKind>)> {
    let v1_v2_apis = versioned_health_reduced_git_ref_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash_full()?;

    env.make_unrelated_commit("unrelated A")?;
    env.create_branch("branch_a")?;

    env.make_unrelated_commit("unrelated B")?;
    env.create_branch("branch_b")?;

    // Capture paths before git ref conversion for conflict tracking.
    let v2_json_path = env
        .find_versioned_document_path("versioned-health", "2.0.0")?
        .expect("v2 should exist as JSON before branching");

    env.checkout_branch("branch_a")?;
    let v1_v2_v3_apis = versioned_health_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    let v3_json_path = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 should exist as JSON on branch_a");
    env.commit_documents()?;

    env.checkout_branch("branch_b")?;
    let v1_v2_v4_apis = versioned_health_v1_v2_v4_git_ref_apis()?;
    env.generate_documents(&v1_v2_v4_apis)?;
    let v4_json_path = env
        .find_versioned_document_path("versioned-health", "4.0.0")?
        .expect("v4 should exist as JSON on branch_b");
    env.commit_documents()?;

    // Git's rename detection sees v2.json "renamed" to v3/v4 on different
    // branches. jj has no rename detection, so only the symlink conflicts.
    let latest_symlink: Utf8PathBuf =
        "documents/versioned-health/versioned-health-latest.json".into();
    let expected_conflicts: BTreeMap<Utf8PathBuf, ExpectedConflictKind> = [
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
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should be a git ref"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should be a git ref"
    );

    let v1_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_ref_commit = v1_ref.trim().split(':').next().unwrap();
    assert_eq!(
        v1_ref_commit, v1_v2_commit,
        "v1 git ref should point to the original commit"
    );

    // v3 is locally added (not blessed), so it should be JSON.
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should exist as JSON (locally added)"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should not be a git ref (locally added, not blessed)"
    );

    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON (latest)"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "4.0.0")?,
        "v4 should not be a git ref"
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
    let env = TestEnvironment::new()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&env)?;

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
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_ref_apis()?;
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
    let env = TestEnvironment::new()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&env)?;

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
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_ref_apis()?;
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

    let env = TestEnvironment::new()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&env)?;
    env.jj_init()?;

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
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_ref_apis()?;
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

    let env = TestEnvironment::new()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&env)?;
    env.jj_init()?;

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
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_ref_apis()?;
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
    let env = TestEnvironment::new()?;
    let (v1_v2_commit, expected_conflicts) =
        rename_conflict_blessed_setup(&env)?;

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
    let v1_v2_v3_v4alt_apis = versioned_health_v1_v2_v3_v4alt_git_ref_apis()?;
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
/// 3. Both branches convert v2 to git ref when adding v3
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
    env: &TestEnvironment,
) -> Result<(String, BTreeMap<Utf8PathBuf, ExpectedConflictKind>)> {
    let v1_v2_apis = versioned_health_reduced_git_ref_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash_full()?;

    env.create_branch("branch_b")?;

    // Capture paths before git ref conversion for conflict tracking.
    let v2_json_path = env
        .find_versioned_document_path("versioned-health", "2.0.0")?
        .expect("v2 should exist as JSON before generating v3");

    // On main: add v3 (standard).
    let v1_v2_v3_apis = versioned_health_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    let v3_json_path_main = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 should exist as JSON on main");
    env.commit_documents()?;

    // On branch_b: add v3-alternate (different content, different hash).
    env.checkout_branch("branch_b")?;
    let v3_alt_apis = versioned_health_v3_alternate_git_ref_apis()?;
    env.generate_documents(&v3_alt_apis)?;
    let v3_json_path_b = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("v3 should exist as JSON on branch_b");
    env.commit_documents()?;

    // Git's rename detection sees v2.json "renamed" to v3 on both branches
    // (different destinations). jj has no rename detection.
    let latest_symlink: Utf8PathBuf =
        "documents/versioned-health/versioned-health-latest.json".into();
    let expected_conflicts: BTreeMap<Utf8PathBuf, ExpectedConflictKind> = [
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
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should be a git ref"
    );
    assert!(
        env.versioned_git_ref_exists("versioned-health", "2.0.0")?,
        "v2 should be a git ref"
    );

    let v1_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_ref_commit = v1_ref.trim().split(':').next().unwrap();
    assert_eq!(
        v1_ref_commit, v1_v2_commit,
        "v1 git ref should point to the original commit"
    );

    // v3 is blessed (from main) and not latest.
    assert!(
        env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should be a git ref (blessed from main, not latest)"
    );

    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON (latest)"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "4.0.0")?,
        "v4 should not be a git ref"
    );

    let result = check_apis_up_to_date(env.environment(), v1_v2_v3_v4alt_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}
