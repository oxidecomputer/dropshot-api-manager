// Copyright 2026 Oxide Computer Company

//! Test environment infrastructure for integration tests.

use anyhow::{Context, Result, anyhow};
use atomicwrites::AtomicFile;
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use camino_tempfile_ext::{fixture::ChildPath, prelude::*};
use clap::Parser;
use dropshot_api_manager::{Environment, GitRef, ManagedApis};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    process::{Command, ExitCode, ExitStatus},
};

/// Result of attempting a git merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeResult {
    /// The merge completed successfully with no conflicts.
    Clean,
    /// The merge resulted in conflicts that need to be resolved.
    ///
    /// Contains the set of conflicted file paths.
    Conflict(BTreeSet<Utf8PathBuf>),
}

/// Result of attempting a git rebase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseResult {
    /// The rebase completed successfully with no conflicts.
    Clean,
    /// The rebase resulted in conflicts that need to be resolved.
    ///
    /// Contains the set of conflicted file paths.
    Conflict(BTreeSet<Utf8PathBuf>),
}

/// Result of attempting a jj merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JjMergeResult {
    /// The merge completed successfully with no conflicts.
    Clean,
    /// The merge resulted in conflicts that need to be resolved.
    ///
    /// Contains the set of conflicted file paths.
    Conflict(BTreeSet<Utf8PathBuf>),
}

/// Result of attempting a jj rebase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JjRebaseResult {
    /// The rebase completed successfully with no conflicts.
    Clean,
    /// The rebase resulted in conflicts that need to be resolved.
    ///
    /// Contains the set of conflicted file paths.
    Conflict(BTreeSet<Utf8PathBuf>),
}

