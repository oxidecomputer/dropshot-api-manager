// Copyright 2026 Oxide Computer Company

//! Integration tests for dropshot-api-manager.

mod environment;
mod fixtures;

use camino::Utf8PathBuf;
pub use environment::{
    JjMergeResult, JjRebaseResult, MergeResult, RebaseResult, TestEnvironment,
    check_jj_available, rel_path_forward_slashes,
};
pub use fixtures::*;
use std::collections::{BTreeMap, BTreeSet};

/// The kind of conflict expected on a file during merge/rebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedConflictKind {
    /// Both branches "rename" the same source file to different destinations.
    ///
    /// Only applies to git; jj does not have rename detection.
    Rename,

    /// Both branches update a symlink to point to different targets.
    Symlink,

    /// Git's rename/delete conflict involving content-similar files.
    ///
    /// When a commit deletes one file and creates another with similar content,
    /// git may detect this as a rename. If the rename source was also deleted
    /// by another branch (e.g., during conflict resolution), git reports a
    /// rename/delete conflict involving the source, destination, and related
    /// files.
    ///
    /// jj does not have rename detection, so these don't occur in jj. (Note
    /// that jj does detect true change/delete conflicts where one side modifies
    /// a file and the other deletes it.)
    RenameDelete,
}

/// A map from file path to expected conflict kind.
pub type ExpectedConflicts = BTreeMap<Utf8PathBuf, ExpectedConflictKind>;

/// Extracts all conflicted file paths. Used by git tests.
pub fn all_conflict_paths(
    conflicts: &ExpectedConflicts,
) -> BTreeSet<Utf8PathBuf> {
    conflicts.keys().cloned().collect()
}

/// Extracts conflict paths that jj would report. Used by jj tests.
///
/// jj doesn't have rename detection. When git detects content-similar files
/// as renames (e.g., v3-alt1.json -> v3-alt2.json at 80% similarity), it
/// reports rename/delete conflicts involving all related files. Without
/// rename detection, jj treats these as independent operations: deleting an
/// already-deleted file is a no-op, creating a new file is clean. Only
/// symlink conflicts (where both sides modify the target) are reported by jj.
pub fn jj_conflict_paths(
    conflicts: &ExpectedConflicts,
) -> BTreeSet<Utf8PathBuf> {
    conflicts
        .iter()
        .filter(|(_, kind)| **kind == ExpectedConflictKind::Symlink)
        .map(|(path, _)| path.clone())
        .collect()
}
