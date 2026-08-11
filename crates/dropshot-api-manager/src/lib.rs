// Copyright 2026 Oxide Computer Company

//! OpenAPI manager for Dropshot.
//!
//! This tool manages OpenAPI documents corresponding to
//! [Dropshot](https://docs.rs/dropshot) API traits. For more information, see
//! the [README](https://crates.io/crates/dropshot-api-manager).

#![warn(missing_docs)]

mod apis;
mod cmd;
mod compatibility;
mod doc_files_blessed;
mod doc_files_generated;
mod doc_files_generic;
mod doc_files_local;
mod environment;
mod iter_only;
mod output;
mod resolved;
pub mod test_util;
mod validation;
mod vcs;

#[macro_use]
extern crate newtype_derive;

pub use apis::*;
pub use cmd::dispatch::{App, FAILURE_EXIT_CODE, NEEDS_UPDATE_EXIT_CODE};
pub use environment::Environment;
