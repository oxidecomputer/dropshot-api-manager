// Copyright 2026 Oxide Computer Company

//! Resolve different sources of API information (blessed, local, upstream)

use crate::{
    apis::{ManagedApi, ManagedApis},
    compatibility::{ApiCompatIssue, api_compatible},
    environment::ResolvedEnv,
    git::{GitCommitHash, GitRef},
    iter_only::iter_only,
    output::{InlineErrorChain, plural},
    spec_files_blessed::{BlessedApiSpecFile, BlessedFiles, BlessedGitRef},
    spec_files_generated::{GeneratedApiSpecFile, GeneratedFiles},
    spec_files_generic::{ApiFiles, UnparseableFile},
    spec_files_local::{LocalApiSpecFile, LocalFiles},
    validation::{
        CheckStale, CheckStatus, DynValidationFn, overwrite_file, validate,
    },
};
use anyhow::{Context, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use dropshot_api_manager_types::{ApiIdent, ApiSpecFileName};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt::{Debug, Display},
};
use thiserror::Error;

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct DisplayableVec<T>(pub Vec<T>);
impl<T> Display for DisplayableVec<T>
where
    T: Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // slice::join would require the use of unstable Rust.
        let mut iter = self.0.iter();
        if let Some(item) = iter.next() {
            write!(f, "{item}")?;
        }

        for item in iter {
            write!(f, ", {item}")?;
        }

        Ok(())
    }
}

/// A non-error note that's worth highlighting to the user.
// These are not technically errors, but it is useful to treat them the same
// way in terms of having an associated message, etc.
#[derive(Debug, Error)]
pub enum Note {
    /// A previously-supported API version has been removed locally.
    ///
    /// This is not an error because we do expect to EOL old API specs. There's
    /// not currently a way for this tool to know if the EOL'ing is correct or
    /// not, so we at least highlight it to the user.
    #[error(
        "API {api_ident} version {version}: formerly blessed version has been \
         removed.  This version will no longer be supported!  This will break \
         upgrade from software that still uses this version.  If this is \
         unexpected, check the list of supported versions in Rust for a \
         possible mismerge."
    )]
    BlessedVersionRemoved { api_ident: ApiIdent, version: semver::Version },
}

/// Describes the result of resolving the blessed spec(s), generated spec(s),
/// and local spec files for a particular API
pub struct Resolution<'a> {
    kind: ResolutionKind,
    problems: Vec<Problem<'a>>,
}

impl<'a> Resolution<'a> {
    pub fn new_lockstep(problems: Vec<Problem<'a>>) -> Resolution<'a> {
        Resolution { kind: ResolutionKind::Lockstep, problems }
    }

    pub fn new_blessed(problems: Vec<Problem<'a>>) -> Resolution<'a> {
        Resolution { kind: ResolutionKind::Blessed, problems }
    }

    pub fn new_new_locally(problems: Vec<Problem<'a>>) -> Resolution<'a> {
        Resolution { kind: ResolutionKind::NewLocally, problems }
    }

    pub fn has_problems(&self) -> bool {
        !self.problems.is_empty()
    }

    /// Add a problem to this resolution.
    pub fn add_problem(&mut self, problem: Problem<'a>) {
        self.problems.push(problem);
    }

    pub fn has_errors(&self) -> bool {
        self.problems().any(|p| !p.is_fixable())
    }

    pub fn problems(&self) -> impl Iterator<Item = &'_ Problem<'a>> + '_ {
        self.problems.iter()
    }

    pub fn kind(&self) -> ResolutionKind {
        self.kind
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionKind {
    /// This is a lockstep API
    Lockstep,
    /// This is a versioned API and this version is blessed
    Blessed,
    /// This version is new to the current workspace (i.e., not present
    /// upstream)
    NewLocally,
}

impl Display for ResolutionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ResolutionKind::Lockstep => "lockstep",
            ResolutionKind::Blessed => "blessed",
            ResolutionKind::NewLocally => "added locally",
        })
    }
}

