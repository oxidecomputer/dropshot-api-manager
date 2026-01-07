// Copyright 2026 Oxide Computer Company

//! Working with OpenAPI documents, whether generated, blessed, or local to this
//! repository

use crate::{apis::ManagedApis, environment::ErrorAccumulator};
use anyhow::anyhow;
use camino::{Utf8Path, Utf8PathBuf};
use debug_ignore::DebugIgnore;
use dropshot_api_manager_types::{
    ApiIdent, ApiSpecFileName, ApiSpecFileNameKind,
};
use openapiv3::OpenAPI;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, btree_map::Entry},
    fmt::Debug,
};
use thiserror::Error;

/// Represents a local file that exists on disk but couldn't be parsed.
///
/// Used to track files that need to be deleted/overwritten during generate.
/// This allows the tool to clean up corrupted files (e.g., files with merge
/// conflict markers) rather than leaving them orphaned.
#[derive(Clone, Debug)]
pub struct UnparseableFile {
    /// The path to the file on disk, relative to the OpenAPI documents
    /// directory.
    pub path: Utf8PathBuf,
}

/// Attempts to parse the given file basename as an ApiSpecFileName of kind
/// `Versioned`
///
/// These look like: `ident-SEMVER-HASH.json`.
fn parse_versioned_file_name(
    apis: &ManagedApis,
    ident: &str,
    basename: &str,
) -> Result<ApiSpecFileName, BadVersionedFileName> {
    let ident = ApiIdent::from(ident.to_string());
    let Some(api) = apis.api(&ident) else {
        return Err(BadVersionedFileName::NoSuchApi);
    };

    if !api.is_versioned() {
        return Err(BadVersionedFileName::NotVersioned);
    }

    let expected_prefix = format!("{}-", &ident);
    let suffix = basename.strip_prefix(&expected_prefix).ok_or_else(|| {
        BadVersionedFileName::UnexpectedName {
            ident: ident.clone(),
            source: anyhow!("unexpected prefix"),
        }
    })?;

    let middle = suffix.strip_suffix(".json").ok_or_else(|| {
        BadVersionedFileName::UnexpectedName {
            ident: ident.clone(),
            source: anyhow!("bad suffix"),
        }
    })?;

    let (version_str, hash) = middle.rsplit_once("-").ok_or_else(|| {
        BadVersionedFileName::UnexpectedName {
            ident: ident.clone(),
            source: anyhow!("cannot extract version and hash"),
        }
    })?;

    let version: semver::Version =
        version_str.parse().map_err(|e: semver::Error| {
            BadVersionedFileName::UnexpectedName {
                ident: ident.clone(),
                source: anyhow!(e).context(format!(
                    "version string is not a semver: {:?}",
                    version_str
                )),
            }
        })?;

    // Dropshot does not support pre-release strings and we don't either.
    // This could probably be made to work, but it's easier to constrain
    // things for now and relax it later.
    if !version.pre.is_empty() {
        return Err(BadVersionedFileName::UnexpectedName {
            ident,
            source: anyhow!(
                "version string has a prerelease field \
                     (not supported): {:?}",
                version_str
            ),
        });
    }

    if !version.build.is_empty() {
        return Err(BadVersionedFileName::UnexpectedName {
            ident,
            source: anyhow!(
                "version string has a build field (not supported): {:?}",
                version_str
            ),
        });
    }

    Ok(ApiSpecFileName::new(
        ident,
        ApiSpecFileNameKind::Versioned { version, hash: hash.to_string() },
    ))
}

/// Attempts to parse the given file basename as an ApiSpecFileName of kind
/// `VersionedGitRef`.
///
/// These look like: `ident-SEMVER-HASH.json.gitref`.
fn parse_versioned_git_ref_file_name(
    apis: &ManagedApis,
    ident: &str,
    basename: &str,
) -> Result<ApiSpecFileName, BadVersionedFileName> {
    // The file name must end with .json.gitref.
    let json_basename = basename.strip_suffix(".gitref").ok_or_else(|| {
        BadVersionedFileName::UnexpectedName {
            ident: ApiIdent::from(ident.to_string()),
            source: anyhow!("expected .json.gitref suffix"),
        }
    })?;

    // Parse the underlying versioned name to get the version and hash.
    let versioned = parse_versioned_file_name(apis, ident, json_basename)?;

    match versioned.kind() {
        ApiSpecFileNameKind::Versioned { version, hash } => {
            Ok(ApiSpecFileName::new(
                versioned.ident().clone(),
                ApiSpecFileNameKind::VersionedGitRef {
                    version: version.clone(),
                    hash: hash.clone(),
                },
            ))
        }
        other => unreachable!(
            "parse_versioned_file_name always returns Versioned, found {other:?}"
        ),
    }
}

