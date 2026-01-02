// Copyright 2025 Oxide Computer Company

//! Integration tests for dropshot-api-manager.

mod environment;
mod fixtures;

pub use environment::{
    JjMergeResult, JjRebaseResult, MergeResult, RebaseResult, TestEnvironment,
    check_jj_available, rel_path_forward_slashes,
};
pub use fixtures::*;
