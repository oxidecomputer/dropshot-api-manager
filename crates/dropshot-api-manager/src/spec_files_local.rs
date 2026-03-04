// Copyright 2026 Oxide Computer Company

//! Newtype and collection to represent OpenAPI documents local to this working
//! tree

use crate::{
    apis::ManagedApis,
    environment::ErrorAccumulator,
    spec_files_generic::{
        ApiFiles, ApiLoad, ApiSpecFile, ApiSpecFilesBuilder, AsRawFiles,
        SpecFileInfo, parse_lockstep_file_name, parse_versioned_file_name,
        parse_versioned_git_stub_file_name,
    },
    vcs::RepoVcs,
};
use anyhow::{Context, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use dropshot_api_manager_types::{ApiIdent, ApiSpecFileName};
use git_stub::{GitCommitHash, GitStub};
use rayon::prelude::*;
use std::{collections::BTreeMap, ops::Deref};

/// A local file that exists but couldn't be parsed.
///
/// This happens when a file has merge conflict markers or is otherwise
/// corrupted. The version is determined from the filename, allowing the
/// generate command to regenerate the correct contents.
///
/// We still store the raw contents so they can be accessed if needed (e.g.,
/// for diffing or debugging).
#[derive(Debug)]
pub struct LocalApiUnparseable {
    /// The parsed filename (contains version and hash info).
    pub name: ApiSpecFileName,
    /// The raw file contents that couldn't be parsed.
    pub contents: Vec<u8>,
}

/// Represents an OpenAPI document found in this working tree.
///
/// This includes documents for lockstep APIs and versioned APIs, for both
/// blessed and locally-added versions.
///
/// Files may be either valid (successfully parsed) or unparseable (e.g., due
/// to merge conflict markers). Unparseable files are tracked so they can be
/// regenerated during the generate command.
#[derive(Debug)]
pub enum LocalApiSpecFile {
    /// A valid, successfully parsed OpenAPI document.
    Valid {
        /// The parsed OpenAPI document.
        spec: Box<ApiSpecFile>,
        /// Commit hash parsed from the `.gitstub` file, if this file was
        /// loaded from one. `None` for regular JSON files.
        git_stub_commit: Option<GitCommitHash>,
    },
    /// A file that exists but couldn't be parsed.
    Unparseable(LocalApiUnparseable),
}

impl LocalApiSpecFile {
    /// Returns the spec file name.
    pub fn spec_file_name(&self) -> &ApiSpecFileName {
        match self {
            Self::Valid { spec, .. } => spec.spec_file_name(),
            Self::Unparseable(u) => &u.name,
        }
    }

    /// Returns the raw file contents.
    ///
    /// This works for both valid and unparseable files.
    pub fn contents(&self) -> &[u8] {
        match self {
            Self::Valid { spec, .. } => spec.contents(),
            Self::Unparseable(u) => &u.contents,
        }
    }

    /// Returns the commit hash from a `.gitstub` file, if this file was
    /// loaded from one.
    pub fn git_stub_commit(&self) -> Option<&GitCommitHash> {
        match self {
            Self::Valid { git_stub_commit, .. } => git_stub_commit.as_ref(),
            Self::Unparseable(_) => None,
        }
    }

    /// Returns true if this file is unparseable.
    pub fn is_unparseable(&self) -> bool {
        matches!(self, Self::Unparseable(_))
    }
}

impl SpecFileInfo for LocalApiSpecFile {
    fn spec_file_name(&self) -> &ApiSpecFileName {
        self.spec_file_name()
    }

    fn version(&self) -> Option<&semver::Version> {
        match self {
            Self::Valid { spec, .. } => Some(spec.version()),
            Self::Unparseable(_) => None,
        }
    }
}

// Trait impls that allow us to use `ApiFiles<Vec<LocalApiSpecFile>>`
//
// Note that this is a `Vec` because it's allowed to have more than one
// LocalApiSpecFile for a given version.

impl ApiLoad for Vec<LocalApiSpecFile> {
    const MISCONFIGURATIONS_ALLOWED: bool = false;
    type Unparseable = LocalApiUnparseable;

    fn try_extend(&mut self, item: ApiSpecFile) -> anyhow::Result<()> {
        self.push(LocalApiSpecFile::Valid {
            spec: Box::new(item),
            git_stub_commit: None,
        });
        Ok(())
    }

    fn make_item(raw: ApiSpecFile) -> Self {
        vec![LocalApiSpecFile::Valid {
            spec: Box::new(raw),
            git_stub_commit: None,
        }]
    }

    fn make_unparseable(
        name: ApiSpecFileName,
        contents: Vec<u8>,
    ) -> Option<Self::Unparseable> {
        Some(LocalApiUnparseable { name, contents })
    }

    fn unparseable_into_self(unparseable: Self::Unparseable) -> Self {
        vec![LocalApiSpecFile::Unparseable(unparseable)]
    }

    fn extend_unparseable(&mut self, unparseable: Self::Unparseable) {
        self.push(LocalApiSpecFile::Unparseable(unparseable));
    }

    fn set_git_stub_commit(&mut self, commit: GitCommitHash) {
        if let Some(LocalApiSpecFile::Valid { git_stub_commit, .. }) =
            self.last_mut()
        {
            *git_stub_commit = Some(commit);
        }
    }
}

impl AsRawFiles for Vec<LocalApiSpecFile> {
    fn as_raw_files<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a dyn SpecFileInfo> + 'a> {
        Box::new(self.iter().map(|f| f as &dyn SpecFileInfo))
    }
}