/// Attempts to parse the given file basename as an ApiSpecFileName of kind
/// `Lockstep`
fn parse_lockstep_file_name(
    apis: &ManagedApis,
    basename: &str,
) -> Result<ApiSpecFileName, BadLockstepFileName> {
    let ident = ApiIdent::from(
        basename
            .strip_suffix(".json")
            .ok_or(BadLockstepFileName::MissingJsonSuffix)?
            .to_owned(),
    );
    let api = apis.api(&ident).ok_or_else(|| {
        BadLockstepFileName::NoSuchApi { ident: ident.clone() }
    })?;
    if !api.is_lockstep() {
        return Err(BadLockstepFileName::NotLockstep);
    }

    Ok(ApiSpecFileName::new(ident, ApiSpecFileNameKind::Lockstep))
}

/// Describes a failure to parse a file name for a lockstep API
#[derive(Debug, Error)]
enum BadLockstepFileName {
    #[error("expected lockstep API file name to end in \".json\"")]
    MissingJsonSuffix,
    #[error("does not match a known API")]
    NoSuchApi {
        /// The API identifier without a trailing `.json` suffix.
        ///
        /// This isn't part of the `Display` impl because callers will print out
        /// the file name anyway, but it is used by internal code.
        ident: ApiIdent,
    },
    #[error("this API is not a lockstep API")]
    NotLockstep,
}

/// Describes a failure to parse a file name for a versioned API
#[derive(Debug, Error)]
enum BadVersionedFileName {
    #[error("does not match a known API")]
    NoSuchApi,
    #[error("this API is not a versioned API")]
    NotVersioned,
    #[error(
        "expected a versioned API document filename for API {ident:?} to look \
         like \"{ident:?}-SEMVER-HASH.json\""
    )]
    UnexpectedName { ident: ApiIdent, source: anyhow::Error },
}

/// Errors that can occur when parsing an API spec file.
#[derive(Debug, Error)]
enum ApiSpecFileParseError {
    #[error("file {path:?}: parsing as JSON")]
    JsonParse { path: Utf8PathBuf, source: serde_json::Error },
    #[error("file {path:?}: parsing OpenAPI document")]
    OpenApiParse { path: Utf8PathBuf, source: serde_json::Error },
    #[error("file {path:?}: parsing version from generated spec")]
    VersionParse { path: Utf8PathBuf, source: semver::Error },
    #[error(
        "file {path:?}: version in the file ({file_version}) differs from \
         the one in the filename"
    )]
    VersionMismatch { path: Utf8PathBuf, file_version: semver::Version },
    #[error(
        "file {path:?}: computed hash {expected:?}, but file name has \
         different hash {actual:?}"
    )]
    HashMismatch { path: Utf8PathBuf, expected: String, actual: String },
}

/// Describes an OpenAPI document
#[derive(Debug)]
pub struct ApiSpecFile {
    /// describes how the document should be named on disk
    name: ApiSpecFileName,
    /// serde_json::Value representation of the document
    value: DebugIgnore<serde_json::Value>,
    /// parsed contents of the document
    contents: DebugIgnore<OpenAPI>,
    /// raw contents of the document
    contents_buf: DebugIgnore<Vec<u8>>,
    /// version of the API described in the document
    version: semver::Version,
}

impl ApiSpecFile {
    /// Parse an OpenAPI document from raw contents.
    ///
    /// On error, returns both the error and the original contents buffer so
    /// that callers can still use the contents (e.g., for unparseable file
    /// tracking).
    pub fn for_contents(
        spec_file_name: ApiSpecFileName,
        contents_buf: Vec<u8>,
    ) -> Result<ApiSpecFile, (anyhow::Error, Vec<u8>)> {
        Self::for_contents_inner(spec_file_name, contents_buf)
            .map_err(|(e, buf)| (e.into(), buf))
    }

