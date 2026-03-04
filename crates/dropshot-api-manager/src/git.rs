// Copyright 2026 Oxide Computer Company

//! Helpers for accessing data stored in git

use anyhow::{Context, bail};
use camino::{Utf8Path, Utf8PathBuf};
use git_stub::GitCommitHash;
use std::process::Command;

/// Newtype String wrapper identifying a Git revision
///
/// This could be a commit, branch name, tag name, etc.  This type does not
/// validate the contents.
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct GitRevision(String);
NewtypeDebug! { () pub struct GitRevision(String); }
NewtypeDeref! { () pub struct GitRevision(String); }
NewtypeDerefMut! { () pub struct GitRevision(String); }
NewtypeDisplay! { () pub struct GitRevision(String); }
NewtypeFrom! { () pub struct GitRevision(String); }

/// Given a revision, return its merge base with the current working state.
///
/// If we're in the middle of a merge (MERGE_HEAD exists), we compute merge
/// bases for both HEAD and MERGE_HEAD, then use whichever is the descendant
/// (more recent). This handles both merge directions correctly:
///
/// - Merging main into branch: HEAD (p1) = branch, MERGE_HEAD (p2) = main.
///   We want main's merge base (which is main itself, containing all blessed
///   files).
/// - Merging branch into main: HEAD (p1) = main, MERGE_HEAD (p2) = branch. We
///   want main's merge base (main itself), not branch's merge base (the common
///   ancestor before main's changes).
///
/// In the rare case where the two merge bases are independent (neither is an
/// ancestor of the other), we fall back to HEAD's merge base.
pub fn git_merge_base_head(
    repo_root: &Utf8Path,
    revision: &GitRevision,
) -> anyhow::Result<GitCommitHash> {
    if git_merge_head_exists(repo_root) {
        // We're in a merge. Compute merge bases for both HEAD and MERGE_HEAD.
        let mb_head = git_merge_base(repo_root, "HEAD", revision)?;
        let mb_merge_head = git_merge_base(repo_root, "MERGE_HEAD", revision)?;

        // Use whichever merge base is the descendant (more recent). If mb_head
        // is an ancestor of mb_merge_head, use mb_merge_head (it's newer).
        // Otherwise, use mb_head (either it's newer, or they're parallel).
        if git_is_ancestor(repo_root, mb_head, mb_merge_head)? {
            Ok(mb_merge_head)
        } else {
            Ok(mb_head)
        }
    } else {
        git_merge_base(repo_root, "HEAD", revision)
    }
}

/// Compute the merge base between a reference and a revision.
fn git_merge_base(
    repo_root: &Utf8Path,
    base_ref: &str,
    revision: &GitRevision,
) -> anyhow::Result<GitCommitHash> {
    let mut cmd = git_start(repo_root);
    cmd.arg("merge-base").arg("--all").arg(base_ref).arg(revision.as_str());
    let label = cmd_label(&cmd);
    let stdout = do_run(&mut cmd)?;
    let stdout = stdout.trim();
    if stdout.contains(" ") || stdout.contains("\n") {
        bail!(
            "unexpected output from {} (contains whitespace -- \
             multiple merge bases?)",
            label
        );
    }
    stdout.parse().with_context(|| {
        format!("git merge-base returned invalid commit hash: {:?}", stdout)
    })
}

/// Check if `potential_ancestor` is an ancestor of `commit`.
pub(crate) fn git_is_ancestor(
    repo_root: &Utf8Path,
    potential_ancestor: GitCommitHash,
    commit: GitCommitHash,
) -> anyhow::Result<bool> {
    let mut cmd = git_start(repo_root);
    cmd.args([
        "merge-base",
        "--is-ancestor",
        &potential_ancestor.to_string(),
        &commit.to_string(),
    ]);
    let output =
        cmd.output().context("running git merge-base --is-ancestor")?;
    // --is-ancestor returns exit code 0 if true, 1 if false.
    // Other exit codes (e.g. 128 for invalid objects) indicate real errors.
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        Some(code) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!(
                "git merge-base --is-ancestor exited with unexpected \
                 code {code} (args: {} {}): {}",
                potential_ancestor,
                commit,
                stderr.trim(),
            ))
        }
        None => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!(
                "git merge-base --is-ancestor terminated by signal \
                 (args: {} {}): {}",
                potential_ancestor,
                commit,
                stderr.trim(),
            ))
        }
    }
}

/// Returns true if MERGE_HEAD exists, indicating we're in the middle of a
/// merge.
fn git_merge_head_exists(repo_root: &Utf8Path) -> bool {
    let mut cmd = git_start(repo_root);
    cmd.args(["rev-parse", "--verify", "--quiet", "MERGE_HEAD"]);
    matches!(cmd.status(), Ok(status) if status.success())
}

