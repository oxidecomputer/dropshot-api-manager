// Copyright 2026 Oxide Computer Company

//! Helpers for accessing data stored in Jujutsu repositories.
//!
//! These functions are the Jujutsu equivalents of the Git operations in
//! `git.rs`. They are called from `RepoVcs` when the detected backend
//! is Jujutsu.

use super::imp::{VcsRevision, cmd_label, do_run, do_run_bytes};
use anyhow::{Context, bail};
use camino::{Utf8Path, Utf8PathBuf};
use git_stub::GitCommitHash;
use std::process::Command;

/// Given a revision, return its merge base with the current working state.
///
/// In jj, `@` is the working-copy commit (which is the merge commit during
/// a merge, unlike Git where HEAD only points to p1). So
/// `heads(::@ & ::REV)` returns the correct merge base without needing to
/// special-case in-progress merges.
pub(super) fn jj_merge_base_head(
    repo_root: &Utf8Path,
    revision: &VcsRevision,
) -> anyhow::Result<GitCommitHash> {
    let mut cmd = jj_start(repo_root);
    cmd.args([
        "log",
        "--revisions",
        // The revision is a user-supplied revset expression like `trunk()`, so
        // we cannot quote it. We use parens instead.
        &format!("heads(::@ & ::({}))", revision),
        "--template",
        "commit_id ++ \"\\n\"",
        "--no-graph",
    ]);
    let stdout = do_run(&mut cmd)?;
    let stdout = stdout.trim();

    if stdout.is_empty() {
        bail!(
            "no merge base found between @ and {revision} \
             (is the revision valid?)"
        );
    }

    // We expect exactly one merge base. Multiple results indicate a
    // criss-cross merge, which we don't support.
    let mut lines = stdout.lines();
    // The empty check above guarantees at least one line.
    let first_line =
        lines.next().expect("non-empty stdout has at least one line");
    if lines.next().is_some() {
        bail!(
            "multiple merge bases found between @ and {revision} \
             (criss-cross merge?)"
        );
    }

    first_line.parse().with_context(|| {
        format!(
            "jj returned unexpected merge-base output {:?} \
             (expected a commit hash)",
            first_line,
        )
    })
}

/// Check if `potential_ancestor` is an ancestor of `commit`.
///
/// The revset used is `potential_ancestor & ::commit`.
pub(super) fn jj_is_ancestor(
    repo_root: &Utf8Path,
    potential_ancestor: GitCommitHash,
    commit: GitCommitHash,
) -> anyhow::Result<bool> {
    let mut cmd = jj_start(repo_root);
    cmd.args([
        "log",
        "--revisions",
        &format!("{potential_ancestor} & ::{commit}"),
        "--template",
        "commit_id",
        "--no-graph",
    ]);
    let stdout = do_run(&mut cmd)?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }
    // The output should be a hex commit ID.
    if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "unexpected output from jj ancestor check: {:?} \
             (expected a commit ID or empty output)",
            trimmed,
        );
    }
    Ok(true)
}