/// Describes a problem resolving the blessed spec(s), generated spec(s), and
/// local spec files for a particular API.
#[derive(Debug, Error)]
pub enum Problem<'a> {
    // These problems are not associated with any *supported* version of an API.
    #[error(
        "A local OpenAPI document was found that does not correspond to a \
         supported version of this API: {spec_file_name}.  This is unusual, \
         but it could happen if you're either retiring an older version of \
         this API or if you created this version in this branch and later \
         merged with upstream and had to change your local version number.  \
         In either case, this tool can remove the unused file for you."
    )]
    LocalSpecFileOrphaned { spec_file_name: ApiSpecFileName },

    #[error(
        "A local OpenAPI document could not be parsed: {}. \
         This may happen if the file has merge conflict markers or is \
         otherwise corrupted. This tool can delete this file and regenerate \
         the correct one for you.",
         unparseable_file.path,
    )]
    UnparseableLocalFile { unparseable_file: UnparseableFile },

    // All other problems are associated with specific supported versions of an
    // API.
    #[error(
        "This version is blessed, and it's a supported version, but it's \
         missing a local OpenAPI document.  This is unusual.  If you intended \
         to remove this version, you must also update the list of supported \
         versions in Rust.  If you didn't, restore the file from git: \
         {spec_file_name}"
    )]
    BlessedVersionMissingLocal { spec_file_name: ApiSpecFileName },

    #[error(
        "For this blessed version, found an extra OpenAPI document that does \
         not match the blessed (upstream) OpenAPI document: {spec_file_name}.  \
         This can happen if you created this version of the API in this branch, \
         then merged with an upstream commit that also added the same version \
         number.  In that case, you likely already bumped your local version \
         number (when you merged the list of supported versions in Rust) and \
         this file is vestigial. This tool can remove the unused file for you."
    )]
    BlessedVersionExtraLocalSpec { spec_file_name: ApiSpecFileName },

    #[error(
        "error comparing OpenAPI document generated from current code with \
         blessed document (from upstream): {}",
        InlineErrorChain::new(error.as_ref())
    )]
    BlessedVersionCompareError { error: anyhow::Error },

    #[error(
        "OpenAPI document generated from the current code is not compatible \
         with the blessed document (from upstream)"
    )]
    BlessedVersionBroken { compatibility_issues: Vec<ApiCompatIssue> },

    #[error(
        "For the latest blessed version, the OpenAPI document generated from \
         the current code is wire-compatible but not bytewise \
         identical to the blessed document. This implies one or more \
         trivial changes such as type renames or documentation updates. \
         To proceed, bump the API version in the `api_versions!` macro; \
         unless you're introducing other changes, there's no need to make \
         changes to any endpoints."
    )]
    BlessedLatestVersionBytewiseMismatch {
        blessed: &'a BlessedApiSpecFile,
        generated: &'a GeneratedApiSpecFile,
    },

    #[error(
        "No local OpenAPI document was found for this lockstep API.  This is \
         only expected if you're adding a new lockstep API.  This tool can \
         generate the file for you."
    )]
    LockstepMissingLocal { generated: &'a GeneratedApiSpecFile },

    #[error(
        "For this lockstep API, OpenAPI document generated from the current \
         code does not match the local file: {:?}.  This tool can update the \
         local file for you.", generated.spec_file_name().path()
    )]
    LockstepStale {
        found: &'a LocalApiSpecFile,
        generated: &'a GeneratedApiSpecFile,
    },

    #[error(
        "No OpenAPI document was found for this locally-added API version.  \
         This is normal if you have added or changed this API version.  \
         This tool can generate the file for you."
    )]
    LocalVersionMissingLocal { generated: &'a GeneratedApiSpecFile },

    #[error(
        "Extra (incorrect) OpenAPI documents were found for locally-added \
         version: {spec_file_names}.  This tool can remove the files for you."
    )]
    LocalVersionExtra { spec_file_names: DisplayableVec<ApiSpecFileName> },

    #[error(
        "For this locally-added version, the OpenAPI document generated \
         from the current code does not match the local file: {}. \
         This tool can update the local file(s) for you.",
        DisplayableVec(
            spec_files.iter().map(|s| s.spec_file_name().to_string()).collect()
        )
    )]
    // For versioned APIs, since the filename has its own hash in it, when the
    // local file is stale, it's not that the file contents will be wrong, but
    // rather that there will be one or more _incorrect_ files and the correct
    // one will be missing.  The fix will be to remove all the incorrect ones
    // and add the correct one.
    LocalVersionStale {
        spec_files: Vec<&'a LocalApiSpecFile>,
        generated: &'a GeneratedApiSpecFile,
    },

    #[error(
        "Generated OpenAPI document for API {api_ident:?} version {version} \
         is not valid"
    )]
    GeneratedValidationError {
        api_ident: ApiIdent,
        version: semver::Version,
        #[source]
        source: anyhow::Error,
    },

    #[error(
        "Additional validated file associated with API {api_ident:?} is \
         stale: {path}"
    )]
    ExtraFileStale {
        api_ident: ApiIdent,
        path: Utf8PathBuf,
        check_stale: CheckStale,
    },

    #[error("\"Latest\" symlink for versioned API {api_ident:?} is missing")]
    LatestLinkMissing { api_ident: ApiIdent, link: &'a ApiSpecFileName },

    #[error(
        "\"Latest\" symlink for versioned API {api_ident:?} is stale: points \
         to {}, but should be {}",
         found.basename(),
         link.basename(),
    )]
    LatestLinkStale {
        api_ident: ApiIdent,
        found: &'a ApiSpecFileName,
        link: &'a ApiSpecFileName,
    },

    #[error(
        "Blessed non-latest version is stored as a full JSON file. This can \
         be converted to a git ref. This tool can perform the conversion for \
         you."
    )]
    BlessedVersionShouldBeGitRef {
        local_file: &'a LocalApiSpecFile,
        git_ref: GitRef,
    },

    #[error(
        "Blessed version is stored as a git ref file, but should be stored as \
         JSON. This tool can perform the conversion for you."
    )]
    GitRefShouldBeJson {
        local_file: &'a LocalApiSpecFile,
        blessed: &'a BlessedApiSpecFile,
    },

    #[error(
        "Local file for this blessed version is corrupted (possibly due to \
         merge conflict markers). This tool can regenerate the file from the \
         blessed version for you."
    )]
    BlessedVersionCorruptedLocal {
        local_file: &'a LocalApiSpecFile,
        blessed: &'a BlessedApiSpecFile,
        /// If Some, regenerate as a git ref instead of JSON.
        git_ref: Option<GitRef>,
    },

    #[error(
        "Duplicate local file found: both JSON and git ref versions exist for \
         this API version. This tool can remove the redundant file for you."
    )]
    DuplicateLocalFile { local_file: &'a LocalApiSpecFile },

    #[error(
        "The first commit for this blessed version could not be determined. This \
         may indicate a corrupted git repository or other git-related issue. Git \
         ref storage requires complete git history access"
         // Note: omitting a trailing period after "access" because we show ":
         // <source>".
    )]
    GitRefFirstCommitUnknown {
        spec_file_name: ApiSpecFileName,
        #[source]
        source: anyhow::Error,
    },
}

impl<'a> Problem<'a> {
    pub fn is_fixable(&self) -> bool {
        self.fix().is_some()
    }

    pub fn fix(&'a self) -> Option<Fix<'a>> {
        match self {
            Problem::LocalSpecFileOrphaned { spec_file_name } => {
                Some(Fix::DeleteFiles {
                    files: DisplayableVec(vec![spec_file_name.clone()]),
                })
            }
            Problem::BlessedVersionMissingLocal { .. } => None,
            Problem::BlessedVersionExtraLocalSpec { spec_file_name } => {
                Some(Fix::DeleteFiles {
                    files: DisplayableVec(vec![spec_file_name.clone()]),
                })
            }
            Problem::BlessedVersionCompareError { .. } => None,
            Problem::BlessedVersionBroken { .. } => None,
            Problem::BlessedLatestVersionBytewiseMismatch { .. } => None,
            Problem::LockstepMissingLocal { generated }
            | Problem::LockstepStale { generated, .. } => {
                Some(Fix::UpdateLockstepFile { generated })
            }
            Problem::LocalVersionMissingLocal { generated } => {
                Some(Fix::UpdateVersionedFiles {
                    old: DisplayableVec(Vec::new()),
                    generated,
                })
            }
            Problem::LocalVersionExtra { spec_file_names } => {
                Some(Fix::DeleteFiles { files: spec_file_names.clone() })
            }
            Problem::LocalVersionStale { spec_files, generated } => {
                Some(Fix::UpdateVersionedFiles {
                    old: DisplayableVec(
                        spec_files.iter().map(|s| s.spec_file_name()).collect(),
                    ),
                    generated,
                })
            }
            Problem::GeneratedValidationError { .. } => None,
            Problem::ExtraFileStale { path, check_stale, .. } => {
                Some(Fix::UpdateExtraFile { path, check_stale })
            }
            Problem::LatestLinkStale { api_ident, link, .. }
            | Problem::LatestLinkMissing { api_ident, link } => {
                Some(Fix::UpdateSymlink { api_ident, link })
            }
            Problem::BlessedVersionShouldBeGitRef { local_file, git_ref } => {
                Some(Fix::ConvertToGitRef { local_file, git_ref })
            }
            Problem::GitRefShouldBeJson { local_file, blessed } => {
                Some(Fix::ConvertToJson { local_file, blessed })
            }
            Problem::BlessedVersionCorruptedLocal {
                local_file,
                blessed,
                git_ref,
            } => Some(Fix::RegenerateFromBlessed {
                local_file,
                blessed,
                git_ref: git_ref.as_ref(),
            }),
            Problem::DuplicateLocalFile { local_file } => {
                Some(Fix::DeleteFiles {
                    files: DisplayableVec(vec![
                        local_file.spec_file_name().clone(),
                    ]),
                })
            }
            Problem::GitRefFirstCommitUnknown { .. } => None,
            Problem::UnparseableLocalFile { unparseable_file } => {
                Some(Fix::DeleteUnparseableFile {
                    path: unparseable_file.path.clone(),
                })
            }
        }
    }
}

pub enum Fix<'a> {
    DeleteFiles {
        files: DisplayableVec<ApiSpecFileName>,
    },
    UpdateLockstepFile {
        generated: &'a GeneratedApiSpecFile,
    },
    UpdateVersionedFiles {
        old: DisplayableVec<&'a ApiSpecFileName>,
        generated: &'a GeneratedApiSpecFile,
    },
    UpdateExtraFile {
        path: &'a Utf8Path,
        check_stale: &'a CheckStale,
    },
    UpdateSymlink {
        api_ident: &'a ApiIdent,
        link: &'a ApiSpecFileName,
    },
    /// Convert a full JSON file to a git ref file.
    ConvertToGitRef {
        local_file: &'a LocalApiSpecFile,
        git_ref: &'a GitRef,
    },
    /// Convert a git ref file back to a full JSON file.
    ConvertToJson {
        local_file: &'a LocalApiSpecFile,
        blessed: &'a BlessedApiSpecFile,
    },
    /// Regenerate a corrupted local file from the blessed content.
    RegenerateFromBlessed {
        local_file: &'a LocalApiSpecFile,
        blessed: &'a BlessedApiSpecFile,
        /// If Some, regenerate as a git ref instead of JSON.
        git_ref: Option<&'a GitRef>,
    },
    /// Delete an unparseable file (e.g., one with merge conflict markers).
    DeleteUnparseableFile {
        path: Utf8PathBuf,
    },
}

