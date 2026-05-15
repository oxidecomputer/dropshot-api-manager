// Copyright 2026 Oxide Computer Company

//! Determine if one OpenAPI document is a subset of another.

mod detect;
mod display;
mod types;

pub use detect::api_compatible;
pub(crate) use detect::{CompatDedupeMap, DedupeStatus};
pub use types::ApiCompatIssue;
pub(crate) use types::CompatIssueLocation;
