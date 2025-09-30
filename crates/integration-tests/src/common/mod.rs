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
use std::{
    fs,
    process::{Command, ExitCode},
};

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
    /// Create a new test environment with temporary directories and git repo.
    pub fn new() -> Result<Self> {
        let temp_dir =
            Utf8TempDir::with_prefix("dropshot-api-manager-integration-")
                .context("failed to create temporary directory")?;

        temp_dir.child("workspace/documents").create_dir_all()?;

        let workspace_root = temp_dir.child("workspace");
        let documents_dir = workspace_root.child("documents");

        // Initialize git repository in workspace root.
        Self::run_git_command(
            &workspace_root,
            &["init", "--initial-branch", "main"],
        )?;
        Self::run_git_command(
            &workspace_root,
            &["config", "user.name", "Test User"],
        )?;
        Self::run_git_command(
            &workspace_root,
            &["config", "user.email", "test@example.com"],
        )?;

        // Create initial commit to establish git history, including disabling
        // Windows line endings.
        workspace_root.child(".gitattributes").write_str("* -text\n")?;
        workspace_root.child("README.md").write_str("# Test workspace\n")?;
        Self::run_git_command(
            &workspace_root,
            &["add", ".gitattributes", "README.md"],
        )?;
        Self::run_git_command(
            &workspace_root,
            &["commit", "-m", "initial commit"],
        )?;

        let environment = Environment::new(
            "test-openapi-manager",
            workspace_root.as_path(),
            "documents",
        )?
        // Use "main" rather than the default "origin/main" since we're not
        // pushing to an upstream. A commit to main automatically marks the
        // document blessed.
        .with_default_git_branch("main");

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

    pub fn read_link(
        &self,
        relative_path: impl AsRef<Utf8Path>,
    ) -> Result<Utf8PathBuf> {
        let path = self.workspace_root.join(relative_path);
        path.read_link_utf8()
            .with_context(|| format!("failed to read link: {}", path))
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

    /// Check if a document exists for a versioned API at a specific version in
    /// the working copy.
    pub fn versioned_local_document_exists(
        &self,
        api_ident: &str,
        version: &str,
    ) -> bool {
        self.find_versioned_document(api_ident, version).is_some()
    }

    /// Check that a versioned document exists for a versioned API at a specific
    /// version, and is blessed.
    pub fn versioned_local_and_blessed_document_exists(
        &self,
        api_ident: &str,
        version: &str,
    ) -> anyhow::Result<bool> {
        let Some(path) = self.find_versioned_document(api_ident, version)
        else {
            return Ok(false);
        };

        // Query git on main at the blessed path (main)
        let output = Self::run_git_command(
            &self.workspace_root,
            &["ls-tree", "-r", "--name-only", "main", path.as_str()],
        )?;
        // If the output equals the path, the document is present and blessed.
        Ok(output.trim() == path)
    }

    fn find_versioned_document(
        &self,
        api_ident: &str,
        version: &str,
    ) -> Option<Utf8PathBuf> {
        let files = self
            .list_document_files()
            .expect("reading document files succeeded");

        // Versioned documents are stored in subdirectories like:
        // documents/api/api-version-hash.json.
        let pattern =
            format!("documents/{}/{}-{}-", api_ident, api_ident, version);

        files.iter().find_map(|f| {
            let rel_path = rel_path_forward_slashes(f.as_ref());
            rel_path.starts_with(&pattern).then(|| Utf8PathBuf::from(rel_path))
        })
    }

    /// Read the content of a versioned API document for a specific version.
    pub fn read_versioned_document(
        &self,
        api_ident: &str,
        version: &str,
    ) -> Result<String> {
        // Find the document file that matches the version pattern.
        let files = self.list_document_files()?;
        let pattern =
            format!("documents/{}/{}-{}-", api_ident, api_ident, version);

        let matching_file = files
            .iter()
            .find(|f| {
                rel_path_forward_slashes(f.as_ref()).starts_with(&pattern)
            })
            .ok_or_else(|| {
                anyhow!(
                    "No versioned document found for {} version {}",
                    api_ident,
                    version
                )
            })?;

        self.read_file(matching_file)
    }

    /// List all versioned documents for a specific API.
    pub fn list_versioned_documents(
        &self,
        api_ident: &str,
    ) -> Result<Vec<Utf8PathBuf>> {
        let files = self.list_document_files()?;
        let prefix = format!("documents/{}/", api_ident);

        Ok(files
            .into_iter()
            .filter(|f| {
                rel_path_forward_slashes(f.as_ref()).starts_with(&prefix)
            })
            .collect())
    }

    /// Check if the latest document exists for a versioned API.
    pub fn versioned_latest_document_exists(&self, api_ident: &str) -> bool {
        self.file_exists(format!(
            "documents/{}/{}-latest.json",
            api_ident, api_ident
        ))
    }

    /// Delete the latest symlink for a versioned API.
    pub fn delete_versioned_latest_symlink(
        &self,
        api_ident: &str,
    ) -> Result<()> {
        let latest_link = self
            .documents_dir()
            .join(format!("{}/{}-latest.json", api_ident, api_ident));
        std::fs::remove_file(&latest_link).with_context(|| {
            format!("failed to delete latest symlink: {latest_link}")
        })
    }

    /// Read the latest document for a versioned API.
    pub fn read_versioned_latest_document(
        &self,
        api_ident: &str,
    ) -> Result<String> {
        // Try reading the link to ensure it's a symlink.
        let file_name =
            format!("documents/{}/{}-latest.json", api_ident, api_ident);
        let target = self.read_link(&file_name)?;
        eprintln!("** symlink target: {}", target);

        self.read_file(&file_name)
    }

    /// Commit documents to git (for blessed document workflow testing).
    pub fn commit_documents(&self) -> Result<()> {
        // Add all files in documents directory to git.
        Self::run_git_command(&self.workspace_root, &["add", "documents/"])?;

        // Check if there are any changes to commit.
        let status_output = Self::run_git_command(
            &self.workspace_root,
            &["status", "--porcelain"],
        )?;
        if status_output.trim().is_empty() {
            // No changes to commit.
            return Ok(());
        }

        // Commit the changes.
        Self::run_git_command(
            &self.workspace_root,
            &["commit", "-m", "Update API documents"],
        )?;
        Ok(())
    }

    /// Check if files in the documents directory have uncommitted changes.
    pub fn has_uncommitted_document_changes(&self) -> Result<bool> {
        let output = Self::run_git_command(
            &self.workspace_root,
            &["status", "--porcelain", "documents/"],
        )?;
        Ok(!output.trim().is_empty())
    }

    /// Get the current git commit hash (short form).
    pub fn get_current_commit_hash(&self) -> Result<String> {
        let output = Self::run_git_command(
            &self.workspace_root,
            &["rev-parse", "--short", "HEAD"],
        )?;
        Ok(output.trim().to_string())
    }

    /// Helper to run git commands in the workspace root.
    fn run_git_command(
        workspace_root: &Utf8Path,
        args: &[&str],
    ) -> Result<String> {
        let git =
            std::env::var("GIT").ok().unwrap_or_else(|| String::from("git"));
        let output = Command::new(git)
            .current_dir(workspace_root)
            .args(args)
            .output()
            .context("failed to execute git command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "git command failed: git {}\nstderr: {}",
                args.join(" "),
                stderr
            ));
        }

        String::from_utf8(output.stdout)
            .context("git command output was not valid UTF-8")
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

#[cfg(windows)]
pub fn rel_path_forward_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(not(windows))]
pub fn rel_path_forward_slashes(path: &str) -> String {
    path.to_string()
}