/// Container for OpenAPI documents found in the local working tree.
///
/// **Be sure to check for load errors and warnings before using this
/// structure.**
///
/// For more on what's been validated at this point, see
/// [`ApiSpecFilesBuilder`].
#[derive(Debug, Default)]
pub struct LocalFiles {
    /// The loaded local files.
    files: BTreeMap<ApiIdent, ApiFiles<Vec<LocalApiSpecFile>>>,
}

impl Deref for LocalFiles {
    type Target = BTreeMap<ApiIdent, ApiFiles<Vec<LocalApiSpecFile>>>;

    fn deref(&self) -> &Self::Target {
        &self.files
    }
}

impl LocalFiles {
    /// Load OpenAPI documents from a given directory tree.
    ///
    /// If it's at all possible to load any documents, this will return an `Ok`
    /// value, but you should still check the `errors` field on the returned
    /// [`LocalFiles`].
    ///
    /// The `repo_root` parameter is needed to resolve `.gitstub` files, which
    /// store a reference to an OpenAPI document rather than the document
    /// itself.
    pub fn load_from_directory(
        dir: &Utf8Path,
        apis: &ManagedApis,
        error_accumulator: &mut ErrorAccumulator,
        repo_root: &Utf8Path,
        vcs: &RepoVcs,
    ) -> anyhow::Result<LocalFiles> {
        let api_files =
            walk_local_directory(dir, apis, error_accumulator, repo_root, vcs)?;
        Ok(LocalFiles { files: api_files.into_map() })
    }
}

impl From<ApiSpecFilesBuilder<'_, Vec<LocalApiSpecFile>>> for LocalFiles {
    fn from(api_files: ApiSpecFilesBuilder<Vec<LocalApiSpecFile>>) -> Self {
        LocalFiles { files: api_files.into_map() }
    }
}