/// Check if jj is available and the test should run.
///
/// Returns `Ok(true)` if jj is available.
/// Returns `Ok(false)` if the `SKIP_JJ_TESTS` env var is set.
/// Returns `Err` with a helpful message if jj is not found.
pub fn check_jj_available() -> Result<bool> {
    if std::env::var("SKIP_JJ_TESTS").is_ok() {
        return Ok(false);
    }

    let jj = std::env::var("JJ").unwrap_or_else(|_| "jj".to_string());
    match Command::new(&jj).arg("--version").output() {
        Ok(o) if o.status.success() => Ok(true),
        Ok(_) | Err(_) => Err(anyhow!(
            "jj not found. Install jj (https://jj-vcs.dev/) or set \
             SKIP_JJ_TESTS=1 to skip these tests"
        )),
    }
}

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
    /// relative to the workspace root. Only matches full JSON files, not git
    /// ref files.
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
            // Only match .json files, not .json.gitref files.
            (rel_path.starts_with(&pattern) && rel_path.ends_with(".json"))
                .then(|| Utf8PathBuf::from(rel_path))
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

    /// Check if a git ref file exists for a versioned API at a specific
    /// version.
    pub fn versioned_git_ref_exists(
        &self,
        api_ident: &str,
        version: &str,
    ) -> Result<bool> {
        let path = self.find_versioned_git_ref_path(api_ident, version)?;
        Ok(path.is_some())
    }

    /// Find the path of a git ref file for a versioned API at a specific
    /// version, relative to the workspace root.
    pub fn find_versioned_git_ref_path(
        &self,
        api_ident: &str,
        version: &str,
    ) -> Result<Option<Utf8PathBuf>> {
        let files = self.list_document_files()?;

        // Git ref files are stored like:
        // documents/api/api-version-hash.json.gitref.
        let pattern =
            format!("documents/{}/{}-{}-", api_ident, api_ident, version);

        let path = files.iter().find_map(|f| {
            let rel_path = rel_path_forward_slashes(f.as_ref());
            (rel_path.starts_with(&pattern)
                && rel_path.ends_with(".json.gitref"))
            .then(|| Utf8PathBuf::from(rel_path))
        });
        Ok(path)
    }

    /// Read the content of a git ref file for a versioned API.
    pub fn read_versioned_git_ref(
        &self,
        api_ident: &str,
        version: &str,
    ) -> Result<String> {
        let path = self
            .find_versioned_git_ref_path(api_ident, version)?
            .with_context(|| {
                format!(
                    "did not find git ref file for {} v{}",
                    api_ident, version
                )
            })?;
        self.read_file(&path)
    }

    /// Check if a git ref file exists for a lockstep API.
    /// (This should never happen - lockstep APIs don't use git refs.)
    pub fn lockstep_git_ref_exists(&self, api_ident: &str) -> bool {
        self.file_exists(format!("documents/{}.json.gitref", api_ident))
    }

    /// Read the actual content referenced by a git ref file.
    ///
    /// This reads the git ref file, parses it to get the commit and path, then
    /// uses git to retrieve the referenced content.
    pub fn read_git_ref_content(
        &self,
        api_ident: &str,
        version: &str,
    ) -> Result<String> {
        let git_ref_content =
            self.read_versioned_git_ref(api_ident, version)?;
        let git_ref: GitRef = git_ref_content.parse().with_context(|| {
            format!("failed to parse git ref for {} v{}", api_ident, version)
        })?;
        let content = git_ref.read_contents(&self.workspace_root)?;
        String::from_utf8(content).with_context(|| {
            format!(
                "git ref content for {} v{} is not valid UTF-8",
                api_ident, version
            )
        })
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

    /// Get the current git commit hash (full form).
    pub fn get_current_commit_hash_full(&self) -> Result<String> {
        let output = Self::run_git_command(
            &self.workspace_root,
            &["rev-parse", "HEAD"],
        )?;
        Ok(output.trim().to_string())
    }

    /// Check if any file matching the given prefix pattern is committed in the
    /// documents directory.
    pub fn is_file_committed(&self, prefix: &str) -> Result<bool> {
        let rel_docs_dir = self
            .documents_dir
            .strip_prefix(&self.workspace_root)
            .context("documents_dir should be under workspace_root")?;
        let pattern =
            rel_path_forward_slashes(&format!("{}/{}", rel_docs_dir, prefix));
        let output = Self::run_git_command(
            &self.workspace_root,
            &["ls-tree", "-r", "--name-only", "HEAD"],
        )?;
        Ok(output.lines().any(|line| line.starts_with(&pattern)))
    }

    /// Make an unrelated commit (useful for advancing HEAD without changing
    /// API documents).
    pub fn make_unrelated_commit(&self, message: &str) -> Result<()> {
        // Create or update a dummy file.
        let dummy_path = self.workspace_root.join("dummy.txt");
        let content = format!("{}\n{}\n", message, chrono::Utc::now());
        AtomicFile::new(
            &dummy_path,
            atomicwrites::OverwriteBehavior::AllowOverwrite,
        )
        .write(|f| f.write_all(content.as_bytes()))?;
        Self::run_git_command(&self.workspace_root, &["add", "dummy.txt"])?;
        Self::run_git_command(
            &self.workspace_root,
            &["commit", "-m", message],
        )?;
        Ok(())
    }

    /// Create a shallow clone of this repository in a new directory.
    ///
    /// This creates an actual shallow clone using `git clone --depth <depth>`,
    /// which means objects from commits before the shallow boundary won't exist
    /// in the clone. This is different from just writing `.git/shallow` (which
    /// only affects `git log` but leaves objects accessible).
    ///
    /// Returns a new `TestEnvironment` pointing to the shallow clone.
    pub fn shallow_clone(&self, depth: u32) -> Result<TestEnvironment> {
        let temp_dir =
            Utf8TempDir::with_prefix("dropshot-api-manager-shallow-")
                .context("failed to create temp dir for shallow clone")?;

        let clone_root = temp_dir.path().join("workspace");
        let depth_str = depth.to_string();

        // --no-local forces git to copy objects rather than using hardlinks,
        // which is necessary for a true shallow clone.
        Self::run_git_command(
            temp_dir.path(),
            &[
                "clone",
                "--no-local",
                "--depth",
                &depth_str,
                self.workspace_root.as_str(),
                clone_root.as_str(),
            ],
        )?;

        Self::run_git_command(
            &clone_root,
            &["config", "user.name", "Test User"],
        )?;
        Self::run_git_command(
            &clone_root,
            &["config", "user.email", "test@example.com"],
        )?;

        let workspace_root = temp_dir.child("workspace");

        let environment = Environment::new(
            "test-openapi-manager",
            workspace_root.as_path(),
            "documents",
        )?
        .with_default_git_branch("main");

        Ok(TestEnvironment {
            temp_dir,
            workspace_root: workspace_root.clone(),
            documents_dir: workspace_root.child("documents"),
            environment,
        })
    }

    /// Create a new branch at the current HEAD.
    pub fn create_branch(&self, name: &str) -> Result<()> {
        Self::run_git_command(&self.workspace_root, &["branch", name])?;
        Ok(())
    }

    /// Checkout a branch.
    pub fn checkout_branch(&self, name: &str) -> Result<()> {
        Self::run_git_command(&self.workspace_root, &["checkout", name])?;
        Ok(())
    }

    /// Merge a branch into the current branch.
    ///
    /// Returns `Ok(())` on a clean merge, `Err` if there's a conflict or other
    /// error.
    ///
    /// Uses `-X no-renames` to disable rename detection during merge. This
    /// tests the scenario without rename/rename conflicts.
    pub fn merge_branch_without_renames(&self, source: &str) -> Result<()> {
        let message = format!("Merge branch '{}'", source);
        Self::run_git_command(
            &self.workspace_root,
            &["merge", "-m", &message, "-X", "no-renames", source],
        )?;
        Ok(())
    }

    /// Attempt to merge a branch, returning whether conflicts occurred.
    ///
    /// Unlike `merge_branch_without_renames`, this method does not use `-X
    /// no-renames`, so Git's rename detection is active. This will cause
    /// rename/rename conflicts when both branches convert the same file to a
    /// git ref and add different new versions.
    ///
    /// Returns `MergeResult::Clean` if the merge completed cleanly, or
    /// `MergeResult::Conflict` with the list of conflicted files if there were
    /// conflicts (the working directory will be in a conflicted state).
    pub fn try_merge_branch(&self, source: &str) -> Result<MergeResult> {
        let message = format!("Merge branch '{}'", source);

        // Run the merge command.
        let status = Self::run_git_command_unchecked(
            &self.workspace_root,
            &["merge", "-m", &message, source],
        )?;

        if status.success() {
            return Ok(MergeResult::Clean);
        }

        // Use git ls-files --unmerged to check for conflicts. Output format:
        // <mode> <object> <stage>\t<file>
        // Each conflicted file appears multiple times (once per stage), so we
        // deduplicate.
        let unmerged = Self::run_git_command(
            &self.workspace_root,
            &["ls-files", "--unmerged"],
        )?;

        if unmerged.is_empty() {
            Ok(MergeResult::Clean)
        } else {
            let conflicted_files: BTreeSet<Utf8PathBuf> = unmerged
                .lines()
                .filter_map(|line| {
                    // Split on tab to get the file path (second part).
                    line.split_once('\t')
                        .map(|(_, path)| Utf8PathBuf::from(path))
                })
                .collect();
            Ok(MergeResult::Conflict(conflicted_files))
        }
    }

    /// Abort an in-progress merge.
    pub fn abort_merge(&self) -> Result<()> {
        Self::run_git_command(&self.workspace_root, &["merge", "--abort"])?;
        Ok(())
    }

    /// Add all changes and complete a merge after conflicts have been
    /// resolved.
    pub fn complete_merge(&self) -> Result<()> {
        Self::run_git_command(&self.workspace_root, &["add", "-A"])?;
        Self::run_git_command(&self.workspace_root, &["commit", "--no-edit"])?;
        Ok(())
    }

    /// Attempt to rebase the current branch onto a target branch.
    ///
    /// Returns `RebaseResult::Clean` if the rebase completed cleanly, or
    /// `RebaseResult::Conflict` with the list of conflicted files if there
    /// were conflicts (the working directory will be in a conflicted state).
    pub fn try_rebase_onto(&self, target: &str) -> Result<RebaseResult> {
        // Run the rebase command.
        let status = Self::run_git_command_unchecked(
            &self.workspace_root,
            &["rebase", target],
        )?;

        if status.success() {
            return Ok(RebaseResult::Clean);
        }

        // Use git ls-files --unmerged to check for conflicts.
        let unmerged = Self::run_git_command(
            &self.workspace_root,
            &["ls-files", "--unmerged"],
        )?;

        if unmerged.is_empty() {
            Ok(RebaseResult::Clean)
        } else {
            let conflicted_files: BTreeSet<Utf8PathBuf> = unmerged
                .lines()
                .filter_map(|line| {
                    line.split_once('\t')
                        .map(|(_, path)| Utf8PathBuf::from(path))
                })
                .collect();
            Ok(RebaseResult::Conflict(conflicted_files))
        }
    }

    /// Abort an in-progress rebase.
    pub fn abort_rebase(&self) -> Result<()> {
        Self::run_git_command(&self.workspace_root, &["rebase", "--abort"])?;
        Ok(())
    }

    /// Add all changes and continue a rebase after conflicts have been
    /// resolved.
    pub fn continue_rebase(&self) -> Result<()> {
        Self::run_git_command(&self.workspace_root, &["add", "-A"])?;
        Self::run_git_command(&self.workspace_root, &["rebase", "--continue"])?;
        Ok(())
    }

    /// Continue a rebase after resolving conflicts, returning the result.
    ///
    /// Unlike `continue_rebase`, this method returns `RebaseResult` to allow
    /// detecting subsequent conflicts during multi-step rebases.
    pub fn try_continue_rebase(&self) -> Result<RebaseResult> {
        Self::run_git_command(&self.workspace_root, &["add", "-A"])?;
        let status = Self::run_git_command_unchecked(
            &self.workspace_root,
            &["rebase", "--continue"],
        )?;

        if status.success() {
            return Ok(RebaseResult::Clean);
        }

        // Use git ls-files --unmerged to check for conflicts.
        let unmerged = Self::run_git_command(
            &self.workspace_root,
            &["ls-files", "--unmerged"],
        )?;

        if unmerged.is_empty() {
            Ok(RebaseResult::Clean)
        } else {
            let conflicted_files: BTreeSet<Utf8PathBuf> = unmerged
                .lines()
                .filter_map(|line| {
                    line.split_once('\t')
                        .map(|(_, path)| Utf8PathBuf::from(path))
                })
                .collect();
            Ok(RebaseResult::Conflict(conflicted_files))
        }
    }

    /// Helper to run git commands in a directory.
    fn run_git_command(cwd: &Utf8Path, args: &[&str]) -> Result<String> {
        let git =
            std::env::var("GIT").ok().unwrap_or_else(|| String::from("git"));
        let output = Command::new(git)
            .current_dir(cwd)
            // Prevent interactive prompts (e.g., during rebase --continue).
            .env("EDITOR", "true")
            .args(args)
            .output()
            .context("failed to execute git command")?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "git command failed: git {}\nstdout: {}\nstderr: {}",
                args.join(" "),
                stdout,
                stderr
            ));
        }

        String::from_utf8(output.stdout)
            .context("git command output was not valid UTF-8")
    }

    /// Helper to run git commands that may fail, returning the exit status.
    fn run_git_command_unchecked(
        cwd: &Utf8Path,
        args: &[&str],
    ) -> Result<ExitStatus> {
        let git =
            std::env::var("GIT").ok().unwrap_or_else(|| String::from("git"));
        let output = Command::new(git)
            .current_dir(cwd)
            // Prevent interactive prompts (e.g., during rebase --continue).
            .env("EDITOR", "true")
            .args(args)
            .output()
            .context("failed to execute git command")?;
        Ok(output.status)
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

    // -------------------------------------------------------------------------
    // jj (Jujutsu) methods
    // -------------------------------------------------------------------------

    /// Initialize jj in the existing git repository (colocated mode).
    pub fn jj_init(&self) -> Result<()> {
        self.run_jj_command(&["git", "init", "--colocate"])?;
        Ok(())
    }

    /// Attempt to create a merge commit from two git branches using jj.
    ///
    /// Uses `jj new branch1 branch2 -m "message"` to create a merge commit.
    /// Returns `JjMergeResult::Clean` if the merge has no conflicts, or
    /// `JjMergeResult::Conflict` with the set of conflicted files.
    pub fn jj_try_merge(
        &self,
        branch1: &str,
        branch2: &str,
        message: &str,
    ) -> Result<JjMergeResult> {
        // Create a merge commit with jj new.
        self.run_jj_command(&["new", branch1, branch2, "-m", message])?;

        // Check if the resulting commit has conflicts.
        if self.jj_has_conflicts("@")? {
            let conflicts = self.jj_get_revision_conflicts("@")?;
            Ok(JjMergeResult::Conflict(conflicts))
        } else {
            Ok(JjMergeResult::Clean)
        }
    }

    /// Attempt to rebase a git branch onto another using jj.
    ///
    /// Uses `jj rebase -b branch -d dest` to rebase the entire branch (from the
    /// common ancestor with dest) onto the destination. Returns
    /// `JjRebaseResult::Clean` if the rebase has no conflicts, or
    /// `JjRebaseResult::Conflict` with the set of conflicted files in the tip.
    pub fn jj_try_rebase(
        &self,
        branch: &str,
        dest: &str,
    ) -> Result<JjRebaseResult> {
        // Rebase the entire branch from common ancestor onto dest.
        self.run_jj_command(&["rebase", "-b", branch, "-d", dest])?;

        // Check if the rebased tip has conflicts.
        if self.jj_has_conflicts(branch)? {
            let conflicts = self.jj_get_revision_conflicts(branch)?;
            Ok(JjRebaseResult::Conflict(conflicts))
        } else {
            Ok(JjRebaseResult::Clean)
        }
    }

    /// Resolve jj conflicts by running generate and committing the resolution.
    ///
    /// After a merge or rebase creates conflicts, this method:
    /// 1. Runs generate to create the correct files
    /// 2. Commits the working copy to complete the merge/rebase
    /// 3. Verifies the result has no conflicts
    pub fn jj_resolve_conflicts(&self, apis: &ManagedApis) -> Result<()> {
        self.generate_documents(apis)?;
        self.run_jj_command(&["commit", "-m", "resolve conflicts"])?;

        // Verify the parent commit (the resolved merge/rebase) has no conflicts.
        if self.jj_has_conflicts("@-")? {
            return Err(anyhow::anyhow!(
                "jj conflict resolution failed: @- still has conflicts"
            ));
        }

        Ok(())
    }

    /// Check if a jj revision has conflicts.
    fn jj_has_conflicts(&self, rev: &str) -> Result<bool> {
        let output = self.run_jj_command(&[
            "log",
            "-r",
            rev,
            "-T",
            "conflict",
            "--no-graph",
        ])?;
        Ok(output.trim() == "true")
    }

    /// Set a jj bookmark to a specific revision.
    pub fn jj_set_bookmark(&self, name: &str, rev: &str) -> Result<()> {
        self.run_jj_command(&["bookmark", "set", name, "-r", rev])?;
        Ok(())
    }

    /// Create a new working copy commit at the given revision.
    ///
    /// This is equivalent to `jj new -r <rev>`.
    pub fn jj_new(&self, rev: &str) -> Result<()> {
        self.run_jj_command(&["new", "-r", rev])?;
        Ok(())
    }

    /// Squash the working copy changes into its parent.
    ///
    /// This is equivalent to `jj squash`.
    pub fn jj_squash(&self) -> Result<()> {
        self.run_jj_command(&["squash"])?;
        Ok(())
    }

    /// Check if a revision has conflicts.
    pub fn jj_revision_has_conflicts(&self, rev: &str) -> Result<bool> {
        self.jj_has_conflicts(rev)
    }

    /// Get the list of conflicted files in a revision.
    ///
    /// Uses `jj resolve --list` to get files with conflicts in the specified
    /// revision. Returns an empty set if the revision has no conflicts.
    pub fn jj_get_revision_conflicts(
        &self,
        rev: &str,
    ) -> Result<BTreeSet<Utf8PathBuf>> {
        // First check if the revision has conflicts at all. This avoids the
        // error from `jj resolve --list` when there are no conflicts.
        if !self.jj_has_conflicts(rev)? {
            return Ok(BTreeSet::new());
        }

        let output = self.run_jj_command(&["resolve", "--list", "-r", rev])?;

        let mut conflicts = BTreeSet::new();
        for line in output.lines() {
            // Output format: "path/to/file <conflict description>".
            // We only want the path, so take the first whitespace-separated field.
            let line = line.trim();
            if let Some(path) = line.split_whitespace().next() {
                conflicts.insert(Utf8PathBuf::from(path));
            }
        }
        Ok(conflicts)
    }

    /// Resolve conflicts in a specific commit using jj new + squash pattern.
    ///
    /// This creates a new working copy on top of the target revision, generates
    /// the correct files, and squashes them back into the target.
    pub fn jj_resolve_commit(
        &self,
        rev: &str,
        apis: &ManagedApis,
    ) -> Result<()> {
        // Create a new working copy as a child of the commit to fix, then
        // generate the correct documents.
        self.jj_new(rev)?;
        self.generate_documents(apis)?;
        self.jj_squash()?;

        // Verify the commit no longer has conflicts.
        if self.jj_has_conflicts(rev)? {
            return Err(anyhow::anyhow!(
                "jj conflict resolution failed: {} still has conflicts",
                rev
            ));
        }

        Ok(())
    }

    /// Helper to run jj commands in the workspace root.
    fn run_jj_command(&self, args: &[&str]) -> Result<String> {
        let jj = std::env::var("JJ").unwrap_or_else(|_| "jj".to_string());
        let output = Command::new(&jj)
            .current_dir(&self.workspace_root)
            .args(args)
            .output()
            .context("failed to execute jj command")?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "jj command failed: jj {}\nstdout: {}\nstderr: {}",
                args.join(" "),
                stdout,
                stderr
            ));
        }

        String::from_utf8(output.stdout)
            .context("jj command output was not valid UTF-8")
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
