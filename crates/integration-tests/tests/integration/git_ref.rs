// Copyright 2025 Oxide Computer Company

//! Tests for git ref storage of blessed API versions.
//!
//! When git ref storage is enabled, older (non-latest) blessed API versions are
//! stored as `.gitref` files containing a git reference (`commit:path`) instead
//! of full JSON files. The content is retrieved via `git show` at runtime.

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
fn test_git_ref_conversion_and_content() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = versioned_health_git_ref_apis()?;

    // Generate and commit initial documents (v1, v2, v3).
    env.generate_documents(&apis)?;
    env.commit_documents()?;

    // Record the commit where v1-v3 were first introduced.
    let first_commit = env.get_current_commit_hash_full()?;

    // Read the original v1 content before conversion for later verification.
    let original_v1 =
        env.read_versioned_document("versioned-health", "1.0.0")?;

    // Initially, all versions should be full JSON files.
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

    // Make unrelated commits to advance past the first commit.
    env.make_unrelated_commit("unrelated change 1")?;
    env.make_unrelated_commit("unrelated change 2")?;
    let current_commit = env.get_current_commit_hash_full()?;
    assert_ne!(
        first_commit, current_commit,
        "current commit should have advanced past the first commit"
    );

    // Add a new version (v4) to make v1-v3 no longer latest. At this point v4
    // is not blessed yet. The key behavior we're testing is that v3 (the
    // previous latest) gets converted to a git ref in the same operation that
    // creates v4. This means git will see:
    //
    // - v3.json deleted
    // - v3.json.gitref created
    // - v4.json created
    //
    // The changes between v3 and v4 will be incremental, so git will generally
    // detect that as a rename.
    //
    // This is important for clean git history.
    let extended_apis = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&extended_apis)?;

    // Verify v4 is NOT blessed yet - this confirms the conversion happens
    // before the new version is committed.
    assert!(
        !env.is_file_committed("versioned-health/versioned-health-4.0.0-")?,
        "v4 should not be committed yet"
    );

    // v1, v2, v3 should now be git ref files (non-latest blessed versions).
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

    // The original JSON files should be removed.
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

    // v4 should be a full JSON file (it's the new latest).
    assert!(
        env.versioned_local_document_exists("versioned-health", "4.0.0")?,
        "v4 should exist as JSON"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "4.0.0")?,
        "v4 should not be a git ref"
    );

    // Verify git refs point to the first commit where each version was
    // introduced.
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

    // Verify the git ref file format is correct.
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
        git_ref.path.as_str().contains("versioned-health"),
        "path should reference versioned-health"
    );
    assert!(
        git_ref.path.as_str().contains("1.0.0"),
        "path should contain version 1.0.0"
    );

    // Verify content is preserved when reading through git ref.
    let git_ref_v1_content =
        env.read_git_ref_content("versioned-health", "1.0.0")?;
    assert_eq!(
        original_v1, git_ref_v1_content,
        "git ref content should match original"
    );

    // Check should pass with the extended APIs (git refs are read correctly).
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

    // Generate and commit v1, v2, v3 in a single commit.
    env.generate_documents(&apis)?;
    env.commit_documents()?;

    // Check should pass - no conversion should be suggested because all
    // versions share the same first commit as the latest (v3).
    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(
        result,
        CheckResult::Success,
        "check should pass when all versions share the same first commit"
    );

    // Regenerate without adding new versions.
    env.generate_documents(&apis)?;

    // v3 is still latest, so it should remain as JSON.
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")?,
        "v3 should still exist as JSON"
    );
    assert!(
        !env.versioned_git_ref_exists("versioned-health", "3.0.0")?,
        "v3 should not be a git ref"
    );

    // v1 and v2 share the same first commit as v3, so they should NOT
    // be converted to git refs (no deduplication benefit).
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
    // Note: lockstep_apis() uses the standard lockstep health API, which
    // doesn't have git ref storage enabled (and even if it did, lockstep APIs
    // should never use git refs since there's only one version).
    let apis = lockstep_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;

    // Regenerate.
    env.generate_documents(&apis)?;

    // Lockstep files should remain as JSON, not git ref.
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

    // Use APIs WITHOUT git ref storage initially, so we can commit v1, v2, v3
    // as JSON files from different commits.

    // First commit: v1, v2 (no git ref storage).
    let v1_v2_no_git_ref = versioned_health_reduced_apis()?;
    env.generate_documents(&v1_v2_no_git_ref)?;
    env.commit_documents()?;
    let first_commit = env.get_current_commit_hash_full()?;

    // Second commit: add v3 (still no git ref storage).
    let v1_v2_v3_no_git_ref = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3_no_git_ref)?;
    env.commit_documents()?;
    let second_commit = env.get_current_commit_hash_full()?;

    assert_ne!(first_commit, second_commit);

    // All versions should be JSON files at this point.
    assert!(env.versioned_local_document_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "2.0.0")?);
    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);

    // Now check WITH git ref storage enabled. v3 is latest (blessed, from
    // second_commit). v1, v2 are from first_commit (different from v3).
    // v1 and v2 should be suggested for conversion.
    let v1_v2_v3_git_ref = versioned_health_git_ref_apis()?;
    let result = check_apis_up_to_date(env.environment(), &v1_v2_v3_git_ref)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should suggest converting v1, v2 (different first commit)"
    );

    // Run generate to perform the conversion.
    env.generate_documents(&v1_v2_v3_git_ref)?;

    // v1, v2 should now be git refs pointing to first_commit.
    assert!(env.versioned_git_ref_exists("versioned-health", "1.0.0")?);
    assert!(env.versioned_git_ref_exists("versioned-health", "2.0.0")?);

    let v1_ref = env.read_versioned_git_ref("versioned-health", "1.0.0")?;
    let v1_commit = v1_ref.trim().split(':').next().unwrap();
    assert_eq!(v1_commit, first_commit);

    // v3 should remain as JSON (it's the latest).
    assert!(env.versioned_local_document_exists("versioned-health", "3.0.0")?);
    assert!(!env.versioned_git_ref_exists("versioned-health", "3.0.0")?);

    // Check should now pass.
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

    // Start with v1 and v2 only.
    let v1_v2_apis = versioned_health_reduced_git_ref_apis()?;
    env.generate_documents(&v1_v2_apis)?;
    env.commit_documents()?;
    let v1_v2_commit = env.get_current_commit_hash_full()?;

    // Make an unrelated commit.
    env.make_unrelated_commit("between v2 and v3")?;

    // Add v3.
    let v1_v2_v3_apis = versioned_health_git_ref_apis()?;
    env.generate_documents(&v1_v2_v3_apis)?;
    env.commit_documents()?;
    let v3_commit = env.get_current_commit_hash_full()?;

    // Verify v1/v2 and v3 were introduced in different commits.
    assert_ne!(v1_v2_commit, v3_commit, "v1/v2 and v3 in different commits");

    // Make another unrelated commit.
    env.make_unrelated_commit("after v3")?;

    // Add v4 and regenerate to convert v1-v3 to git refs.
    let extended_apis = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&extended_apis)?;
    env.commit_documents()?;

    // Check should pass - the git refs should be read correctly.
    let result = check_apis_up_to_date(env.environment(), &extended_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Verify the files are in the expected state.
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

    // Verify git refs point to their respective first commits.
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

    // Verify v1/v2 and v3 point to different commits.
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
    // Use the regular versioned_health_apis which doesn't have git ref enabled.
    let apis = versioned_health_apis()?;

    // Generate and commit.
    env.generate_documents(&apis)?;
    env.commit_documents()?;

    // Regenerate.
    env.generate_documents(&apis)?;

    // All versions should still be JSON files.
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

    // No git refs should exist.
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
/// This is the reverse of `test_git_ref_conversion_and_content`. When a user
/// disables git ref storage (by removing `use_git_ref_storage()` from their API
/// config), existing git ref files should be converted back to full JSON files.
#[test]
fn test_git_ref_to_json_when_disabled() -> Result<()> {
    let env = TestEnvironment::new()?;

    // Use APIs with git ref storage enabled.
    let apis_with_git_ref = versioned_health_git_ref_apis()?;
    env.generate_documents(&apis_with_git_ref)?;
    env.commit_documents()?;

    // Read original content before any conversion.
    let original_v1 =
        env.read_versioned_document("versioned-health", "1.0.0")?;
    let original_v2 =
        env.read_versioned_document("versioned-health", "2.0.0")?;

    // Add v4 to trigger conversion of v1, v2, v3 to git refs.
    let extended_with_git_ref = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&extended_with_git_ref)?;

    // Verify git refs exist.
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

    // Check with git ref disabled should report fixable problems.
    let extended_without_git_ref = versioned_health_with_v4_apis()?;
    let result =
        check_apis_up_to_date(env.environment(), &extended_without_git_ref)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update when git refs exist but git ref \
         storage is disabled"
    );

    // Now use APIs without git ref storage (disabled) to perform conversion.
    env.generate_documents(&extended_without_git_ref)?;

    // Git refs should be converted back to JSON.
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

    // JSON files should exist.
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

    // Content should match the original.
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
fn test_duplicate_git_ref_and_json_handling() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = versioned_health_git_ref_apis()?;

    // Generate and commit v1-v3.
    env.generate_documents(&apis)?;
    env.commit_documents()?;

    // Add v4 to make v1-v3 non-latest, converting them to git refs.
    let extended = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&extended)?;

    // Verify v1 is a git ref and not a JSON file.
    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "v1 should be a git ref"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should not have a JSON file"
    );

    // Manually create a duplicate JSON file for v1 (simulating an edge case).
    let json_content = env.read_git_ref_content("versioned-health", "1.0.0")?;
    let git_ref_path = env
        .find_versioned_git_ref_path("versioned-health", "1.0.0")?
        .expect("git ref should exist");
    let json_path = git_ref_path.with_extension(""); // Removes .gitref
    env.create_file(&json_path, &json_content)?;

    // Both should exist now.
    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "git ref should still exist"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "duplicate JSON should exist"
    );

    // Check should report fixable problems (duplicate file).
    let result = check_apis_up_to_date(env.environment(), &extended)?;
    assert_eq!(
        result,
        CheckResult::NeedsUpdate,
        "check should report needs update when duplicate files exist"
    );

    // Generate with git ref enabled should delete the JSON.
    env.generate_documents(&extended)?;

    // Only .gitref should remain.
    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "git ref should still exist after generate"
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "duplicate JSON should be deleted"
    );

    // Now test the opposite: create a duplicate again and disable git refs.
    env.create_file(&json_path, &json_content)?;
    assert!(
        env.versioned_git_ref_exists("versioned-health", "1.0.0")?,
        "git ref should exist"
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "JSON should exist"
    );

    // Generate with git refs disabled should delete the git ref.
    let extended_no_git_ref = versioned_health_with_v4_apis()?;
    env.generate_documents(&extended_no_git_ref)?;

    // Only JSON should remain.
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
fn test_git_ref_points_to_most_recent_addition_after_remove_readd() -> Result<()>
{
    let env = TestEnvironment::new()?;

    // Commit 1: add v1, v2, v3.
    let v1_v2_v3 = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3)?;
    env.commit_documents()?;
    let commit_1 = env.get_current_commit_hash_full()?;

    // Get the path to v1 before it's removed.
    let v1_path_before = env
        .find_versioned_document_path("versioned-health", "1.0.0")?
        .expect("v1 should exist after commit 1");

    // Commit 2: remove v1 (only keep v2, v3).
    let v2_v3_only = versioned_health_no_v1_apis()?;
    env.generate_documents(&v2_v3_only)?;
    env.commit_documents()?;
    let commit_2 = env.get_current_commit_hash_full()?;

    // v1 should no longer exist in the working copy.
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")?,
        "v1 should be removed after commit 2"
    );

    // Commit 3: re-add v1 (v1, v2, v3 again).
    let v1_v2_v3_again = versioned_health_apis()?;
    env.generate_documents(&v1_v2_v3_again)?;
    env.commit_documents()?;
    let commit_3 = env.get_current_commit_hash_full()?;

    // v1 should exist again. The path should be the same (same content hash).
    let v1_path_after = env
        .find_versioned_document_path("versioned-health", "1.0.0")?
        .expect("v1 should exist after commit 3");
    assert_eq!(
        v1_path_before, v1_path_after,
        "v1 path should be the same (same content hash)"
    );

    // All three commits should be different.
    assert_ne!(commit_1, commit_2, "commits 1 and 2 should differ");
    assert_ne!(commit_2, commit_3, "commits 2 and 3 should differ");
    assert_ne!(commit_1, commit_3, "commits 1 and 3 should differ");

    // Now add v4 with git ref storage to convert v1-v3 to git refs.
    let v4_with_git_ref = versioned_health_with_v4_git_ref_apis()?;
    env.generate_documents(&v4_with_git_ref)?;

    // v1 should now be a git ref.
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

    // v2 and v3 should point to commit 1 (they were never removed).
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