/// Entry discovered during the directory walk (Phase 1).
///
/// No file content is read here; only metadata from `readdir` and
/// `file_type()`.
enum LocalDiscoveredEntry {
    /// A regular file in the top-level directory. This is likely a lockstep
    /// API.
    TopLevelFile { file_name: String, path: Utf8PathBuf },
    /// A regular `.json` file inside a versioned API directory.
    VersionedFile { dir_basename: String, file_name: String, path: Utf8PathBuf },
    /// A `.json.gitstub` file inside a versioned API directory.
    GitStub { dir_basename: String, file_name: String, path: Utf8PathBuf },
    /// A symlink matching the `{ident}-latest.json` pattern.
    LatestSymlink { dir_basename: String, path: Utf8PathBuf, target: String },
    /// A file matching the latest symlink pattern but not actually a
    /// symlink (e.g., corrupted by a merge conflict).
    LatestNotSymlink { path: Utf8PathBuf },
    /// A non-fatal issue discovered during the walk.
    Warning(anyhow::Error),
    /// A fatal issue discovered during the walk.
    Error(anyhow::Error),
}

/// Result from parallel I/O and deserialization (Phase 2).
enum LocalFileResult {
    // --- Successfully parsed filename + deserialized ---
    LockstepDeserialized {
        file_name: ApiSpecFileName,
        result: Result<ApiSpecFile, (anyhow::Error, Vec<u8>)>,
    },
    VersionedDeserialized {
        file_name: ApiSpecFileName,
        result: Result<ApiSpecFile, (anyhow::Error, Vec<u8>)>,
    },
    GitStubDeserialized {
        file_name: ApiSpecFileName,
        result: Result<ApiSpecFile, (anyhow::Error, Vec<u8>)>,
        commit: GitCommitHash,
    },

    // --- Git stub that couldn't be resolved ---
    GitStubUnresolvable {
        file_name: ApiSpecFileName,
        original_contents: Vec<u8>,
        reason: anyhow::Error,
    },

    // --- Filename parse failures (diagnostics happen at the reduce phase) ---
    LockstepParseFailed {
        file_name: String,
    },
    VersionedParseFailed {
        dir_basename: String,
        file_name: String,
    },
    GitStubParseFailed {
        dir_basename: String,
        file_name: String,
    },

    // --- Symlinks ---
    LatestSymlink {
        dir_basename: String,
        path: Utf8PathBuf,
        target: String,
    },
    LatestNotSymlink {
        path: Utf8PathBuf,
    },

    // --- Errors and warnings ---
    Warning(anyhow::Error),
    Error(anyhow::Error),
}

// ---- Phase 1: sequential directory walk ----

/// Walk the two-level directory structure, collecting entries without
/// reading file contents.
///
/// Returns `Err` only if the top-level `readdir` fails.
fn discover_local_entries(
    dir: &Utf8Path,
) -> anyhow::Result<Vec<LocalDiscoveredEntry>> {
    let mut entries = Vec::new();
    let top_iter =
        dir.read_dir_utf8().with_context(|| format!("readdir {:?}", dir))?;

    for maybe_entry in top_iter {
        let entry = match maybe_entry {
            Ok(e) => e,
            Err(error) => {
                entries.push(LocalDiscoveredEntry::Error(
                    anyhow!(error).context(format!("readdir {:?} entry", dir)),
                ));
                continue;
            }
        };

        let path = entry.path().to_owned();
        let file_name = entry.file_name().to_owned();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(error) => {
                entries.push(LocalDiscoveredEntry::Error(
                    anyhow!(error).context(format!("file type of {:?}", path)),
                ));
                continue;
            }
        };

        if file_type.is_file() {
            entries
                .push(LocalDiscoveredEntry::TopLevelFile { file_name, path });
        } else if file_type.is_dir() {
            discover_versioned_directory(&mut entries, &path, &file_name);
        } else {
            entries.push(LocalDiscoveredEntry::Warning(anyhow!(
                "ignored (not a file or directory): {:?}",
                path
            )));
        }
    }

    Ok(entries)
}