    fn for_contents_inner(
        spec_file_name: ApiSpecFileName,
        contents_buf: Vec<u8>,
    ) -> Result<ApiSpecFile, (ApiSpecFileParseError, Vec<u8>)> {
        // Parse a serde_json::Value from the contents buffer.
        let value: serde_json::Value =
            match serde_json::from_slice(&contents_buf) {
                Ok(v) => v,
                Err(e) => {
                    return Err((
                        ApiSpecFileParseError::JsonParse {
                            path: spec_file_name.path().to_owned(),
                            source: e,
                        },
                        contents_buf,
                    ));
                }
            };

        // Parse the OpenAPI document from the contents buffer rather than the
        // value for better error messages.
        let openapi: OpenAPI = match serde_json::from_slice(&contents_buf) {
            Ok(o) => o,
            Err(e) => {
                return Err((
                    ApiSpecFileParseError::OpenApiParse {
                        path: spec_file_name.path().to_owned(),
                        source: e,
                    },
                    contents_buf,
                ));
            }
        };

        let parsed_version: semver::Version = match openapi.info.version.parse()
        {
            Ok(v) => v,
            Err(e) => {
                return Err((
                    ApiSpecFileParseError::VersionParse {
                        path: spec_file_name.path().to_owned(),
                        source: e,
                    },
                    contents_buf,
                ));
            }
        };

        match spec_file_name.kind() {
            ApiSpecFileNameKind::Versioned { version, hash } => {
                if *version != parsed_version {
                    return Err((
                        ApiSpecFileParseError::VersionMismatch {
                            path: spec_file_name.path().to_owned(),
                            file_version: parsed_version,
                        },
                        contents_buf,
                    ));
                }

                let expected_hash = hash_contents(&contents_buf);
                if expected_hash != *hash {
                    return Err((
                        ApiSpecFileParseError::HashMismatch {
                            path: spec_file_name.path().to_owned(),
                            expected: expected_hash,
                            actual: hash.clone(),
                        },
                        contents_buf,
                    ));
                }
            }
            ApiSpecFileNameKind::VersionedGitRef { version, .. } => {
                // Git ref files: validate that the version matches, but skip
                // hash check. The content came from git, so the git ref itself
                // is the source of truth.
                if *version != parsed_version {
                    return Err((
                        ApiSpecFileParseError::VersionMismatch {
                            path: spec_file_name.path().to_owned(),
                            file_version: parsed_version,
                        },
                        contents_buf,
                    ));
                }
            }
            ApiSpecFileNameKind::Lockstep => {}
        }

        Ok(ApiSpecFile {
            name: spec_file_name,
            value: DebugIgnore(value),
            contents: DebugIgnore(openapi),
            contents_buf: DebugIgnore(contents_buf),
            version: parsed_version,
        })
    }

    /// Returns the name of the OpenAPI document
    pub fn spec_file_name(&self) -> &ApiSpecFileName {
        &self.name
    }

    /// Returns the version of the API described in the document
    pub fn version(&self) -> &semver::Version {
        &self.version
    }

    /// Returns the [`serde_json::Value`] representation of the document
    pub fn value(&self) -> &serde_json::Value {
        &self.value
    }

    /// Returns a parsed representation of the document itself
    pub fn openapi(&self) -> &OpenAPI {
        &self.contents
    }

    /// Returns the raw (byte) representation of the document itself
    pub fn contents(&self) -> &[u8] {
        &self.contents_buf
    }
}

/// Builder for constructing a set of found OpenAPI documents
///
/// The builder is agnostic to where the documents came from, whether it's the
/// local filesystem, dynamic generation, Git, etc.  The caller supplies that.
///
/// **Be sure to check for load errors and warnings before using this
/// structure.**
///
/// The source `T` is generally a Newtype wrapper around `ApiSpecFile`.  `T`
/// must impl `ApiLoad` (which applies constraints on loading these documents)
/// and `AsRawFiles` (which converts the Newtype back to `ApiSpecFile` for
/// consumers that don't care which Newtype they're dealing with).  There are
/// three values of `T` that get used here:
///
/// * `BlessedApiSpecFile`: only one allowed per version, and it's okay if we
///   find (and ignore) a file that doesn't match the API's configured type
///   (e.g., a lockstep file for a versioned API or vice versa).  This is
///   important for supporting changing the type of an API (e.g., converting
///   from lockstep to versioned).
/// * `GeneratedApiSpecFile`: only one allowed per version.  It is an error to
///   find files of a different type than the API (e.g., a lockstep file for a
///   versioned API or vice versa).
/// * `Vec<LocalApiSpecFile>`: as the type suggests, more than one is allowed
///   per version.  It is an error to find files of a different type than the
///   API (e.g., a lockstep file for a versioned API or vice versa).
///
/// Assuming no errors, the caller can assume:
///
/// * Each OpenAPI document was valid (valid JSON and valid OpenAPI).
/// * For versioned APIs, the version number in each file name corresponds to
///   the version number inside the OpenAPI document.
/// * For versioned APIs, the checksum in each file name matches the computed
///   checksum for the file.
/// * The files that were found correspond with whether the API is lockstep or
///   versioned.  That is: if an API is lockstep, then if it has a file here,
///   it's a lockstep file.  If an API is versioned, then if it has a file here,
///   then it's a versioned file.
///
///   The question of whether it's an error to find a lockstep file for a
///   versioned API or vice versa depends on the source `T` (see above).  If
///   it's not an error when this happens, the file is still ignored.  Hence,
///   any files present in this structure _do_ match the expected type.
pub struct ApiSpecFilesBuilder<'a, T> {
    apis: &'a ManagedApis,
    spec_files: BTreeMap<ApiIdent, ApiFiles<T>>,
    error_accumulator: &'a mut ErrorAccumulator,
}

