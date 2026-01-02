// Copyright 2025 Oxide Computer Company

//! Newtype and collection to represent OpenAPI documents local to this working
//! tree

use crate::{
    apis::ManagedApis,
    environment::ErrorAccumulator,
    git::GitRef,
    spec_files_generic::{
        ApiFiles, ApiLoad, ApiSpecFile, ApiSpecFilesBuilder, AsRawFiles,
        SpecFileInfo,
    },
};
use anyhow::{Context, anyhow};
use camino::Utf8Path;
use dropshot_api_manager_types::{ApiIdent, ApiSpecFileName};
use std::collections::BTreeMap;

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
    Valid(Box<ApiSpecFile>),
    /// A file that exists but couldn't be parsed.
    ///
    /// This happens when a file has merge conflict markers or is otherwise
    /// corrupted. The version is known from the filename, allowing the
    /// generate command to regenerate the correct content.
    ///
    /// We still store the raw contents so they can be accessed if needed
    /// (e.g., for diffing or debugging).
    Unparseable {
        /// The parsed filename (contains version and hash info).
        name: ApiSpecFileName,
        /// The raw file contents that couldn't be parsed.
        contents: Vec<u8>,
    },
}

impl LocalApiSpecFile {
    /// Returns the spec file name.
    pub fn spec_file_name(&self) -> &ApiSpecFileName {
        match self {
            LocalApiSpecFile::Valid(spec) => spec.spec_file_name(),
            LocalApiSpecFile::Unparseable { name, .. } => name,
        }
    }

    /// Returns the raw file contents.
    ///
    /// This works for both valid and unparseable files.
    pub fn contents(&self) -> &[u8] {
        match self {
            LocalApiSpecFile::Valid(spec) => spec.contents(),
            LocalApiSpecFile::Unparseable { contents, .. } => contents,
        }
    }

    /// Returns true if this file is unparseable.
    pub fn is_unparseable(&self) -> bool {
        matches!(self, LocalApiSpecFile::Unparseable { .. })
    }
}

impl SpecFileInfo for LocalApiSpecFile {
    fn spec_file_name(&self) -> &ApiSpecFileName {
        match self {
            LocalApiSpecFile::Valid(spec) => spec.spec_file_name(),
            LocalApiSpecFile::Unparseable { name, .. } => name,
        }
    }

    fn parsed_version(&self) -> Option<&semver::Version> {
        match self {
            LocalApiSpecFile::Valid(spec) => Some(spec.version()),
            LocalApiSpecFile::Unparseable { .. } => None,
        }
    }
}

// Trait impls that allow us to use `ApiFiles<Vec<LocalApiSpecFile>>`
//
// Note that this is a `Vec` because it's allowed to have more than one
// LocalApiSpecFile for a given version.

impl ApiLoad for Vec<LocalApiSpecFile> {
    const MISCONFIGURATIONS_ALLOWED: bool = false;
    const UNPARSEABLE_FILES_ALLOWED: bool = true;

    fn try_extend(&mut self, item: ApiSpecFile) -> anyhow::Result<()> {
        self.push(LocalApiSpecFile::Valid(Box::new(item)));
        Ok(())
    }

    fn make_item(raw: ApiSpecFile) -> Self {
        vec![LocalApiSpecFile::Valid(Box::new(raw))]
    }

    fn make_unparseable_item(
        name: ApiSpecFileName,
        contents: Vec<u8>,
    ) -> Option<Self> {
        Some(vec![LocalApiSpecFile::Unparseable { name, contents }])
    }

    fn try_extend_unparseable(
        &mut self,
        name: ApiSpecFileName,
        contents: Vec<u8>,
    ) {
        self.push(LocalApiSpecFile::Unparseable { name, contents });
    }
}

impl AsRawFiles for Vec<LocalApiSpecFile> {
    fn as_raw_files<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a dyn SpecFileInfo> + 'a> {
        Box::new(self.iter().map(|f| f as &dyn SpecFileInfo))
    }
}

