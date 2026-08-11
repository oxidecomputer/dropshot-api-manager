// Copyright 2026 Oxide Computer Company

//! Newtype and collection to represent OpenAPI documents generated from the
//! API definitions

use crate::{
    apis::{ManagedApi, ManagedApis},
    doc_files_generic::{
        ApiDocFile, ApiDocFilesBuilder, ApiFiles, ApiLoad, AsRawFiles,
        DocFileInfo, hash_contents,
    },
    environment::ErrorAccumulator,
};
use anyhow::{anyhow, bail};
use dropshot_api_manager_types::{
    ApiDocFileName, ApiIdent, LockstepApiDocFileName, VersionedApiDocFileName,
};
use rayon::prelude::*;
use std::{collections::BTreeMap, ops::Deref};

/// Newtype wrapper around [`ApiDocFile`] to describe OpenAPI documents
/// generated from API definitions
///
/// This includes documents for lockstep APIs and versioned APIs, for both
/// blessed and locally-added versions.
pub struct GeneratedApiDocFile(ApiDocFile);
NewtypeDebug! { () pub struct GeneratedApiDocFile(ApiDocFile); }
NewtypeDeref! { () pub struct GeneratedApiDocFile(ApiDocFile); }
NewtypeDerefMut! { () pub struct GeneratedApiDocFile(ApiDocFile); }
NewtypeFrom! { () pub struct GeneratedApiDocFile(ApiDocFile); }

// Trait impls that allow us to use `ApiFiles<GeneratedApiDocFile>`
//
// Note that this is NOT a `Vec` because it's NOT allowed to have more than one
// GeneratedApiDocFile for a given version.

impl ApiLoad for GeneratedApiDocFile {
    const MISCONFIGURATIONS_ALLOWED: bool = false;
    type Unparseable = std::convert::Infallible;

    fn make_item(raw: ApiDocFile) -> Self {
        GeneratedApiDocFile(raw)
    }

    fn try_extend(&mut self, item: ApiDocFile) -> anyhow::Result<()> {
        // This should be impossible.
        bail!(
            "found more than one generated OpenAPI document for a given \
             API version: at least {} and {}",
            self.doc_file_name(),
            item.doc_file_name()
        );
    }

