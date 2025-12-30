// Copyright 2025 Oxide Computer Company

//! Newtype and collection to represent OpenAPI documents from the "blessed"
//! source

use crate::{
    apis::ManagedApis,
    environment::ErrorAccumulator,
    git::{
        CommitHash, GitRef, GitRevision, git_first_commit_for_file,
        git_ls_tree, git_merge_base_head, git_show_file,
    },
    spec_files_generic::{
        ApiFiles, ApiLoad, ApiSpecFile, ApiSpecFilesBuilder, AsRawFiles,
    },
};
use anyhow::{anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use dropshot_api_manager_types::ApiIdent;
use std::{collections::BTreeMap, ops::Deref};

/// Newtype wrapper around [`ApiSpecFile`] to describe OpenAPI documents from
/// the "blessed" source.
///
/// The blessed source contains the documents that are not allowed to be changed
/// locally because they've been committed-to upstream.
///
/// Note that this type can represent documents for both lockstep APIs and
/// versioned APIs, but it's meaningless for lockstep APIs.  Any documents for
/// versioned APIs are blessed by definition.
pub struct BlessedApiSpecFile(ApiSpecFile);
NewtypeDebug! { () pub struct BlessedApiSpecFile(ApiSpecFile); }
NewtypeDeref! { () pub struct BlessedApiSpecFile(ApiSpecFile); }
NewtypeDerefMut! { () pub struct BlessedApiSpecFile(ApiSpecFile); }
NewtypeFrom! { () pub struct BlessedApiSpecFile(ApiSpecFile); }

// Trait impls that allow us to use `ApiFiles<BlessedApiSpecFile>`
//
// Note that this is NOT a `Vec` because it's NOT allowed to have more than one
// BlessedApiSpecFile for a given version.

impl ApiLoad for BlessedApiSpecFile {
    const MISCONFIGURATIONS_ALLOWED: bool = true;

    fn make_item(raw: ApiSpecFile) -> Self {
        BlessedApiSpecFile(raw)
    }

    fn try_extend(&mut self, item: ApiSpecFile) -> anyhow::Result<()> {
        // This should be impossible.
        bail!(
            "found more than one blessed OpenAPI document for a given \
             API version: at least {} and {}",
            self.spec_file_name(),
            item.spec_file_name()
        );
    }
}

impl AsRawFiles for BlessedApiSpecFile {
    fn as_raw_files<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a ApiSpecFile> + 'a> {
        Box::new(std::iter::once(self.deref()))
    }
}

/// Git reference information for a blessed file.
///
/// This tracks where a blessed file came from in git, so we can create git ref
/// files that point back to the original content.
///
/// For `.gitref` files, the commit is already known from parsing the file. For
/// JSON files, the commit is computed lazily to avoid slow `git log` calls when
/// git ref storage is disabled.
#[derive(Clone, Debug)]
pub enum BlessedGitRef {
    /// Already known (from parsing a `.gitref` file).
    Known {
        /// The git commit hash where this file was blessed.
        commit: CommitHash,
        /// The path within the repository (relative to repo root).
        path: Utf8PathBuf,
    },
    /// Needs computation (for JSON files that might need conversion).
    Lazy {
        /// The git revision to search within (typically the merge-base).
        revision: GitRevision,
        /// The path within the repository (relative to repo root).
        path: Utf8PathBuf,
    },
}

impl BlessedGitRef {
    /// Convert to a `GitRef` for reading content.
    ///
    /// For `Known` variants, this is a simple conversion. For `Lazy` variants,
    /// this calls `git log` to find the first commit that introduced the file.
    pub fn to_git_ref(&self, repo_root: &Utf8Path) -> anyhow::Result<GitRef> {
        match self {
            BlessedGitRef::Known { commit, path } => {
                Ok(GitRef { commit: *commit, path: path.clone() })
            }
            BlessedGitRef::Lazy { revision, path } => {
                let commit =
                    git_first_commit_for_file(repo_root, revision, path)?;
                Ok(GitRef { commit, path: path.clone() })
            }
        }
    }
}

/// Key for looking up git refs by API and version.
#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
struct GitRefKey {
    ident: ApiIdent,
    version: semver::Version,
}

/// Container for OpenAPI documents from the "blessed" source (usually Git).
///
/// **Be sure to check for load errors and warnings before using this
/// structure.**
///
/// For more on what's been validated at this point, see
/// [`ApiSpecFilesBuilder`].
#[derive(Debug)]
pub struct BlessedFiles {
    /// The loaded blessed files.
    files: BTreeMap<ApiIdent, ApiFiles<BlessedApiSpecFile>>,
    /// Git refs for each blessed file, keyed by (ident, version).
    git_refs: BTreeMap<GitRefKey, BlessedGitRef>,
}

impl Deref for BlessedFiles {
    type Target = BTreeMap<ApiIdent, ApiFiles<BlessedApiSpecFile>>;

    fn deref(&self) -> &Self::Target {
        &self.files
    }
}

impl BlessedFiles {
    /// Returns the git ref for the given API and version, if available.
    ///
    /// This is used to create git ref files that point back to the original
    /// blessed content in git.
    pub fn git_ref(
        &self,
        ident: &ApiIdent,
        version: &semver::Version,
    ) -> Option<&BlessedGitRef> {
        self.git_refs
            .get(&GitRefKey { ident: ident.clone(), version: version.clone() })
    }
}

