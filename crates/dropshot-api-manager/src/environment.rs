// Copyright 2026 Oxide Computer Company

//! Describes the environment the command is running in, and particularly where
//! different sets of specifications are loaded from

use crate::{
    apis::ManagedApis,
    output::{
        Styles,
        headers::{GENERATING, HEADER_WIDTH},
    },
    spec_files_blessed::{BlessedApiSpecFile, BlessedFiles},
    spec_files_generated::GeneratedFiles,
    spec_files_generic::ApiSpecFilesBuilder,
    spec_files_local::{LocalFiles, walk_local_directory},
    vcs::{RepoVcs, RepoVcsKind, VcsRevision},
};
use anyhow::Context;
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use owo_colors::OwoColorize;

/// Default Git branch for the blessed source.
const DEFAULT_GIT_BRANCH: &str = "origin/main";

/// Default Jujutsu revset for the blessed source.
const DEFAULT_JJ_REVSET: &str = "trunk()";

/// Configuration for the Dropshot API manager.
///
/// This struct describes various properties of the environment the API manager
/// is running within, such as the command to invoke the OpenAPI manager, and
/// the repository root directory. For the full list of properties, see the
/// methods on this struct.
#[derive(Clone, Debug)]
pub struct Environment {
    /// The command to run the OpenAPI manager.
    pub(crate) command: String,

    /// Path to the root of this repository.
    pub(crate) repo_root: Utf8PathBuf,

    /// The default OpenAPI directory.
    pub(crate) default_openapi_dir: Utf8PathBuf,

    /// The default Git branch for the blessed source (e.g.,
    /// `"origin/main"`).
    pub(crate) default_git_branch: String,

    /// The default Jujutsu revset for the blessed source (e.g.,
    /// `"trunk()"`).
    pub(crate) default_jj_revset: String,

    /// The detected VCS backend.
    pub(crate) vcs: RepoVcs,
}

impl Environment {
    /// Creates a new environment with:
    ///
    /// * the command to invoke the OpenAPI manager (e.g. `"cargo openapi"`
    ///   or `"cargo xtask openapi"`)
    /// * the provided repository root
    /// * the default OpenAPI directory as a relative path within the
    ///   repository root
    ///
    /// The VCS backend is auto-detected from the repository root. The
    /// default blessed branch is `"origin/main"` for Git and `"trunk()"`
    /// for Jujutsu; the appropriate default is selected based on the
    /// detected VCS at resolution time.
    ///
    /// Returns an error if `repo_root` is not an absolute path or
    /// `default_openapi_dir` is not a relative path.
    pub fn new(
        command: impl Into<String>,
        repo_root: impl Into<Utf8PathBuf>,
        default_openapi_dir: impl Into<Utf8PathBuf>,
    ) -> anyhow::Result<Self> {
        let command = command.into();
        let repo_root = repo_root.into();
        let default_openapi_dir = default_openapi_dir.into();

        validate_paths(&repo_root, &default_openapi_dir)?;

        let vcs = RepoVcs::detect(&repo_root)?;

        Ok(Self {
            repo_root,
            default_openapi_dir,
            default_git_branch: DEFAULT_GIT_BRANCH.to_owned(),
            default_jj_revset: DEFAULT_JJ_REVSET.to_owned(),
            command,
            vcs,
        })
    }

    /// Sets the default Git branch used as the blessed source.
    ///
    /// By default, this is `origin/main`. The value should be a valid
    /// Git ref, e.g. `origin/main`, `upstream/dev`, or `main`.
    ///
    /// For individual commands, the revision can be overridden through
    /// the `--blessed-from-vcs` argument (or
    /// `OPENAPI_MGR_BLESSED_FROM_VCS`), and the path within the
    /// revision can be overridden through `--blessed-from-vcs-path`
    /// (or `OPENAPI_MGR_BLESSED_FROM_VCS_PATH`).
    pub fn with_default_git_branch(
        mut self,
        branch: impl Into<String>,
    ) -> Self {
        self.default_git_branch = branch.into();
        self
    }