/// Container for OpenAPI documents found in the local working tree
///
/// **Be sure to check for load errors and warnings before using this
/// structure.**
///
/// For more on what's been validated at this point, see
/// [`ApiSpecFilesBuilder`].
#[derive(Debug)]
pub struct LocalFiles(BTreeMap<ApiIdent, ApiFiles<Vec<LocalApiSpecFile>>>);

NewtypeDeref! {
    () pub struct LocalFiles(
        BTreeMap<ApiIdent, ApiFiles<Vec<LocalApiSpecFile>>>
    );
}

impl LocalFiles {
    /// Load OpenAPI documents from a given directory tree.
    ///
    /// If it's at all possible to load any documents, this will return an `Ok`
    /// value, but you should still check the `errors` field on the returned
    /// [`LocalFiles`].
    ///
    /// The `repo_root` parameter is needed to resolve `.gitref` files, which
    /// store a reference to an OpenAPI document rather than the document
    /// itself.
    pub fn load_from_directory(
        dir: &Utf8Path,
        apis: &ManagedApis,
        error_accumulator: &mut ErrorAccumulator,
        repo_root: &Utf8Path,
    ) -> anyhow::Result<LocalFiles> {
        let api_files =
            walk_local_directory(dir, apis, error_accumulator, repo_root)?;
        Ok(Self::from(api_files))
    }
}

impl From<ApiSpecFilesBuilder<'_, Vec<LocalApiSpecFile>>> for LocalFiles {
    fn from(api_files: ApiSpecFilesBuilder<Vec<LocalApiSpecFile>>) -> Self {
        LocalFiles(api_files.into_map())
    }
}

/// Load OpenAPI documents for the local directory tree.
///
/// Under `dir`, we expect to find either:
///
/// * for each lockstep API, a file called `api-ident.json` (e.g.,
///   `wicketd.json`)
/// * for each versioned API, a directory called `api-ident` that contains:
///     * any number of files called `api-ident-SEMVER-HASH.json`
///       (e.g., dns-server-1.0.0-eb52aeeb.json)
///     * any number of git ref files called `api-ident-SEMVER-HASH.json.gitref`
///       that contain a `commit:path` reference to the actual content
///     * one symlink called `api-ident-latest.json` that points to a file in
///       the same directory
///
/// Here's an example:
///
/// ```text
/// wicketd.json                                     # file for lockstep API
/// dns-server/                                      # directory for versioned API
/// dns-server/dns-server-1.0.0-eb2aeeb.json         # file for versioned API
/// dns-server/dns-server-2.0.0-fba287a.json.gitref  # git ref for versioned API
/// dns-server/dns-server-3.0.0-298ea47.json         # file for versioned API
/// dns-server/dns-server-latest.json                # symlink
/// ```
// This function is always used for the "local" files. It can sometimes be
// used for both generated and blessed files, if the user asks to load those
// from the local filesystem instead of their usual sources.
pub fn walk_local_directory<'a, T: ApiLoad + AsRawFiles>(
    dir: &'_ Utf8Path,
    apis: &'a ManagedApis,
    error_accumulator: &'a mut ErrorAccumulator,
    repo_root: &Utf8Path,
) -> anyhow::Result<ApiSpecFilesBuilder<'a, T>> {
    let mut api_files = ApiSpecFilesBuilder::new(apis, error_accumulator);
    let entry_iter =
        dir.read_dir_utf8().with_context(|| format!("readdir {:?}", dir))?;
    for maybe_entry in entry_iter {
        let entry =
            maybe_entry.with_context(|| format!("readdir {:?} entry", dir))?;

        // If this entry is a file, then we'd expect it to be the JSON file
        // for one of our lockstep APIs. Check and see.
        let path = entry.path();
        let file_name = entry.file_name();
        let file_type = entry
            .file_type()
            .with_context(|| format!("file type of {:?}", path))?;
        if file_type.is_file() {
            match fs_err::read(path) {
                Ok(contents) => {
                    if let Some(file_name) =
                        api_files.lockstep_file_name(file_name)
                    {
                        api_files.load_contents(file_name, contents);
                    }
                }
                Err(error) => {
                    api_files.load_error(anyhow!(error));
                }
            };
        } else if file_type.is_dir() {
            load_versioned_directory(
                &mut api_files,
                path,
                file_name,
                repo_root,
            );
        } else {
            // This is not something the tool cares about, but it's not
            // obviously a problem, either.
            api_files.load_warning(anyhow!(
                "ignored (not a file or directory): {:?}",
                path
            ));
        };
    }

    Ok(api_files)
}

