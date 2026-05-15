// Copyright 2026 Oxide Computer Company

//! Determine if one OpenAPI document is a subset of another.

mod detect;
mod display;
mod types;

pub use detect::api_compatible;
pub use types::ApiCompatIssue;