    /// Sets the default Jujutsu revset used as the blessed source.
    ///
    /// By default, this is `trunk()`. The value should be a valid [jj
    /// revset](https://docs.jj-vcs.dev/latest/revsets/) expression (e.g.
    /// `trunk()`, `main`).
    ///
    /// For individual commands, this can be overridden through the command line
    /// or environment variables.
    pub fn with_default_jj_revset(mut self, revset: impl Into<String>) -> Self {
        self.default_jj_revset = revset.into();
        self
    }

    /// Creates a new environment without auto-detecting VCS.
    ///
    /// Uses the Git backend by default. This is intended for unit tests that
    /// don't exercise VCS operations and only need a valid `Environment` object
    /// for argument parsing tests.
    #[cfg(test)]
    pub(crate) fn new_for_test(
        command: impl Into<String>,
        repo_root: impl Into<Utf8PathBuf>,
        default_openapi_dir: impl Into<Utf8PathBuf>,
    ) -> anyhow::Result<Self> {
        let command = command.into();
        let repo_root = repo_root.into();
        let default_openapi_dir = default_openapi_dir.into();

        validate_paths(&repo_root, &default_openapi_dir)?;

        let vcs = RepoVcs::git()?;

        Ok(Self {
            repo_root,
            default_openapi_dir,
            default_git_branch: DEFAULT_GIT_BRANCH.to_owned(),
            default_jj_revset: DEFAULT_JJ_REVSET.to_owned(),
            command,
            vcs,
        })
    }

    pub(crate) fn resolve(
        &self,
        openapi_dir: Option<Utf8PathBuf>,
    ) -> anyhow::Result<ResolvedEnv> {
        // This is a bit tricky:
        //
        // * if the openapi_dir is provided:
        //   * first we determine the absolute path using `camino::absolute_utf8`
        //   * then we determine the path relative to the workspace root (erroring
        //     out if it is not a subdirectory)
        // * if the openapi_dir is not provided, we use default_openapi_dir as
        //   the relative directory, then join it with the workspace root to
        //   obtain the absolute directory.
        let (abs_dir, rel_dir) = match &openapi_dir {
            Some(provided_dir) => {
                // Determine the absolute path.
                let abs_dir = camino::absolute_utf8(provided_dir)
                    .with_context(|| {
                        format!(
                            "error making provided OpenAPI directory \
                             absolute: {}",
                            provided_dir
                        )
                    })?;

                // Determine the path relative to the workspace root.
                let rel_dir = abs_dir
                    .strip_prefix(&self.repo_root)
                    .with_context(|| {
                        format!(
                            "provided OpenAPI directory {} is not a \
                             subdirectory of repository root {}",
                            abs_dir, self.repo_root
                        )
                    })?
                    .to_path_buf();

                (abs_dir, rel_dir)
            }
            None => {
                let rel_dir = self.default_openapi_dir.clone();
                let abs_dir = self.repo_root.join(&rel_dir);
                (abs_dir, rel_dir)
            }
        };

        // Select the appropriate default blessed branch based on the
        // detected VCS backend.
        let default_blessed_branch = match self.vcs.kind() {
            RepoVcsKind::Git => self.default_git_branch.clone(),
            RepoVcsKind::Jj => self.default_jj_revset.clone(),
        };

        Ok(ResolvedEnv {
            command: self.command.clone(),
            repo_root: self.repo_root.clone(),
            local_source: LocalSource::Directory { abs_dir, rel_dir },
            default_blessed_branch,
            vcs: self.vcs.clone(),
        })
    }
}

/// Validate that `repo_root` is absolute and `default_openapi_dir` is a
/// normal relative path.
fn validate_paths(
    repo_root: &Utf8Path,
    default_openapi_dir: &Utf8Path,
) -> anyhow::Result<()> {
    if !repo_root.is_absolute() {
        return Err(anyhow::anyhow!(
            "repo_root must be an absolute path, found: {}",
            repo_root
        ));
    }

    if !is_normal_relative(default_openapi_dir) {
        return Err(anyhow::anyhow!(
            "default_openapi_dir must be a relative path with \
             normal components, found: {}",
            default_openapi_dir
        ));
    }

    Ok(())
}

