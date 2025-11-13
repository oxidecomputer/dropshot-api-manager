// Copyright 2025 Oxide Computer Company

//! Test environment infrastructure for integration tests.

use anyhow::{Context, Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use camino_tempfile_ext::{fixture::ChildPath, prelude::*};
use clap::Parser;
use dropshot_api_manager::{Environment, ManagedApis};
use std::{
    fs,
    process::{Command, ExitCode},
};

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
    ) -> anyhow::Result<bool> {
        let maybe_path =
            self.find_versioned_document_path(api_ident, version)?;
        Ok(maybe_path.is_some())
    }

    /// Check that a versioned document exists for a versioned API at a
    /// specific version, and is blessed.
    pub fn versioned_local_and_blessed_document_exists(
        &self,
        api_ident: &str,
        version: &str,
    ) -> anyhow::Result<bool> {
        let Some(path) =
            self.find_versioned_document_path(api_ident, version)?
        else {
            return Ok(false);
        };

        // Query git on main at the blessed path (main).
        let output = Self::run_git_command(
            &self.workspace_root,
            &["ls-tree", "-r", "--name-only", "main", path.as_str()],
        )?;
        // If the output equals the path, the document is present and blessed.
        Ok(output.trim() == path)
    }

    /// Find the path of a versioned API document for a specific version,
    /// relative to the workspace root.
    pub fn find_versioned_document_path(
        &self,
        api_ident: &str,
        version: &str,
    ) -> Result<Option<Utf8PathBuf>> {
        let files = self.list_document_files()?;

        // Versioned documents are stored in subdirectories like:
        // documents/api/api-version-hash.json.
        let pattern =
            format!("documents/{}/{}-{}-", api_ident, api_ident, version);

        let path = files.iter().find_map(|f| {
            let rel_path = rel_path_forward_slashes(f.as_ref());
            rel_path.starts_with(&pattern).then(|| Utf8PathBuf::from(rel_path))
        });
        Ok(path)
    }

    /// Read the content of a versioned API document for a specific version.
    pub fn read_versioned_document(
        &self,
        api_ident: &str,
        version: &str,
    ) -> Result<String> {
        let path = self
            .find_versioned_document_path(api_ident, version)?
            .with_context(|| {
                format!(
                    "did not find versioned document for {} v{}",
                    api_ident, version
                )
            })?;
        self.read_file(&path)
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

    /// Add files to git staging area.
    pub fn git_add(&self, paths: &[&Utf8Path]) -> Result<()> {
        let mut args = vec!["add"];
        for path in paths {
            args.push(path.as_str());
        }
        Self::run_git_command(&self.workspace_root, &args)?;
        Ok(())
    }

    /// Commit staged changes to git.
    pub fn git_commit(&self, message: &str) -> Result<()> {
        Self::run_git_command(
            &self.workspace_root,
            &["commit", "-m", message],
        )?;
        Ok(())
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
