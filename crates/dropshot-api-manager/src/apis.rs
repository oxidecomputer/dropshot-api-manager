// Copyright 2025 Oxide Computer Company

use anyhow::{Context, bail};
use dropshot::{ApiDescription, ApiDescriptionBuildErrors, StubContext};
use dropshot_api_manager_types::{
    ApiIdent, IterVersionsSemvers, ManagedApiMetadata, SupportedVersion,
    ValidationContext, Versions,
};
use openapiv3::OpenAPI;
use std::collections::{BTreeMap, BTreeSet};

/// Describes an API managed by the Dropshot API manager.
///
/// Each API listed within a `ManagedApiConfig` forms a unit managed by the
/// Dropshot API manager.
#[derive(Clone, Debug)]
pub struct ManagedApiConfig {
    /// The API-specific part of the filename that's used for API descriptions
    ///
    /// This string is sometimes used as an identifier for developers.
    pub ident: &'static str,

    /// how this API is versioned
    pub versions: Versions,

    /// title of the API (goes into OpenAPI spec)
    pub title: &'static str,

    /// metadata about the API
    pub metadata: ManagedApiMetadata,

    /// The API description function, typically a reference to
    /// `stub_api_description`
    ///
    /// This is used to generate the OpenAPI document that matches the current
    /// server implementation.
    pub api_description:
        fn() -> Result<ApiDescription<StubContext>, ApiDescriptionBuildErrors>,

    /// Extra validation to perform on the OpenAPI document, if any.
    ///
    /// For versioned APIs, extra validation is performed on *all* versions,
    /// including blessed ones. You may want to skip performing validation on
    /// blessed versions, though, because they're immutable. To do so, use
    /// [`ValidationContext::is_blessed`].
    pub extra_validation: Option<fn(&OpenAPI, ValidationContext<'_>)>,
}

/// Describes an API managed by the Dropshot API manager.
///
/// This type is typically created from a [`ManagedApiConfig`] and can be
/// further configured using builder methods before being passed to
/// [`ManagedApis::new`].
#[derive(Debug)]
pub struct ManagedApi {
    /// The API-specific part of the filename that's used for API descriptions
    ///
    /// This string is sometimes used as an identifier for developers.
    ident: ApiIdent,

    /// how this API is versioned
    versions: Versions,

    /// title of the API (goes into OpenAPI spec)
    title: &'static str,

    /// metadata about the API
    metadata: ManagedApiMetadata,

    /// The API description function, typically a reference to
    /// `stub_api_description`
    ///
    /// This is used to generate the OpenAPI document that matches the current
    /// server implementation.
    api_description:
        fn() -> Result<ApiDescription<StubContext>, ApiDescriptionBuildErrors>,

    /// Extra validation to perform on the OpenAPI document, if any.
    extra_validation: Option<fn(&OpenAPI, ValidationContext<'_>)>,

    /// If true, allow trivial changes (doc updates, type renames) for the
    /// latest blessed version without requiring version bumps.
    ///
    /// Default: false (bytewise check is performed for latest version).
    allow_trivial_changes_for_latest: bool,
}

impl From<ManagedApiConfig> for ManagedApi {
    fn from(value: ManagedApiConfig) -> Self {
        ManagedApi {
            ident: ApiIdent::from(value.ident.to_owned()),
            versions: value.versions,
            title: value.title,
            metadata: value.metadata,
            api_description: value.api_description,
            extra_validation: value.extra_validation,
            allow_trivial_changes_for_latest: false,
        }
    }
}

impl ManagedApi {
    pub fn ident(&self) -> &ApiIdent {
        &self.ident
    }

    pub fn versions(&self) -> &Versions {
        &self.versions
    }

    pub fn title(&self) -> &'static str {
        self.title
    }

    pub fn metadata(&self) -> &ManagedApiMetadata {
        &self.metadata
    }

    pub fn is_lockstep(&self) -> bool {
        self.versions.is_lockstep()
    }

    pub fn is_versioned(&self) -> bool {
        self.versions.is_versioned()
    }

    pub fn iter_versioned_versions(
        &self,
    ) -> Option<impl Iterator<Item = &SupportedVersion> + '_> {
        self.versions.iter_versioned_versions()
    }