impl<'a, T: ApiLoad + AsRawFiles> ApiSpecFilesBuilder<'a, T> {
    pub fn new(
        apis: &'a ManagedApis,
        error_accumulator: &'a mut ErrorAccumulator,
    ) -> ApiSpecFilesBuilder<'a, T> {
        ApiSpecFilesBuilder {
            apis,
            spec_files: BTreeMap::new(),
            error_accumulator,
        }
    }

    /// Report an error loading OpenAPI documents
    ///
    /// Errors imply that the caller can't assume the returned documents are
    /// complete or correct.
    pub fn load_error(&mut self, error: anyhow::Error) {
        self.error_accumulator.error(error);
    }

    /// Report a warning loading OpenAPI documents
    ///
    /// Warnings generally do not affect correctness.  An example warning would
    /// be an extra unexpected file.
    pub fn load_warning(&mut self, error: anyhow::Error) {
        self.error_accumulator.warning(error);
    }

    /// Returns an `ApiSpecFileName` for the given lockstep API
    ///
    /// On success, this does not load anything into `self`.  Callers generally
    /// invoke `load_contents()` with the returned value.  On failure, warnings
    /// or errors will be recorded.
    pub fn lockstep_file_name(
        &mut self,
        basename: &str,
    ) -> Option<ApiSpecFileName> {
        match parse_lockstep_file_name(self.apis, basename) {
            // When we're looking at the blessed files, the caller provides
            // `misconfigurations_okay: true` and we treat these as
            // warnings because the configuration for an API may have
            // changed between the blessed files and the local changes.
            Err(BadLockstepFileName::NoSuchApi { ident })
                if T::MISCONFIGURATIONS_ALLOWED =>
            {
                // If the ident is part of unknown_apis, then we don't print a
                // warning here (it will be printed for the generated spec).
                if !self.apis.unknown_apis().contains(&ident) {
                    let warning = anyhow!(
                        "skipping file {basename:?}: {} \
                        (this is expected if you are deleting an API)",
                        BadLockstepFileName::NoSuchApi { ident },
                    );
                    self.load_warning(warning);
                }
                None
            }
            Err(warning @ BadLockstepFileName::NotLockstep)
                if T::MISCONFIGURATIONS_ALLOWED =>
            {
                let warning = anyhow!(
                    "skipping file {basename:?}: {warning} \
                    (this is expected if you are converting \
                    a lockstep API to a versioned one)"
                );
                self.load_warning(warning);
                None
            }

            Err(warning @ BadLockstepFileName::MissingJsonSuffix) => {
                // Even if the caller didn't provide `problems_okay: true`, it's
                // not a big deal to have an extra file here.  This could be an
                // editor swap file or something.
                let warning = anyhow!(warning)
                    .context(format!("skipping file {:?}", basename));
                self.load_warning(warning);
                None
            }
            Err(BadLockstepFileName::NoSuchApi { ident })
                if self.apis.unknown_apis().contains(&ident) =>
            {
                // In this case, we show a warning rather than an error.
                let warning = anyhow!(BadLockstepFileName::NoSuchApi { ident })
                    .context(format!("skipping file {:?}", basename));
                self.load_warning(warning);
                None
            }

            Err(error) => {
                self.load_error(
                    anyhow!(error).context(format!("file {:?}", basename)),
                );
                None
            }
            Ok(file_name) => Some(file_name),
        }
    }

    /// Returns an identifier for the versioned API identified by `basename`.
    ///
    /// On success, this does not load anything into `self`.  Callers generally
    /// invoke `versioned_file_name()` with the returned value.  On failure,
    /// warnings or errors will be recorded.
    pub fn versioned_directory(&mut self, basename: &str) -> Option<ApiIdent> {
        let ident = ApiIdent::from(basename.to_owned());
        match self.apis.api(&ident) {
            Some(api) if api.is_versioned() => Some(ident),
            Some(_) => {
                // See lockstep_file_name().  This is not always a problem.
                let error = anyhow!(
                    "skipping directory for lockstep API: {:?}",
                    basename,
                );
                if T::MISCONFIGURATIONS_ALLOWED {
                    self.load_warning(error);
                } else {
                    self.load_error(error);
                }
                None
            }
            None => {
                let error = anyhow!(
                    "skipping directory for unknown API: {:?}",
                    basename,
                );
                if T::MISCONFIGURATIONS_ALLOWED {
                    self.load_warning(error);
                } else {
                    self.load_error(error);
                }
                None
            }
        }
    }

    /// Returns an `ApiSpecFileName` for the given versioned API
    ///
    /// On success, this does not load anything into `self`.  Callers generally
    /// invoke `load_contents()` with the returned value.  On failure, warnings
    /// or errors will be recorded.
    pub fn versioned_file_name(
        &mut self,
        ident: &ApiIdent,
        basename: &str,
    ) -> Option<ApiSpecFileName> {
        match parse_versioned_file_name(self.apis, ident, basename) {
            Ok(file_name) => Some(file_name),
            Err(
                warning @ (BadVersionedFileName::NoSuchApi
                | BadVersionedFileName::NotVersioned),
            ) if T::MISCONFIGURATIONS_ALLOWED => {
                // See lockstep_file_name().
                self.load_warning(
                    anyhow!(warning)
                        .context(format!("skipping file {}", basename)),
                );
                None
            }
            Err(warning @ BadVersionedFileName::UnexpectedName { .. }) => {
                // See lockstep_file_name().
                self.load_warning(
                    anyhow!(warning)
                        .context(format!("skipping file {}", basename)),
                );
                None
            }
            Err(error) => {
                self.load_error(
                    anyhow!(error).context(format!("file {}", basename)),
                );
                None
            }
        }
    }

    /// Returns an `ApiSpecFileName` for the given versioned git ref file.
    ///
    /// On success, this does not load anything into `self`. Callers generally
    /// invoke `load_contents()` with the returned value after dereferencing the
    /// git ref. On failure, warnings or errors will be recorded.
    pub fn versioned_git_ref_file_name(
        &mut self,
        ident: &ApiIdent,
        basename: &str,
    ) -> Option<ApiSpecFileName> {
        match parse_versioned_git_ref_file_name(self.apis, ident, basename) {
            Ok(file_name) => Some(file_name),
            Err(
                warning @ (BadVersionedFileName::NoSuchApi
                | BadVersionedFileName::NotVersioned),
            ) if T::MISCONFIGURATIONS_ALLOWED => {
                self.load_warning(
                    anyhow!(warning)
                        .context(format!("skipping git ref file {}", basename)),
                );
                None
            }
            Err(warning @ BadVersionedFileName::UnexpectedName { .. }) => {
                self.load_warning(
                    anyhow!(warning)
                        .context(format!("skipping git ref file {}", basename)),
                );
                None
            }
            Err(error) => {
                self.load_error(
                    anyhow!(error)
                        .context(format!("git ref file {}", basename)),
                );
                None
            }
        }
    }

    /// Like `versioned_file_name()`, but the error message for a bogus path
    /// better communicates that the problem is with the symlink
    pub fn symlink_contents(
        &mut self,
        symlink_path: &Utf8Path,
        ident: &ApiIdent,
        basename: &str,
    ) -> Option<ApiSpecFileName> {
        match parse_versioned_file_name(self.apis, ident, basename) {
            Ok(file_name) => Some(file_name),
            Err(
                warning @ (BadVersionedFileName::NoSuchApi
                | BadVersionedFileName::NotVersioned),
            ) if T::MISCONFIGURATIONS_ALLOWED => {
                // See lockstep_file_name().
                self.load_warning(anyhow!(warning).context(format!(
                    "ignoring symlink {} pointing to {}",
                    symlink_path, basename
                )));
                None
            }
            Err(warning @ BadVersionedFileName::UnexpectedName { .. }) => {
                // See lockstep_file_name().
                self.load_warning(anyhow!(warning).context(format!(
                    "ignoring symlink {} pointing to {}",
                    symlink_path, basename
                )));
                None
            }
            Err(error) => {
                self.load_error(anyhow!(error).context(format!(
                    "bad symlink {} pointing to {}",
                    symlink_path, basename
                )));
                None
            }
        }
    }

    /// Load an API document.
    ///
    /// On failure, records errors or warnings. For local files (where
    /// `T::UNPARSEABLE_FILES_ALLOWED` is true), unparseable files are recorded
    /// as warnings and tracked so they can be cleaned up during generate.
    pub fn load_contents(
        &mut self,
        file_name: ApiSpecFileName,
        contents: Vec<u8>,
    ) {
        let maybe_file = ApiSpecFile::for_contents(file_name.clone(), contents);
        match maybe_file {
            Ok(file) => {
                let ident = file.spec_file_name().ident();
                let api_version = file.version();
                let entry = self
                    .spec_files
                    .entry(ident.clone())
                    .or_insert_with(ApiFiles::new)
                    .spec_files
                    .entry(api_version.clone());

                match entry {
                    Entry::Vacant(vacant_entry) => {
                        vacant_entry.insert(T::make_item(file));
                    }
                    Entry::Occupied(mut occupied_entry) => {
                        match occupied_entry.get_mut().try_extend(file) {
                            Ok(()) => (),
                            Err(error) => self.load_error(error),
                        };
                    }
                };
            }
            Err((error, contents)) => {
                match T::make_unparseable(file_name.clone(), contents) {
                    Some(unparseable) => {
                        // For local files, track the unparseable file so it
                        // can be cleaned up during generate. Record a warning
                        // so the user knows about it.
                        self.load_warning(
                            error.context("skipping unparseable file"),
                        );

                        // Can the file be associated with a version?
                        if let Some(version) = file_name.version() {
                            let ident = file_name.ident().clone();
                            let entry = self
                                .spec_files
                                .entry(ident)
                                .or_insert_with(ApiFiles::new)
                                .spec_files
                                .entry(version.clone());

                            match entry {
                                Entry::Vacant(vacant_entry) => {
                                    vacant_entry.insert(
                                        T::unparseable_into_self(unparseable),
                                    );
                                }
                                Entry::Occupied(mut occupied_entry) => {
                                    occupied_entry
                                        .get_mut()
                                        .extend_unparseable(unparseable);
                                }
                            }
                        } else {
                            // No version info, fall back to old behavior.
                            self.record_unparseable_file(
                                file_name.ident().clone(),
                                UnparseableFile {
                                    path: file_name.path().to_owned(),
                                },
                            );
                        }
                    }
                    None => {
                        self.load_error(error);
                    }
                }
            }
        }
    }

    /// Record an unparseable file for later cleanup.
    fn record_unparseable_file(
        &mut self,
        ident: ApiIdent,
        unparseable: UnparseableFile,
    ) {
        self.spec_files
            .entry(ident)
            .or_insert_with(ApiFiles::new)
            .unparseable_files
            .push(unparseable);
    }

    /// Load the "latest" symlink for a versioned API
    ///
    /// On failure, warnings or errors are recorded.
    pub fn load_latest_link(
        &mut self,
        ident: &ApiIdent,
        links_to: ApiSpecFileName,
    ) {
        let Some(api) = self.apis.api(ident) else {
            let error =
                anyhow!("link for unknown API {:?} ({})", ident, links_to);
            if T::MISCONFIGURATIONS_ALLOWED {
                self.load_warning(error);
            } else {
                self.load_error(error);
            }

            return;
        };

        if !api.is_versioned() {
            let error = anyhow!(
                "link for non-versioned API {:?} ({})",
                ident,
                links_to
            );
            if T::MISCONFIGURATIONS_ALLOWED {
                self.load_warning(error);
            } else {
                self.load_error(error);
            }
            return;
        }

        let api_files =
            self.spec_files.entry(ident.clone()).or_insert_with(ApiFiles::new);
        if let Some(previous) = api_files.latest_link.replace(links_to) {
            // unwrap(): we just put this here.
            let new_link = api_files.latest_link.as_ref().unwrap().to_string();
            self.load_error(anyhow!(
                "API {:?}: multiple \"latest\" links (at least {}, {})",
                ident,
                previous,
                new_link,
            ));
        }
    }

    /// Returns the underlying set of files loaded
    pub fn into_map(self) -> BTreeMap<ApiIdent, ApiFiles<T>> {
        self.spec_files
    }
}