/// Load the contents of a directory that corresponds to a versioned API.
///
/// See [`walk_local_directory()`] for what we expect to find.
fn load_versioned_directory<T: ApiLoad + AsRawFiles>(
    api_files: &mut ApiSpecFilesBuilder<'_, T>,
    path: &Utf8Path,
    basename: &str,
    repo_root: &Utf8Path,
) {
    let Some(ident) = api_files.versioned_directory(basename) else {
        return;
    };

    let entries = match path
        .read_dir_utf8()
        .and_then(|entry_iter| entry_iter.collect::<Result<Vec<_>, _>>())
    {
        Ok(entries) => entries,
        Err(error) => {
            api_files.load_error(
                anyhow!(error).context(format!("readdir {:?}", path)),
            );
            return;
        }
    };

    for entry in entries {
        let file_name = entry.file_name();

        if ident.versioned_api_is_latest_symlink(file_name) {
            // We should be looking at a symlink. However, VCS tools like jj
            // can turn symlinks into regular files with conflict markers when
            // there's a symlink conflict. In that case, we treat it as a
            // missing/corrupted symlink and let generate recreate it.
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(error) => {
                    api_files.load_warning(anyhow!(error).context(format!(
                        "failed to get file type for {:?}",
                        entry.path()
                    )));
                    continue;
                }
            };

            if !file_type.is_symlink() {
                // This is not a symlink (likely corrupted by a merge conflict).
                // Skip it so generate will recreate it.
                api_files.load_warning(anyhow!(
                    "expected symlink but found regular file {:?}; \
                     will regenerate",
                    entry.path()
                ));
                continue;
            }

            let symlink = match entry.path().read_link_utf8() {
                Ok(s) => s,
                Err(error) => {
                    api_files.load_error(anyhow!(error).context(format!(
                        "read what should be a symlink {:?}",
                        entry.path()
                    )));
                    continue;
                }
            };

            if let Some(v) = api_files.symlink_contents(
                entry.path(),
                &ident,
                symlink.as_str(),
            ) {
                api_files.load_latest_link(&ident, v);
            }
            continue;
        }

        // Handle .gitref files: these contain a `commit:path` reference to the
        // actual content in git.
        if file_name.ends_with(".json.gitref") {
            let Some(spec_file_name) =
                api_files.versioned_git_ref_file_name(&ident, file_name)
            else {
                continue;
            };

            let git_ref_contents = match fs_err::read_to_string(entry.path()) {
                Ok(content) => content,
                Err(error) => {
                    api_files.load_error(anyhow!(error).context(format!(
                        "failed to read git ref file {:?}",
                        entry.path()
                    )));
                    continue;
                }
            };

            let git_ref = match git_ref_contents.parse::<GitRef>() {
                Ok(git_ref) => git_ref,
                Err(error) => {
                    api_files.load_error(anyhow!(error).context(format!(
                        "failed to parse git ref file {:?}",
                        entry.path()
                    )));
                    continue;
                }
            };

            let contents = match git_ref.read_contents(repo_root) {
                Ok(contents) => contents,
                Err(error) => {
                    api_files.load_error(error.context(format!(
                        "failed to read content for git ref {:?}",
                        entry.path()
                    )));
                    continue;
                }
            };

            api_files.load_contents(spec_file_name, contents);
            continue;
        }

        // Handle regular .json files.
        let Some(file_name) = api_files.versioned_file_name(&ident, file_name)
        else {
            continue;
        };

        let contents = match fs_err::read(entry.path()) {
            Ok(contents) => contents,
            Err(error) => {
                api_files.load_error(anyhow!(error));
                continue;
            }
        };

        api_files.load_contents(file_name, contents);
    }
}
