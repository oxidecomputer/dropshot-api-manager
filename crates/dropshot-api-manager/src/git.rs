// Copyright 2025 Oxide Computer Company

//! Helpers for accessing data stored in git

use anyhow::{Context, bail};
use camino::{Utf8Path, Utf8PathBuf};
use std::{fmt, process::Command, str::FromStr};
use thiserror::Error;

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

/// A Git commit hash.
///
/// This type guarantees the contained string is either:
///
/// - 40 lowercase hex digits (SHA-1)
/// - 64 lowercase hex digits (SHA-256)
///
/// Use this type when you need to ensure a git reference is a specific commit,
/// and not a branch name, tag, or other symbolic reference.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitHash(String);

impl CommitHash {
    /// Returns the commit hash as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CommitHash {
    type Err = CommitHashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let len = s.len();
        if len != 40 && len != 64 {
            return Err(CommitHashParseError::InvalidLength(len));
        }

        for (position, c) in s.chars().enumerate() {
            if !matches!(c, '0'..='9' | 'a'..='f') {
                return Err(CommitHashParseError::InvalidCharacter {
                    position,
                    character: c,
                });
            }
        }

        Ok(CommitHash(s.to_owned()))
    }
}

impl fmt::Display for CommitHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<CommitHash> for GitRevision {
    fn from(hash: CommitHash) -> Self {
        GitRevision::from(hash.0)
    }
}

/// An error that occurs while parsing a commit hash.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CommitHashParseError {
    /// The commit hash has an invalid length.
    #[error(
        "invalid length: expected 40 (SHA-1) or 64 (SHA-256) hex characters, \
         got {0}"
    )]
    InvalidLength(usize),

    /// The commit hash contains an invalid character.
    #[error(
        "invalid character at position {position}: expected lowercase hex \
         digit, got {character:?}"
    )]
    InvalidCharacter {
        /// The position of the invalid character (0-indexed).
        position: usize,
        /// The invalid character.
        character: char,
    },
}

/// Given a revision, return its merge base with HEAD
pub fn git_merge_base_head(
    repo_root: &Utf8Path,
    revision: &GitRevision,
) -> anyhow::Result<GitRevision> {
    let mut cmd = git_start(repo_root);
    cmd.arg("merge-base").arg("--all").arg("HEAD").arg(revision.as_str());
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
    Ok(GitRevision::from(stdout.to_owned()))
}

/// List files recursively under some path `path` in Git revision `revision`.
pub fn git_ls_tree(
    repo_root: &Utf8Path,
    revision: &GitRevision,
    directory: &Utf8Path,
) -> anyhow::Result<Vec<Utf8PathBuf>> {
    let mut cmd = git_start(repo_root);
    cmd.arg("ls-tree")
        .arg("-r")
        .arg("-z")
        .arg("--name-only")
        .arg("--full-tree")
        .arg(revision.as_str())
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
    revision: &GitRevision,
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
/// This is used to find a stable, canonical commit for git ref storage. Using
/// the first commit ensures the git ref remains valid even as the repository
/// history evolves (e.g., when the merge-base changes).
pub fn git_first_commit_for_file(
    repo_root: &Utf8Path,
    revision: &GitRevision,
    path: &Utf8Path,
) -> anyhow::Result<CommitHash> {
    // Use --diff-filter=A to find the commit that *added* the file, limiting
    // search to the given revision.
    //
    // We intentionally don't use --follow because our API spec files have
    // content hashes in their names (e.g., api-1.0.0-abc123.json). Git's rename
    // detection can incorrectly match unrelated files with similar content,
    // causing it to return the wrong commit.
    let mut cmd = git_start(repo_root);
    cmd.arg("log")
        .arg("--diff-filter=A")
        .arg("--format=%H")
        .arg(revision.as_str())
        .arg("--")
        .arg(path);
    let stdout = do_run(&mut cmd)?;
    let commit = stdout.trim();
    if commit.is_empty() {
        bail!(
            "no commit found that added file {:?} (searched up to {})",
            path,
            revision
        );
    }
    // If there are multiple lines (shouldn't happen for --diff-filter=A in
    // normal use), take the last one (the earliest commit in which the file was
    // newly added).
    let first_commit = commit.lines().last().unwrap_or(commit);
    // Git's --format=%H always returns full SHA-1/SHA-256 hashes.
    first_commit.parse().with_context(|| {
        format!(
            "git returned invalid commit hash {:?} for {:?}",
            first_commit, path
        )
    })
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

/// Represents a Git reference to a file at a specific commit.
///
/// A git ref is stored as a string in the format `commit:path`, and can be
/// used to retrieve file contents via `git show`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRef {
    /// The commit hash (validated SHA-1 or SHA-256).
    pub commit: CommitHash,
    /// The path within the repository.
    pub path: Utf8PathBuf,
}

impl GitRef {
    /// Read the contents of the file at this git ref.
    pub fn read_contents(
        &self,
        repo_root: &Utf8Path,
    ) -> anyhow::Result<Vec<u8>> {
        git_show_file(
            repo_root,
            &GitRevision::from(self.commit.clone()),
            &self.path,
        )
    }
}

impl fmt::Display for GitRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.commit, self.path)
    }
}

