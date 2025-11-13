// Copyright 2025 Oxide Computer Company

//! Types used by trait-based API definitions to define the versions that they
//! support.

use std::collections::BTreeMap;

/// Describes how an API is versioned
#[derive(Clone, Debug)]
pub enum Versions {
    /// There is only ever one version of this API
    ///
    /// Clients and servers are updated at runtime in lockstep.
    Lockstep { version: semver::Version },

    /// There are multiple supported versions of this API
    ///
    /// Clients and servers may be updated independently of each other.  Other
    /// parts of the system may constrain things so that either clients or
    /// servers are always updated first, but this tool does not assume that.
    Versioned { supported_versions: SupportedVersions },
}

impl Versions {
    /// Constructor for a lockstep API
    pub fn new_lockstep(version: semver::Version) -> Versions {
        Versions::Lockstep { version }
    }

    /// Constructor for a versioned API
    pub fn new_versioned(supported_versions: SupportedVersions) -> Versions {
        Versions::Versioned { supported_versions }
    }

    /// Returns whether this API is versioned (as opposed to lockstep)
    pub fn is_versioned(&self) -> bool {
        match self {
            Versions::Lockstep { .. } => false,
            Versions::Versioned { .. } => true,
        }
    }

    /// Returns whether this API is lockstep (as opposed to versioned)
    pub fn is_lockstep(&self) -> bool {
        match self {
            Versions::Lockstep { .. } => true,
            Versions::Versioned { .. } => false,
        }
    }

    /// Iterate over the semver versions of an API that are supported
    pub fn iter_versions_semvers(&self) -> IterVersionsSemvers<'_> {
        match self {
            Versions::Lockstep { version } => IterVersionsSemvers {
                inner: IterVersionsSemversInner::Lockstep(Some(version)),
            },
            Versions::Versioned { supported_versions } => IterVersionsSemvers {
                inner: IterVersionsSemversInner::Versioned(
                    supported_versions.versions.iter(),
                ),
            },
        }
    }

    /// For versioned APIs only, iterate over the SupportedVersions
    pub fn iter_versioned_versions(
        &self,
    ) -> Option<impl Iterator<Item = &SupportedVersion> + '_> {
        match self {
            Versions::Lockstep { .. } => None,
            Versions::Versioned { supported_versions } => {
                Some(supported_versions.iter())
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct SupportedVersion {
    semver: semver::Version,
    label: &'static str,
}

impl SupportedVersion {
    pub const fn new(
        semver: semver::Version,
        label: &'static str,
    ) -> SupportedVersion {
        SupportedVersion { semver, label }
    }

    pub fn semver(&self) -> &semver::Version {
        &self.semver
    }

    pub fn label(&self) -> &str {
        self.label
    }
}

#[derive(Clone, Debug)]
pub struct SupportedVersions {
    versions: Vec<SupportedVersion>,
}

impl SupportedVersions {
    #[track_caller]
    pub fn new(versions: Vec<SupportedVersion>) -> SupportedVersions {
        assert!(
            !versions.is_empty(),
            "at least one version of an API must be supported"
        );

        // We require that the list of supported versions for an API be sorted
        // because this helps ensure a git conflict when two people attempt to
        // add or modify the same version in different branches.
        assert!(
            versions.iter().map(|v| v.semver()).is_sorted(),
            "list of supported versions for an API must be sorted"
        );

        // Each semver and each label must be unique.
        let mut unique_versions = BTreeMap::new();
        let mut unique_labels = BTreeMap::new();
        for v in &versions {
            if let Some(previous) =
                unique_versions.insert(v.semver(), v.label())
            {
                panic!(
                    "version {} appears multiple times (labels: {:?}, {:?})",
                    v.semver(),
                    previous,
                    v.label()
                );
            }

            if let Some(previous) = unique_labels.insert(v.label(), v.semver())
            {
                panic!(
                    "label {:?} appears multiple times (versions: {}, {})",
                    v.label(),
                    previous,
                    v.semver()
                );
            }
        }

        SupportedVersions { versions }
    }

    pub fn iter(&self) -> impl Iterator<Item = &'_ SupportedVersion> + '_ {
        self.versions.iter()
    }
}

#[derive(Debug)]
pub struct IterVersionsSemvers<'a> {
    inner: IterVersionsSemversInner<'a>,
}

impl<'a> Iterator for IterVersionsSemvers<'a> {
    type Item = &'a semver::Version;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl<'a> ExactSizeIterator for IterVersionsSemvers<'a> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<'a> DoubleEndedIterator for IterVersionsSemvers<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back()
    }
}

#[derive(Debug)]
enum IterVersionsSemversInner<'a> {
    Lockstep(Option<&'a semver::Version>),
    Versioned(std::slice::Iter<'a, SupportedVersion>),
}

impl<'a> Iterator for IterVersionsSemversInner<'a> {
    type Item = &'a semver::Version;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            IterVersionsSemversInner::Lockstep(version) => version.take(),
            IterVersionsSemversInner::Versioned(versions) => {
                versions.next().map(|v| &v.semver)
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len(), Some(self.len()))
    }
}

impl<'a> ExactSizeIterator for IterVersionsSemversInner<'a> {
    fn len(&self) -> usize {
        match self {
            IterVersionsSemversInner::Lockstep(version) => {
                usize::from(version.is_some())
            }
            IterVersionsSemversInner::Versioned(versions) => versions.len(),
        }
    }
}

