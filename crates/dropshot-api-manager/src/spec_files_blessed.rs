// Copyright 2026 Oxide Computer Company

//! Newtype and collection to represent OpenAPI documents from the "blessed"
//! source

use crate::{
    apis::ManagedApis,
    environment::ErrorAccumulator,
    git::{
        GitRevision, git_first_commit_for_file, git_is_ancestor, git_ls_tree,
        git_merge_base_head, git_show_file,
    },
    spec_files_generic::{
        ApiFiles, ApiLoad, ApiSpecFile, ApiSpecFilesBuilder, AsRawFiles,
        GitStubKey, SpecFileInfo,
    },
};
use anyhow::{anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use dropshot_api_manager_types::{
    ApiIdent, ApiSpecFileName, VersionedApiSpecFileName,
};
use git_stub::{GitCommitHash, GitStub};
use git_stub_vcs::Vcs;
use std::{collections::BTreeMap, ops::Deref};

/// Newtype wrapper around [`ApiSpecFile`] to describe OpenAPI documents from
/// the "blessed" source.
///
/// The blessed source contains the documents that are not allowed to be changed
/// locally because they've been committed-to upstream.
///
/// This type only represents versioned APIs, not lockstep APIs. Lockstep APIs
/// don't have a meaningful "blessed" source since they're always regenerated.
/// The type system enforces this invariant: construction will panic if given a
/// lockstep spec.
pub struct BlessedApiSpecFile {
    inner: ApiSpecFile,
    /// Cached versioned filename, avoiding repeated conversion.
    versioned_name: VersionedApiSpecFileName,
}

impl std::fmt::Debug for BlessedApiSpecFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlessedApiSpecFile")
            .field("inner", &self.inner)
            .finish()
    }
}

impl Deref for BlessedApiSpecFile {
    type Target = ApiSpecFile;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl BlessedApiSpecFile {
    /// Creates a new `BlessedApiSpecFile` from an `ApiSpecFile`.
    ///
    /// # Panics
    ///
    /// Panics if the spec file is for a lockstep API. Blessed files only exist
    /// for versioned APIs.
    pub fn new(inner: ApiSpecFile) -> Self {
        let versioned_name = inner
            .spec_file_name()
            .as_versioned()
            .unwrap_or_else(|| {
                panic!(
                    "BlessedApiSpecFile requires a versioned API spec, \
                     got lockstep: {}",
                    inner.spec_file_name()
                )
            })
            .clone();
        Self { inner, versioned_name }
    }

    /// Returns the versioned spec file name.
    ///
    /// Unlike `spec_file_name()` which returns `&ApiSpecFileName`, this method
    /// returns the more specific `&VersionedApiSpecFileName` since blessed
    /// files are always versioned.
    pub fn versioned_spec_file_name(&self) -> &VersionedApiSpecFileName {
        &self.versioned_name
    }
}

// Trait impls that allow us to use `ApiFiles<BlessedApiSpecFile>`
//
// Note that this is NOT a `Vec` because it's NOT allowed to have more than one
// BlessedApiSpecFile for a given version.

impl ApiLoad for BlessedApiSpecFile {
    const MISCONFIGURATIONS_ALLOWED: bool = true;
    type Unparseable = std::convert::Infallible;