fn is_normal_relative(default_openapi_dir: &Utf8Path) -> bool {
    default_openapi_dir
        .components()
        .all(|c| matches!(c, Utf8Component::Normal(_) | Utf8Component::CurDir))
}

/// Internal type for the environment where the OpenAPI directory is known.
#[derive(Debug)]
pub(crate) struct ResolvedEnv {
    pub(crate) command: String,
    pub(crate) repo_root: Utf8PathBuf,
    pub(crate) local_source: LocalSource,
    pub(crate) default_blessed_branch: String,
    pub(crate) vcs: RepoVcs,
}

impl ResolvedEnv {
    pub(crate) fn openapi_abs_dir(&self) -> &Utf8Path {
        match &self.local_source {
            LocalSource::Directory { abs_dir, .. } => abs_dir,
        }
    }

    pub(crate) fn openapi_rel_dir(&self) -> &Utf8Path {
        match &self.local_source {
            LocalSource::Directory { rel_dir, .. } => rel_dir,
        }
    }
}

/// Specifies where to find blessed OpenAPI documents (the ones that are
/// considered immutable because they've been committed-to upstream).
#[derive(Debug, Eq, PartialEq)]
pub enum BlessedSource {
    /// Blessed OpenAPI documents come from the VCS merge base between the
    /// current working state and the specified revision, in the specified
    /// directory.
    VcsRevisionMergeBase { revision: VcsRevision, directory: Utf8PathBuf },

    /// Blessed OpenAPI documents come from this directory.
    ///
    /// This is basically for testing and debugging this tool.
    Directory { local_directory: Utf8PathBuf },
}

impl BlessedSource {
    /// Load the blessed OpenAPI documents.
    pub fn load(
        &self,
        repo_root: &Utf8Path,
        apis: &ManagedApis,
        styles: &Styles,
        vcs: &RepoVcs,
    ) -> anyhow::Result<(BlessedFiles, ErrorAccumulator)> {
        let mut errors = ErrorAccumulator::new();
        match self {
            BlessedSource::Directory { local_directory } => {
                eprintln!(
                    "{:>HEADER_WIDTH$} blessed OpenAPI documents from {:?}",
                    "Loading".style(styles.success_header),
                    local_directory,
                );
                let api_files: ApiSpecFilesBuilder<'_, BlessedApiSpecFile> =
                    walk_local_directory(
                        local_directory,
                        apis,
                        &mut errors,
                        repo_root,
                        vcs,
                    )?;
                Ok((BlessedFiles::from(api_files), errors))
            }
            BlessedSource::VcsRevisionMergeBase { revision, directory } => {
                eprintln!(
                    "{:>HEADER_WIDTH$} blessed OpenAPI documents from VCS \
                     revision {:?} path {:?}",
                    "Loading".style(styles.success_header),
                    revision,
                    directory
                );
                Ok((
                    BlessedFiles::load_from_vcs_parent_branch(
                        repo_root,
                        revision,
                        directory,
                        apis,
                        &mut errors,
                        vcs,
                    )?,
                    errors,
                ))
            }
        }
    }
}

/// Specifies how to find generated OpenAPI documents
#[derive(Debug)]
pub enum GeneratedSource {
    /// Generate OpenAPI documents from the API implementation (default)
    Generated,

    /// Load "generated" OpenAPI documents from the specified directory
    ///
    /// This is basically just for testing and debugging this tool.
    Directory { local_directory: Utf8PathBuf },
}