/// List files recursively under `directory` in the given revision.
///
/// Returns paths relative to `directory`.
pub(super) fn jj_list_files(
    repo_root: &Utf8Path,
    revision: GitCommitHash,
    directory: &Utf8Path,
) -> anyhow::Result<Vec<Utf8PathBuf>> {
    let mut cmd = jj_start(repo_root);
    cmd.args([
        "file",
        "list",
        "--revision",
        &revision.to_string(),
        "--",
        directory.as_str(),
    ]);
    let label = cmd_label(&cmd);
    let stdout = do_run(&mut cmd)?;

    stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            // `jj file list` returns paths relative to the repo root. To match
            // `git ls-tree`, strip the directory prefix.
            let found_path = Utf8PathBuf::from(line);
            let Ok(relative) = found_path.strip_prefix(directory) else {
                bail!(
                    "jj file list returned a path that did not start \
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

/// Return the contents of a file at the given path in a revision.
pub(super) fn jj_show_file(
    repo_root: &Utf8Path,
    revision: GitCommitHash,
    path: &Utf8Path,
) -> anyhow::Result<Vec<u8>> {
    let mut cmd = jj_start(repo_root);
    cmd.args(["file", "show", "--revision", &revision.to_string(), "--"])
        .arg(path);
    do_run_bytes(&mut cmd)
}

/// Find the most recent commit that *introduced* a file at a given
/// path, searching backwards from the given revision.
///
/// This is the jj equivalent of Git's `git log --diff-filter=A`. The
/// revset language has no built-in filter for addition vs modification,
/// so we use a two-stage approach:
///
/// 1. The revset `files("path") & ::revision` narrows to commits that
///    touched the file within the ancestor set.
/// 2. A template filter checks each commit's diff to see if the file
///    was introduced at this path. We match added, renamed, and
///    copied statuses, because (particularly with Git stubs) jj's
///    rename detection is likely to kick in and classify a new API
///    version as renamed.
///
/// We take the first (most recent) introducing commit, matching Git's
/// behavior for files that were removed and re-added.
pub(super) fn jj_first_commit_for_file(
    repo_root: &Utf8Path,
    revision: GitCommitHash,
    path: &Utf8Path,
) -> anyhow::Result<GitCommitHash> {
    let quoted_path = revset_quote(path.as_str());

    // The template checks each commit's diff for the file and only
    // emits the commit ID when the file was introduced at this path.
    let template = format!(
        // A/R/C stand for added/renamed (not removed, which is D)/copied.
        "if(self.diff({quoted_path}).files().any(\
             |entry| entry.status_char() == \"A\" \
                  || entry.status_char() == \"R\" \
                  || entry.status_char() == \"C\"\
         ), commit_id ++ \"\\n\", \"\")",
    );

    let mut cmd = jj_start(repo_root);
    cmd.args([
        "log",
        "--revisions",
        &format!("files({quoted_path}) & ::{revision}"),
        "--template",
        &template,
        "--no-graph",
        // We cannot use --limit 1 here unfortunately, since the limit doesn't
        // take into account the template producing an empty string.
    ]);
    let stdout = do_run(&mut cmd)?;
    let commit = stdout.trim();

    // Take the first line (most recent commit that added the file).
    let first_commit = commit.lines().next().with_context(|| {
        format!(
            "no commit found that added file {:?} \
             (searched backwards from {})",
            path, revision,
        )
    })?;

    first_commit.parse().with_context(|| {
        format!(
            "jj returned invalid commit hash {:?} for {:?}",
            first_commit, path
        )
    })
}

/// Quote a string for use inside a jj revset expression.
///
/// Uses double quotes with `"` and `\` escaped. File paths might contain
/// characters with special meaning in the revset grammar, a common one being
/// `-`.
fn revset_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Begin assembling an invocation of jj.
///
/// Passes `--no-pager`, `--color=never`, and
/// `--ignore-working-copy` so that output is deterministic and
/// parseable regardless of user configuration.
fn jj_start(repo_root: &Utf8Path) -> Command {
    let jj = std::env::var("JJ").ok().unwrap_or_else(|| String::from("jj"));
    let mut command = Command::new(&jj);
    command.current_dir(repo_root);
    command.args(["--no-pager", "--color", "never", "--ignore-working-copy"]);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_revset_quote() {
        assert_eq!(revset_quote("trunk()"), r#""trunk()""#);
        assert_eq!(revset_quote("main"), r#""main""#);

        // Dashes, parentheses, and other revset-significant characters
        // should pass through unescaped since they're inside quotes.
        assert_eq!(revset_quote("my-feature"), r#""my-feature""#);
        assert_eq!(revset_quote("path/to/file.json"), r#""path/to/file.json""#);

        // Quotes and backslashes are escaped.
        assert_eq!(
            revset_quote(r#"they said "hello""#),
            r#""they said \"hello\"""#
        );
        assert_eq!(revset_quote(r"path\to\file"), r#""path\\to\\file""#);

        assert_eq!(revset_quote(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(revset_quote(""), r#""""#);
    }
}
