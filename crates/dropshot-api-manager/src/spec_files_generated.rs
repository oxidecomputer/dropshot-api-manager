// Copyright 2026 Oxide Computer Company

//! Newtype and collection to represent OpenAPI documents generated from the
//! API definitions

use crate::{
    apis::{ManagedApi, ManagedApis},
    environment::ErrorAccumulator,
    spec_files_generic::{
        ApiFiles, ApiLoad, ApiSpecFile, ApiSpecFilesBuilder, AsRawFiles,
        SpecFileInfo, hash_contents,
    },
};
use anyhow::{anyhow, bail};
use dropshot_api_manager_types::{
    ApiIdent, ApiSpecFileName, LockstepApiSpecFileName,
    VersionedApiSpecFileName,
};
use rayon::prelude::*;
use std::{collections::BTreeMap, ops::Deref};

/// Newtype wrapper around [`ApiSpecFile`] to describe OpenAPI documents
/// generated from API definitions
///
/// This includes documents for lockstep APIs and versioned APIs, for both
/// blessed and locally-added versions.
pub struct GeneratedApiSpecFile(ApiSpecFile);
NewtypeDebug! { () pub struct GeneratedApiSpecFile(ApiSpecFile); }
NewtypeDeref! { () pub struct GeneratedApiSpecFile(ApiSpecFile); }
NewtypeDerefMut! { () pub struct GeneratedApiSpecFile(ApiSpecFile); }
NewtypeFrom! { () pub struct GeneratedApiSpecFile(ApiSpecFile); }

// Trait impls that allow us to use `ApiFiles<GeneratedApiSpecFile>`
//
// Note that this is NOT a `Vec` because it's NOT allowed to have more than one
// GeneratedApiSpecFile for a given version.

impl ApiLoad for GeneratedApiSpecFile {
    const MISCONFIGURATIONS_ALLOWED: bool = false;
    type Unparseable = std::convert::Infallible;

    fn make_item(raw: ApiSpecFile) -> Self {
        GeneratedApiSpecFile(raw)
    }

    fn try_extend(&mut self, item: ApiSpecFile) -> anyhow::Result<()> {
        // This should be impossible.
        bail!(
            "found more than one generated OpenAPI document for a given \
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

impl AsRawFiles for GeneratedApiSpecFile {
    fn as_raw_files<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a dyn SpecFileInfo> + 'a> {
        Box::new(std::iter::once(self.deref() as &dyn SpecFileInfo))
    }
}

/// Container for OpenAPI documents generated from API definitions
///
/// **Be sure to check for load errors and warnings before using this
/// structure.**
///
/// For more on what's been validated at this point, see
/// [`ApiSpecFilesBuilder`].
pub struct GeneratedFiles(BTreeMap<ApiIdent, ApiFiles<GeneratedApiSpecFile>>);
NewtypeDeref! {
    () pub struct GeneratedFiles(
        BTreeMap<ApiIdent, ApiFiles<GeneratedApiSpecFile>>
    );
}

/// Intermediate result from generating all versions for a single API.
///
/// This is produced in parallel (one per API) and then fed sequentially
/// into `ApiSpecFilesBuilder`. Each version is fully deserialized in the
/// parallel phase so that serde work doesn't bottleneck the reduce phase.
enum GeneratedApiResult {
    Lockstep {
        versions: Vec<Result<ApiSpecFile, anyhow::Error>>,
    },
    Versioned {
        ident: ApiIdent,
        versions: Vec<Result<ApiSpecFile, anyhow::Error>>,
        latest: Option<VersionedApiSpecFileName>,
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
                api.generate_spec_bytes(version)
                    .and_then(|contents| {
                        let file_name =
                            LockstepApiSpecFileName::new(api.ident().clone());
                        ApiSpecFile::for_contents(file_name.into(), contents)
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
                api.generate_spec_bytes(version)
                    .and_then(|contents| {
                        let file_name = VersionedApiSpecFileName::new(
                            api.ident().clone(),
                            version.clone(),
                            hash_contents(&contents),
                        );
                        ApiSpecFile::for_contents(file_name.into(), contents)
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
            r.as_ref().ok().and_then(|file| match file.spec_file_name() {
                ApiSpecFileName::Versioned(v) => Some(v.clone()),
                ApiSpecFileName::Lockstep(_) => unreachable!(
                    "lockstep file name in versioned API path"
                ),
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
        let mut api_files: ApiSpecFilesBuilder<GeneratedApiSpecFile> =
            ApiSpecFilesBuilder::new(apis, error_accumulator);

        for result in results {
            match result {
                GeneratedApiResult::Lockstep { versions } => {
                    for version_result in versions {
                        match version_result {
                            Ok(file) => api_files.load_parsed(file),
                            Err(error) => api_files.load_error(error),
                        }
                    }
                }
                GeneratedApiResult::Versioned { ident, versions, latest } => {
                    for version_result in versions {
                        match version_result {
                            Ok(file) => api_files.load_parsed(file),
                            Err(error) => api_files.load_error(error),
                        }
                    }
                    match latest {
                        Some(latest) => {
                            api_files.load_latest_link(&ident, latest)
                        }
                        None => api_files.load_error(anyhow!(
                            "versioned API {:?} symlink: there is no \
                             working version (fix above error(s) first)",
                            ident,
                        )),
                    }
                }
            }
        }

        Ok(Self::from(api_files))
    }
}

impl<'a> From<ApiSpecFilesBuilder<'a, GeneratedApiSpecFile>>
    for GeneratedFiles
{
    fn from(api_files: ApiSpecFilesBuilder<'a, GeneratedApiSpecFile>) -> Self {
        GeneratedFiles(api_files.into_map())
    }
}
