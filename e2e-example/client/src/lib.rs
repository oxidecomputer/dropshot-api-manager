// Copyright 2026 Oxide Computer Company

//! Example client generated from a Git stub file.
//!
//! This crate demonstrates the end-to-end workflow for generating progenitor
//! clients from OpenAPI specs stored as Git stubs:
//!
//! 1. The build.rs uses `git-stub-vcs` to fetch the actual JSON content
//!    from git history and write it to `OUT_DIR/git-stub-vcs/`.
//!
//! 2. The `generate_api!` macro uses `relative_to = OutDir` to read the
//!    materialized file from `OUT_DIR`.

// Generate a client from the v1.0.0 API spec which is stored as a Git stub.
// The build script materializes this to OUT_DIR/git-stub-vcs/.
progenitor::generate_api!(
    spec = {
        path = "git-stub-vcs/e2e-example/documents/versioned-git-stub/versioned-git-stub-1.0.0-50a3d4.json",
        relative_to = OutDir,
    },
    interface = Builder,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_compiles() {
        // Verify that the generated client compiles and has the expected
        // structure.
        let _client = Client::new("http://localhost:8080");
    }

    #[test]
    fn test_types_exist() {
        // Verify that the expected types from v1.0.0 exist.
        let _thing: types::ThingV1 =
            types::ThingV1 { thing_str: "hello".to_string() };
    }
}
