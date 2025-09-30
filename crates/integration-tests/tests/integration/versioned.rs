// Copyright 2025 Oxide Computer Company

//! Integration tests for versioned APIs in dropshot-api-manager.
//!
//! Versioned APIs support multiple versions where each version has a separate
//! OpenAPI document. These are "blessed" documents that are checked into git
//! and must remain stable across changes.

use anyhow::{Context, Result};
use dropshot_api_manager::test_util::{CheckResult, check_apis_up_to_date};
use integration_tests::common::{
    create_versioned_health_test_apis_incompatible,
    create_versioned_health_test_apis_skip_middle, *,
};
use openapiv3::OpenAPI;

/// Test basic versioned API document generation.
#[test]
fn test_versioned_generate_basic() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_versioned_health_test_apis()?;

    // Initially, no documents should exist.
    assert!(
        !env.versioned_local_document_exists("versioned-health", "1.0.0")
            .unwrap()
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "2.0.0")
            .unwrap()
    );
    assert!(
        !env.versioned_local_document_exists("versioned-health", "3.0.0")
            .unwrap()
    );
    assert!(!env.versioned_latest_document_exists("versioned-health"));

    // Generate the documents.
    env.generate_documents(&apis)?;

    // Now the version documents should exist.
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")
            .unwrap()
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "2.0.0")
            .unwrap()
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")
            .unwrap()
    );
    assert!(env.versioned_latest_document_exists("versioned-health"));

    // Read and validate one of the documents is valid JSON.
    let document_content =
        env.read_versioned_document("versioned-health", "1.0.0")?;
    let parsed: OpenAPI = serde_json::from_str(&document_content)
        .expect("Generated document should be valid JSON");

    // Basic OpenAPI structure validation.
    assert_eq!(parsed.openapi, "3.0.3");
    assert_eq!(parsed.info.title, "Versioned Health API");
    assert_eq!(parsed.info.version, "1.0.0");

    // Version 1.0.0 should only have the basic health endpoint.
    assert!(parsed.paths.paths.contains_key("/health"));
    assert!(!parsed.paths.paths.contains_key("/health/detailed"));
    assert!(!parsed.paths.paths.contains_key("/metrics"));

    Ok(())
}

/// Test versioned API document content differs by version.
#[test]
fn test_versioned_content_by_version() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_versioned_health_test_apis()?;

    // Generate documents.
    env.generate_documents(&apis)?;

    // Parse all version documents.
    let v1_content =
        env.read_versioned_document("versioned-health", "1.0.0")?;
    let v1_spec: OpenAPI = serde_json::from_str(&v1_content)?;

    let v2_content =
        env.read_versioned_document("versioned-health", "2.0.0")?;
    let v2_spec: OpenAPI = serde_json::from_str(&v2_content)?;

    let v3_content =
        env.read_versioned_document("versioned-health", "3.0.0")?;
    let v3_spec: OpenAPI = serde_json::from_str(&v3_content)?;

    // Version 1.0.0 should only have /health endpoint.
    assert!(v1_spec.paths.paths.contains_key("/health"));
    assert!(!v1_spec.paths.paths.contains_key("/health/detailed"));
    assert!(!v1_spec.paths.paths.contains_key("/metrics"));

    // Version 2.0.0 should have /health and /health/detailed endpoints.
    assert!(v2_spec.paths.paths.contains_key("/health"));
    assert!(v2_spec.paths.paths.contains_key("/health/detailed"));
    assert!(!v2_spec.paths.paths.contains_key("/metrics"));

    // Version 3.0.0 should have all endpoints.
    assert!(v3_spec.paths.paths.contains_key("/health"));
    assert!(v3_spec.paths.paths.contains_key("/health/detailed"));
    assert!(v3_spec.paths.paths.contains_key("/metrics"));

    Ok(())
}

