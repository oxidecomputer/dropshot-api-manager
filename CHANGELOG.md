# Changelog

<!-- next-header -->
## Unreleased - ReleaseDate

## [0.2.2] - 2025-10-01

### Added

- The `api_versions!` macro now generates a `latest_version` function.
- The README has a new note about how to create versioned Dropshot servers using the `latest_version` function.

## [0.2.1] - 2025-09-30

### Added

- For versioned APIs, comparisons between blessed and generated documents now use the [`drift`](https://docs.rs/drift) crate rather than simple string comparisons. This means that trivial/wire-compatible changes between blessed and generated documents (such as adding or removing newtypes) are now allowed.

### Fixed

- Git commands are now run in the repository root instead of the current directory.
- Changed some error output to use stderr instead of stdout.

## [0.2.0] - 2025-09-26

### Added

- Add a way to specify an allowlist of unmanaged APIs within a local directory. See `ManagedApis::with_unknown_apis` for more.

### Changed

- `Environment` now accepts `impl Into<String>` and `impl Into<Utf8PathBuf>` for ease of use.
- Hide private types and methods.
- Update documentation.

## [0.1.1] - 2025-09-24

- README updates.
- Windows path fixes.

## [0.1.0] - 2025-09-24

Initial release.

<!-- next-url -->
[0.2.2]: https://github.com/oxidecomputer/dropshot-api-manager/releases/tag/dropshot-api-manager-0.2.2
[0.2.1]: https://github.com/oxidecomputer/dropshot-api-manager/releases/tag/dropshot-api-manager-0.2.1
[0.2.0]: https://github.com/oxidecomputer/dropshot-api-manager/releases/tag/dropshot-api-manager-0.2.0
[0.1.1]: https://github.com/oxidecomputer/dropshot-api-manager/releases/tag/dropshot-api-manager-0.1.1
[0.1.0]: https://github.com/oxidecomputer/dropshot-api-manager/releases/tag/dropshot-api-manager-0.1.0