/// Create a versioned health API test configuration.
pub fn versioned_health_test_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            supported_versions: fixtures::versioned_health::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some("A versioned health API for testing version evolution"),
            ..Default::default()
        },
        api_description: fixtures::versioned_health::versioned_health_api_mod::stub_api_description,
        extra_validation: None,
    }
}

/// Create a versioned user API test configuration.
pub fn versioned_user_test_api() -> ManagedApiConfig {
    ManagedApiConfig {
        ident: "versioned-user",
        versions: Versions::Versioned {
            supported_versions: fixtures::versioned_user::supported_versions(),
        },
        title: "Versioned User API",
        metadata: ManagedApiMetadata {
            description: Some("A versioned user API for testing complex schema evolution"),
            ..Default::default()
        },
        api_description: fixtures::versioned_user::versioned_user_api_mod::stub_api_description,
        extra_validation: None,
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

/// Create a versioned health API for testing.
pub fn create_versioned_health_test_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![versioned_health_test_api()])
        .context("failed to create versioned health ManagedApis")
}

/// Create a versioned user API for testing.
pub fn create_versioned_user_test_apis() -> Result<ManagedApis> {
    ManagedApis::new(vec![versioned_user_test_api()])
        .context("failed to create versioned user ManagedApis")
}

/// Helper to create multiple versioned test APIs.
pub fn create_multi_versioned_test_apis() -> Result<ManagedApis> {
    let configs = vec![versioned_health_test_api(), versioned_user_test_api()];
    ManagedApis::new(configs).context("failed to create versioned ManagedApis")
}