impl Display for Fix<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fix::DeleteFiles { files } => {
                writeln!(
                    f,
                    "delete {}: {files}",
                    plural::files(files.0.len())
                )?;
            }
            Fix::UpdateLockstepFile { generated } => {
                writeln!(
                    f,
                    "rewrite lockstep file {} from generated",
                    generated.spec_file_name().path()
                )?;
            }
            Fix::UpdateVersionedFiles { old, generated } => {
                if !old.0.is_empty() {
                    writeln!(
                        f,
                        "remove old {}: {old}",
                        plural::files(old.0.len())
                    )?;
                }
                writeln!(
                    f,
                    "write new file {} from generated",
                    generated.spec_file_name().path()
                )?;
            }
            Fix::UpdateExtraFile { path, check_stale } => {
                let label = match check_stale {
                    CheckStale::Modified { .. } => "rewrite",
                    CheckStale::New { .. } => "write new",
                };
                writeln!(f, "{label} file {path} from generated")?;
            }
            Fix::UpdateSymlink { link, .. } => {
                writeln!(
                    f,
                    "update symlink to point to {}",
                    link.to_json_filename().basename()
                )?;
            }
            Fix::ConvertToGitRef { local_file, .. } => {
                writeln!(
                    f,
                    "convert {} to git ref",
                    local_file.spec_file_name().path()
                )?;
            }
            Fix::ConvertToJson { local_file, .. } => {
                writeln!(
                    f,
                    "convert {} from git ref to JSON",
                    local_file.spec_file_name().path()
                )?;
            }
            Fix::RegenerateFromBlessed { local_file, git_ref, .. } => {
                if git_ref.is_some() {
                    writeln!(
                        f,
                        "regenerate {} from blessed content as git ref",
                        local_file.spec_file_name().path()
                    )?;
                } else {
                    writeln!(
                        f,
                        "regenerate {} from blessed content",
                        local_file.spec_file_name().path()
                    )?;
                }
            }
            Fix::DeleteUnparseableFile { path } => {
                writeln!(f, "delete unparseable file {path}")?;
            }
        };
        Ok(())
    }
}

