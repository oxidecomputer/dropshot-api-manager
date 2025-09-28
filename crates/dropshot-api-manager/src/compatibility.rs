// Copyright 2025 Oxide Computer Company

//! Determine if one OpenAPI spec is a subset of another

use std::fmt;

use drift::{Change, ChangeClass};

#[derive(Debug)]
pub struct OpenApiCompatibilityError(Change);

impl fmt::Display for OpenApiCompatibilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Change {
            message,
            old_path,
            new_path,
            comparison: _,
            class,
            details: _,
        } = &self.0;
        let old_path_str = old_path.iter().next().unwrap();
        let new_path_str = new_path.iter().next().unwrap();
        write!(f, "{} change at {}", change_class_str(class), old_path_str)?;
        if new_path_str != old_path_str {
            write!(f, " (-> {})", new_path_str)?;
        }
        write!(f, ": {}", message)?;
        // foo bar
        Ok(())
    }

    // another change
}

pub fn api_compatible(
    spec1: &serde_json::Value,
    spec2: &serde_json::Value,
) -> anyhow::Result<Vec<OpenApiCompatibilityError>> {
    let changes = drift::compare(spec1, spec2)?;
    let changes = changes
        .into_iter()
        .filter_map(|change| match change.class {
            ChangeClass::BackwardIncompatible
            | ChangeClass::ForwardIncompatible
            | ChangeClass::Incompatible
            | ChangeClass::Unhandled => Some(OpenApiCompatibilityError(change)),
            ChangeClass::Trivial => None,
        })
        .collect();
    Ok(changes)
}

pub fn change_class_str(class: &ChangeClass) -> &'static str {
    match class {
        ChangeClass::BackwardIncompatible => "backward-incompatible",
        ChangeClass::ForwardIncompatible => "forward-incompatible",
        ChangeClass::Incompatible => "incompatible",
        ChangeClass::Unhandled => "unhandled",
        ChangeClass::Trivial => "trivial",
    }
}