impl GeneratedSource {
    /// Load the generated OpenAPI documents (i.e., generating them as needed).
    pub fn load(
        &self,
        apis: &ManagedApis,
        styles: &Styles,
        repo_root: &Utf8Path,
        vcs: &RepoVcs,
    ) -> anyhow::Result<(GeneratedFiles, ErrorAccumulator)> {
        let mut errors = ErrorAccumulator::new();
        match self {
            GeneratedSource::Generated => {
                eprintln!(
                    "{:>HEADER_WIDTH$} OpenAPI documents from API \
                     definitions ... ",
                    GENERATING.style(styles.success_header)
                );
                Ok((GeneratedFiles::generate(apis, &mut errors)?, errors))
            }
            GeneratedSource::Directory { local_directory } => {
                eprintln!(
                    "{:>HEADER_WIDTH$} \"generated\" OpenAPI documents from \
                     {:?} ... ",
                    "Loading".style(styles.success_header),
                    local_directory,
                );
                let api_files = walk_local_directory(
                    local_directory,
                    apis,
                    &mut errors,
                    repo_root,
                    vcs,
                )?;
                Ok((GeneratedFiles::from(api_files), errors))
            }
        }
    }
}

/// Specifies where to find local OpenAPI documents
#[derive(Debug)]
pub enum LocalSource {
    /// Local OpenAPI documents come from this directory
    Directory {
        /// The absolute directory path.
        abs_dir: Utf8PathBuf,
        /// The directory path relative to the repo root. Used for VCS commands
        /// that read contents of other commits.
        rel_dir: Utf8PathBuf,
    },
}

impl LocalSource {
    /// Load the local OpenAPI documents.
    ///
    /// The `repo_root` parameter is needed to resolve `.gitstub` files.
    pub fn load(
        &self,
        apis: &ManagedApis,
        styles: &Styles,
        repo_root: &Utf8Path,
        vcs: &RepoVcs,
    ) -> anyhow::Result<(LocalFiles, ErrorAccumulator)> {
        let mut errors = ErrorAccumulator::new();

        // Shallow clones and Git stub storage are incompatible.
        let any_uses_git_stub =
            apis.iter_apis().any(|a| apis.uses_git_stub_storage(a));
        if any_uses_git_stub && vcs.is_shallow_clone(repo_root) {
            errors.error(anyhow::anyhow!(
                "this repository is a shallow clone, but Git stub storage is \
                 enabled for some APIs. Git stubs cannot be resolved in a \
                 shallow clone because the referenced commits may not be \
                 available. To fix this, fetch complete history (e.g. \
                 `git fetch --unshallow`) or make a fresh clone without \
                 --depth."
            ));
            return Ok((LocalFiles::default(), errors));
        }

        match self {
            LocalSource::Directory { abs_dir, .. } => {
                eprintln!(
                    "{:>HEADER_WIDTH$} local OpenAPI documents from \
                     {:?} ... ",
                    "Loading".style(styles.success_header),
                    abs_dir,
                );
                Ok((
                    LocalFiles::load_from_directory(
                        abs_dir,
                        apis,
                        &mut errors,
                        repo_root,
                        vcs,
                    )?,
                    errors,
                ))
            }
        }
    }
}

/// Stores errors and warnings accumulated during loading
pub struct ErrorAccumulator {
    /// errors that reflect incorrectness or incompleteness of the loaded data
    errors: Vec<anyhow::Error>,
    /// problems that do not affect the correctness or completeness of the data
    warnings: Vec<anyhow::Error>,
}

impl ErrorAccumulator {
    pub fn new() -> ErrorAccumulator {
        ErrorAccumulator { errors: Vec::new(), warnings: Vec::new() }
    }

    /// Record an error
    pub fn error(&mut self, error: anyhow::Error) {
        self.errors.push(error);
    }

    /// Record a warning
    pub fn warning(&mut self, error: anyhow::Error) {
        self.warnings.push(error);
    }

    pub fn iter_errors(&self) -> impl Iterator<Item = &'_ anyhow::Error> + '_ {
        self.errors.iter()
    }

    pub fn iter_warnings(
        &self,
    ) -> impl Iterator<Item = &'_ anyhow::Error> + '_ {
        self.warnings.iter()
    }
}