    fn make_item(raw: ApiSpecFile) -> Self {
        BlessedApiSpecFile::new(raw)
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

    fn make_unparseable(
        _name: ApiSpecFileName,
        _contents: Vec<u8>,
    ) -> Option<Self::Unparseable> {
        None
    }

    fn unparseable_into_self(unparseable: Self::Unparseable) -> Self {
        match unparseable {}
    }

    fn extend_unparseable(&mut self, unparseable: Self::Unparseable) {
        match unparseable {}
    }
}

impl AsRawFiles for BlessedApiSpecFile {
    fn as_raw_files<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a dyn SpecFileInfo> + 'a> {
        Box::new(std::iter::once(self.deref() as &dyn SpecFileInfo))
    }
}

/// Git stub information for a blessed file.
///
/// This tracks where a blessed file came from in git, so we can create
/// `.gitstub` files that point back to the original content.
///
/// For `.gitstub` files, the commit is already known from parsing the file.
/// For JSON files, the commit is computed lazily to avoid slow `git log`
/// calls when Git stub storage is disabled.
#[derive(Clone, Debug)]
pub enum BlessedGitStub {
    /// The Git stub is already known. Obtained by from parsing a `.gitstub`
    /// file.
    Known {
        /// The git commit hash where this file was blessed.
        commit: GitCommitHash,
        /// The path within the repository, relative to the repo root.
        path: Utf8PathBuf,
    },
    /// The Git stub needs to be computed. Obtained through JSON files, and
    /// only resolved if conversions are required.
    Lazy {
        /// The git revision to search within (typically the merge-base).
        revision: GitRevision,
        /// The path within the repository, relative to the repo root.
        path: Utf8PathBuf,
    },
}

impl BlessedGitStub {
    /// Convert to a `GitStub` for reading content.
    ///
    /// For `Known` variants, this validates that the stored commit is an
    /// ancestor of the merge base. If it is not (e.g., after a rebase or
    /// force-push), the correct commit is recomputed via `git log`. For
    /// `Lazy` variants, this always calls `git log` to find the first commit
    /// that introduced the file.
    ///
    /// If `merge_base` is `None` (directory-based loading), `Known` commits
    /// are trusted as-is.
    pub fn to_git_stub(
        &self,
        repo_root: &Utf8Path,
        merge_base: Option<&GitRevision>,
    ) -> anyhow::Result<GitStub> {
        match self {
            BlessedGitStub::Known { commit, path } => {
                if let Some(merge_base) = merge_base {
                    // Check that the stored commit is still an ancestor
                    // of the merge base. If not, the commit is stale
                    // (e.g., after a rebase) and needs to be recomputed.
                    let commit_rev = GitRevision::from(*commit);
                    if !git_is_ancestor(repo_root, &commit_rev, merge_base)? {
                        let commit = git_first_commit_for_file(
                            repo_root, merge_base, path,
                        )?;
                        return Ok(GitStub::new(commit, path.clone())?);
                    }
                }
                Ok(GitStub::new(*commit, path.clone())?)
            }
            BlessedGitStub::Lazy { revision, path } => {
                let commit =
                    git_first_commit_for_file(repo_root, revision, path)?;
                Ok(GitStub::new(commit, path.clone())?)
            }
        }
    }
}

/// Represents the structure of a path found during blessed file enumeration.
///
/// This enum captures what we can determine from path structure alone, before
/// any API-level validation.
enum BlessedPathKind<'a> {
    /// Single-component path (e.g., "api.json"). Potential lockstep file.
    /// These are skipped since blessed files only exist for versioned APIs.
    Lockstep,

    /// Two-component path with `.json.gitstub` extension. Potential versioned
    /// Git stub.
    GitStubFile { api_dir: &'a str, basename: &'a str },

    /// Two-component path (e.g., "api/api-1.2.3-hash.json"). Could be a
    /// versioned file or latest symlink - requires API validation.
    VersionedFile { api_dir: &'a str, basename: &'a str },
}

/// Path structure we don't understand (empty, >2 components, etc.).
struct UnrecognizedPath;

impl<'a> BlessedPathKind<'a> {
    /// Parse a path from git ls-tree output into its structural kind.
    fn parse(path: &'a Utf8Path) -> Result<Self, UnrecognizedPath> {
        let parts: Vec<_> = path.iter().collect();
        match parts.as_slice() {
            [_basename] => Ok(BlessedPathKind::Lockstep),
            [api_dir, basename] if basename.ends_with(".json.gitstub") => {
                Ok(BlessedPathKind::GitStubFile { api_dir, basename })
            }
            [api_dir, basename] => {
                Ok(BlessedPathKind::VersionedFile { api_dir, basename })
            }
            _ => Err(UnrecognizedPath),
        }
    }
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
    /// Git stubs for each blessed file, keyed by (ident, version).
    git_stubs: BTreeMap<GitStubKey, BlessedGitStub>,
    /// The merge base used when loading blessed files from git.
    ///
    /// This is `Some` when loaded via `load_from_git_parent_branch` or
    /// `load_from_git_revision`, and `None` when loaded from a directory.
    merge_base: Option<GitRevision>,
}

impl Deref for BlessedFiles {
    type Target = BTreeMap<ApiIdent, ApiFiles<BlessedApiSpecFile>>;

    fn deref(&self) -> &Self::Target {
        &self.files
    }
}

impl BlessedFiles {
    /// Returns the Git stub for the given API and version, if available.
    ///
    /// This is used to create `.gitstub` files that point back to the
    /// original blessed content in git.
    pub fn git_stub(
        &self,
        ident: &ApiIdent,
        version: &semver::Version,
    ) -> Option<&BlessedGitStub> {
        self.git_stubs
            .get(&GitStubKey { ident: ident.clone(), version: version.clone() })
    }