/// Describes a set of OpenAPI documents and associated "latest" symlink for a
/// given API.
///
/// Parametrized by `T` because callers use newtypes around `ApiSpecFile` to
/// avoid confusing them.  See the documentation on [`ApiSpecFilesBuilder`].
#[derive(Debug)]
pub struct ApiFiles<T> {
    spec_files: BTreeMap<semver::Version, T>,
    latest_link: Option<ApiSpecFileName>,
    /// Files that exist on disk but couldn't be parsed. These are tracked so
    /// that generate can delete them and create correct files in their place.
    unparseable_files: Vec<UnparseableFile>,
}

impl<T: AsRawFiles> ApiFiles<T> {
    fn new() -> ApiFiles<T> {
        ApiFiles {
            spec_files: BTreeMap::new(),
            latest_link: None,
            unparseable_files: Vec::new(),
        }
    }

    pub fn versions(&self) -> &BTreeMap<semver::Version, T> {
        &self.spec_files
    }

    pub fn latest_link(&self) -> Option<&ApiSpecFileName> {
        self.latest_link.as_ref()
    }

    /// Returns files that couldn't be parsed but should be tracked for cleanup.
    pub fn unparseable_files(&self) -> &[UnparseableFile] {
        &self.unparseable_files
    }
}

/// Trait for types that provide spec file metadata.
///
/// This allows iterating over both valid and unparseable files while still
/// being able to access their names.
pub trait SpecFileInfo {
    /// Returns the spec file name.
    fn spec_file_name(&self) -> &ApiSpecFileName;