/// Walk a single versioned-API subdirectory, appending discovered entries
/// to `out`.
fn discover_versioned_directory(
    out: &mut Vec<LocalDiscoveredEntry>,
    path: &Utf8Path,
    dir_basename: &str,
) {
    let sub_entries = match path
        .read_dir_utf8()
        .and_then(|iter| iter.collect::<Result<Vec<_>, _>>())
    {
        Ok(entries) => entries,
        Err(error) => {
            out.push(LocalDiscoveredEntry::Error(
                anyhow!(error).context(format!("readdir {:?}", path)),
            ));
            return;
        }
    };

    // Construct a temporary ApiIdent so we can use its canonical
    // symlink-detection method. This ident is not validated against the
    // known API set (that happens in phase 3); it's used only for the
    // filename pattern check.
    let ident = ApiIdent::from(dir_basename.to_owned());

    for entry in sub_entries {
        let file_name = entry.file_name().to_owned();
        let entry_path = entry.path().to_owned();

        if ident.versioned_api_is_latest_symlink(&file_name) {
            // Check whether it's actually a symlink.
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(error) => {
                    out.push(LocalDiscoveredEntry::Warning(
                        anyhow!(error).context(format!(
                            "failed to get file type for {:?}",
                            entry_path
                        )),
                    ));
                    continue;
                }
            };

            if file_type.is_symlink() {
                let target = match entry_path.read_link_utf8() {
                    Ok(s) => s.to_string(),
                    Err(error) => {
                        out.push(LocalDiscoveredEntry::Error(
                            anyhow!(error).context(format!(
                                "read what should be a symlink {:?}",
                                entry_path
                            )),
                        ));
                        continue;
                    }
                };

                out.push(LocalDiscoveredEntry::LatestSymlink {
                    dir_basename: dir_basename.to_owned(),
                    path: entry_path,
                    target,
                });
            } else {
                out.push(LocalDiscoveredEntry::LatestNotSymlink {
                    path: entry_path,
                });
            }
            continue;
        }

        if file_name.ends_with(".json.gitstub") {
            out.push(LocalDiscoveredEntry::GitStub {
                dir_basename: dir_basename.to_owned(),
                file_name,
                path: entry_path,
            });
        } else {
            out.push(LocalDiscoveredEntry::VersionedFile {
                dir_basename: dir_basename.to_owned(),
                file_name,
                path: entry_path,
            });
        }
    }
}

// ---- Phase 2: parallel I/O + filename parse + deserialization ----