    fn make_unparseable(
        _name: ApiDocFileName,
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

impl AsRawFiles for GeneratedApiDocFile {
    fn as_raw_files<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a dyn DocFileInfo> + 'a> {
        Box::new(std::iter::once(self.deref() as &dyn DocFileInfo))
    }
}

/// Container for OpenAPI documents generated from API definitions
///
/// **Be sure to check for load errors and warnings before using this
/// structure.**
///
/// For more on what's been validated at this point, see
/// [`ApiDocFilesBuilder`].
pub struct GeneratedFiles(BTreeMap<ApiIdent, ApiFiles<GeneratedApiDocFile>>);
NewtypeDeref! {
    () pub struct GeneratedFiles(
        BTreeMap<ApiIdent, ApiFiles<GeneratedApiDocFile>>
    );
}

/// Intermediate result from generating all versions for a single API.
///
/// This is produced in parallel (one per API) and then fed sequentially
/// into `ApiDocFilesBuilder`. Each version is fully deserialized in the
/// parallel phase so that serde work doesn't bottleneck the reduce phase.
enum GeneratedApiResult {
    Lockstep {
        versions: Vec<Result<ApiDocFile, anyhow::Error>>,
    },
    Versioned {
        ident: ApiIdent,
        versions: Vec<Result<ApiDocFile, anyhow::Error>>,
        latest: Option<VersionedApiDocFileName>,
    },
}

/// Generate and deserialize all versions for a single API.
///
/// This is called in parallel.
fn generate_api(api: &ManagedApi) -> GeneratedApiResult {
    if api.is_lockstep() {
        let versions = api
            .iter_versions_semver()
            .map(|version| {
                api.generate_doc_bytes(version)
                    .and_then(|contents| {
                        let file_name =
                            LockstepApiDocFileName::new(api.ident().clone());
                        ApiDocFile::for_contents(file_name.into(), contents)
                            .map_err(|(e, _buf)| e)
                    })
                    .map_err(|error| {
                        error.context(format!(
                            "generating OpenAPI document for lockstep \
                             API {:?}",
                            api.ident()
                        ))
                    })
            })
            .collect();
        GeneratedApiResult::Lockstep { versions }
    } else {
        // Parallelize generation across versions.
        let supported_versions: Vec<_> = api
            .iter_versioned_versions()
            .expect(
                "iter_versioned_versions() returns `Some` for versioned APIs",
            )
            .collect();
        let versions: Vec<_> = supported_versions
            .par_iter()
            .map(|supported_version| {
                let version = supported_version.semver();
                api.generate_doc_bytes(version)
                    .and_then(|contents| {
                        let file_name = VersionedApiDocFileName::new(
                            api.ident().clone(),
                            version.clone(),
                            hash_contents(&contents),
                        );
                        ApiDocFile::for_contents(file_name.into(), contents)
                            .map_err(|(e, _buf)| e)
                    })
                    .map_err(|error| {
                        error.context(format!(
                            "generating OpenAPI document for versioned \
                             API {:?} version {}",
                            api.ident(),
                            version
                        ))
                    })
            })
            .collect();
        // The latest version is the last one that succeeded. Versions
        // are in ascending order, so iterate from the back.
        //
        // (Note that ParallelIterator::map does not reorder items.)
        let latest = versions.iter().rev().find_map(|r| {
            r.as_ref().ok().map(|file| match file.doc_file_name() {
                ApiDocFileName::Versioned(v) => v.clone(),
                ApiDocFileName::Lockstep(_) => {
                    unreachable!("lockstep file name in versioned API path")
                }
            })
        });
        GeneratedApiResult::Versioned {
            ident: api.ident().clone(),
            versions,
            latest,
        }
    }
}

impl GeneratedFiles {
    /// Generate OpenAPI documents for all supported versions of all managed
    /// APIs.
    ///
    /// This function loads all APIs in parallel.
    pub fn generate(
        apis: &ManagedApis,
        error_accumulator: &mut ErrorAccumulator,
    ) -> anyhow::Result<GeneratedFiles> {
        // Map: generate and deserialize in parallel.
        let results: Vec<GeneratedApiResult> = apis
            .iter_apis()
            .collect::<Vec<_>>()
            .par_iter()
            .map(|api| generate_api(api))
            .collect();

        // Reduce: feed results into the builder sequentially.
        let mut api_files: ApiDocFilesBuilder<GeneratedApiDocFile> =
            ApiDocFilesBuilder::new(apis, error_accumulator);

        for result in results {
            let (versions, latest_info) = match result {
                GeneratedApiResult::Lockstep { versions } => (versions, None),
                GeneratedApiResult::Versioned { ident, versions, latest } => {
                    (versions, Some((ident, latest)))
                }
            };

            for version_result in versions {
                match version_result {
                    Ok(file) => api_files.load_parsed(file),
                    Err(error) => api_files.load_error(error),
                }
            }

            if let Some((ident, latest)) = latest_info {
                match latest {
                    Some(latest) => api_files.load_latest_link(&ident, latest),
                    None => api_files.load_error(anyhow!(
                        "versioned API {:?} symlink: there is no \
                         working version (fix above error(s) first)",
                        ident,
                    )),
                }
            }
        }

        Ok(Self::from(api_files))
    }
}

impl<'a> From<ApiDocFilesBuilder<'a, GeneratedApiDocFile>> for GeneratedFiles {
    fn from(api_files: ApiDocFilesBuilder<'a, GeneratedApiDocFile>) -> Self {
        GeneratedFiles(api_files.into_map())
    }
}