    /// Returns the merge base used when loading blessed files from git.
    ///
    /// This is `Some` when loaded from git, and `None` when loaded from a
    /// directory.
    pub fn merge_base(&self) -> Option<&GitRevision> {
        self.merge_base.as_ref()
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
        let mut git_stubs: BTreeMap<GitStubKey, BlessedGitStub> =
            BTreeMap::new();

        let files_found = git_ls_tree(repo_root, commit, directory)?;
        for f in files_found {
            let kind = match BlessedPathKind::parse(&f) {
                Ok(kind) => kind,
                Err(UnrecognizedPath) => {
                    api_files.load_warning(anyhow!(
                        "path {:?}: can't understand this path name",
                        f
                    ));
                    continue;
                }
            };

            // Lockstep files are not loaded from blessed sources. They're
            // always regenerated from the current code, so there's no
            // "blessed" version to compare against.
            if matches!(kind, BlessedPathKind::Lockstep) {
                continue;
            }

            // Read the contents. Use "/" rather than "\" on Windows.
            let git_path = format!("{directory}/{f}");
            let contents = git_show_file(repo_root, commit, git_path.as_ref())?;

            match kind {
                BlessedPathKind::Lockstep => {
                    unreachable!("handled above");
                }

                BlessedPathKind::VersionedFile { api_dir, basename } => {
                    let Some(ident) = api_files.versioned_directory(api_dir)
                    else {
                        continue;
                    };

                    // This is the "latest" symlink. We could dereference it and
                    // report it here, but it's not relevant for anything this
                    // tool does, so we don't bother.
                    if ident.versioned_api_is_latest_symlink(basename) {
                        continue;
                    }

                    let Some(spec_file_name) =
                        api_files.versioned_file_name(&ident, basename)
                    else {
                        continue;
                    };

                    // Track the Git stub for this versioned file. Use Lazy
                    // so the first commit is only computed when needed
                    // (i.e., when Git stub storage is enabled).
                    if let Some(version) = spec_file_name.version() {
                        git_stubs.insert(
                            GitStubKey {
                                ident: ident.clone(),
                                version: version.clone(),
                            },
                            BlessedGitStub::Lazy {
                                revision: commit.clone(),
                                path: Utf8PathBuf::from(&git_path),
                            },
                        );
                    }

                    api_files.load_contents(spec_file_name, contents);
                }

                BlessedPathKind::GitStubFile { api_dir, basename } => {
                    let Some(ident) = api_files.versioned_directory(api_dir)
                    else {
                        continue;
                    };
                    let Some(spec_file_name) = api_files
                        .versioned_git_stub_file_name(&ident, basename)
                    else {
                        continue;
                    };

                    // Parse the Git stub content to get the referenced
                    // commit and path.
                    let git_stub_str =
                        String::from_utf8_lossy(&contents).to_string();
                    let git_stub: GitStub = match git_stub_str.parse() {
                        Ok(g) => g,
                        Err(err) => {
                            api_files.load_error(anyhow!(err).context(
                                format!("parsing Git stub {:?}", git_path),
                            ));
                            continue;
                        }
                    };

                    // Load the actual JSON content from the Git stub.
                    let json_contents = match Vcs::git()?
                        .read_git_stub_contents(&git_stub, repo_root)
                    {
                        Ok(c) => c,
                        Err(err) => {
                            api_files.load_error(
                                anyhow::Error::new(err).context(format!(
                                    "reading content for Git stub {:?}",
                                    git_path
                                )),
                            );
                            continue;
                        }
                    };

                    // Track the Git stub for this versioned file. The Git
                    // stub already contains the first commit, so we use it
                    // directly.
                    if let Some(version) = spec_file_name.version() {
                        git_stubs.insert(
                            GitStubKey {
                                ident: ident.clone(),
                                version: version.clone(),
                            },
                            BlessedGitStub::Known {
                                commit: git_stub.commit(),
                                path: git_stub.path().to_owned(),
                            },
                        );
                    }

                    api_files.load_contents(spec_file_name, json_contents);
                }
            }
        }

        let files = api_files.into_map();
        Ok(BlessedFiles { files, git_stubs, merge_base: Some(commit.clone()) })
    }
}

impl<'a> From<ApiSpecFilesBuilder<'a, BlessedApiSpecFile>> for BlessedFiles {
    fn from(api_files: ApiSpecFilesBuilder<'a, BlessedApiSpecFile>) -> Self {
        // When loading from a directory, we don't have Git stubs or a merge
        // base.
        BlessedFiles {
            files: api_files.into_map(),
            git_stubs: BTreeMap::new(),
            merge_base: None,
        }
    }
}