impl<'a> DoubleEndedIterator for IterVersionsSemversInner<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self {
            IterVersionsSemversInner::Lockstep(version) => version.take(),
            IterVersionsSemversInner::Versioned(versions) => {
                versions.next_back().map(|v| &v.semver)
            }
        }
    }
}

/// Helper macro used to define API versions.
///
/// ```
/// use dropshot_api_manager_types::{
///     SupportedVersion, SupportedVersions, api_versions,
/// };
///
/// api_versions!([
///     // Define the API versions here. They must be in descending order.
///     (2, ADD_FOOBAR_OPERATION),
///     (1, INITIAL),
/// ]);
/// ```
///
/// This example says that there are two API versions: `1.0.0` (the initial
/// version) and `2.0.0` (which adds an operation called "foobar").  This macro
/// invocation defines symbolic constants of type `semver::Version` for each of
/// these, equivalent to:
///
/// ```
/// pub const VERSION_ADD_FOOBAR_OPERATION: semver::Version =
///     semver::Version::new(2, 0, 0);
/// pub const VERSION_INITIAL: semver::Version = semver::Version::new(1, 0, 0);
/// ```
///
/// It also defines two functions:
///
/// * `pub fn supported_versions() -> SupportedVersions` that,
///   as the name suggests, returns a [`SupportedVersions`] that describes these
///   two supported API versions.
///
/// * `pub fn latest_version() -> semver::Version` that returns the latest
///   supported API version. The latest supported version is the first version
///   in the list (hence versions must be in descending order).
// Design constraints:
// - For each new API version, we need a developer-chosen semver and label that
//   can be used to construct an identifier.
// - We want to produce:
//   - a symbolic constant for each version that won't change if the developer
//     needs to change the semver value for this API version
//   - a list of supported API versions
// - Critically, we want to ensure that if two developers both add new API
//   versions in separate branches, whether or not they choose the same value,
//   there must be a git conflict that requires manual resolution.
//   - To achieve this, we put the list of versions in a list.
// - We want to make it hard to do this merge wrong without noticing.
//   - We want to require that the list be sorted (so that someone hasn't put
//     something in the wrong order).
//   - The list should have no duplicates.
// - We want to minimize boilerplate.
//
// That's how we've landed on defining API versions using this macro where:
// - each API definition is simple and fits on a single line
// - there will necessarily be a conflict if two people try to add a line in the
//   same spot of the file, even if they overlap, assuming they choose different
//   labels for their API version
// - the consumer of this value will be able to do those checks that help make
//   sure there wasn't a mismerge.
#[macro_export]
macro_rules! api_versions {
    (
        [
            (
                $latest_major:literal,
                $latest_name: ident
            )
            $(,
                (
                    $major:literal,
                    $name:ident
                )
            )*
            $(,)?
        ] ) => {
        dropshot_api_manager_types::paste! {
            pub const [<VERSION_ $latest_name>]: $crate::semver::Version =
                $crate::semver::Version::new($latest_major, 0, 0);

            $(
                pub const [<VERSION_ $name>]: $crate::semver::Version =
                    $crate::semver::Version::new($major, 0, 0);
            )*

            pub fn supported_versions() -> $crate::SupportedVersions {
                let mut literal_versions = vec![
                    $crate::SupportedVersion::new([<VERSION_ $latest_name>], stringify!($latest_name)),
                    $( $crate::SupportedVersion::new([<VERSION_ $name>], stringify!($name)) ),*
                ];
                literal_versions.reverse();
                $crate::SupportedVersions::new(literal_versions)
            }

            pub const fn latest_version() -> $crate::semver::Version {
                [<VERSION_ $latest_name>]
            }
        }
    };
}

/// "picky" version of `api_versions` that lets you specify the minor and patch
/// numbers, too
///
/// It is not yet clear why we'd ever need this.  Our approach to versioning is
/// oriented around not having to care whether a change is a major bump or not
/// so we can just always bump the major number.
#[macro_export]
macro_rules! api_versions_picky {
    ( [
        (
            $latest_major:literal,
            $latest_minor:literal,
            $latest_patch:literal,
            $latest_name: ident
        )
        $(,
            (
                $major:literal,
                $minor:literal,
                $patch:literal,
                $name:ident
            )
        )* $(,)? ] ) => {
        dropshot_api_manager_types::paste! {
            pub const [<VERSION_ $latest_name>]: $crate::semver::Version =
                $crate::semver::Version::new($latest_major, $latest_minor, $latest_patch);

            $(
                pub const [<VERSION_ $name>]: $crate::semver::Version =
                    $crate::semver::Version::new($major, $minor, $patch);
            )*

            pub fn supported_versions() -> $crate::SupportedVersions {
                let mut literal_versions = vec![
                    $crate::SupportedVersion::new([<VERSION_ $latest_name>], stringify!($latest_name)),
                    $( $crate::SupportedVersion::new([<VERSION_ $name>], stringify!($name)) ),*
                ];
                literal_versions.reverse();
                $crate::SupportedVersions::new(literal_versions)
            }

            pub const fn latest_version() -> $crate::semver::Version {
                [<VERSION_ $latest_name>]
            }
        }
    };
}