/// List files recursively under some path `path` in Git revision `revision`.
pub fn git_ls_tree(
    repo_root: &Utf8Path,
    revision: GitCommitHash,
    directory: &Utf8Path,
) -> anyhow::Result<Vec<Utf8PathBuf>> {
    let mut cmd = git_start(repo_root);
    cmd.arg("ls-tree")
        .arg("-r")
        .arg("-z")
        .arg("--name-only")
        .arg("--full-tree")
        .arg(revision.to_string())
        .arg(directory);
    let label = cmd_label(&cmd);
    let stdout = do_run(&mut cmd)?;
    stdout
        .trim()
        .split("\0")
        .filter(|s| !s.is_empty())
        .map(|path| {
            let found_path = Utf8PathBuf::from(path);
            let Ok(relative) = found_path.strip_prefix(directory) else {
                bail!(
                "git ls-tree unexpectedly returned a path that did not start \
                 with {:?}: {:?} (cmd: {})",
                directory,
                found_path,
                label,
            );
            };
            Ok(relative.to_owned())
        })
        .collect::<Result<Vec<_>, _>>()
}

/// Returns the contents of the file at the given path `path` in Git revision
/// `revision`.
pub fn git_show_file(
    repo_root: &Utf8Path,
    revision: GitCommitHash,
    path: &Utf8Path,
) -> anyhow::Result<Vec<u8>> {
    let mut cmd = git_start(repo_root);
    cmd.arg("cat-file").arg("blob").arg(format!("{}:{}", revision, path));
    let stdout = do_run(&mut cmd)?;
    Ok(stdout.into_bytes())
}

/// Returns the first commit where a file was introduced, searching up to and
/// including the given revision.
///
/// This is used to find a stable, canonical commit for Git stub storage. Using
/// the first commit (as opposed to something more readily available like the
/// merge base) ensures that if two different developers make changes to the
/// same API starting from different merge bases, this tool will convert the
/// previous blessed version into having the same contents for both developers.
/// This avoids an unnecessary merge conflict in the contents of the `.gitstub`
/// file.
pub fn git_first_commit_for_file(
    repo_root: &Utf8Path,
    revision: GitCommitHash,
    path: &Utf8Path,
) -> anyhow::Result<GitCommitHash> {
    // Use --diff-filter=A to find the commit that *added* the file, limiting
    // search to the given revision.
    //
    // We intentionally don't use --follow because Git's rename detection can
    // incorrectly match unrelated files with similar content, causing it to
    // return the wrong commit.
    //
    // We use -m to split merge commits, so that files added in merge commits
    // are properly detected. Without -m, git log may not show files that were
    // added in merge commits.
    let mut cmd = git_start(repo_root);
    cmd.arg("log")
        .arg("-m")
        .arg("--diff-filter=A")
        .arg("--format=%H")
        .arg(revision.to_string())
        .arg("--")
        .arg(path);
    let stdout = do_run(&mut cmd)?;
    let commit = stdout.trim();

    // If a file was removed and re-added, git log will show multiple commits
    // with --diff-filter=A. Take the first line (i.e. the most recent commit)
    // since that's the commit where the current version of the file was
    // introduced. The choice here is somewhat arbitrary, but it is consistent
    // across clones (which is important to minimize merge conflicts).
    let first_commit = commit.lines().next().with_context(|| {
        format!(
            "no commit found that added file {:?} \
             (searched backwards from {})",
            path, revision,
        )
    })?;

    // Git's --format=%H always returns full SHA-1 or SHA-256 hashes.
    first_commit.parse().with_context(|| {
        format!(
            "git returned invalid commit hash {:?} for {:?}",
            first_commit, path
        )
    })
}

/// Returns true if the repository is a shallow clone.
///
/// Shallow clones have truncated history, which can cause `git log` to return
/// incorrect results when searching for the commit that added a file. In a
/// shallow clone, files present at the shallow boundary appear to have been
/// "added" in the boundary commit, even if they were actually added earlier.
pub fn is_shallow_clone(repo_root: &Utf8Path) -> bool {
    let mut cmd = git_start(repo_root);
    cmd.arg("rev-parse").arg("--is-shallow-repository");
    match do_run(&mut cmd) {
        Ok(output) => output.trim() == "true",
        // If this failed, don't print a warning.
        Err(_) => false,
    }
}

/// Begin assembling an invocation of git(1)
fn git_start(repo_root: &Utf8Path) -> Command {
    let git = std::env::var("GIT").ok().unwrap_or_else(|| String::from("git"));
    let mut command = Command::new(&git);
    command.current_dir(repo_root);
    command
}

/// Runs an assembled git(1) command, returning stdout on success and an error
/// including the exit status and stderr contents on failure.
fn do_run(cmd: &mut Command) -> anyhow::Result<String> {
    let label = cmd_label(cmd);
    let output = cmd.output().with_context(|| format!("invoking {:?}", cmd))?;
    let status = output.status;
    let stdout = output.stdout;
    let stderr = output.stderr;
    if status.success() {
        if let Ok(stdout) = String::from_utf8(stdout) {
            return Ok(stdout);
        } else {
            bail!("command succeeded, but output was not UTF-8: {}:\n", label);
        }
    }

    bail!(
        "command failed: {}: {}\n\
        stderr:\n\
        -----\n\
        {}\n\
        -----\n",
        label,
        status,
        String::from_utf8_lossy(&stderr)
    );
}

/// Returns a string describing an assembled command (for debugging and error
/// reporting)
fn cmd_label(cmd: &Command) -> String {
    format!(
        "{:?} {}",
        cmd.get_program(),
        cmd.get_args()
            .map(|a| format!("{:?}", a))
            .collect::<Vec<_>>()
            .join(" ")
    )
}
