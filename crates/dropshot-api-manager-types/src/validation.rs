// Copyright 2025 Oxide Computer Company

use crate::{ManagedApiMetadata, Versions};
use camino::Utf8PathBuf;
use std::{fmt, ops::Deref};

/// Context for validation of OpenAPI specifications.
pub struct ValidationContext<'a> {
    backend: &'a mut dyn ValidationBackend,
}

impl<'a> ValidationContext<'a> {
    /// Not part of the public API -- only called by the OpenAPI manager.
    #[doc(hidden)]
    pub fn new(backend: &'a mut dyn ValidationBackend) -> Self {
        Self { backend }
    }

    /// Retrieves the identifier of the API being validated.
    ///
    /// This identifier is set via the OpenAPI manager's `ManagedApiConfig`
    /// type.
    pub fn ident(&self) -> &ApiIdent {
        self.backend.ident()
    }

    /// Returns a descriptor for the API's file name.
    ///
    /// The file name can be used to identify the version of the API being
    /// validated.
    pub fn file_name(&self) -> &ApiSpecFileName {
        self.backend.file_name()
    }

    /// Returns true if this is the latest version of a versioned API, or if the
    /// API is lockstep.
    ///
    /// This is particularly useful for extra files which might not themselves
    /// be versioned. In that case, you may wish to only generate the extra file
    /// for the latest version.
    pub fn is_latest(&self) -> bool {
        self.backend.is_latest()
    }

    /// Returns whether this version is blessed, or None if this is not a
    /// versioned API.
    pub fn is_blessed(&self) -> Option<bool> {
        self.backend.is_blessed()
    }

    /// Retrieves the versioning strategy for this API.
    pub fn versions(&self) -> &Versions {
        self.backend.versions()
    }

    /// Retrieves the title of the API being validated.
    pub fn title(&self) -> &str {
        self.backend.title()
    }

    /// Retrieves optional metadata for the API being validated.
    pub fn metadata(&self) -> &ManagedApiMetadata {
        self.backend.metadata()
    }

    /// Reports a validation error.
    pub fn report_error(&mut self, error: anyhow::Error) {
        self.backend.report_error(error);
    }

    /// Records that the file has the given contents.
    ///
    /// In check mode, if the files differ, an error is logged.
    ///
    /// In generate mode, the file is overwritten with the given contents.
    ///
    /// The path is treated as relative to the root of the repository.
    pub fn record_file_contents(
        &mut self,
        path: impl Into<Utf8PathBuf>,
        contents: Vec<u8>,
    ) {
        self.backend.record_file_contents(path.into(), contents);
    }
}

/// The backend for validation.
///
/// Not part of the public API -- only implemented by the OpenAPI manager.
#[doc(hidden)]
pub trait ValidationBackend {
    fn ident(&self) -> &ApiIdent;
    fn file_name(&self) -> &ApiSpecFileName;
    fn versions(&self) -> &Versions;
    fn is_latest(&self) -> bool;
    fn is_blessed(&self) -> Option<bool>;
    fn title(&self) -> &str;
    fn metadata(&self) -> &ManagedApiMetadata;
    fn report_error(&mut self, error: anyhow::Error);
    fn record_file_contents(&mut self, path: Utf8PathBuf, contents: Vec<u8>);
}

/// A lockstep API spec filename.
///
/// Lockstep APIs have a single OpenAPI document with no versioning. The
/// filename is simply `{ident}.json`.
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub struct LockstepApiSpecFileName {
    ident: ApiIdent,
}

impl LockstepApiSpecFileName {
    /// Creates a new lockstep API spec filename.
    pub fn new(ident: ApiIdent) -> Self {
        Self { ident }
    }

    /// Returns the API identifier.
    pub fn ident(&self) -> &ApiIdent {
        &self.ident
    }

    /// Returns the path of this file relative to the root of the OpenAPI
    /// documents.
    pub fn path(&self) -> Utf8PathBuf {
        Utf8PathBuf::from(self.basename())
    }

    /// Returns the base name of this file path.
    pub fn basename(&self) -> String {
        format!("{}.json", self.ident)
    }
}

impl fmt::Display for LockstepApiSpecFileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.path().as_str())
    }
}

/// A versioned API spec filename.
///
/// Versioned APIs can have multiple versions coexisting. The filename includes
/// the version and a content hash: `{ident}/{ident}-{version}-{hash}.json` (or
/// `.json.gitref` for git ref storage).
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub struct VersionedApiSpecFileName {
    ident: ApiIdent,
    version: semver::Version,
    hash: String,
    kind: VersionedApiSpecKind,
}

impl VersionedApiSpecFileName {
    /// Creates a new versioned API spec filename (JSON format).
    pub fn new(
        ident: ApiIdent,
        version: semver::Version,
        hash: String,
    ) -> Self {
        Self { ident, version, hash, kind: VersionedApiSpecKind::Json }
    }

    /// Creates a new versioned API spec filename (git ref format).
    pub fn new_git_ref(
        ident: ApiIdent,
        version: semver::Version,
        hash: String,
    ) -> Self {
        Self { ident, version, hash, kind: VersionedApiSpecKind::GitRef }
    }