/// Process a single discovered entry: parse the filename, read file
/// contents, and deserialize.
///
/// This is called in parallel.
fn process_local_entry(
    entry: LocalDiscoveredEntry,
    apis: &ManagedApis,
    repo_root: &Utf8Path,
    vcs: &RepoVcs,
) -> LocalFileResult {
    match entry {
        LocalDiscoveredEntry::TopLevelFile { file_name, path } => {
            // Try to parse as a lockstep filename. If that fails, defer
            // diagnostics to the reduce phase (the builder methods produce
            // the correct warnings/errors depending on T).
            let Some(spec_file_name) =
                parse_lockstep_file_name(apis, &file_name)
                    .ok()
                    .map(ApiSpecFileName::from)
            else {
                return LocalFileResult::LockstepParseFailed { file_name };
            };

            let contents = match fs_err::read(&path) {
                Ok(c) => c,
                Err(error) => {
                    return LocalFileResult::Error(anyhow!(error));
                }
            };

            let result =
                ApiSpecFile::for_contents(spec_file_name.clone(), contents);
            LocalFileResult::LockstepDeserialized {
                file_name: spec_file_name,
                result,
            }
        }

        LocalDiscoveredEntry::VersionedFile {
            dir_basename,
            file_name,
            path,
        } => {
            let Some(spec_file_name) =
                parse_versioned_file_name(apis, &dir_basename, &file_name)
                    .ok()
                    .map(ApiSpecFileName::from)
            else {
                return LocalFileResult::VersionedParseFailed {
                    dir_basename,
                    file_name,
                };
            };

            let contents = match fs_err::read(&path) {
                Ok(c) => c,
                Err(error) => {
                    return LocalFileResult::Error(anyhow!(error));
                }
            };

            let result =
                ApiSpecFile::for_contents(spec_file_name.clone(), contents);
            LocalFileResult::VersionedDeserialized {
                file_name: spec_file_name,
                result,
            }
        }

        LocalDiscoveredEntry::GitStub { dir_basename, file_name, path } => {
            let Some(spec_file_name) = parse_versioned_git_stub_file_name(
                apis,
                &dir_basename,
                &file_name,
            )
            .ok()
            .map(ApiSpecFileName::from) else {
                return LocalFileResult::GitStubParseFailed {
                    dir_basename,
                    file_name,
                };
            };

            let git_stub_contents = match fs_err::read_to_string(&path) {
                Ok(c) => c,
                Err(error) => {
                    return LocalFileResult::Error(anyhow!(error).context(
                        format!("failed to read Git stub {:?}", path,),
                    ));
                }
            };

            // Parse the Git stub.
            let git_stub = match git_stub_contents.parse::<GitStub>() {
                Ok(g) => g,
                Err(error) => {
                    return LocalFileResult::GitStubUnresolvable {
                        file_name: spec_file_name,
                        original_contents: git_stub_contents.into_bytes(),
                        reason: anyhow!(error).context(format!(
                            "Git stub {:?} could not be parsed",
                            path,
                        )),
                    };
                }
            };

            // Check if the stub needs rewriting to canonical format.
            if git_stub.needs_rewrite() {
                return LocalFileResult::GitStubUnresolvable {
                    file_name: spec_file_name,
                    original_contents: git_stub_contents.into_bytes(),
                    reason: anyhow!(
                        "Git stub {:?} needs to be rewritten to \
                         canonical format (forward slashes, trailing \
                         newline)",
                        path,
                    ),
                };
            }

            // Resolve the git stub to actual file contents.
            let contents = match vcs.resolve_stub_contents(&git_stub, repo_root)
            {
                Ok(c) => c,
                Err(error) => {
                    return LocalFileResult::GitStubUnresolvable {
                        file_name: spec_file_name,
                        original_contents: git_stub_contents.into_bytes(),
                        reason: error.context(format!(
                            "Git stub {:?} could not be resolved",
                            path,
                        )),
                    };
                }
            };

            // Deserialize the resolved contents.
            let commit = git_stub.commit();
            let result =
                ApiSpecFile::for_contents(spec_file_name.clone(), contents);
            LocalFileResult::GitStubDeserialized {
                file_name: spec_file_name,
                result,
                commit,
            }
        }

        LocalDiscoveredEntry::LatestSymlink { dir_basename, path, target } => {
            LocalFileResult::LatestSymlink { dir_basename, path, target }
        }

        LocalDiscoveredEntry::LatestNotSymlink { path } => {
            LocalFileResult::LatestNotSymlink { path }
        }

        LocalDiscoveredEntry::Warning(err) => LocalFileResult::Warning(err),
        LocalDiscoveredEntry::Error(err) => LocalFileResult::Error(err),
    }
}

// ---- Phase 3: sequential reduce into builder ----