    /// Returns the version from the parsed file, if available.
    ///
    /// For unparseable files, this returns `None`. Use
    /// `spec_file_name().version()` to get the version from the filename
    /// instead.
    fn version(&self) -> Option<&semver::Version>;
}

impl SpecFileInfo for ApiSpecFile {
    fn spec_file_name(&self) -> &ApiSpecFileName {
        &self.name
    }

    fn version(&self) -> Option<&semver::Version> {
        Some(&self.version)
    }
}

/// Implemented by Newtype wrappers around `ApiSpecFile` to convert back to an
/// iterator of `&'a dyn SpecFileInfo` for callers that do not care which
/// Newtype they're operating on.
///
/// This is sort of like `Deref` except that some of the implementors are
/// collections.  See [`ApiSpecFilesBuilder`] for more on this.
pub trait AsRawFiles: Debug {
    fn as_raw_files<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a dyn SpecFileInfo> + 'a>;
}

impl AsRawFiles for Vec<ApiSpecFile> {
    fn as_raw_files<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a dyn SpecFileInfo> + 'a> {
        Box::new(self.iter().map(|f| f as &dyn SpecFileInfo))
    }
}

/// Implemented by Newtype wrappers around `ApiSpecFile` to load the newtype
/// from an `ApiSpecFile`.
///
/// This is a bit like `TryFrom<Vec<ApiSpecFile>>` but we cannot use that
/// directly because of the orphan rules (neither `TryFrom` nor `Vec` is defined
/// in this package).
pub trait ApiLoad {
    /// Determines whether it's allowed in this context to load the wrong kind
    /// of file for an API.
    ///
    /// Recall that there are basically three implementors here:
    ///
    /// * Local files (from the local filesystem)
    /// * Generated files (generated from Rust source)
    /// * Blessed files (generally from Git)
    ///
    /// For blessed files (and only blessed files), it is okay to find a
    /// lockstep file for an API that we think is versioned because this is
    /// necessary in order to convert an API from lockstep to versioned.
    const MISCONFIGURATIONS_ALLOWED: bool;