impl FromStr for GitRef {
    type Err = GitRefParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (commit, path) = s
            .split_once(':')
            .ok_or_else(|| GitRefParseError::InvalidFormat(s.to_owned()))?;
        let commit: CommitHash = commit.parse()?;
        Ok(GitRef { commit, path: Utf8PathBuf::from(path) })
    }
}

/// An error that occurs while parsing a git ref.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GitRefParseError {
    /// The git ref string did not contain the expected 'commit:path' format.
    #[error("invalid git ref format: expected 'commit:path', got {0}")]
    InvalidFormat(String),

    /// The commit hash in the git ref was invalid.
    #[error("invalid commit hash")]
    InvalidCommitHash(#[from] CommitHashParseError),
}

#[cfg(test)]
mod tests {
    use super::*;

    // A valid SHA-1 hash for testing (40 lowercase hex chars).
    const VALID_SHA1: &str = "0123456789abcdef0123456789abcdef01234567";
    // A valid SHA-256 hash for testing (64 lowercase hex chars).
    const VALID_SHA256: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn test_commit_hash_valid() {
        // SHA-1 (40 chars).
        let hash: CommitHash = VALID_SHA1.parse().unwrap();
        assert_eq!(hash.as_str(), VALID_SHA1);

        // SHA-256 (64 chars).
        let hash: CommitHash = VALID_SHA256.parse().unwrap();
        assert_eq!(hash.as_str(), VALID_SHA256);

        // Display trait.
        let hash: CommitHash = VALID_SHA1.parse().unwrap();
        assert_eq!(hash.to_string(), VALID_SHA1);

        // Conversion to GitRevision.
        let hash: CommitHash = VALID_SHA1.parse().unwrap();
        let revision: GitRevision = hash.into();
        assert_eq!(revision.as_str(), VALID_SHA1);
    }

    #[test]
    fn test_commit_hash_invalid() {
        // Too short.
        assert!(matches!(
            "abc123".parse::<CommitHash>(),
            Err(CommitHashParseError::InvalidLength(6))
        ));

        // 39 chars (one short of SHA-1).
        assert!(matches!(
            VALID_SHA1[..39].parse::<CommitHash>(),
            Err(CommitHashParseError::InvalidLength(39))
        ));

        // 41 chars (one over SHA-1).
        let input = format!("{}0", VALID_SHA1);
        assert!(matches!(
            input.parse::<CommitHash>(),
            Err(CommitHashParseError::InvalidLength(41))
        ));

        // Empty string.
        assert!(matches!(
            "".parse::<CommitHash>(),
            Err(CommitHashParseError::InvalidLength(0))
        ));

        // Uppercase hex (valid length, invalid chars).
        assert!(matches!(
            "0123456789ABCDEF0123456789abcdef01234567".parse::<CommitHash>(),
            Err(CommitHashParseError::InvalidCharacter {
                position: 10,
                character: 'A'
            })
        ));

        // Non-hex character 'g'.
        assert!(matches!(
            "0123456789abcdefg123456789abcdef01234567".parse::<CommitHash>(),
            Err(CommitHashParseError::InvalidCharacter {
                position: 16,
                character: 'g'
            })
        ));

        // Leading whitespace (not trimmed).
        let input = format!(" {}", VALID_SHA1);
        assert!(matches!(
            input.parse::<CommitHash>(),
            Err(CommitHashParseError::InvalidLength(41))
        ));
    }

    #[test]
    fn test_git_ref_parse() {
        let input = format!("{}:openapi/api/api-1.0.0-def456.json", VALID_SHA1);
        let git_ref = input.parse::<GitRef>().unwrap();
        assert_eq!(git_ref.commit.as_str(), VALID_SHA1);
        assert_eq!(git_ref.path.as_str(), "openapi/api/api-1.0.0-def456.json");
    }

    #[test]
    fn test_git_ref_parse_with_whitespace() {
        let input = format!("  {}:path/file.json\n", VALID_SHA1);
        let git_ref = input.parse::<GitRef>().unwrap();
        assert_eq!(git_ref.commit.as_str(), VALID_SHA1);
        assert_eq!(git_ref.path.as_str(), "path/file.json");
    }

    #[test]
    fn test_git_ref_parse_invalid_no_colon() {
        let result = "no-colon".parse::<GitRef>();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GitRefParseError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_git_ref_parse_invalid_empty() {
        let result = "".parse::<GitRef>();
        assert!(result.is_err());
    }

    #[test]
    fn test_git_ref_parse_invalid_commit_hash() {
        // Valid format but invalid commit hash (too short).
        let result = "abc123:path/file.json".parse::<GitRef>();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GitRefParseError::InvalidCommitHash(_)
        ));
    }

    #[test]
    fn test_git_ref_roundtrip() {
        let git_ref = GitRef {
            commit: VALID_SHA1.parse().unwrap(),
            path: Utf8PathBuf::from("path/to/file.json"),
        };
        let s = git_ref.to_string();
        let expected = format!("{}:path/to/file.json", VALID_SHA1);
        assert_eq!(s, expected);
        let parsed = s.parse::<GitRef>().unwrap();
        assert_eq!(git_ref, parsed);
    }
}