/// Load OpenAPI documents for the local directory tree.
///
/// Under `dir`, we expect to find either:
///
/// * for each lockstep API, a file called `api-ident.json` (e.g.,
///   `wicketd.json`)
/// * for each versioned API, a directory called `api-ident` that contains:
///     * any number of files called `api-ident-SEMVER-HASH.json`
///       (e.g., dns-server-1.0.0-eb52aeeb.json)
///     * any number of Git stubs called `api-ident-SEMVER-HASH.json.gitstub`
///       that contain a `commit:path` reference to the actual content
///     * one symlink called `api-ident-latest.json` that points to a file in
///       the same directory
///
/// Here's an example:
///
/// ```text
/// wicketd.json                                      # file for lockstep API
/// dns-server/                                       # directory for versioned API
/// dns-server/dns-server-1.0.0-eb2aeeb.json          # file for versioned API
/// dns-server/dns-server-2.0.0-fba287a.json.gitstub  # Git stub for versioned API
/// dns-server/dns-server-3.0.0-298ea47.json          # file for versioned API
/// dns-server/dns-server-latest.json                 # symlink
/// ```
// This function is always used for the "local" files. It can sometimes be
// used for both generated and blessed files, if the user asks to load those
// from the local filesystem instead of their usual sources.
pub fn walk_local_directory<'a, T: ApiLoad + AsRawFiles>(
    dir: &'_ Utf8Path,
    apis: &'a ManagedApis,
    error_accumulator: &'a mut ErrorAccumulator,
    repo_root: &Utf8Path,
    vcs: &RepoVcs,
) -> anyhow::Result<ApiSpecFilesBuilder<'a, T>> {
    // Phase 1: discover entries (sequential, fast).
    let entries = discover_local_entries(dir)?;

    // Phase 2: I/O + filename parse + deserialization (parallel).
    let file_results: Vec<LocalFileResult> = entries
        .into_par_iter()
        .map(|entry| process_local_entry(entry, apis, repo_root, vcs))
        .collect();

    // Phase 3: reduce into builder (sequential).
    let mut api_files = ApiSpecFilesBuilder::new(apis, error_accumulator);

    // Cache for `versioned_directory()` results to avoid duplicate
    // warnings for entries from the same directory.
    let mut seen_dirs: BTreeMap<String, Option<ApiIdent>> = BTreeMap::new();

    for result in file_results {
        match result {
            LocalFileResult::LockstepDeserialized { file_name, result } => {
                api_files.load_maybe_unparseable(file_name, result);
            }
            LocalFileResult::VersionedDeserialized { file_name, result } => {
                // parse_versioned_file_name() already validated that the
                // API exists and is versioned.
                api_files.load_maybe_unparseable(file_name, result);
            }
            LocalFileResult::GitStubDeserialized {
                file_name,
                result,
                commit,
            } => {
                let version = file_name.version().cloned();
                let ident = file_name.ident().clone();
                api_files.load_maybe_unparseable(file_name, result);
                if let Some(version) = version {
                    api_files.set_git_stub_commit(&ident, &version, commit);
                }
            }
            LocalFileResult::GitStubUnresolvable {
                file_name,
                original_contents,
                reason,
            } => {
                api_files.load_unparseable(
                    file_name,
                    original_contents,
                    reason,
                );
            }
            LocalFileResult::LockstepParseFailed { file_name } => {
                // The builder's `lockstep_file_name` produces the correct
                // warnings/errors.
                api_files.lockstep_file_name(&file_name);
            }
            LocalFileResult::VersionedParseFailed {
                dir_basename,
                file_name,
            } => {
                let ident = api_files
                    .lookup_versioned_dir(&mut seen_dirs, &dir_basename);
                if let Some(ident) = ident {
                    api_files.versioned_file_name(&ident, &file_name);
                }
            }
            LocalFileResult::GitStubParseFailed { dir_basename, file_name } => {
                let ident = api_files
                    .lookup_versioned_dir(&mut seen_dirs, &dir_basename);
                if let Some(ident) = ident {
                    api_files.versioned_git_stub_file_name(&ident, &file_name);
                }
            }
            LocalFileResult::LatestSymlink { dir_basename, path, target } => {
                let ident = api_files
                    .lookup_versioned_dir(&mut seen_dirs, &dir_basename);
                if let Some(ident) = ident
                    && let Some(v) =
                        api_files.symlink_contents(&path, &ident, &target)
                {
                    api_files.load_latest_link(&ident, v);
                }
            }
            LocalFileResult::LatestNotSymlink { path } => {
                api_files.load_warning(anyhow!(
                    "expected symlink but found regular file {:?}; \
                     will regenerate",
                    path
                ));
            }
            LocalFileResult::Warning(err) => {
                api_files.load_warning(err);
            }
            LocalFileResult::Error(err) => {
                api_files.load_error(err);
            }
        }
    }

    Ok(api_files)
}
