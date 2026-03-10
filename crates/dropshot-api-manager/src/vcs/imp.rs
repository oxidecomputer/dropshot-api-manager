// Copyright 2026 Oxide Computer Company

use anyhow::{Context, bail};
use camino::{Utf8Path, Utf8PathBuf};
use git_stub::{GitCommitHash, GitStub};
use git_stub_vcs::Vcs;
use std::process::Command;

/// Newtype String wrapper identifying a VCS revision.
///
/// For Git, this could be a commit hash, branch name, tag name, etc.
/// For Jujutsu, this could be a revset expression like `"trunk()"` or
/// a commit ID.
///
/// This type does not validate the contents.
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct VcsRevision(String);
NewtypeDebug! { () pub struct VcsRevision(String); }
NewtypeDeref! { () pub struct VcsRevision(String); }
NewtypeDisplay! { () pub struct VcsRevision(String); }
NewtypeFrom! { () pub struct VcsRevision(String); }

/// VCS abstraction for repository operations.
///
/// This wraps the detected VCS backend (Git or Jujutsu) and provides
/// methods for the operations the API manager needs: merge-base
/// computation, file listing, file content retrieval, and ancestry
/// checks.
///
/// The cached [`Vcs`] is used for operations that delegate to
/// `git_stub_vcs` (shallow clone detection, stub resolution).
#[derive(Clone, Debug)]
pub(crate) struct RepoVcs {
    // These two are kept in sync by from_git_stub_vcs. (Why store RepoVcsKind
    // at all? Because git_stub_vcs::VcsName is non-exhaustive, and it would be
    // annoying to have to have `unreachable` or some other panic over and
    // over.)
    kind: RepoVcsKind,
    stub_vcs: Vcs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepoVcsKind {
    Git,
    Jj,
}

impl RepoVcs {
    /// Detect the VCS backend for the given repository root.
    ///
    /// Delegates to `git_stub_vcs::Vcs::detect()`, which checks for `.jj`
    /// first (including colocated repos), then `.git`.
    pub(crate) fn detect(repo_root: &Utf8Path) -> anyhow::Result<Self> {
        let vcs = Vcs::detect(repo_root)
            .with_context(|| format!("detecting VCS at {repo_root}"))?;
        Self::from_git_stub_vcs(vcs)
    }

    /// Create a `RepoVcs` for the Git backend.
    #[allow(dead_code)]
    pub(crate) fn git() -> anyhow::Result<Self> {
        let vcs = Vcs::git().context("initializing Git VCS")?;
        Self::from_git_stub_vcs(vcs)
    }

    /// Create a `RepoVcs` for the Jujutsu backend.
    #[allow(dead_code)]
    pub(crate) fn jj() -> anyhow::Result<Self> {
        let vcs = Vcs::jj().context("initializing Jujutsu VCS")?;
        Self::from_git_stub_vcs(vcs)
    }

    fn from_git_stub_vcs(vcs: Vcs) -> anyhow::Result<Self> {
        let kind = match vcs.name() {
            git_stub_vcs::VcsName::Git => RepoVcsKind::Git,
            git_stub_vcs::VcsName::Jj => RepoVcsKind::Jj,
            // git_stub_vcs::VcsName is non-exhaustive. Return an error
            // so we notice if a new variant is added.
            other => bail!("unsupported VCS backend: {other:?}"),
        };
        Ok(Self { kind, stub_vcs: vcs })
    }

    /// Returns the VCS backend kind.
    pub(crate) fn kind(&self) -> RepoVcsKind {
        self.kind
    }

    /// Compute the merge base between the current working state and a
    /// revision.
    ///
    /// For Git, this handles in-progress merges by also checking
    /// MERGE_HEAD. For Jujutsu, `@` is the merge commit, so
    /// `heads(::@ & ::REV)` naturally handles all parent histories.
    pub(crate) fn merge_base_head(
        &self,
        repo_root: &Utf8Path,
        revision: &VcsRevision,
    ) -> anyhow::Result<GitCommitHash> {
        match &self.kind {
            RepoVcsKind::Git => {
                super::git::git_merge_base_head(repo_root, revision)
            }
            RepoVcsKind::Jj => {
                super::jj::jj_merge_base_head(repo_root, revision)
            }
        }
    }

    /// Check if `potential_ancestor` is an ancestor of `commit`.
    pub(crate) fn is_ancestor(
        &self,
        repo_root: &Utf8Path,
        potential_ancestor: GitCommitHash,
        commit: GitCommitHash,
    ) -> anyhow::Result<bool> {
        match &self.kind {
            RepoVcsKind::Git => super::git::git_is_ancestor(
                repo_root,
                potential_ancestor,
                commit,
            ),
            RepoVcsKind::Jj => {
                super::jj::jj_is_ancestor(repo_root, potential_ancestor, commit)
            }
        }
    }