/// Helper to create mixed lockstep and versioned test APIs.
pub fn create_mixed_test_apis() -> Result<ManagedApis> {
    let configs = vec![
        health_test_api(),
        counter_test_api(),
        versioned_health_test_api(),
        versioned_user_test_api(),
    ];
    ManagedApis::new(configs).context("failed to create mixed ManagedApis")
}

/// Create versioned health API with a trivial change (title/metadata updated).
pub fn create_versioned_health_test_apis_with_trivial_change()
-> Result<ManagedApis> {
    // Create a modified API config that would produce different OpenAPI
    // documents.
    let mut config = versioned_health_test_api();

    // Modify the title to create a different document signature.
    config.title = "Modified Versioned Health API";
    config.metadata.description =
        Some("A versioned health API with breaking changes");

    ManagedApis::new(vec![config])
        .context("failed to create trivial change versioned health ManagedApis")
}

/// Create versioned health API with reduced versions (simulating version
/// removal).
pub fn create_versioned_health_test_apis_reduced_versions()
-> Result<ManagedApis> {
    // Create a configuration similar to versioned health but with fewer
    // versions. We'll create a new fixture for this.
    let config = ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            // Use a subset of versions (only 1.0.0 and 2.0.0, not 3.0.0).
            supported_versions: fixtures::versioned_health_reduced::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some("A versioned health API with reduced versions"),
            ..Default::default()
        },
        api_description: fixtures::versioned_health_reduced::versioned_health_api_mod::stub_api_description,
        extra_validation: None,
    };

    ManagedApis::new(vec![config])
        .context("failed to create reduced versioned health ManagedApis")
}

pub fn create_versioned_health_test_apis_skip_middle() -> Result<ManagedApis> {
    // Create a configuration similar to versioned health but skipping the
    // middle version. This has versions 3.0.0 and 1.0.0, simulating retirement
    // of version 2.0.0.
    let config = ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            // Use versions 3.0.0 and 1.0.0 (skip 2.0.0).
            supported_versions: fixtures::versioned_health_skip_middle::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some("A versioned health API that skips middle version"),
            ..Default::default()
        },
        api_description: fixtures::versioned_health_skip_middle::versioned_health_api_mod::stub_api_description,
        extra_validation: None,
    };

    ManagedApis::new(vec![config])
        .context("failed to create skip middle versioned health ManagedApis")
}

/// Create a versioned health API with incompatible changes that break backward
/// compatibility.
pub fn create_versioned_health_test_apis_incompatible() -> Result<ManagedApis> {
    // Create a configuration similar to versioned health but with incompatible
    // changes that break backward compatibility.
    let config = ManagedApiConfig {
        ident: "versioned-health",
        versions: Versions::Versioned {
            supported_versions: fixtures::versioned_health_incompatible::supported_versions(),
        },
        title: "Versioned Health API",
        metadata: ManagedApiMetadata {
            description: Some("A versioned health API with incompatible changes"),
            ..Default::default()
        },
        api_description: fixtures::versioned_health_incompatible::versioned_health_api_mod::stub_api_description,
        extra_validation: None,
    };

    ManagedApis::new(vec![config])
        .context("failed to create incompatible versioned health ManagedApis")
}