impl BlessedFiles {
    /// Load OpenAPI documents from the given directory in the merge base
    /// between HEAD and the given branch.
    ///
    /// This is usually what users want.  For example, if these is the Git
    /// repository history:
    ///
    /// ```text
    /// main:  M1 -> M2 -> M3 -> M4
    ///         \
    /// branch:  +-- B1 --> B2
    /// ```
    ///
    /// and you're on `B2`, `main` refers to `M4`, but you want to be looking at
    /// `M1` for blessed documents because you haven't yet merged in commits M2,
    /// M3, and M4.
    pub fn load_from_git_parent_branch(
        repo_root: &Utf8Path,
        branch: &GitRevision,
        directory: &Utf8Path,
        apis: &ManagedApis,
        error_accumulator: &mut ErrorAccumulator,
    ) -> anyhow::Result<BlessedFiles> {
        let revision = git_merge_base_head(repo_root, branch)?;
        Self::load_from_git_revision(
            repo_root,
            &revision,
            directory,
            apis,
            error_accumulator,
        )
    }

    /// Load OpenAPI documents from the given Git revision and directory.
    pub fn load_from_git_revision(
        repo_root: &Utf8Path,
        commit: &GitRevision,
        directory: &Utf8Path,
        apis: &ManagedApis,
        error_accumulator: &mut ErrorAccumulator,
    ) -> anyhow::Result<BlessedFiles> {
        let mut api_files: ApiSpecFilesBuilder<BlessedApiSpecFile> =
            ApiSpecFilesBuilder::new(apis, error_accumulator);
        let mut git_refs: BTreeMap<GitRefKey, BlessedGitRef> = BTreeMap::new();

        let files_found = git_ls_tree(repo_root, commit, directory)?;
        for f in files_found {
            // We should be looking at either a single-component path
            // ("api.json") or a file inside one level of directory hierarchy
            // ("api/api-1.2.3-hash.json").  Figure out which case we're in.
            let parts: Vec<_> = f.iter().collect();
            if parts.is_empty() || parts.len() > 2 {
                api_files.load_warning(anyhow!(
                    "path {:?}: can't understand this path name",
                    f
                ));
                continue;
            }

            // Read the contents. Use "/" rather than "\" on Windows.
            let git_path = format!("{directory}/{f}");
            let contents = git_show_file(repo_root, commit, git_path.as_ref())?;

            if parts.len() == 1 {
                if let Some(spec_file_name) =
                    api_files.lockstep_file_name(parts[0])
                {
                    api_files.load_contents(spec_file_name, contents);
                    // Lockstep files don't need git refs since they're always
                    // regenerated.
                }
            } else if parts.len() == 2 {
                if let Some(ident) = api_files.versioned_directory(parts[0]) {
                    if ident.versioned_api_is_latest_symlink(parts[1]) {
                        // This is the "latest" symlink. We could dereference
                        // it and report it here, but it's not relevant for
                        // anything this tool does, so we don't bother.
                        continue;
                    }

                    // Handle .gitref files: read the git ref content, parse
                    // it, and load the actual JSON content from the referenced
                    // commit.
                    if parts[1].ends_with(".json.gitref") {
                        if let Some(spec_file_name) = api_files
                            .versioned_git_ref_file_name(&ident, parts[1])
                        {
                            // Parse the git ref content to get the referenced
                            // commit and path.
                            let git_ref_str =
                                String::from_utf8_lossy(&contents).to_string();
                            let git_ref: GitRef = match git_ref_str.parse() {
                                Ok(g) => g,
                                Err(err) => {
                                    api_files.load_error(anyhow!(err).context(
                                        format!(
                                            "parsing git ref file {:?}",
                                            git_path
                                        ),
                                    ));
                                    continue;
                                }
                            };

                            // Load the actual JSON content from the git ref.
                            let json_contents = match git_ref
                                .read_contents(repo_root)
                            {
                                Ok(c) => c,
                                Err(err) => {
                                    api_files.load_error(err.context(format!(
                                        "reading content for git ref {:?}",
                                        git_path
                                    )));
                                    continue;
                                }
                            };

                            // Track the git ref for this versioned file. The
                            // git ref already contains the first commit, so we
                            // use it directly.
                            if let Some(version) = spec_file_name.version() {
                                git_refs.insert(
                                    GitRefKey {
                                        ident: ident.clone(),
                                        version: version.clone(),
                                    },
                                    BlessedGitRef::Known {
                                        commit: git_ref.commit,
                                        path: git_ref.path.clone(),
                                    },
                                );
                            }

                            api_files
                                .load_contents(spec_file_name, json_contents);
                        }
                        continue;
                    }

                    if let Some(spec_file_name) =
                        api_files.versioned_file_name(&ident, parts[1])
                    {
                        // Track the git ref for this versioned file. Use Lazy
                        // so the first commit is only computed when needed
                        // (i.e., when git ref storage is enabled).
                        if let Some(version) = spec_file_name.version() {
                            git_refs.insert(
                                GitRefKey {
                                    ident: ident.clone(),
                                    version: version.clone(),
                                },
                                BlessedGitRef::Lazy {
                                    revision: commit.clone(),
                                    path: Utf8PathBuf::from(&git_path),
                                },
                            );
                        }

                        api_files.load_contents(spec_file_name, contents);
                    }
                }
            }
        }

        Ok(BlessedFiles { files: api_files.into_map(), git_refs })
    }
}

impl<'a> From<ApiSpecFilesBuilder<'a, BlessedApiSpecFile>> for BlessedFiles {
    fn from(api_files: ApiSpecFilesBuilder<'a, BlessedApiSpecFile>) -> Self {
        // When loading from a directory (e.g., for testing), we don't have
        // git refs.
        BlessedFiles { files: api_files.into_map(), git_refs: BTreeMap::new() }
    }
}
