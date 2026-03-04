// Copyright 2026 Oxide Computer Company

//! VCS abstraction for accessing repository data.
//!
//! This module provides a unified interface for VCS operations needed by
//! the API manager. It wraps both Git and Jujutsu backends, delegating
//! to the appropriate implementation based on repository detection.

mod git;
mod imp;
mod jj;

pub use imp::VcsRevision;
pub(crate) use imp::{RepoVcs, RepoVcsKind};