/// Test versioned API latest document points to newest version.
#[test]
fn test_versioned_latest_document() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_versioned_health_test_apis()?;

    // Generate documents.
    env.generate_documents(&apis)?;

    // Read latest document and newest version document.
    let latest_content =
        env.read_versioned_latest_document("versioned-health")?;
    let v3_content =
        env.read_versioned_document("versioned-health", "3.0.0")?;

    let latest_spec: OpenAPI = serde_json::from_str(&latest_content)?;
    let v3_spec: OpenAPI = serde_json::from_str(&v3_content)?;

    // Latest should match version 3.0.0 (newest version).
    assert_eq!(latest_spec.info.version, "3.0.0");
    assert_eq!(latest_spec.paths.paths.len(), v3_spec.paths.paths.len());

    // Both should have all endpoints.
    assert!(latest_spec.paths.paths.contains_key("/health"));
    assert!(latest_spec.paths.paths.contains_key("/health/detailed"));
    assert!(latest_spec.paths.paths.contains_key("/metrics"));

    Ok(())
}

/// Test generating multiple versioned APIs.
#[test]
fn test_multiple_versioned_apis() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_multi_versioned_test_apis()?;

    // Generate all documents.
    env.generate_documents(&apis)?;

    // Check that documents exist for both APIs and all their versions.
    // Versioned health API (3 versions).
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")
            .unwrap()
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "2.0.0")
            .unwrap()
    );
    assert!(
        env.versioned_local_document_exists("versioned-health", "3.0.0")
            .unwrap()
    );
    assert!(env.versioned_latest_document_exists("versioned-health"));

    // Versioned user API (3 versions).
    assert!(
        env.versioned_local_document_exists("versioned-user", "1.0.0").unwrap()
    );
    assert!(
        env.versioned_local_document_exists("versioned-user", "2.0.0").unwrap()
    );
    assert!(
        env.versioned_local_document_exists("versioned-user", "3.0.0").unwrap()
    );
    assert!(env.versioned_latest_document_exists("versioned-user"));

    // List all versioned documents for each API.
    let health_docs = env.list_versioned_documents("versioned-health")?;
    let user_docs = env.list_versioned_documents("versioned-user")?;

    // Each API should have 4 documents (3 versions + latest).
    assert_eq!(health_docs.len(), 4);
    assert_eq!(user_docs.len(), 4);

    Ok(())
}

/// Test mixed lockstep and versioned APIs.
#[test]
fn test_mixed_lockstep_and_versioned_apis() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_mixed_test_apis()?;

    // Generate all documents.
    env.generate_documents(&apis)?;

    // Check lockstep APIs exist as simple JSON files.
    assert!(env.lockstep_document_exists("health"));
    assert!(env.lockstep_document_exists("counter"));

    // Check versioned APIs exist as version-specific files.
    assert!(
        env.versioned_local_document_exists("versioned-health", "1.0.0")
            .unwrap()
    );
    assert!(
        env.versioned_local_document_exists("versioned-user", "1.0.0").unwrap()
    );

    // List all document files to verify proper structure.
    let all_files = env.list_document_files()?;

    // Should have lockstep files and versioned directories.
    let lockstep_files: Vec<_> = all_files
        .iter()
        .filter(|f| {
            let path_str = rel_path_forward_slashes(f.as_ref());
            f.extension() == Some("json")
                && path_str.starts_with("documents/")
                && !path_str[10..].contains('/') // No subdirectories after "documents/"
        })
        .collect();
    let versioned_files: Vec<_> = all_files
        .iter()
        .filter(|f| {
            let path_str = rel_path_forward_slashes(f.as_ref());
            path_str.starts_with("documents/") && path_str[10..].contains('/') // Has subdirectories
        })
        .collect();

    // Should have 2 lockstep files (health.json, counter.json).
    assert_eq!(lockstep_files.len(), 2);

    // Should have versioned files (each API has 4 files: 3 versions + latest).
    assert_eq!(versioned_files.len(), 8);

    Ok(())
}