    /// Returns the API identifier.
    pub fn ident(&self) -> &ApiIdent {
        &self.ident
    }

    /// Returns the version.
    pub fn version(&self) -> &semver::Version {
        &self.version
    }

    /// Returns the hash.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Returns the storage kind (JSON or git ref).
    pub fn kind(&self) -> VersionedApiSpecKind {
        self.kind
    }

    /// Returns true if this is a git ref file.
    pub fn is_git_ref(&self) -> bool {
        self.kind == VersionedApiSpecKind::GitRef
    }

    /// Returns the path of this file relative to the root of the OpenAPI
    /// documents.
    pub fn path(&self) -> Utf8PathBuf {
        Utf8PathBuf::from_iter([self.ident.deref().clone(), self.basename()])
    }

    /// Returns the base name of this file path.
    pub fn basename(&self) -> String {
        match self.kind {
            VersionedApiSpecKind::Json => {
                format!("{}-{}-{}.json", self.ident, self.version, self.hash)
            }
            VersionedApiSpecKind::GitRef => {
                format!(
                    "{}-{}-{}.json.gitref",
                    self.ident, self.version, self.hash
                )
            }
        }
    }

    /// Converts this filename to its JSON equivalent.
    ///
    /// If already JSON, returns a clone of self.
    pub fn to_json(&self) -> Self {
        Self {
            ident: self.ident.clone(),
            version: self.version.clone(),
            hash: self.hash.clone(),
            kind: VersionedApiSpecKind::Json,
        }
    }

    /// Converts this filename to its git ref equivalent.
    ///
    /// If already a git ref, returns a clone of self.
    pub fn to_git_ref(&self) -> Self {
        Self {
            ident: self.ident.clone(),
            version: self.version.clone(),
            hash: self.hash.clone(),
            kind: VersionedApiSpecKind::GitRef,
        }
    }

    /// Returns the basename as a git ref filename.
    ///
    /// - If already a git ref, returns `basename()` directly.
    /// - If JSON, returns `basename() + ".gitref"`.
    pub fn git_ref_basename(&self) -> String {
        match self.kind {
            VersionedApiSpecKind::GitRef => self.basename(),
            VersionedApiSpecKind::Json => format!("{}.gitref", self.basename()),
        }
    }

    /// Returns the basename as a JSON filename.
    ///
    /// - If already JSON, returns `basename()` directly.
    /// - If git ref, returns the basename without `.gitref`.
    pub fn json_basename(&self) -> String {
        match self.kind {
            VersionedApiSpecKind::Json => self.basename(),
            VersionedApiSpecKind::GitRef => {
                format!("{}-{}-{}.json", self.ident, self.version, self.hash)
            }
        }
    }
}

impl fmt::Display for VersionedApiSpecFileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.path().as_str())
    }
}

/// Describes how a versioned API spec file is stored.
#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum VersionedApiSpecKind {
    /// The spec is stored as a JSON file containing the full OpenAPI document.
    Json,
    /// The spec is stored as a git ref file.
    ///
    /// Instead of storing the full JSON content, a `.gitref` file contains a
    /// reference in the format `commit:path` that can be used to retrieve the
    /// content via `git show`.
    GitRef,
}

/// Describes the path to an OpenAPI document file, relative to some root where
/// similar documents are found.
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum ApiSpecFileName {
    /// A lockstep API: single OpenAPI document, no versioning.
    Lockstep(LockstepApiSpecFileName),
    /// A versioned API: multiple versions can coexist.
    Versioned(VersionedApiSpecFileName),
}

impl fmt::Display for ApiSpecFileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.path().as_str())
    }
}

impl ApiSpecFileName {
    /// Returns the API identifier.
    pub fn ident(&self) -> &ApiIdent {
        match self {
            ApiSpecFileName::Lockstep(l) => l.ident(),
            ApiSpecFileName::Versioned(v) => v.ident(),
        }
    }

    /// Returns the path of this file relative to the root of the OpenAPI
    /// documents.
    pub fn path(&self) -> Utf8PathBuf {
        match self {
            ApiSpecFileName::Lockstep(l) => l.path(),
            ApiSpecFileName::Versioned(v) => v.path(),
        }
    }

    /// Returns the base name of this file path.
    pub fn basename(&self) -> String {
        match self {
            ApiSpecFileName::Lockstep(l) => l.basename(),
            ApiSpecFileName::Versioned(v) => v.basename(),
        }
    }

    /// For versioned APIs, returns the version part of the filename.
    pub fn version(&self) -> Option<&semver::Version> {
        match self {
            ApiSpecFileName::Lockstep(_) => None,
            ApiSpecFileName::Versioned(v) => Some(v.version()),
        }
    }

    /// For versioned APIs, returns the hash part of the filename.
    pub fn hash(&self) -> Option<&str> {
        match self {
            ApiSpecFileName::Lockstep(_) => None,
            ApiSpecFileName::Versioned(v) => Some(v.hash()),
        }
    }