    /// The type representing unparseable file data.
    ///
    /// For contexts in which unparseable files are allowed (local files), this
    /// is a concrete type holding the filename and contents. Otherwise, this is
    /// `std::convert::Infallible`, making it impossible to construct.
    type Unparseable;

    /// Record having loaded a single OpenAPI document for an API.
    fn make_item(raw: ApiSpecFile) -> Self;

    /// Try to record additional OpenAPI documents for an API.
    ///
    /// (This trait API might seem a little strange.  It looks this way because
    /// every implementor supports loading a single OpenAPI document, but only
    /// some allow more than one.)
    fn try_extend(&mut self, raw: ApiSpecFile) -> anyhow::Result<()>;

    /// Try to create unparseable file data.
    ///
    /// Returns `Some` with the unparseable data if unparseable files are
    /// allowed in this context, `None` otherwise. When `Self::Unparseable` is
    /// `Infallible`, this always returns `None`.
    fn make_unparseable(
        name: ApiSpecFileName,
        contents: Vec<u8>,
    ) -> Option<Self::Unparseable>;

    /// Convert unparseable file data into a `Self` for insertion.
    ///
    /// This is used when inserting into a vacant entry.
    fn unparseable_into_self(unparseable: Self::Unparseable) -> Self
    where
        Self: Sized;

    /// Add unparseable file data to an existing entry.
    fn extend_unparseable(&mut self, unparseable: Self::Unparseable);
}

