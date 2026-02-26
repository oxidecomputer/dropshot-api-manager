// Copyright 2026 Oxide Computer Company

//! A fake git binary for testing error injection.
//!
//! This binary passes most commands through to the real git, but can be
//! configured via environment variables to fail on specific commands.
//!
//! ## Environment variables
//!
//! - `REAL_GIT`: Path to the real git binary. Defaults to `git`.
//! - `FAKE_GIT_FAIL`: Comma-separated list of failure modes to enable.
//!   Available modes:
//!   - `diff_filter_a`: Fail on `git log --diff-filter=A` commands.
//!   - `is_ancestor`: Fail on `git merge-base --is-ancestor` commands.
//!
//!   If unset, `diff_filter_a` is enabled for backwards compatibility
//!   with existing tests.

use std::{
    env,
    process::{Command, exit},
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    let fail_modes = env::var("FAKE_GIT_FAIL")
        .unwrap_or_else(|_| "diff_filter_a".to_string());
    let fail_modes: Vec<&str> = fail_modes.split(',').collect();

    // Fail on `git log --diff-filter=A` to test GitStubFirstCommitUnknown.
    if fail_modes.contains(&"diff_filter_a")
        && args.iter().any(|arg| arg == "--diff-filter=A")
    {
        eprintln!("fatal: simulated git failure for testing (diff_filter_a)");
        exit(128);
    }

    // Fail on `git merge-base --is-ancestor` to test git_is_ancestor error
    // handling.
    if fail_modes.contains(&"is_ancestor")
        && args.iter().any(|arg| arg == "--is-ancestor")
    {
        eprintln!("fatal: simulated git failure for testing (is_ancestor)");
        exit(128);
    }

    // Otherwise, pass through to the real git.
    let git = env::var("REAL_GIT").unwrap_or_else(|_| "git".to_string());
    let status = Command::new(git)
        .args(&args)
        .status()
        .expect("failed to execute real git");

    match status.code() {
        Some(code) => exit(code),
        None => {
            // No exit code was available (maybe a signal?)
            exit(101);
        }
    }
}
