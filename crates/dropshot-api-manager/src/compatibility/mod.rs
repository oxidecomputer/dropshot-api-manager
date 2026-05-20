// Copyright 2026 Oxide Computer Company

//! Determine if one OpenAPI document is a subset of another.

mod detect;
mod display;
mod types;
mod wrap;

pub(crate) use detect::{
    CompatDedupMap, FinalizedCompatDedupMap, api_compatible,
};
pub(crate) use types::{
    ApiCompatIssue, CompatIssueLocation, CompatRenderStatus,
};
