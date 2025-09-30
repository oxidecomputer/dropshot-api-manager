// Copyright 2025 Oxide Computer Company

//! Integration tests for versioned APIs in dropshot-api-manager.
//!
//! Versioned APIs support multiple versions where each version has a separate
//! OpenAPI document. These are "blessed" documents that are checked into git
//! and must remain stable across changes.

use anyhow::Result;
use integration_tests::common::*;
use openapiv3::OpenAPI;

/// Test basic versioned API document generation.
#[test]
fn test_versioned_generate_basic() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = create_versioned_health_test_apis()?;

    // Initially, no documents should exist.
    assert!(!env.versioned_document_exists("versioned-health", "1.0.0"));
    assert!(!env.versioned_document_exists("versioned-health", "2.0.0"));
    assert!(!env.versioned_document_exists("versioned-health", "3.0.0"));
    assert!(!env.versioned_latest_document_exists("versioned-health"));

    // Generate the documents.
    env.generate_documents(&apis)?;

    // Now the version documents should exist.
    assert!(env.versioned_document_exists("versioned-health", "1.0.0"));
    assert!(env.versioned_document_exists("versioned-health", "2.0.0"));
    assert!(env.versioned_document_exists("versioned-health", "3.0.0"));
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
    assert!(env.versioned_document_exists("versioned-health", "1.0.0"));
    assert!(env.versioned_document_exists("versioned-health", "2.0.0"));
    assert!(env.versioned_document_exists("versioned-health", "3.0.0"));
    assert!(env.versioned_latest_document_exists("versioned-health"));

    // Versioned user API (3 versions).
    assert!(env.versioned_document_exists("versioned-user", "1.0.0"));
    assert!(env.versioned_document_exists("versioned-user", "2.0.0"));
    assert!(env.versioned_document_exists("versioned-user", "3.0.0"));
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
    assert!(env.versioned_document_exists("versioned-health", "1.0.0"));
    assert!(env.versioned_document_exists("versioned-user", "1.0.0"));

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
