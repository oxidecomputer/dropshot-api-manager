# General guidelines

This document captures code conventions for the dropshot-api-manager project. It is intended to help LLMs understand how to work effectively with this codebase.

## General conventions

### Correctness over convenience

- Model the full error space—no shortcuts or simplified error handling.
- Handle and test for all edge cases.
- Use the type system to encode correctness constraints.
- Prefer compile-time guarantees over runtime checks where possible.

### User experience as a primary driver

- Provide structured, helpful error messages using `anyhow` with rich context chains.
- Make progress reporting responsive and informative (e.g., `Checking`, `Fresh`, `Stale` headers).
- Maintain consistency across platforms even when underlying OS capabilities differ.
- Evolve the design incrementally rather than attempting perfect upfront architecture.
- Document design decisions and trade-offs in code comments.

### Production-grade engineering

- Use type system extensively: newtypes, builder patterns, type states, lifetimes.
- Test comprehensively, including edge cases.
- Pay attention to what facilities already exist for testing, and aim to reuse them.
- Getting the details right is really important!

### Documentation

- Use inline comments to explain "why," not "what."
- Module-level documentation should explain purpose and responsibilities.
- **Always** use periods at the end of code comments.
- **Never** use title case in headings and titles. Always use sentence case.
- Wrap comments to 80 characters.

## Code style

### File headers

Every Rust source file must start with:
```rust
// Copyright (current year) Oxide Computer Company
```

**Important:** When editing a file, compare the current year to the year in the copyright header. If it is different, update the copyright header to the current year.

### Rust edition and formatting

- Use Rust 2024 edition.
- Format with `cargo xfmt` (custom formatting script).
- Formatting is enforced in CI—always run `cargo xfmt` before committing.

### Type system patterns

- **Newtypes** for domain types (e.g., `ApiIdent`, `GitRevision`, `GitCommitHash`, `ApiSpecFileName`)
- **Builder patterns** for complex construction (e.g., `ManagedApi` with `with_extra_validation`, `with_git_ref_storage`)
- **Type states** encoded in generics when state transitions matter
- **Lifetimes** used extensively to avoid cloning (e.g., `Problem<'a>`, `Resolution<'a>`, `Fix<'a>`)
- **Restricted visibility**: Use `pub(crate)` and `pub(super)` liberally
- **Non-exhaustive**: Consider forward compatibility for public error types

### Error handling

- Use `thiserror` for error types with `#[derive(Error)]`.
- Group errors by category with specific enums (e.g., `Problem`, `Note`).
- Provide rich error context using `anyhow::Context`.
- Two-tier model:
  - **Problems**: User-visible issues that may be fixable or unfixable.
  - **Internal errors**: Programming errors that may panic or use internal error types.
- Error display messages should be lowercase sentence fragments suitable for "failed to {error}".
- Errors should explain: What happened? Why? How to fix it?

### Module organization

- Use `mod.rs` files to re-export public items.
- Do not put any nontrivial logic in `mod.rs`—instead, it should go in specific submodules.
- Keep module boundaries strict with restricted visibility.
- Platform-specific code in separate files: `unix.rs`, `windows.rs`.
- Use `#[cfg(unix)]` and `#[cfg(windows)]` for conditional compilation.
- Test helpers in dedicated modules/files (e.g., `test_util`, `integration-tests` crate).

### Memory and performance

- Use `Arc` or borrows for shared immutable data.
- Use `&'static str` when appropriate.
- Careful attention to copying vs. referencing.
- Use `debug-ignore` to hide irrelevant information.
- Use iterators where possible rather than buffering.

## Testing practices

### Running tests

**CRITICAL**: Always use `cargo nextest run` to run unit and integration tests. Never use `cargo test` for these! This project uses nextest for its execution model, including process isolation which makes it safe to alter environment variables within tests.

For doctests, use `cargo test --doc` (doctests are not supported by nextest).

### Altering environment variables in tests

Since this repository uses nextest, which is process-per-test, it is safe to alter the environment within tests. Whenever you do that, add to the unsafe block:

```rust
// SAFETY:
// https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
```

### Test organization

- Unit tests in the same file as the code they test.
- Integration tests in the `integration-tests/` crate.
- Test fixtures in `integration-tests/src/fixtures.rs`.
  - This file provides model APIs for common test scenarios (lockstep, versioned, git-ref).
  - Prefer using these fixtures over implementing spot checks by hand.