impl Fix<'_> {
    /// Adds the paths (relative to the OpenAPI documents directory) that this
    /// fix will write to. Used to determine if an unparseable file will be
    /// overwritten.
    pub fn add_paths_written(&self, paths: &mut HashSet<Utf8PathBuf>) {
        match self {
            Fix::DeleteFiles { .. } => {}
            Fix::UpdateLockstepFile { generated } => {
                paths.insert(generated.spec_file_name().path().to_owned());
            }
            Fix::UpdateVersionedFiles { generated, .. } => {
                paths.insert(generated.spec_file_name().path().to_owned());
            }
            Fix::UpdateExtraFile { path, .. } => {
                paths.insert((*path).to_owned());
            }
            Fix::UpdateSymlink { .. } => {}
            Fix::ConvertToGitRef { local_file, .. } => {
                // Writes to the .gitref path, not the JSON path.
                let json_path = local_file.spec_file_name().path();
                paths
                    .insert(Utf8PathBuf::from(format!("{}.gitref", json_path)));
            }
            Fix::ConvertToJson { local_file, .. } => {
                // Writes to the JSON path (removing .gitref suffix).
                let git_ref_path = local_file.spec_file_name().path();
                if let Some(json_path) =
                    git_ref_path.as_str().strip_suffix(".gitref")
                {
                    paths.insert(Utf8PathBuf::from(json_path));
                }
            }
            Fix::RegenerateFromBlessed { local_file, git_ref, .. } => {
                if git_ref.is_some() {
                    // When regenerating as git ref, writes to a .gitref file.
                    let json_path = local_file.spec_file_name().path();
                    paths.insert(Utf8PathBuf::from(format!(
                        "{}.gitref",
                        json_path
                    )));
                } else {
                    // Overwrites the corrupted local file.
                    paths.insert(local_file.spec_file_name().path().to_owned());
                }
            }
            Fix::DeleteUnparseableFile { .. } => {}
        }
        // No wildcard match: adding a new Fix variant should cause a compile
        // error here, forcing consideration of what paths it writes.
    }

    pub fn execute(&self, env: &ResolvedEnv) -> anyhow::Result<Vec<String>> {
        let root = env.openapi_abs_dir();
        match self {
            Fix::DeleteFiles { files } => {
                let mut rv = Vec::new();
                for f in &files.0 {
                    let path = root.join(f.path());
                    fs_err::remove_file(&path)?;
                    rv.push(format!("removed {}", path));
                }
                Ok(rv)
            }
            Fix::UpdateLockstepFile { generated } => {
                let path = root.join(generated.spec_file_name().path());
                Ok(vec![format!(
                    "updated {}: {:?}",
                    &path,
                    overwrite_file(&path, generated.contents())?
                )])
            }
            Fix::UpdateVersionedFiles { old, generated } => {
                let mut rv = Vec::new();
                for f in &old.0 {
                    let path = root.join(f.path());
                    fs_err::remove_file(&path)?;
                    rv.push(format!("removed {}", path));
                }

                let path = root.join(generated.spec_file_name().path());
                rv.push(format!(
                    "created {}: {:?}",
                    &path,
                    overwrite_file(&path, generated.contents())?
                ));
                Ok(rv)
            }
            Fix::UpdateExtraFile { path, check_stale } => {
                let expected_contents = match check_stale {
                    CheckStale::Modified { expected, .. } => expected,
                    CheckStale::New { expected } => expected,
                };
                // Extra file paths are relative to the repo root, not the
                // documents directory.
                let full_path = env.repo_root.join(path);
                Ok(vec![format!(
                    "wrote {}: {:?}",
                    &path,
                    overwrite_file(&full_path, expected_contents)?
                )])
            }
            Fix::UpdateSymlink { api_ident, link } => {
                let path = root
                    .join(api_ident.to_string())
                    .join(api_ident.versioned_api_latest_symlink());
                // We want the link to contain a relative path to a file in the
                // same directory so that it's correct no matter where it's
                // resolved from. If the link target is a gitref, convert it to
                // the JSON filename (the symlink should always point to JSON).
                let target = link.to_json_filename().basename();
                match fs_err::remove_file(&path) {
                    Ok(_) => (),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => {
                        return Err(anyhow!(err).context("removing old link"));
                    }
                };
                symlink_file(&target, &path)?;
                Ok(vec![format!("wrote link {} -> {}", path, target)])
            }
            Fix::ConvertToGitRef { local_file, git_ref } => {
                let json_path = root.join(local_file.spec_file_name().path());

                let git_ref_basename = format!(
                    "{}.gitref",
                    local_file.spec_file_name().basename()
                );
                let git_ref_path = json_path
                    .parent()
                    .ok_or_else(|| anyhow!("cannot get parent directory"))?
                    .join(&git_ref_basename);

                // Write the git ref file. Add a trailing newline so diffs don't
                // have the "\ No newline at end of file" message. Otherwise,
                // the extra newline has no impact on usability or correctness.
                let overwrite_status = overwrite_file(
                    &git_ref_path,
                    format!("{}\n", git_ref).as_bytes(),
                )?;

                // Remove the original JSON file.
                fs_err::remove_file(&json_path)?;

                Ok(vec![
                    format!("converted {} to git ref", json_path),
                    format!("created {}: {:?}", git_ref_path, overwrite_status),
                ])
            }
            Fix::ConvertToJson { local_file, blessed } => {
                let git_ref_path =
                    root.join(local_file.spec_file_name().path());

                // Use the blessed file's contents since it's guaranteed to be
                // valid.
                let contents = blessed.contents();

                // Compute the JSON file path by removing the .gitref suffix.
                let git_ref_basename = local_file.spec_file_name().basename();
                let json_basename = git_ref_basename
                    .strip_suffix(".gitref")
                    .ok_or_else(|| {
                        anyhow!(
                            "expected git ref file to end with .gitref: {}",
                            git_ref_basename
                        )
                    })?;

                let json_path = git_ref_path
                    .parent()
                    .ok_or_else(|| anyhow!("cannot get parent directory"))?
                    .join(json_basename);

                let overwrite_status = overwrite_file(&json_path, contents)?;

                fs_err::remove_file(&git_ref_path)?;

                Ok(vec![
                    format!("converted {} from git ref to JSON", git_ref_path),
                    format!("created {}: {:?}", json_path, overwrite_status),
                ])
            }
            Fix::RegenerateFromBlessed { local_file, blessed, git_ref } => {
                let local_path = root.join(local_file.spec_file_name().path());

                // Remove the corrupted file.
                match fs_err::remove_file(&local_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }

                if let Some(git_ref) = git_ref {
                    // Write as a git ref file.
                    let git_ref_basename = format!(
                        "{}.gitref",
                        local_file.spec_file_name().basename()
                    );
                    let git_ref_path = local_path
                        .parent()
                        .ok_or_else(|| anyhow!("cannot get parent directory"))?
                        .join(&git_ref_basename);

                    // Add a trailing newline for clean diffs.
                    let overwrite_status = overwrite_file(
                        &git_ref_path,
                        format!("{}\n", git_ref).as_bytes(),
                    )?;

                    Ok(vec![
                        format!("removed corrupted file {}", local_path),
                        format!(
                            "created git ref {}: {:?}",
                            git_ref_path, overwrite_status
                        ),
                    ])
                } else {
                    // Write the JSON content directly.
                    let overwrite_status =
                        overwrite_file(&local_path, blessed.contents())?;
                    Ok(vec![format!(
                        "regenerated {} from blessed content: {:?}",
                        local_path, overwrite_status
                    )])
                }
            }
            Fix::DeleteUnparseableFile { path } => {
                let full_path = root.join(path);
                fs_err::remove_file(&full_path)?;
                Ok(vec![format!("removed unparseable file {}", full_path)])
            }
        }
    }
}

#[cfg(unix)]
fn symlink_file(target: &str, path: &Utf8Path) -> std::io::Result<()> {
    fs_err::os::unix::fs::symlink(target, path)
}

#[cfg(windows)]
fn symlink_file(target: &str, path: &Utf8Path) -> std::io::Result<()> {
    fs_err::os::windows::fs::symlink_file(target, path)
}

/// Resolve differences between blessed spec(s), the generated spec, and any
/// local spec files for a given API
pub struct Resolved<'a> {
    notes: Vec<Note>,
    non_version_problems: Vec<Problem<'a>>,
    api_results: BTreeMap<ApiIdent, ApiResolved<'a>>,
    nexpected_documents: usize,
}

impl<'a> Resolved<'a> {
    pub fn new(
        env: &'a ResolvedEnv,
        apis: &'a ManagedApis,
        blessed: &'a BlessedFiles,
        generated: &'a GeneratedFiles,
        local: &'a LocalFiles,
    ) -> Resolved<'a> {
        // First, assemble a list of supported versions for each API, as defined
        // in the Rust list of supported versions.  We'll use this to identify
        // any blessed spec files or local spec files that don't belong at all.
        let supported_versions_by_api: BTreeMap<
            &ApiIdent,
            BTreeSet<&semver::Version>,
        > = apis
            .iter_apis()
            .map(|api| {
                (
                    api.ident(),
                    api.iter_versions_semver().collect::<BTreeSet<_>>(),
                )
            })
            .collect();

        let nexpected_documents =
            supported_versions_by_api.values().map(|v| v.len()).sum::<usize>();

        // Get one easy case out of the way: if there are any blessed API
        // versions that aren't supported any more, note that.
        let notes: Vec<Note> = resolve_removed_blessed_versions(
            &supported_versions_by_api,
            blessed,
        )
        .map(|(ident, version)| Note::BlessedVersionRemoved {
            api_ident: ident.clone(),
            version: version.clone(),
        })
        .collect();

        // Get the other easy case out of the way: if there are any local spec
        // files for APIs or API versions that aren't supported any more, that's
        // a (fixable) problem.
        let mut non_version_problems: Vec<Problem<'_>> =
            resolve_orphaned_local_specs(&supported_versions_by_api, local)
                .map(|spec_file_name| Problem::LocalSpecFileOrphaned {
                    spec_file_name: spec_file_name.clone(),
                })
                .collect();

        // Resolve each of the supported API versions first, so we know what
        // paths will be written.
        let api_results: BTreeMap<ApiIdent, ApiResolved<'_>> = apis
            .iter_apis()
            .map(|api| {
                let ident = api.ident().clone();
                let api_blessed = blessed.get(&ident);
                // We should have generated an API for every supported version.
                let api_generated = generated.get(&ident).unwrap();
                let api_local = local.get(&ident);
                (
                    api.ident().clone(),
                    resolve_api(
                        env,
                        api,
                        apis.validation(),
                        apis.uses_git_ref_storage(api),
                        blessed,
                        api_blessed,
                        api_generated,
                        api_local,
                    ),
                )
            })
            .collect();

