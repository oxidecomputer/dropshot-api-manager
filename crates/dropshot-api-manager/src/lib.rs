// Copyright 2025 Oxide Computer Company

//! OpenAPI manager for Dropshot.
//!
//! This tool manages OpenAPI documents corresponding to
//! [Dropshot](https://docs.rs/dropshot) API traits. For more information, see
//! the [README](https://crates.io/crates/dropshot-api-manager).

#![warn(missing_docs)]

mod apis;
mod cmd;
mod compatibility;
mod environment;
/// Git utilities for accessing files and contents from git history.
pub mod git;
mod iter_only;
mod output;
mod resolved;
mod spec_files_blessed;
mod spec_files_generated;
mod spec_files_generic;
mod spec_files_local;
pub mod test_util;
mod validation;

#[macro_use]
extern crate newtype_derive;

pub use apis::*;
pub use cmd::dispatch::{App, FAILURE_EXIT_CODE, NEEDS_UPDATE_EXIT_CODE};
pub use environment::Environment;