    pub fn iter_versions_semver(&self) -> IterVersionsSemvers<'_> {
        self.versions.iter_versions_semvers()
    }

    pub fn generate_openapi_doc(
        &self,
        version: &semver::Version,
    ) -> anyhow::Result<OpenAPI> {
        // It's a bit weird to first convert to bytes and then back to OpenAPI,
        // but this is the easiest way to do so (currently, Dropshot doesn't
        // return the OpenAPI type directly). It is also consistent with the
        // other code paths.
        let contents = self.generate_spec_bytes(version)?;
        serde_json::from_slice(&contents)
            .context("generated document is not valid OpenAPI")
    }

    pub fn generate_spec_bytes(
        &self,
        version: &semver::Version,
    ) -> anyhow::Result<Vec<u8>> {
        let description = (self.api_description)().map_err(|error| {
            // ApiDescriptionBuildError is actually a list of errors so it
            // doesn't implement std::error::Error itself. Its Display
            // impl formats the errors appropriately.
            anyhow::anyhow!("{}", error)
        })?;
        let mut openapi_def = description.openapi(self.title, version.clone());
        if let Some(description) = self.metadata.description {
            openapi_def.description(description);
        }
        if let Some(contact_url) = self.metadata.contact_url {
            openapi_def.contact_url(contact_url);
        }
        if let Some(contact_email) = self.metadata.contact_email {
            openapi_def.contact_email(contact_email);
        }

        // Use write because it's the most reliable way to get the canonical
        // JSON order. The `json` method returns a serde_json::Value which may
        // or may not have preserve_order enabled.
        let mut contents = Vec::new();
        openapi_def.write(&mut contents)?;
        Ok(contents)
    }

    pub fn extra_validation(
        &self,
        openapi: &OpenAPI,
        validation_context: ValidationContext<'_>,
    ) {
        if let Some(extra_validation) = self.extra_validation {
            extra_validation(openapi, validation_context);
        }
    }

    /// Allow trivial changes (doc updates, type renames) for the latest
    /// blessed version without requiring a version bump.
    ///
    /// By default, the latest blessed version requires bytewise equality
    /// between blessed and generated documents. This prevents trivial changes
    /// from accumulating invisibly. Calling this method allows semantic-only
    /// checking for all versions, including the latest.
    pub fn allow_trivial_changes_for_latest(mut self) -> Self {
        self.allow_trivial_changes_for_latest = true;
        self
    }

    pub(crate) fn allows_trivial_changes_for_latest(&self) -> bool {
        self.allow_trivial_changes_for_latest
    }
}

/// Describes the Rust-defined configuration for all of the APIs managed by this
/// tool.
///
/// This is repo-specific state that's passed into the OpenAPI manager.
#[derive(Debug)]
pub struct ManagedApis {
    apis: BTreeMap<ApiIdent, ManagedApi>,
    unknown_apis: BTreeSet<ApiIdent>,
    validation: Option<fn(&OpenAPI, ValidationContext<'_>)>,
}

impl ManagedApis {
    /// Constructs a new `ManagedApis` instance from a list of API
    /// configurations.
    ///
    /// This is the main entry point for creating a new `ManagedApis` instance.
    /// Accepts any iterable of items that can be converted into [`ManagedApi`],
    /// including `Vec<ManagedApiConfig>` and `Vec<ManagedApi>`.
    pub fn new<I>(api_list: I) -> anyhow::Result<ManagedApis>
    where
        I: IntoIterator,
        I::Item: Into<ManagedApi>,
    {
        let mut apis = BTreeMap::new();
        for api in api_list {
            let api = api.into();
            if let Some(old) = apis.insert(api.ident.clone(), api) {
                bail!("API is defined twice: {:?}", &old.ident);
            }
        }

        Ok(ManagedApis {
            apis,
            unknown_apis: BTreeSet::new(),
            validation: None,
        })
    }

    /// Adds the given API identifiers (without the ending `.json`) to the list
    /// of unknown APIs.
    ///
    /// By default, if an unknown `.json` file is encountered within the OpenAPI
    /// directory, a failure is produced. Use this method to produce a warning
    /// for an allowlist of APIs instead.
    pub fn with_unknown_apis<I, S>(mut self, apis: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<ApiIdent>,
    {
        self.unknown_apis.extend(apis.into_iter().map(|s| s.into()));
        self
    }

    /// Sets a validation function to be used for all APIs.
    ///
    /// This function will be called for each API document. The
    /// [`ValidationContext`] can be used to report errors, as well as extra
    /// files for which the contents need to be compared with those on disk.
    pub fn with_validation(
        mut self,
        validation: fn(&OpenAPI, ValidationContext<'_>),
    ) -> Self {
        self.validation = Some(validation);
        self
    }

    /// Returns the validation function for all APIs.
    pub fn validation(&self) -> Option<fn(&OpenAPI, ValidationContext<'_>)> {
        self.validation
    }

    /// Returns the number of APIs managed by this instance.
    pub fn len(&self) -> usize {
        self.apis.len()
    }

    /// Returns true if there are no APIs managed by this instance.
    pub fn is_empty(&self) -> bool {
        self.apis.is_empty()
    }

    pub(crate) fn iter_apis(
        &self,
    ) -> impl Iterator<Item = &'_ ManagedApi> + '_ {
        self.apis.values()
    }

    pub(crate) fn api(&self, ident: &ApiIdent) -> Option<&ManagedApi> {
        self.apis.get(ident)
    }

    /// Returns the set of unknown APIs.
    pub fn unknown_apis(&self) -> &BTreeSet<ApiIdent> {
        &self.unknown_apis
    }
}
