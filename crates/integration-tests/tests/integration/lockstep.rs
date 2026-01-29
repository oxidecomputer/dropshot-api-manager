// Copyright 2026 Oxide Computer Company

//! Integration tests for lockstep APIs in dropshot-api-manager.
//!
//! Lockstep APIs are unversioned APIs where the OpenAPI document is always
//! generated from the current code. There are no "blessed" documents for
//! lockstep APIs - they're always fresh from the API trait definition.

use anyhow::Result;
use dropshot_api_manager::{
    ManagedApiConfig, ManagedApis,
    test_util::{CheckResult, check_apis_up_to_date},
};
use integration_tests::*;
use openapiv3::OpenAPI;

/// Test basic lockstep API document generation.
#[test]
fn test_lockstep_generate_basic() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = lockstep_health_apis()?;

    // Initially, no documents should exist.
    assert!(!env.lockstep_document_exists("health"));

    // Generate the documents.
    env.generate_documents(&apis)?;

    // Now the document should exist.
    assert!(env.lockstep_document_exists("health"));

    // Read and validate the document is valid JSON.
    let document_content = env.read_lockstep_document("health")?;
    let parsed: OpenAPI = serde_json::from_str(&document_content)
        .expect("Generated document should be valid JSON");

    // Basic OpenAPI structure validation.
    assert_eq!(parsed.openapi, "3.0.3");
    assert_eq!(parsed.info.title, "Health API");
    assert_eq!(parsed.info.version, "1.0.0");

    // Should have the health endpoint.
    assert!(parsed.paths.paths.contains_key("/health"));

    Ok(())
}

/// Test that lockstep APIs always pass the up-to-date check.
#[test]
fn test_lockstep_always_up_to_date() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = lockstep_multi_apis()?;

    // Generate all documents.
    env.generate_documents(&apis)?;

    // Check should pass - lockstep APIs are always up to date.
    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert!(result == CheckResult::Success);

    Ok(())
}

/// Test generating multiple lockstep APIs.
#[test]
fn test_lockstep_multiple_apis() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = lockstep_multi_apis()?;

    // Generate all documents.
    env.generate_documents(&apis)?;

    // Only the listed documents should exist.
    let files = env.list_document_files()?;
    let mut file_names: Vec<_> =
        files.iter().map(|f| f.file_name().unwrap()).collect();
    file_names.sort_unstable();

    assert_eq!(file_names, vec!["counter.json", "health.json", "user.json"]);

    // All should be valid JSON OpenAPI documents.
    for api_name in ["health", "counter", "user"] {
        let contents = env.read_lockstep_document(api_name)?;
        let parsed: OpenAPI =
            serde_json::from_str(&contents).unwrap_or_else(|_| {
                panic!("{} document should be valid JSON", api_name)
            });
        assert_eq!(parsed.openapi, "3.0.3");
    }

    Ok(())
}

/// Test empty API set handling.
#[test]
fn test_empty_api_set() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = ManagedApis::new(Vec::<ManagedApiConfig>::new())?;

    env.generate_documents(&apis)?;

    // No documents should be generated.
    let files = env.list_document_files()?;
    assert!(files.is_empty());

    Ok(())
}

/// Test that lockstep documents with conflict markers are handled gracefully.
///
/// When a lockstep document has conflict markers, the file becomes unparseable.
/// The API manager should detect this and regenerate the document on the next
/// generate run.
#[test]
fn test_unparseable_conflict_markers() -> Result<()> {
    let env = TestEnvironment::new()?;
    let apis = lockstep_health_apis()?;

    env.generate_documents(&apis)?;
    env.commit_documents()?;

    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::Success);

    let conflict_content = r#"<<<<<<< HEAD
{
  "openapi": "3.0.3",
  "info": {
    "title": "Health API",
    "version": "1.0.0"
  }
}
=======
{
  "openapi": "3.0.3",
  "info": {
    "title": "Health API Modified",
    "version": "1.0.0"
  }
}
>>>>>>> branch
"#;
    env.create_file("documents/health.json", conflict_content)?;

    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::NeedsUpdate);

    env.generate_documents(&apis)?;

    let result = check_apis_up_to_date(env.environment(), &apis)?;
    assert_eq!(result, CheckResult::Success);

    let document_content = env.read_lockstep_document("health")?;
    let parsed: OpenAPI = serde_json::from_str(&document_content)
        .expect("regenerated document is valid JSON");
    assert_eq!(parsed.openapi, "3.0.3");
    assert_eq!(parsed.info.title, "Health API");

    Ok(())
}