/// Test git integration: commit documents.
#[test]
fn test_git_commit_documents() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_versioned_health_test_apis()?;

    // Initially no uncommitted changes.
    assert!(!env.has_uncommitted_document_changes()?);

    // Generate documents.
    env.generate_documents(&apis)?;

    // Now there should be uncommitted changes.
    assert!(env.has_uncommitted_document_changes()?);

    // Commit the documents.
    env.commit_documents()?;

    // Should no longer have uncommitted changes.
    assert!(!env.has_uncommitted_document_changes()?);

    // Should be able to get current commit hash.
    let commit_hash = env.get_current_commit_hash()?;
    assert!(!commit_hash.is_empty());
    assert!(commit_hash.len() >= 7); // Git short hash is typically 7+ chars.

    Ok(())
}

/// Test blessed document lifecycle - generate, commit, then verify check passes.
#[test]
fn test_blessed_document_lifecycle() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_versioned_health_test_apis()?;

    // Initially, APIs should fail the up-to-date check (no documents exist).
    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    // Generate the documents.
    env.generate_documents(&apis)?;

    // After generation, for new APIs, they are considered fresh/up-to-date.
    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::Success);

    // Commit the documents to "bless" them.
    env.commit_documents()?;

    // Should still pass after committing.
    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that API changes require regeneration and recommitting.