- Test utilities in `test_util` module within the main crate.

### Testing tools

- **assert_matches**: For matching enum variants in assertions.
- **camino-tempfile**: For temporary file/directory operations with UTF-8 paths.
- **camino-tempfile-ext**: Extensions for camino-tempfile.

## Architecture

### Purpose

The dropshot-api-manager manages OpenAPI documents corresponding to [Dropshot](https://docs.rs/dropshot) API traits. It supports:

- **Lockstep versioning**: Clients and servers always match; single OpenAPI document per API.
- **Versioned APIs**: Multiple versions supported simultaneously; enables online upgrades where clients and servers can be temporarily mismatched.

### Core concepts

#### API sources

The tool reconciles OpenAPI documents from three sources:

1. **Blessed source**: Immutable upstream versions from Git (typically the merge-base with main). They form a source of truth. These represent committed/shipped API documents that cannot be changed incompatibly.

2. **Generated source**: Documents generated fresh from the current API trait definitions. Rust code is the source for these.

3. **Local source**: Documents in the working tree. These may be blessed versions, or locally-added versions being developed in a branch.

#### Source of truth

For blessed versions, the corresponding API documents are the source of truth (authoritative). The API manager ensures that the Rust API traits are wire-compatible with these.

For locally-added versions, the Rust API trait is the source of truth. Once a version is committed in main, the document becomes the source of truth.

#### Version management

```rust
pub enum Versions {
    Lockstep { version: semver::Version },
    Versioned { supported_versions: SupportedVersions },
}
```

For versioned APIs, use the `api_versions!` macro to define supported versions:

```rust
api_versions!([
    (3, WITH_METRICS),
    (2, WITH_DETAILED_STATUS),
    (1, INITIAL),
]);
```

This defines constants `VERSION_WITH_METRICS`, `VERSION_WITH_DETAILED_STATUS`, `VERSION_INITIAL` and functions `supported_versions()`, `latest_version()`.

#### Resolution and problems

The `Resolved` type represents the result of comparing blessed, generated, and local sources:

```rust
pub enum ResolutionKind {
    Lockstep,      // Single-version API.
    Blessed,       // Versioned, present in upstream.
    NewLocally,    // Versioned, only in current branch.
}

pub enum Problem<'a> {
    LocalSpecFileOrphaned { ... },
    BlessedVersionMissingLocal { ... },
    BlessedVersionBroken { ... },
    LockstepStale { ... },
    LocalVersionMissingLocal { ... },
    // ... more variants
}
```

Problems are either **fixable** (tool can auto-correct) or **unfixable** (require manual intervention).

### Key design principles

1. **Three-way reconciliation**—blessed, generated, and local sources are compared to detect drift and ensure compatibility.

2. **Wire compatibility checking**—uses the `drift` crate to semantically compare OpenAPI specs. Trivial changes (doc updates, type renames) are allowed in blessed versions; semantic changes (forward compatible, backwards compatible, or incompatible) are not.

3. **Atomic file operations**—uses `atomicwrites` crate to prevent corruption on interruption.

4. **Git integration**—blessed versions are loaded from Git history. Git ref storage optionally stores older versions as `.gitref` files containing commit references rather than full JSON.

5. **UTF-8 paths throughout**—uses `camino` crate (`Utf8Path`, `Utf8PathBuf`) for easier path handling.

### Crate structure

```
crates/
├── dropshot-api-manager/          # Main implementation
│   ├── src/
│   │   ├── lib.rs                 # Public API exports
│   │   ├── apis.rs                # ManagedApiConfig, ManagedApi, ManagedApis
│   │   ├── cmd/                   # CLI commands (dispatch, check, generate, list, debug)
│   │   ├── environment.rs         # Environment configuration and resolution
│   │   ├── resolved.rs            # Resolution logic, Problem enum, Fix enum
│   │   ├── compatibility.rs       # Wire compatibility checking via drift
│   │   ├── validation.rs          # OpenAPI document validation
│   │   ├── git.rs                 # Git operations and types
│   │   ├── output.rs              # User-facing output formatting
│   │   ├── spec_files_blessed.rs  # Blessed source file handling
│   │   ├── spec_files_generated.rs # Generated source file handling
│   │   ├── spec_files_local.rs    # Local source file handling
│   │   └── test_util/             # Test utilities
│   └── Cargo.toml
├── dropshot-api-manager-types/    # Core types (minimal deps, for API crates to depend on)
│   └── src/
│       ├── lib.rs
│       ├── apis.rs                # ApiIdent, ApiSpecFileName
│       ├── validation.rs          # ValidationContext, ValidationBackend
│       └── versions.rs            # Versions, SupportedVersions, api_versions! macro
└── integration-tests/             # Integration test suite
    ├── src/
    │   ├── lib.rs
    │   ├── fixtures.rs            # Test API fixtures (lockstep, versioned, etc.)
    │   └── environment.rs         # Test environment utilities
    └── tests/
        └── integration/           # Integration tests
```

### Cross-platform strategy

- Unix: Symlinks for "latest" version pointers, standard file operations.
- Windows: Requires developer mode for symlinks, CRLF conversion disabled via `.gitattributes`.
- Conditional compilation: `#[cfg(unix)]`, `#[cfg(windows)]`.
- Document platform differences and trade-offs in code comments.

## Dependencies

### Workspace dependencies

- All versions managed in root `Cargo.toml` `[workspace.dependencies]`.
- Comment on dependency choices when non-obvious.

### Key dependencies

- **dropshot**: Dropshot HTTP framework and API description generation.
- **openapiv3**: OpenAPI spec parsing and representation.
- **drift**: Semantic diff of OpenAPI specs for compatibility checking.
- **thiserror**: Error derive macros.
- **anyhow**: Error handling and context.
- **camino**: UTF-8 paths (`Utf8PathBuf`).
- **serde_json**: JSON manipulation.
- **semver**: Version parsing and comparison.
- **clap**: CLI parsing with derives.
- **atomicwrites**: Safe atomic file writing.
- **fs-err**: Better filesystem error messages.
- **sha2**: Hashing for versioned spec filenames.
- **owo-colors**: Colored terminal output.
- **similar**: Diff generation for display.
- **supports-color**: Terminal color capability detection.
- **openapi-lint**: Custom linting for OpenAPI specs.

## Quick reference

### Commands

```bash
# Run tests (ALWAYS use nextest for unit/integration tests)
cargo nextest run
cargo nextest run --all-features

# Run doctests (nextest doesn't support these)
cargo test --doc

# Format code (REQUIRED before committing)
cargo xfmt

# Lint
cargo clippy --all-features --all-targets

# Build
cargo build --all-targets --all-features

# Run the e2e example
cargo example-openapi list
cargo example-openapi check
cargo example-openapi generate
```

### Exit codes

- `0` (SUCCESS): All checks passed.
- `4` (NEEDS_UPDATE): Documents are stale and need regeneration.
- `100` (FAILURE): Validation errors or unfixable problems.

### Common workflows

#### Adding a new lockstep API

1. Create the API trait crate.
2. Add to `ManagedApis::new()` with `Versions::Lockstep { version: "1.0.0".parse().unwrap() }`.
3. Run `cargo openapi generate`.

#### Adding a new versioned API

1. Create the API trait crate with `api_versions!` macro.
2. Add to `ManagedApis::new()` with `Versions::Versioned { supported_versions: my_api::supported_versions() }`.
3. Run `cargo openapi generate`.

#### Iterating on versioned APIs

1. Add a new version to `api_versions!` (first position = latest).
2. Annotate changed endpoints with `versions = VERSION_NEW..` or `versions = ..VERSION_NEW`.
3. Run `cargo openapi generate`.

#### Resolving merge conflicts

1. Remove the "latest" symlink if conflicted.
2. Fix up the `api_versions!` call (take all upstream versions, renumber your local version).
3. Run `cargo openapi generate`.

## Commit message style

### Format

Commits follow a conventional format with crate-specific scoping:

```
[crate-name] brief description
```

Examples:
- `[dropshot-api-manager] add git ref storage for older API versions`
- `[dropshot-api-manager-types] version 0.3.0`
- `[meta] prepare changelog`

### Conventions

- Use `[meta]` for cross-cutting concerns (releases, CI changes).
- Version bump commits: `[crate-name] version X.Y.Z`. These are generated by cargo-release.
- Keep descriptions concise but descriptive.

### Commit quality

- **Atomic commits**: Each commit should be a logical unit of change.
- **Bisectable history**: Every commit must build and pass all checks.
- **Separate concerns**: Format fixes and refactoring should be in separate commits from feature changes.