    /// Returns true if this is a git ref file.
    pub fn is_git_ref(&self) -> bool {
        match self {
            ApiSpecFileName::Lockstep(_) => false,
            ApiSpecFileName::Versioned(v) => v.is_git_ref(),
        }
    }

    /// For versioned APIs, returns the kind of storage.
    pub fn versioned_kind(&self) -> Option<VersionedApiSpecKind> {
        match self {
            ApiSpecFileName::Lockstep(_) => None,
            ApiSpecFileName::Versioned(v) => Some(v.kind()),
        }
    }

    /// Converts a git ref filename to its JSON equivalent.
    ///
    /// For non-git ref files, returns a clone of self.
    pub fn to_json_filename(&self) -> ApiSpecFileName {
        match self {
            ApiSpecFileName::Lockstep(_) => self.clone(),
            ApiSpecFileName::Versioned(v) => {
                ApiSpecFileName::Versioned(v.to_json())
            }
        }
    }

    /// Converts a JSON filename to its git ref equivalent.
    ///
    /// For git ref files, returns a clone of self.
    /// For lockstep files, returns a clone of self (lockstep files are not
    /// converted to git refs).
    pub fn to_git_ref_filename(&self) -> ApiSpecFileName {
        match self {
            ApiSpecFileName::Lockstep(_) => self.clone(),
            ApiSpecFileName::Versioned(v) => {
                ApiSpecFileName::Versioned(v.to_git_ref())
            }
        }
    }

    /// Returns the basename for this file as a git ref.
    ///
    /// - If this is already a git ref, returns `basename()` directly.
    /// - If this is a versioned JSON file, returns `basename() + ".gitref"`.
    /// - For lockstep, returns `basename()` (lockstep files are not converted
    ///   to git refs).
    pub fn git_ref_basename(&self) -> String {
        match self {
            ApiSpecFileName::Lockstep(l) => l.basename(),
            ApiSpecFileName::Versioned(v) => v.git_ref_basename(),
        }
    }

    /// Returns the basename for this file as a JSON file.
    ///
    /// - If this is a git ref, returns the basename without the `.gitref`
    ///   suffix.
    /// - Otherwise, returns `basename()` directly.
    pub fn json_basename(&self) -> String {
        match self {
            ApiSpecFileName::Lockstep(l) => l.basename(),
            ApiSpecFileName::Versioned(v) => v.json_basename(),
        }
    }

    /// Returns a reference to the inner `VersionedApiSpecFileName` if this is
    /// a versioned API, or `None` if this is a lockstep API.
    pub fn as_versioned(&self) -> Option<&VersionedApiSpecFileName> {
        match self {
            ApiSpecFileName::Lockstep(_) => None,
            ApiSpecFileName::Versioned(v) => Some(v),
        }
    }

    /// Consumes `self` and returns the inner `VersionedApiSpecFileName` if
    /// this is a versioned API, or `None` if this is a lockstep API.
    pub fn into_versioned(self) -> Option<VersionedApiSpecFileName> {
        match self {
            ApiSpecFileName::Lockstep(_) => None,
            ApiSpecFileName::Versioned(v) => Some(v),
        }
    }

    /// Returns a reference to the inner `LockstepApiSpecFileName` if this is
    /// a lockstep API, or `None` if this is a versioned API.
    pub fn as_lockstep(&self) -> Option<&LockstepApiSpecFileName> {
        match self {
            ApiSpecFileName::Lockstep(l) => Some(l),
            ApiSpecFileName::Versioned(_) => None,
        }
    }

    /// Consumes `self` and returns the inner `LockstepApiSpecFileName` if
    /// this is a lockstep API, or `None` if this is a versioned API.
    pub fn into_lockstep(self) -> Option<LockstepApiSpecFileName> {
        match self {
            ApiSpecFileName::Lockstep(l) => Some(l),
            ApiSpecFileName::Versioned(_) => None,
        }
    }
}

impl From<LockstepApiSpecFileName> for ApiSpecFileName {
    fn from(l: LockstepApiSpecFileName) -> Self {
        ApiSpecFileName::Lockstep(l)
    }
}

impl From<VersionedApiSpecFileName> for ApiSpecFileName {
    fn from(v: VersionedApiSpecFileName) -> Self {
        ApiSpecFileName::Versioned(v)
    }
}

/// Newtype for API identifiers
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct ApiIdent(String);

impl fmt::Debug for ApiIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Deref for ApiIdent {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for ApiIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<S: Into<String>> From<S> for ApiIdent {
    fn from(value: S) -> Self {
        Self(value.into())
    }
}

impl ApiIdent {
    /// Given an API identifier, return the basename of its "latest" symlink
    pub fn versioned_api_latest_symlink(&self) -> String {
        format!("{self}-latest.json")
    }

    /// Given an API identifier and a file name, determine if we're looking at
    /// this API's "latest" symlink
    pub fn versioned_api_is_latest_symlink(&self, base_name: &str) -> bool {
        base_name == self.versioned_api_latest_symlink()
    }
}