#[test]
fn test_blessed_api_changes_should_not_do_trivial_updates() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_versioned_health_test_apis()?;

    // Generate and commit initial documents.
    env.generate_documents(&apis)?;
    env.commit_documents()?;

    // Verify initial state is up-to-date.
    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::Success);

    // Create a modified API with trivial changes (different title/description).
    let modified_apis =
        create_versioned_health_test_apis_with_trivial_change()?;

    // The check should indicate that *no updates are needed* to the blessed version.
    let result = check_apis_up_to_date(env.environment(), &modified_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test multiple versioned APIs with mixed blessed document states.
#[test]
fn test_mixed_blessed_document_states() -> Result<()> {
    let env = TestEnvironment::new()?;

    // Start with combined APIs to establish the proper context.
    let combined_apis = create_multi_versioned_test_apis()?;

    // Initially, combined APIs should need update.
    let result = check_apis_up_to_date(env.environment(), &combined_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    // Generate only health API documents first.
    let health_apis = create_versioned_health_test_apis()?;
    env.generate_documents(&health_apis)?;
    env.commit_documents()?;

    // Combined APIs should still need update (user API missing).
    let result = check_apis_up_to_date(env.environment(), &combined_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    // Generate and commit all APIs documents.
    env.generate_documents(&combined_apis)?;
    env.commit_documents()?;

    // Now combined APIs should pass.
    let result = check_apis_up_to_date(env.environment(), &combined_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test that removing API versions fails the check.
#[test]
fn test_removing_api_version_fails_check() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_versioned_health_test_apis()?;

    // Generate and commit initial documents (3 versions).
    env.generate_documents(&apis)?;
    env.commit_documents()?;

    // Verify all versions exist.
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "1.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "2.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "3.0.0"
        )
        .unwrap()
    );

    // Create API with fewer versions (simulating version removal).
    let reduced_apis = create_versioned_health_test_apis_reduced_versions()?;

    // The check should result in NeedsUpdate when versions are removed.
    let result = check_apis_up_to_date(env.environment(), &reduced_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    Ok(())
}

/// Test that adding new API versions passes the check.
#[test]
fn test_adding_new_api_version_passes_check() -> Result<()> {
    let env = TestEnvironment::new()?;

    // Start with reduced version API.
    let reduced_apis = create_versioned_health_test_apis_reduced_versions()?;
    env.generate_documents(&reduced_apis)?;
    env.commit_documents()?;

    // Should pass check with reduced versions.
    let result = check_apis_up_to_date(env.environment(), &reduced_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Add more versions.
    let expanded_apis = create_versioned_health_test_apis()?;

    // Adding versions should require update (new documents to generate).
    let result = check_apis_up_to_date(env.environment(), &expanded_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    // Generate the new versions.
    env.generate_documents(&expanded_apis)?;

    // Should now pass with all versions.
    let result = check_apis_up_to_date(env.environment(), &expanded_apis)?;
    assert_eq!(result, CheckResult::Success);

    Ok(())
}

/// Test retirement of the latest blessed API version.
#[test]
fn test_retiring_latest_blessed_version() -> Result<()> {
    let env = TestEnvironment::new()?;

    // Start with the full versioned health API (3 versions).
    let full_apis = create_versioned_health_test_apis()?;

    // Generate and commit the initial "blessed" documents.
    env.generate_documents(&full_apis)?;
    env.commit_documents()?;

    // Verify initial state is up-to-date.
    let result = check_apis_up_to_date(env.environment(), &full_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Verify all 3 versions exist and are blessed.
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "1.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "2.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "3.0.0"
        )
        .unwrap()
    );

    // Now remove version 3.0.0 by switching to the reduced API.
    // This simulates a developer deciding to remove a version that was previously blessed.
    let reduced_apis = create_versioned_health_test_apis_reduced_versions()?;

    // This check should return NeedsUpdate because the v3.0.0 document exists
    // and needs to be removed.
    let result = check_apis_up_to_date(env.environment(), &reduced_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    // Generate documents with the retired version.
    env.generate_documents(&reduced_apis)?;

    // After generation, should be up-to-date with the new API definition.
    let result = check_apis_up_to_date(env.environment(), &reduced_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Verify the v3.0.0 document was removed and v1/v2 were updated.
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "1.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "2.0.0"
        )
        .unwrap()
    );
    assert!(
        !env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "3.0.0"
        )
        .unwrap()
    );

    // Verify the latest document now points to v2.0.0 (the new highest version).
    let latest_content =
        env.read_versioned_latest_document("versioned-health")?;
    let latest_spec: OpenAPI = serde_json::from_str(&latest_content)?;
    assert_eq!(latest_spec.info.version, "2.0.0");

    // Commit the retired version.
    env.commit_documents()?;

    // Should still pass after committing the retired change.
    let result = check_apis_up_to_date(env.environment(), &reduced_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Delete the latest symlink and ensure that we need to perform updates.
    env.delete_versioned_latest_symlink("versioned-health")?;
    let result = check_apis_up_to_date(env.environment(), &reduced_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    // Regenerate documents (i.e. the symlink) and retry.
    env.generate_documents(&reduced_apis)?;
    let result = check_apis_up_to_date(env.environment(), &reduced_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Verify the latest document points to v2.0.0 as before. Note that this
    // should be the blessed version, not the generated version.
    let latest_content =
        env.read_versioned_latest_document("versioned-health")?;
    let latest_spec: OpenAPI = serde_json::from_str(&latest_content)?;
    assert_eq!(latest_spec.info.version, "2.0.0");

    // Verify we can no longer use the old full API against the new blessed
    // documents.
    let result = check_apis_up_to_date(env.environment(), &full_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    Ok(())
}

#[test]
fn test_retiring_older_blessed_version() -> Result<()> {
    let env = TestEnvironment::new()?;

    // Start with the full versioned health API (3 versions).
    let full_apis = create_versioned_health_test_apis()?;

    // Generate and commit the initial "blessed" documents.
    env.generate_documents(&full_apis)?;
    env.commit_documents()?;

    // Verify initial state is up-to-date.
    let result = check_apis_up_to_date(env.environment(), &full_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Verify all 3 versions exist and are blessed.
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "1.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "2.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "3.0.0"
        )
        .unwrap()
    );

    // Now remove version 2.0.0 by switching to the skip middle API.
    // This simulates a developer deciding to retire an older version that was previously blessed.
    let skip_middle_apis = create_versioned_health_test_apis_skip_middle()?;

    // This check should return NeedsUpdate because the v2.0.0 document exists
    // and needs to be removed.
    let result = check_apis_up_to_date(env.environment(), &skip_middle_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    // Generate documents with the retired older version.
    env.generate_documents(&skip_middle_apis)?;

    // After generation, should be up-to-date with the new API definition.
    let result = check_apis_up_to_date(env.environment(), &skip_middle_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Verify the v2.0.0 document was removed and v1/v3 remain.
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "1.0.0"
        )
        .unwrap()
    );
    assert!(
        !env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "2.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "3.0.0"
        )
        .unwrap()
    );

    // Verify the latest document still points to v3.0.0 (the highest version).
    let latest_content =
        env.read_versioned_latest_document("versioned-health")?;
    let latest_spec: OpenAPI = serde_json::from_str(&latest_content)?;
    assert_eq!(latest_spec.info.version, "3.0.0");

    // Commit the retired version.
    env.commit_documents()?;

    // Should still pass after committing the retired change.
    let result = check_apis_up_to_date(env.environment(), &skip_middle_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Delete the latest symlink and ensure that we need to perform updates.
    env.delete_versioned_latest_symlink("versioned-health")?;
    let result = check_apis_up_to_date(env.environment(), &skip_middle_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    // Regenerate documents (i.e. the symlink) and retry.
    env.generate_documents(&skip_middle_apis)?;
    let result = check_apis_up_to_date(env.environment(), &skip_middle_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Verify the latest document points to v3.0.0 as before. Note that this
    // should be the blessed version, not the generated version.
    let latest_content =
        env.read_versioned_latest_document("versioned-health")?;
    let latest_spec: OpenAPI = serde_json::from_str(&latest_content)?;
    assert_eq!(latest_spec.info.version, "3.0.0");

    // Verify we can no longer use the old full API against the new blessed
    // documents.
    let result = check_apis_up_to_date(env.environment(), &full_apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    Ok(())
}

#[test]
fn test_incompatible_blessed_api_change() -> Result<()> {
    let env = TestEnvironment::new()?;

    // Start with the original versioned health API (3 versions).
    let original_apis = create_versioned_health_test_apis()?;

    // Generate and commit the initial "blessed" documents.
    env.generate_documents(&original_apis)?;
    env.commit_documents()?;

    // Verify initial state is up-to-date.
    let result = check_apis_up_to_date(env.environment(), &original_apis)?;
    assert_eq!(result, CheckResult::Success);

    // Verify all 3 versions exist.
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "1.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "2.0.0"
        )
        .unwrap()
    );
    assert!(
        env.versioned_local_and_blessed_document_exists(
            "versioned-health",
            "3.0.0"
        )
        .unwrap()
    );

    // Now introduce incompatible changes. This adds a new endpoint, which
    // (while forward-compatible) we treat as a breaking change.
    let incompatible_apis = create_versioned_health_test_apis_incompatible()?;

    // This check should return Failures.
    let result = check_apis_up_to_date(env.environment(), &incompatible_apis)?;
    assert_eq!(result, CheckResult::Failures);

    Ok(())
}

/// Test BlessedVersionExtraLocalSpec problems.
///
/// This test:
///
/// * creates blessed versions
/// * in a separate environment, creates another blessed version
/// * copies over this extra version
#[test]
fn test_blessed_version_extra_local_spec() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_versioned_health_test_apis()?;

    // Generate and commit initial documents to make them blessed.
    env.generate_documents(&apis)?;
    env.commit_documents()?;

    // Verify initial state is up-to-date.
    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::Success);

    // Generate with the incompatible APIs.
    let env2 = TestEnvironment::new()?;
    let incompatible_apis = create_versioned_health_test_apis_incompatible()?;

    env2.generate_documents(&incompatible_apis)?;

    // Ensure that the v3 documents are actually different between env and env2.
    let env_path = env
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("should find v3.0.0 document");
    let env2_path = env2
        .find_versioned_document_path("versioned-health", "3.0.0")?
        .expect("should find v3.0.0 document");
    assert_ne!(
        env_path, env2_path,
        "incompatible APIs should lead to different hashes"
    );

    // Copy env2's document into env's documents directory.
    let src = env2.workspace_root().join(&env2_path);
    let dst = env
        .documents_dir()
        .join("versioned-health")
        .join(env2_path.file_name().unwrap());

    std::fs::copy(&src, &dst)
        .with_context(|| format!("failed to copy {} to {}", src, dst))?;
    assert!(dst.exists(), "destination path {dst} exists");

    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    // Regenerating documents should remove the file.
    env.generate_documents(&apis)?;

    // After fix-up, should be up-to-date again.
    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::Success);

    // The destination path should be missing now.
    assert!(!dst.exists(), "destination path {dst} no longer exists");

    Ok(())
}
