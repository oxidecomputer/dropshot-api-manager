// Copyright 2025 Oxide Computer Company

//! Common test utilities and infrastructure for dropshot-api-manager integration tests.

use anyhow::{Context, Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use camino_tempfile_ext::{fixture::ChildPath, prelude::*};
use clap::Parser;
use dropshot_api_manager::{Environment, ManagedApiConfig, ManagedApis};
use dropshot_api_manager_types::{ManagedApiMetadata, Versions};
use semver::Version;
use std::{fs, process::ExitCode};

pub mod fixtures;

/// A temporary test environment that manages directories and cleanup.
pub struct TestEnvironment {
    /// Temporary directory that will be cleaned up automatically.
    #[expect(dead_code)]
    temp_dir: Utf8TempDir,
    /// Path to the workspace root within the temp directory.
    workspace_root: ChildPath,
    /// Path to the documents directory.
    documents_dir: ChildPath,
    /// The dropshot-api-manager Environment.
    environment: Environment,
}

impl TestEnvironment {
    /// Create a new test environment with temporary directories.
    pub fn new() -> Result<Self> {
        let temp_dir =
            Utf8TempDir::with_prefix("dropshot-api-manager-integration-")
                .context("failed to create temporary directory")?;

        temp_dir.child("workspace/documents").create_dir_all()?;

        let workspace_root = temp_dir.child("workspace");
        let documents_dir = workspace_root.child("documents");

        let environment = Environment::new(
            "test-openapi-manager",
            workspace_root.as_path(),
            "documents",
        )?;

        Ok(Self { temp_dir, workspace_root, documents_dir, environment })
    }

    /// Get the workspace root path.
    pub fn workspace_root(&self) -> &Utf8Path {
        &self.workspace_root
    }

    /// Get the documents directory path.
    pub fn documents_dir(&self) -> &Utf8Path {
        &self.documents_dir
    }

    /// Get the dropshot-api-manager Environment.
    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// Create a file within the workspace.
    pub fn create_file(
        &self,
        relative_path: impl AsRef<Utf8Path>,
        content: &str,
    ) -> Result<()> {
        self.workspace_root.child(relative_path.as_ref()).write_str(content)?;
        Ok(())
    }

    /// Read a file from the workspace.
    pub fn read_file(
        &self,
        relative_path: impl AsRef<Utf8Path>,
    ) -> Result<String> {
        let path = self.workspace_root.join(relative_path);
        fs::read_to_string(&path)
            .with_context(|| format!("failed to read file: {}", path))
    }

    /// Check if a file exists in the workspace.
    pub fn file_exists(&self, relative_path: impl AsRef<Utf8Path>) -> bool {
        self.workspace_root.join(relative_path.as_ref()).exists()
    }

    /// Check if a document exists for a lockstep API.
    pub fn lockstep_document_exists(&self, api_ident: &str) -> bool {
        self.file_exists(format!("documents/{}.json", api_ident))
    }

    /// Read the content of a lockstep API document.
    pub fn read_lockstep_document(&self, api_ident: &str) -> Result<String> {
        self.read_file(format!("documents/{}.json", api_ident))
    }

    /// List all files in the documents directory.
    pub fn list_document_files(&self) -> Result<Vec<Utf8PathBuf>> {
        let mut files = Vec::new();
        self.collect_files_recursive(&self.documents_dir, &mut files)?;
        Ok(files)
    }

    /// Generate documents without committing (useful for lockstep APIs).
    pub fn generate_documents(&self, apis: &ManagedApis) -> Result<()> {
        let args = ["bin", "generate"];
        let app = dropshot_api_manager::App::try_parse_from(args)?;

        if app.exec(&self.environment, apis) == ExitCode::SUCCESS {
            Ok(())
        } else {
            Err(anyhow!("failed to generate documents"))
        }
    }

    fn collect_files_recursive(
        &self,
        dir: &Utf8Path,
        files: &mut Vec<Utf8PathBuf>,
    ) -> Result<()> {
        for entry in dir
            .read_dir_utf8()
            .with_context(|| format!("failed to read directory: {}", dir))?
        {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.collect_files_recursive(path, files)?;
            } else {
                // Make path relative to workspace root.
                let relative_path =
                    path.strip_prefix(&self.workspace_root).with_context(
                        || format!("path not within workspace: {}", path),
                    )?;
                files.push(relative_path.to_path_buf());
            }
        }
        Ok(())
    }
}

pub fn health_test_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "health",
        versions: Versions::Lockstep { version: Version::new(1, 0, 0) },
        title: "Health API",
        metadata: ManagedApiMetadata {
            description: Some("A health API for testing schema evolution"),
            ..Default::default()
        },
        api_description: fixtures::health_api_mod::stub_api_description,
        extra_validation: None,
    }
}

pub fn counter_test_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "counter",
        versions: Versions::Lockstep { version: Version::new(1, 0, 0) },
        title: "Counter Test API",
        metadata: ManagedApiMetadata {
            description: Some("A counter API for testing state changes"),
            ..Default::default()
        },
        api_description: fixtures::counter_api_mod::stub_api_description,
        extra_validation: None,
    }
}

pub fn user_test_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "user",
        versions: Versions::Lockstep { version: Version::new(1, 0, 0) },
        title: "User Test API",
        metadata: ManagedApiMetadata {
            description: Some("A user API for testing state changes"),
            ..Default::default()
        },
        api_description: fixtures::user_api_mod::stub_api_description,
        extra_validation: None,
    }
}

/// Create a health API for basic testing.
pub fn create_health_test_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![health_test_api()])
        .context("failed to create ManagedApis")
}

/// Create a counter test API configuration.
pub fn create_counter_test_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![counter_test_api()])
        .context("failed to create ManagedApis")
}

/// Create a user test API configuration.
pub fn create_user_test_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![user_test_api()])
        .context("failed to create ManagedApis")
}

/// Helper to create multiple test APIs.
pub fn create_multi_test_apis() -> Result<ManagedApis> {
    let configs = vec![health_test_api(), counter_test_api(), user_test_api()];
    ManagedApis::new(configs).context("failed to create ManagedApis")
}