    /// List files recursively under `directory` in the given revision.
    ///
    /// Returns paths relative to `directory`.
    pub(crate) fn list_files(
        &self,
        repo_root: &Utf8Path,
        revision: GitCommitHash,
        directory: &Utf8Path,
    ) -> anyhow::Result<Vec<Utf8PathBuf>> {
        match &self.kind {
            RepoVcsKind::Git => {
                super::git::git_ls_tree(repo_root, revision, directory)
            }
            RepoVcsKind::Jj => {
                super::jj::jj_list_files(repo_root, revision, directory)
            }
        }
    }

    /// Return the contents of a file at the given path in a revision.
    pub(crate) fn show_file(
        &self,
        repo_root: &Utf8Path,
        revision: GitCommitHash,
        path: &Utf8Path,
    ) -> anyhow::Result<Vec<u8>> {
        match &self.kind {
            RepoVcsKind::Git => {
                super::git::git_show_file(repo_root, revision, path)
            }
            RepoVcsKind::Jj => {
                super::jj::jj_show_file(repo_root, revision, path)
            }
        }
    }

    /// Find the most recent commit that *added* a file, searching
    /// backwards from the given revision.
    ///
    /// For Git, this uses `--diff-filter=A`. For Jujutsu, this uses a
    /// revset to find touching commits and a template filter to select
    /// only those where the file's diff status is `"added"`.
    pub(crate) fn first_commit_for_file(
        &self,
        repo_root: &Utf8Path,
        revision: GitCommitHash,
        path: &Utf8Path,
    ) -> anyhow::Result<GitCommitHash> {
        match &self.kind {
            RepoVcsKind::Git => {
                super::git::git_first_commit_for_file(repo_root, revision, path)
            }
            RepoVcsKind::Jj => {
                super::jj::jj_first_commit_for_file(repo_root, revision, path)
            }
        }
    }

    /// Returns true if the repository is a shallow clone.
    ///
    /// If the check fails (e.g. because the VCS binary is missing or the
    /// repository is corrupted), a warning is printed and `false` is
    /// returned. This avoids blocking on an environment issue at the
    /// cost of potentially incorrect behavior if the repo truly is
    /// shallow; the downstream git-stub resolution will surface a
    /// clearer error in that case.
    pub(crate) fn is_shallow_clone(&self, repo_root: &Utf8Path) -> bool {
        match self.stub_vcs.is_shallow_clone(repo_root) {
            Ok(is_shallow) => is_shallow,
            Err(err) => {
                eprintln!(
                    "warning: failed to check if repository is a \
                     shallow clone: {err:#}"
                );
                false
            }
        }
    }

    /// Resolve a Git stub to its JSON document contents.
    pub(crate) fn resolve_stub_contents(
        &self,
        git_stub: &GitStub,
        repo_root: &Utf8Path,
    ) -> anyhow::Result<Vec<u8>> {
        Ok(self.stub_vcs.read_git_stub_contents(git_stub, repo_root)?)
    }
}

// ---- Shared command-runner utilities for git.rs and jj.rs ----

/// Runs a command, returning stdout as raw bytes on success. Unlike
/// [`do_run`], this does not require the output to be valid UTF-8 and
/// is suitable for commands that return file contents.
pub(super) fn do_run_bytes(cmd: &mut Command) -> anyhow::Result<Vec<u8>> {
    let label = cmd_label(cmd);
    let output = cmd.output().with_context(|| format!("invoking {:?}", cmd))?;
    if output.status.success() {
        return Ok(output.stdout);
    }

    bail!(
        "command failed: {}: {}\n\
        stderr:\n\
        -----\n\
        {}\n\
        -----\n",
        label,
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs a command, returning stdout on success and an error including
/// the exit status and stderr contents on failure.
pub(super) fn do_run(cmd: &mut Command) -> anyhow::Result<String> {
    let label = cmd_label(cmd);
    let stdout = do_run_bytes(cmd)?;
    String::from_utf8(stdout).with_context(|| {
        format!("command succeeded, but output was not UTF-8: {label}")
    })
}

/// Returns a string describing an assembled command (for debugging and
/// error reporting).
pub(super) fn cmd_label(cmd: &Command) -> String {
    format!(
        "{:?} {}",
        cmd.get_program(),
        cmd.get_args()
            .map(|a| format!("{:?}", a))
            .collect::<Vec<_>>()
            .join(" ")
    )
}