        // Now collect any unparseable files. These are local files that exist
        // but couldn't be parsed (e.g., due to merge conflict markers).
        //
        // Only report unparseable files whose paths won't be overwritten by a
        // fix. We check the actual fixes (not just generated paths) because
        // some fixes write git refs instead of JSON files.
        let mut paths_written: HashSet<Utf8PathBuf> = HashSet::new();
        for api_resolved in api_results.values() {
            for resolution in api_resolved.by_version.values() {
                for problem in &resolution.problems {
                    if let Some(fix) = problem.fix() {
                        fix.add_paths_written(&mut paths_written);
                    }
                }
            }
        }

        for (_ident, api_files) in local.iter() {
            for unparseable in api_files.unparseable_files() {
                // Only report if no fix will overwrite this path.
                if !paths_written.contains(&unparseable.path) {
                    non_version_problems.push(Problem::UnparseableLocalFile {
                        unparseable_file: unparseable.clone(),
                    });
                }
            }
        }

        Resolved {
            notes,
            non_version_problems,
            api_results,
            nexpected_documents,
        }
    }

    pub fn nexpected_documents(&self) -> usize {
        self.nexpected_documents
    }

    pub fn notes(&self) -> impl Iterator<Item = &Note> + '_ {
        self.notes.iter()
    }

    pub fn general_problems(&self) -> impl Iterator<Item = &Problem<'a>> + '_ {
        self.non_version_problems.iter()
    }

    pub fn resolution_for_api_version(
        &self,
        ident: &ApiIdent,
        version: &semver::Version,
    ) -> Option<&Resolution<'_>> {
        self.api_results.get(ident).and_then(|v| v.by_version.get(version))
    }

    pub fn symlink_problem(&self, ident: &ApiIdent) -> Option<&Problem<'_>> {
        self.api_results.get(ident).and_then(|v| v.symlink.as_ref())
    }

    pub fn has_unfixable_problems(&self) -> bool {
        self.general_problems().any(|p| !p.is_fixable())
            || self.api_results.values().any(|a| a.has_unfixable_problems())
    }
}

struct ApiResolved<'a> {
    by_version: BTreeMap<semver::Version, Resolution<'a>>,
    symlink: Option<Problem<'a>>,
}

impl ApiResolved<'_> {
    fn has_unfixable_problems(&self) -> bool {
        self.symlink.as_ref().is_some_and(|f| !f.is_fixable())
            || self.by_version.values().any(|r| r.has_errors())
    }
}

fn resolve_removed_blessed_versions<'a>(
    supported_versions_by_api: &'a BTreeMap<
        &'a ApiIdent,
        BTreeSet<&'a semver::Version>,
    >,
    blessed: &'a BlessedFiles,
) -> impl Iterator<Item = (&'a ApiIdent, &'a semver::Version)> + 'a {
    blessed.iter().flat_map(|(ident, api_files)| {
        let set = supported_versions_by_api.get(ident);
        api_files.versions().keys().filter_map(move |version| match set {
            Some(set) if set.contains(version) => None,
            _ => Some((ident, version)),
        })
    })
}

fn resolve_orphaned_local_specs<'a>(
    supported_versions_by_api: &'a BTreeMap<
        &'a ApiIdent,
        BTreeSet<&'a semver::Version>,
    >,
    local: &'a LocalFiles,
) -> impl Iterator<Item = &'a ApiSpecFileName> + 'a {
    local.iter().flat_map(|(ident, api_files)| {
        let set = supported_versions_by_api.get(ident);
        api_files
            .versions()
            .iter()
            .filter_map(move |(version, files)| match set {
                Some(set) if !set.contains(version) => {
                    Some(files.iter().map(|f| f.spec_file_name()))
                }
                _ => None,
            })
            .flatten()
    })
}

