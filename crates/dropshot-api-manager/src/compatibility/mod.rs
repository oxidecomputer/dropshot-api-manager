// Copyright 2026 Oxide Computer Company

//! Determine if one OpenAPI document is a subset of another.

mod detect;

pub use detect::{ApiCompatIssue, api_compatible};
