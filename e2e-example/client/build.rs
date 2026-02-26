// Copyright 2026 Oxide Computer Company

//! Build script that materializes Git stubs for progenitor to consume.
//!
//! This demonstrates the workflow for generating clients from older API
//! versions that are stored as git stubs rather than full JSON files.

use git_stub_vcs::Materializer;

fn main() {
    // This file is two levels down from the root of the repository.
    let materializer = Materializer::for_build_script("../..")
        .expect("detected VCS at repo root");

    // Materialize the v1.0.0 API spec from its Git stub.
    //
    // The .gitstub file contains a reference like:
    //   11ce810ee5...:e2e-example/documents/versioned-git-stub/versioned-git-stub-1.0.0-50a3d4.json
    //
    // The materializer will:
    //
    // 1. Read the git-stub file.
    // 2. Fetch the content from git (or jj) history.
    // 3. Write it to OUT_DIR/git-stub-vcs/e2e-example/documents/versioned-git-stub/versioned-git-stub-1.0.0-50a3d4.json.
    let spec_path = materializer
        .materialize(
            "e2e-example/documents/versioned-git-stub/versioned-git-stub-1.0.0-50a3d4.json.gitstub",
        )
        .expect("materialized git-stub file");

    // Print the path for debugging purposes.
    println!("cargo::warning=materialized spec to: {}", spec_path);
}