#[expect(clippy::too_many_arguments)]
fn resolve_api<'a>(
    env: &'a ResolvedEnv,
    api: &'a ManagedApi,
    validation: Option<&DynValidationFn>,
    use_git_ref_storage: bool,
    all_blessed: &'a BlessedFiles,
    api_blessed: Option<&'a ApiFiles<BlessedApiSpecFile>>,
    api_generated: &'a ApiFiles<GeneratedApiSpecFile>,
    api_local: Option<&'a ApiFiles<Vec<LocalApiSpecFile>>>,
) -> ApiResolved<'a> {
    let (by_version, symlink) = if api.is_lockstep() {
        (
            resolve_api_lockstep(
                env,
                api,
                validation,
                api_generated,
                api_local,
            ),
            None,
        )
    } else {
        let latest_version = api
            .iter_versions_semver()
            .next_back()
            .expect("versioned API has at least one version");

        // Compute the first commit for the latest version, capturing any errors.
        let (latest_first_commit, latest_first_commit_error) = {
            let latest_is_blessed = api_blessed
                .is_some_and(|b| b.versions().contains_key(latest_version));

            if !latest_is_blessed {
                (LatestFirstCommit::NotBlessed, None)
            } else {
                // The latest version is blessed. Try to find its first commit.
                match all_blessed.git_ref(api.ident(), latest_version) {
                    Some(gr) => match gr.to_git_ref(&env.repo_root) {
                        Ok(git_ref) => {
                            (LatestFirstCommit::Blessed(git_ref.commit), None)
                        }
                        Err(error) => {
                            // Capture the error to report it for the latest
                            // version.
                            let blessed_file = api_blessed
                                .and_then(|b| b.versions().get(latest_version));
                            let spec_file_name = blessed_file
                                .map(|f| f.spec_file_name().clone());
                            (
                                LatestFirstCommit::BlessedError,
                                Some((spec_file_name, error)),
                            )
                        }
                    },
                    None => (LatestFirstCommit::BlessedError, None),
                }
            }
        };

        let mut by_version: BTreeMap<_, _> = api
            .iter_versions_semver()
            // Reverse the order of versions: they are stored in sorted order,
            // so the last version (first one from the back) is the latest.
            .rev()
            .enumerate()
            .map(|(index, version)| {
                let is_latest = index == 0;
                let version = version.clone();
                let blessed =
                    api_blessed.and_then(|b| b.versions().get(&version));
                let is_blessed = Some(blessed.is_some());
                let generated = api_generated.versions().get(&version).unwrap();
                let local = api_local
                    .and_then(|b| b.versions().get(&version))
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);

                // Look up the git ref for this version.
                let git_ref = all_blessed.git_ref(api.ident(), &version);

                let resolution = resolve_api_version(
                    env,
                    api,
                    validation,
                    use_git_ref_storage,
                    ApiVersion { version: &version, is_latest, is_blessed },
                    blessed,
                    git_ref,
                    generated,
                    local,
                    latest_first_commit,
                );

                (version, resolution)
            })
            .collect();

        // If there was an error computing the first commit for the latest
        // version, add the error to the latest version's resolution.
        if let Some((spec_file_name, error)) = latest_first_commit_error {
            if let Some(resolution) = by_version.get_mut(latest_version) {
                if let Some(spec_file_name) = spec_file_name {
                    resolution.add_problem(Problem::GitRefFirstCommitUnknown {
                        spec_file_name,
                        source: error,
                    });
                }
            }
        }

        // Check the "latest" symlink.
        let latest_generated = api_generated.latest_link().expect(
            "\"generated\" source should always have a \"latest\" link",
        );
        let generated_version =
            latest_generated.version().expect("versioned APIs have a version");
        let resolution =
            by_version.get(generated_version).unwrap_or_else(|| {
                panic!(
                    "by_version map should have a version \
                     corresponding to latest_generated ({})",
                    latest_generated
                )
            });

        let latest_local = api_local.and_then(|l| l.latest_link());
        let symlink = match latest_local {
            Some(latest_local) => {
                if latest_local == latest_generated {
                    None
                } else {
                    // latest_local is different from latest_generated.
                    //
                    // We never want to update the local copies of blessed
                    // documents. But latest_generated might have
                    // wire-compatible (trivial) changes which would cause the
                    // hash to change, so we need to handle this case with care.
                    //
                    // The possibilities are:
                    //
                    // 1. latest_local is blessed, latest_generated has the same
                    //    version as latest_local, and it has wire-compatible
                    //    changes. In that case, don't update the symlink.
                    //
                    // 2. latest_local is blessed, latest_generated has the same
                    //    version as latest_local, and latest_generated has
                    //    wire-*incompatible* changes. In that case, we'd have
                    //    returned errors in the by_version map above, and we
                    //    wouldn't want to update the symlink in any case.
                    //
                    // 3. latest_local is blessed, and latest_generated is
                    //    blessed but a *different* version. This means that
                    //    the latest version was retired. In this case,
                    //    we want to update the symlink to the blessed hash
                    //    corresponding to the latest generated version.
                    //
                    // 4. latest_local is not blessed. In that case, we do
                    //    want to update the symlink.
                    let local_version = latest_local
                        .version()
                        .expect("versioned APIs have a version");
                    match resolution.kind() {
                        ResolutionKind::Lockstep => {
                            unreachable!("this is a versioned API");
                        }
                        // Case 1 and 2 above.
                        ResolutionKind::Blessed
                            if generated_version == local_version =>
                        {
                            // latest_generated is blessed and the same
                            // version as latest_local, so don't update the
                            // symlink.
                            None
                        }
                        ResolutionKind::Blessed => {
                            // latest_generated is blessed, and has a
                            // different version from latest_local. In this
                            // case, we want to update the symlink to the
                            // blessed version matching latest_generated
                            // (not latest_generated, in case it's different
                            // from the blessed version in a wire-compatible
                            // way!)
                            let api_blessed =
                                api_blessed.unwrap_or_else(|| {
                                    panic!(
                                        "for {}, Blessed means \
                                         api_blessed exists",
                                        api.ident()
                                    )
                                });
                            let blessed = api_blessed
                                .versions()
                                .get(generated_version)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "for {} v{}, Blessed means \
                                         generated_version exists",
                                        api.ident(),
                                        generated_version
                                    );
                                });
                            Some(Problem::LatestLinkStale {
                                api_ident: api.ident().clone(),
                                link: blessed.spec_file_name(),
                                found: latest_local,
                            })
                        }
                        ResolutionKind::NewLocally => {
                            // latest_generated is not blessed, so update
                            // the symlink.
                            Some(Problem::LatestLinkStale {
                                api_ident: api.ident().clone(),
                                link: latest_generated,
                                found: latest_local,
                            })
                        }
                    }
                }
            }
            None => {
                // As in case 3 above, if the resolution is blessed, we want to
                // update the symlink to the *blessed() hash corresponding to
                // the latest generated version.
                match resolution.kind() {
                    ResolutionKind::Lockstep => {
                        unreachable!("this is a versioned API");
                    }
                    ResolutionKind::Blessed => {
                        let api_blessed = api_blessed.unwrap_or_else(|| {
                            panic!(
                                "for {}, Blessed means api_blessed exists",
                                api.ident()
                            )
                        });
                        let blessed = api_blessed
                            .versions()
                            .get(generated_version)
                            .unwrap_or_else(|| {
                                panic!(
                                    "for {} v{}, Blessed means \
                                     generated_version exists",
                                    api.ident(),
                                    generated_version
                                );
                            });
                        Some(Problem::LatestLinkMissing {
                            api_ident: api.ident().clone(),
                            link: blessed.spec_file_name(),
                        })
                    }
                    ResolutionKind::NewLocally => {
                        // latest_generated is not blessed, so update
                        // the symlink to the generated version.
                        Some(Problem::LatestLinkMissing {
                            api_ident: api.ident().clone(),
                            link: latest_generated,
                        })
                    }
                }
            }
        };

        (by_version, symlink)
    };

    ApiResolved { by_version, symlink }
}

fn resolve_api_lockstep<'a>(
    env: &'a ResolvedEnv,
    api: &'a ManagedApi,
    validation: Option<&DynValidationFn>,
    api_generated: &'a ApiFiles<GeneratedApiSpecFile>,
    api_local: Option<&'a ApiFiles<Vec<LocalApiSpecFile>>>,
) -> BTreeMap<semver::Version, Resolution<'a>> {
    assert!(api.is_lockstep());

    // unwrap(): Lockstep APIs by construction always have exactly one version.
    let version = iter_only(api.iter_versions_semver())
        .with_context(|| {
            format!("list of versions for lockstep API {}", api.ident())
        })
        .unwrap();

    let generated = api_generated
        .versions()
        .get(version)
        .expect("generated OpenAPI document for lockstep API");

    // We may or may not have found a local OpenAPI document for this API.
    let local = api_local
        .and_then(|by_version| by_version.versions().get(version))
        .and_then(|list| match &list.as_slice() {
            &[first] => Some(first),
            &[] => None,
            items => {
                // Structurally, it's not possible to have more than one
                // local file for a lockstep API because the file is named
                // by the API itself.
                unreachable!(
                    "unexpectedly found more than one local OpenAPI \
                     document for lockstep API {}: {:?}",
                    api.ident(),
                    items
                );
            }
        });

    let mut problems = Vec::new();

    // Validate the generated API document.
    validate_generated(
        env,
        api,
        validation,
        ApiVersion {
            version,
            is_latest: true, // is_latest is always true for lockstep APIs
            is_blessed: None,
        },
        generated,
        &mut problems,
    );

    match local {
        Some(local_file) if local_file.contents() == generated.contents() => (),
        Some(found) => {
            problems.push(Problem::LockstepStale { found, generated })
        }
        None => problems.push(Problem::LockstepMissingLocal { generated }),
    };

    BTreeMap::from([(version.clone(), Resolution::new_lockstep(problems))])
}

struct ApiVersion<'a> {
    version: &'a semver::Version,
    is_latest: bool,
    is_blessed: Option<bool>,
}

#[expect(clippy::too_many_arguments)]
fn resolve_api_version<'a>(
    env: &'_ ResolvedEnv,
    api: &'_ ManagedApi,
    validation: Option<&DynValidationFn>,
    use_git_ref_storage: bool,
    version: ApiVersion<'_>,
    blessed: Option<&'a BlessedApiSpecFile>,
    git_ref: Option<&'a BlessedGitRef>,
    generated: &'a GeneratedApiSpecFile,
    local: &'a [LocalApiSpecFile],
    latest_first_commit: LatestFirstCommit,
) -> Resolution<'a> {
    match blessed {
        Some(blessed) => resolve_api_version_blessed(
            env,
            api,
            validation,
            use_git_ref_storage,
            version,
            blessed,
            git_ref,
            generated,
            local,
            latest_first_commit,
        ),
        None => resolve_api_version_local(
            env, api, validation, version, generated, local,
        ),
    }
}