/// Return the hash of an OpenAPI document file for the purposes of this tool
///
/// The purpose of this hash is to isolate distinct versions of a given API
/// version, as might happen if two people both try to create the the same
/// (semver) version in two different branches.  By putting these into
/// separate files, when one person merges with the other's changes, they'll
/// wind up with two distinct files rather than having a ton of merge
/// conflicts in one file.  This tool can then fix things up.
///
/// The upshot is: this hash is not required for security or even data
/// integrity.  We use SHA-256 and truncate it to just the first four bytes
/// to avoid the annoyance of super long filenames.
pub(crate) fn hash_contents(contents: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents);
    let computed_hash = hasher.finalize();
    hex::encode(&computed_hash.as_slice()[0..3])
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::ManagedApiConfig;
    use anyhow::Context;
    use assert_matches::assert_matches;
    use dropshot::{ApiDescription, ApiDescriptionBuildErrors, StubContext};
    use dropshot_api_manager_types::{
        ManagedApiMetadata, SupportedVersion, SupportedVersions, Versions,
    };
    use semver::Version;

    #[test]
    fn test_parse_name_lockstep() {
        let apis = all_apis().unwrap();
        let name = parse_lockstep_file_name(&apis, "lockstep.json").unwrap();
        assert_eq!(
            name,
            ApiSpecFileName::new(
                ApiIdent::from("lockstep".to_owned()),
                ApiSpecFileNameKind::Lockstep,
            )
        );
    }

    #[test]
    fn test_parse_name_versioned() {
        let apis = all_apis().unwrap();
        let name = parse_versioned_file_name(
            &apis,
            "versioned",
            "versioned-1.2.3-feedface.json",
        )
        .unwrap();
        assert_eq!(
            name,
            ApiSpecFileName::new(
                ApiIdent::from("versioned".to_owned()),
                ApiSpecFileNameKind::Versioned {
                    version: Version::new(1, 2, 3),
                    hash: "feedface".to_owned(),
                },
            )
        );
    }

    #[test]
    fn test_parse_name_lockstep_fail() {
        let apis = all_apis().unwrap();
        let error = parse_lockstep_file_name(&apis, "lockstep").unwrap_err();
        assert_matches!(error, BadLockstepFileName::MissingJsonSuffix);
        let error =
            parse_lockstep_file_name(&apis, "bart-simpson.json").unwrap_err();
        assert_matches!(
            error,
            BadLockstepFileName::NoSuchApi { ident } if ident == "bart-simpson".into()
        );
        let error =
            parse_lockstep_file_name(&apis, "versioned.json").unwrap_err();
        assert_matches!(error, BadLockstepFileName::NotLockstep);
    }

    #[test]
    fn test_parse_name_versioned_fail() {
        let apis = all_apis().unwrap();
        let error = parse_versioned_file_name(
            &apis,
            "bart-simpson",
            "bart-simpson-1.2.3-hash.json",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::NoSuchApi);

        let error = parse_versioned_file_name(
            &apis,
            "lockstep",
            "lockstep-1.2.3-hash.json",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::NotVersioned);

        let error =
            parse_versioned_file_name(&apis, "versioned", "1.2.3-hash.json")
                .unwrap_err();
        assert_matches!(error, BadVersionedFileName::UnexpectedName { .. });

        let error = parse_versioned_file_name(
            &apis,
            "versioned",
            "versioned-1.2.3.json",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::UnexpectedName { .. });

        let error = parse_versioned_file_name(
            &apis,
            "versioned",
            "versioned-hash.json",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::UnexpectedName { .. });

        let error = parse_versioned_file_name(
            &apis,
            "versioned",
            "versioned-1.2.3-hash",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::UnexpectedName { .. });

        let error = parse_versioned_file_name(
            &apis,
            "versioned",
            "versioned-bogus-hash",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::UnexpectedName { .. });
    }

    #[test]
    fn test_parse_name_versioned_git_ref_valid() {
        let apis = all_apis().unwrap();
        let name = parse_versioned_git_ref_file_name(
            &apis,
            "versioned",
            "versioned-1.2.3-feedface.json.gitref",
        )
        .unwrap();
        assert_eq!(
            name,
            ApiSpecFileName::new(
                ApiIdent::from("versioned".to_owned()),
                ApiSpecFileNameKind::VersionedGitRef {
                    version: Version::new(1, 2, 3),
                    hash: "feedface".to_owned(),
                },
            )
        );
    }

    #[test]
    fn test_parse_name_versioned_git_ref_invalid() {
        let apis = all_apis().unwrap();

        // Wrong suffix - missing .gitref.
        let error = parse_versioned_git_ref_file_name(
            &apis,
            "versioned",
            "versioned-1.2.3-feedface.json",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::UnexpectedName { .. });

        // Unknown API.
        let error = parse_versioned_git_ref_file_name(
            &apis,
            "unknown",
            "unknown-1.2.3-feedface.json.gitref",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::NoSuchApi);

        // Lockstep API (not versioned).
        let error = parse_versioned_git_ref_file_name(
            &apis,
            "lockstep",
            "lockstep-1.2.3-feedface.json.gitref",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::NotVersioned);

        // Bad version in the name.
        let error = parse_versioned_git_ref_file_name(
            &apis,
            "versioned",
            "versioned-badversion-feedface.json.gitref",
        )
        .unwrap_err();
        assert_matches!(error, BadVersionedFileName::UnexpectedName { .. });
    }

    fn all_apis() -> anyhow::Result<ManagedApis> {
        let apis = vec![
            ManagedApiConfig {
                ident: "lockstep",
                versions: Versions::Lockstep {
                    version: "1.0.0".parse().unwrap(),
                },
                title: "Lockstep API",
                metadata: ManagedApiMetadata {
                    description: Some("A simple lockstep-versioned API"),
                    ..ManagedApiMetadata::default()
                },
                api_description: unimplemented_fn,
            },
            ManagedApiConfig {
                ident: "versioned",
                versions: Versions::Versioned {
                    supported_versions: SupportedVersions::new(vec![
                        SupportedVersion::new(Version::new(1, 0, 0), "initial"),
                    ]),
                },
                title: "Versioned API",
                metadata: ManagedApiMetadata {
                    description: Some("A simple lockstep-versioned API"),
                    ..ManagedApiMetadata::default()
                },
                api_description: unimplemented_fn,
            },
        ];

        let apis =
            ManagedApis::new(apis).context("error creating ManagedApis")?;
        Ok(apis)
    }

    fn unimplemented_fn()
    -> Result<ApiDescription<StubContext>, ApiDescriptionBuildErrors> {
        unimplemented!("this shouldn't be called, not part of test")
    }
}
