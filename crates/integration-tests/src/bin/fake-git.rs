// Copyright 2025 Oxide Computer Company

//! A fake git binary for testing error injection.
//!
//! This binary passes most commands through to the real git, but fails on
//! specific commands to test error handling.

use std::{
    env,
    process::{Command, exit},
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    // Fail on `git log --diff-filter=A` to test GitRefFirstCommitUnknown.
    if args.iter().any(|arg| arg == "--diff-filter=A") {
        eprintln!("fatal: simulated git failure for testing");
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