#[expect(clippy::too_many_arguments)]
fn resolve_api_version_blessed<'a>(
    env: &'_ ResolvedEnv,
    api: &'_ ManagedApi,
    validation: Option<&DynValidationFn>,
    use_git_ref_storage: bool,
    version: ApiVersion<'_>,
    blessed: &'a BlessedApiSpecFile,
    git_ref: Option<&'a BlessedGitRef>,
    generated: &'a GeneratedApiSpecFile,
    local: &'a [LocalApiSpecFile],
    latest_first_commit: LatestFirstCommit,
) -> Resolution<'a> {
    let mut problems = Vec::new();
    let is_latest = version.is_latest;

    // Validate the generated API document.
    //
    // Blessed versions are immutable, so why do we call validation on them in a
    // way that can fail? The reason is that validation may also want to
    // generate extra files, particularly for the latest version. Whether or not
    // the API version is blessed, the user might still want to generate extra
    // files for that version. So we validate unconditionally, but let the user
    // know via `is_blessed`, letting them skip validation where appropriate.
    validate_generated(env, api, validation, version, generated, &mut problems);

    // First off, the blessed spec must be a subset of the generated one.
    // If not, someone has made an incompatible change to the API
    // *implementation*, such that the implementation no longer faithfully
    // implements this older, supported version.
    match api_compatible(blessed.value(), generated.value()) {
        Ok(issues) => {
            if !issues.is_empty() {
                problems.push(Problem::BlessedVersionBroken {
                    compatibility_issues: issues,
                });
            }
        }
        Err(error) => {
            problems.push(Problem::BlessedVersionCompareError { error })
        }
    };

    // For the latest version, also require bytewise equality. This ensures that
    // trivial changes don't accumulate invisibly. If the generated spec is
    // semantically equivalent but bytewise different, require a version bump.
    //
    // This check can be disabled via `allow_trivial_changes_for_latest()`.
    if is_latest
        && !api.allows_trivial_changes_for_latest()
        && problems.is_empty()
        && generated.contents() != blessed.contents()
    {
        problems.push(Problem::BlessedLatestVersionBytewiseMismatch {
            blessed,
            generated,
        });
    }

    // Now, there should be at least one local spec that exactly matches the
    // blessed one.
    //
    // We partition local files into three categories:
    // 1. Valid files with matching hash/contents -> matching
    // 2. Unparseable files with matching hash -> corrupted (need regeneration)
    // 3. Everything else -> non-matching
    let blessed_hash = blessed
        .spec_file_name()
        .hash()
        .expect("this should be a versioned file so it should have a hash");

    let mut matching = Vec::new();
    let mut corrupted = Vec::new();
    let mut non_matching = Vec::new();

    for local_file in local {
        let local_hash = local_file
            .spec_file_name()
            .hash()
            .expect("this should be a versioned file so it should have a hash");
        let hashes_match = local_hash == blessed_hash;

        if local_file.is_unparseable() {
            // Unparseable files can't have their contents compared, so we rely
            // solely on the hash. If the hash matches, the file is corrupted
            // and needs regeneration.
            if hashes_match {
                corrupted.push(local_file);
            } else {
                non_matching.push(local_file);
            }
        } else {
            // For valid files, verify that hash matching implies content
            // matching (and vice versa).
            let contents_match = local_file.contents() == blessed.contents();
            assert_eq!(
                hashes_match, contents_match,
                "hash and contents should match for valid files"
            );

            if hashes_match {
                matching.push(local_file);
            } else {
                non_matching.push(local_file);
            }
        }
    }

    // Local function to compute the storage format for this version. This is
    // expensive because it may need to resolve a git revision to a commit
    // hash.
    let compute_storage_format =
        |problems: &mut Vec<Problem<'a>>| -> VersionStorageFormat {
            match git_ref {
                Some(r) => match r.to_git_ref(&env.repo_root) {
                    Ok(current) => {
                        storage_format_for_blessed(latest_first_commit, current)
                    }
                    Err(error) => {
                        problems.push(Problem::GitRefFirstCommitUnknown {
                            spec_file_name: blessed.spec_file_name().clone(),
                            source: error,
                        });
                        VersionStorageFormat::Error
                    }
                },
                None => VersionStorageFormat::Json,
            }
        };

    if matching.is_empty() && corrupted.is_empty() {
        // No valid or corrupted local files match the blessed version.
        problems.push(Problem::BlessedVersionMissingLocal {
            spec_file_name: blessed.spec_file_name().clone(),
        });
    } else if !use_git_ref_storage || is_latest {
        // Fast path: git ref storage disabled or this is the latest version.
        // We know we always want JSON in this case, so we can avoid computing
        // git refs here.

        // Report corrupted local files that need regeneration from blessed.
        for local_file in &corrupted {
            problems.push(Problem::BlessedVersionCorruptedLocal {
                local_file,
                blessed,
                git_ref: None,
            });
        }

        if matching.is_empty() {
            // Only corrupted files match - they'll be regenerated. Still need
            // to mark non-matching files as extra.
        } else if matching.len() > 1 {
            // We might have both api.json and api.json.gitref for the same
            // version. Mark the redundant file (always the gitref file in this
            // case) for deletion.
            for local_file in matching {
                if local_file.spec_file_name().is_git_ref() {
                    problems.push(Problem::DuplicateLocalFile { local_file });
                }
            }
        } else {
            let local_file = matching[0];
            if local_file.spec_file_name().is_git_ref() {
                problems
                    .push(Problem::GitRefShouldBeJson { local_file, blessed });
            }
        }

        problems.extend(non_matching.into_iter().map(|s| {
            Problem::BlessedVersionExtraLocalSpec {
                spec_file_name: s.spec_file_name().clone(),
            }
        }));
    } else {
        // Slow path: git ref storage enabled and not latest. Compute what
        // storage format this version should use.
        let storage_format = compute_storage_format(&mut problems);

        // Report corrupted local files that need regeneration from blessed
        // versions.
        for local_file in &corrupted {
            let git_ref = match &storage_format {
                VersionStorageFormat::GitRef(g) => Some(g.clone()),
                VersionStorageFormat::Json | VersionStorageFormat::Error => {
                    None
                }
            };
            problems.push(Problem::BlessedVersionCorruptedLocal {
                local_file,
                blessed,
                git_ref,
            });
        }

        if matching.is_empty() {
            // Only corrupted files match - they'll be regenerated. Still need
            // to mark non-matching files as extra.
        } else if matching.len() > 1 {
            // We might have both api.json and api.json.gitref for the same
            // version. Mark the redundant file for deletion.
            for local_file in matching {
                let redundant = match (
                    &storage_format,
                    local_file.spec_file_name().is_git_ref(),
                ) {
                    // Should be git ref but have JSON, or should be JSON but
                    // have git ref: this file is redundant.
                    (VersionStorageFormat::GitRef(_), false)
                    | (VersionStorageFormat::Json, true) => true,
                    // Format matches, or we had an error: not redundant.
                    (VersionStorageFormat::GitRef(_), true)
                    | (VersionStorageFormat::Json, false)
                    | (VersionStorageFormat::Error, _) => false,
                };
                if redundant {
                    problems.push(Problem::DuplicateLocalFile { local_file });
                }
            }
        } else {
            let local_file = matching[0];

            match (&storage_format, local_file.spec_file_name().is_git_ref()) {
                (VersionStorageFormat::GitRef(git_ref), false) => {
                    // Should be git ref but is JSON: convert to git ref.
                    problems.push(Problem::BlessedVersionShouldBeGitRef {
                        local_file,
                        git_ref: git_ref.clone(),
                    });
                }
                (VersionStorageFormat::Json, true) => {
                    // Should be JSON but is git ref: convert to JSON.
                    problems.push(Problem::GitRefShouldBeJson {
                        local_file,
                        blessed,
                    });
                }
                (VersionStorageFormat::GitRef(_), true)
                | (VersionStorageFormat::Json, false) => {
                    // Format matches preference: no conversion needed.
                }
                (VersionStorageFormat::Error, _) => {
                    // Error determining format: don't suggest any changes.
                }
            }
        }

        problems.extend(non_matching.into_iter().map(|s| {
            Problem::BlessedVersionExtraLocalSpec {
                spec_file_name: s.spec_file_name().clone(),
            }
        }));
    }

    Resolution::new_blessed(problems)
}

fn resolve_api_version_local<'a>(
    env: &'_ ResolvedEnv,
    api: &'_ ManagedApi,
    validation: Option<&DynValidationFn>,
    version: ApiVersion<'_>,
    generated: &'a GeneratedApiSpecFile,
    local: &'a [LocalApiSpecFile],
) -> Resolution<'a> {
    let mut problems = Vec::new();

    // alidate the generated API document.
    validate_generated(env, api, validation, version, generated, &mut problems);

    let (matching, non_matching): (Vec<_>, Vec<_>) = local
        .iter()
        .partition(|local| local.contents() == generated.contents());

    if matching.is_empty() {
        // There was no matching spec.
        if non_matching.is_empty() {
            // There were no non-matching specs, either.
            problems.push(Problem::LocalVersionMissingLocal { generated });
        } else {
            // There were non-matching specs.  This is your basic "stale" case.
            problems.push(Problem::LocalVersionStale {
                spec_files: non_matching,
                generated,
            });
        }
    } else if !non_matching.is_empty() {
        // There was a matching spec, but also some non-matching ones.
        // These are superfluous.  (It's not clear how this could happen.)
        let spec_file_names = DisplayableVec(
            non_matching.iter().map(|s| s.spec_file_name().clone()).collect(),
        );
        problems.push(Problem::LocalVersionExtra { spec_file_names });
    }

    Resolution::new_new_locally(problems)
}

fn validate_generated(
    env: &ResolvedEnv,
    api: &ManagedApi,
    validation: Option<&DynValidationFn>,
    version: ApiVersion<'_>,
    generated: &GeneratedApiSpecFile,
    problems: &mut Vec<Problem<'_>>,
) {
    match validate(
        env,
        api,
        version.is_latest,
        version.is_blessed,
        validation,
        generated,
    ) {
        Err(source) => {
            problems.push(Problem::GeneratedValidationError {
                api_ident: api.ident().clone(),
                version: version.version.clone(),
                source,
            });
        }
        Ok(extra_files) => {
            for (path, status) in extra_files {
                match status {
                    CheckStatus::Fresh => (),
                    CheckStatus::Stale(check_stale) => {
                        problems.push(Problem::ExtraFileStale {
                            api_ident: api.ident().clone(),
                            path,
                            check_stale,
                        });
                    }
                }
            }
        }
    }
}

/// Describes the first commit for the latest version.
///
/// Used to decide whether to suggest git ref conversion for older versions.
#[derive(Clone, Copy, Debug)]
enum LatestFirstCommit {
    NotBlessed,
    Blessed(GitCommitHash),
    BlessedError,
}

/// Describes what storage format a blessed version should use.
#[derive(Clone, Debug)]
enum VersionStorageFormat {
    /// The version should be stored as a git ref file.
    GitRef(GitRef),
    /// The version should be stored as a JSON file.
    Json,
    /// An error occurred while determining the storage format. The version
    /// should not be modified.
    Error,
}

/// Returns the storage format for a blessed version, assuming git ref storage
/// is enabled and the current version's potential git ref is known.
fn storage_format_for_blessed(
    latest: LatestFirstCommit,
    current: GitRef,
) -> VersionStorageFormat {
    // This match statement captures the decision table:
    //
    //      status         |  storage format
    //                     |
    //    NotBlessed       |    GitRef (always)
    //   Blessed(same)     |       Json
    // Blessed(different)  |      GitRef
    //    BlessedError     |      Error
    match latest {
        LatestFirstCommit::NotBlessed => {
            // The latest version is not blessed. This means that a new version
            // is being added, so we should always convert blessed versions to
            // git refs.
            VersionStorageFormat::GitRef(current)
        }

        LatestFirstCommit::Blessed(latest_first_commit) => {
            // The latest version is blessed. Only suggest conversions if the
            // version's first commit is different from the latest version's
            // first commit.
            if current.commit != latest_first_commit {
                VersionStorageFormat::GitRef(current)
            } else {
                VersionStorageFormat::Json
            }
        }

        LatestFirstCommit::BlessedError => {
            // The latest version is blessed, but an error occurred while
            // determining its first commit. Don't suggest any changes.
            VersionStorageFormat::Error
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_displayable_vec() {
        let v = DisplayableVec(Vec::<usize>::new());
        assert_eq!(v.to_string(), "");

        let v = DisplayableVec(vec![8]);
        assert_eq!(v.to_string(), "8");

        let v = DisplayableVec(vec![8, 12, 14]);
        assert_eq!(v.to_string(), "8, 12, 14");
    }

    #[test]
    fn test_storage_format_for_blessed() {
        let current = git_ref(COMMIT_A);

        assert!(
            matches!(
                storage_format_for_blessed(
                    LatestFirstCommit::NotBlessed,
                    current.clone()
                ),
                VersionStorageFormat::GitRef(_)
            ),
            "latest NotBlessed => always GitRef"
        );

        let latest = LatestFirstCommit::Blessed(commit(COMMIT_A));
        assert!(
            matches!(
                storage_format_for_blessed(latest, current.clone()),
                VersionStorageFormat::Json
            ),
            "latest Blessed with same commit => Json"
        );

        let latest = LatestFirstCommit::Blessed(commit(COMMIT_B));
        assert!(
            matches!(
                storage_format_for_blessed(latest, current.clone()),
                VersionStorageFormat::GitRef(_)
            ),
            "latest Blessed with different commit => GitRef"
        );

        assert!(
            matches!(
                storage_format_for_blessed(
                    LatestFirstCommit::BlessedError,
                    current
                ),
                VersionStorageFormat::Error
            ),
            "latest BlessedError => Error"
        );
    }

    // Test commit hashes.
    const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn commit(s: &str) -> GitCommitHash {
        s.parse().unwrap()
    }

    fn git_ref(s: &str) -> GitRef {
        use camino::Utf8PathBuf;
        GitRef { commit: commit(s), path: Utf8PathBuf::from("test/path.json") }
    }
}
