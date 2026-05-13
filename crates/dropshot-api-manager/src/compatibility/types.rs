// Copyright 2026 Oxide Computer Company

//! Data model for compatibility issues.
//!
//! These types are shared between [`super::detect`] (which builds them from
//! drift output) and [`super::display`] (which renders them for the CLI). The
//! detection algorithm lives in `detect`; this module is purely the shape of
//! what gets passed between the two.

use super::display::ApiCompatIssueDisplay;
use crate::output::Styles;
use drift::ChangeClass;
use std::collections::{BTreeMap, HashMap};

/// A compatibility error between two OpenAPI documents.
///
/// Each issue corresponds to one component or endpoint that has non-trivial
/// changes. The same component may be reachable from multiple endpoints; the
/// inverted reference tree records every such chain so we can show all affected
/// endpoints.
#[derive(Debug)]
pub struct ApiCompatIssue {
    /// Base location in the blessed (old) document, e.g. a schema component
    /// or an endpoint.
    pub(super) blessed_base: DocumentBasePath,
    /// Base location in the generated (new) document.
    ///
    /// Differs from `blessed_base` only when the component or endpoint was
    /// renamed.
    pub(super) generated_base: DocumentBasePath,
    /// Non-trivial changes detected within this base location.
    pub(super) changes: Vec<SubpathChange>,
    /// Inverted reference tree.
    ///
    /// Empty when the change is directly at an endpoint with no `$ref`
    /// indirection.
    pub(super) tree: PathTree,
    /// Cached blessed JSON value at the base, for diff display.
    pub(super) blessed_value: Option<serde_json::Value>,
    /// Cached generated JSON value at the base, for diff display.
    pub(super) generated_value: Option<serde_json::Value>,
}

impl ApiCompatIssue {
    pub(crate) fn blessed_json(&self) -> String {
        to_json_pretty(self.blessed_value.as_ref())
    }

    pub(crate) fn generated_json(&self) -> String {
        to_json_pretty(self.generated_value.as_ref())
    }

    pub(crate) fn display<'a>(
        &'a self,
        styles: &'a Styles,
    ) -> ApiCompatIssueDisplay<'a> {
        ApiCompatIssueDisplay { issue: self, styles }
    }
}

/// A non-trivial change at a particular subpath within a base location.
///
/// One [`ApiCompatIssue`] may carry several `SubpathChange`s when multiple
/// non-trivial changes were detected within the same component or
/// endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SubpathChange {
    pub(super) class: ChangeClass,
    pub(super) message: String,
    /// Subpath of the change within the base location, on the blessed side.
    pub(super) old_subpath: DocumentPath,
    /// Subpath on the generated side. Same as `old_subpath` unless a field
    /// within the base was renamed.
    pub(super) new_subpath: DocumentPath,
}

/// A base location within an OpenAPI document.
///
/// This enum classifies base document paths so the rendering layer can pattern
/// match on the variant rather than asking "is this a component? is this an
/// endpoint?" each time it inspects a base path. For endpoint variants, the
/// spec's `operationId` is carried alongside the path so that display code can
/// annotate it on the rendered endpoint.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum DocumentBasePath {
    /// A reusable component, e.g., `#/components/schemas/Foo`.
    Component(DocumentPath),
    /// An endpoint, e.g., `#/paths/~1users~1{id}/get`.
    Endpoint {
        /// The path (`paths/<route>/<method>`).
        name: DocumentPath,
        /// The endpoint's `operationId` from the spec, if found.
        operation_id: Option<String>,
    },
    /// The `.paths` container itself — not a meaningful change location.
    ///
    /// Drift emits `.paths` on whichever side is *missing* an endpoint when
    /// one is added or removed: the `paths` map exists on both sides, even
    /// though the leaf doesn't. Classifying this explicitly (rather than
    /// letting it fall into [`Self::Other`]) lets the renderer treat it as a
    /// "non-location" — see how `display::write_at_content` drops a
    /// `PathsRoot` paired with a real `Component`/`Endpoint`.
    ///
    /// Drift 0.2's `compare.rs` only diffs operations, not top-level
    /// component additions/removals, so `.components.<kind>` containers
    /// never need their own variant here. If drift later adds
    /// component-level comparison, expect a similar variant per container.
    PathsRoot,
    /// Anything that doesn't match a known base shape.
    ///
    /// This shouldn't happen in practice, but we preserve these paths anyway
    /// so the renderer can still fall back to something reasonable (and not
    /// just drop the path).
    Other(DocumentPath),
}

impl DocumentBasePath {
    /// Classify a raw `path`:
    ///
    /// * `components/<kind>/<name>` is [`Self::Component`].
    /// * `paths/<route>/<method>` is [`Self::Endpoint`], with the operation id
    ///   looked up in `op_ids` (absent or non-string `operationId`
    ///   becomes `None`).
    /// * `paths` (1 segment) is [`Self::PathsRoot`] — the parent container
    ///   of endpoints, which drift uses when an endpoint is added or
    ///   removed on one side.
    /// * Anything else is [`Self::Other`].
    pub(super) fn classify(
        path: DocumentPath,
        op_ids: &OperationIdMap<'_>,
    ) -> Self {
        match path.segments.as_slice() {
            [a, _, _] if a == "components" => Self::Component(path),
            [a, _, _] if a == "paths" => {
                let operation_id = op_ids.get(&path).map(|s| s.to_string());
                Self::Endpoint { name: path, operation_id }
            }
            [a] if a == "paths" => Self::PathsRoot,
            _ => Self::Other(path),
        }
    }

    /// Returns true if this base is the `.paths` container root rather than
    /// a real endpoint or component location. Used by display code to drop
    /// the missing side of an add/remove pair.
    pub(super) fn is_paths_root(&self) -> bool {
        matches!(self, Self::PathsRoot)
    }
}

/// Composite key for [`PathTree`]: a base location together with the
/// subpath of the `$ref` source within that base. Each tree edge
/// corresponds to one such `(base, subpath)` pair.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct PathTreeKey {
    pub(super) base: DocumentBasePath,
    pub(super) subpath: DocumentPath,
}

impl PathTreeKey {
    /// Parse a `JsonPathStack` ref entry from drift into a typed
    /// `PathTreeKey`.
    ///
    /// Drift returns refs as `<base>/<subpath>/$ref`. For Dropshot (which is
    /// what we're targeting), the base is either
    /// `#/paths/<escaped-path>/<method>` or `#/components/<kind>/<name>` —
    /// three segments once the leading `#/` is stripped.
    ///
    /// Anything not ending in `/$ref` is unexpected, so we fall back to
    /// treating the whole entry as the base with no subpath. (This happens
    /// naturally because a ≤ 3-segment path leaves nothing for the subpath
    /// half after [`DocumentPath::split_at`].)
    ///
    /// `op_ids` is consulted by [`DocumentBasePath::classify`] when the
    /// parsed base shapes up as an endpoint.
    pub(super) fn parse(entry: &str, op_ids: &OperationIdMap<'_>) -> Self {
        /// Number of segments in a Dropshot ref base
        /// (`components/<kind>/<name>` or `paths/<route>/<method>`),
        /// matching the length check in [`DocumentBasePath::classify`].
        const BASE_SEGMENTS: usize = 3;

        let without_ref = entry.strip_suffix("/$ref").unwrap_or(entry);
        let (base, subpath) =
            DocumentPath::parse(without_ref).split_at(BASE_SEGMENTS);
        Self { base: DocumentBasePath::classify(base, op_ids), subpath }
    }
}

/// An inverted reference tree, rooted at the directly-changed component or
/// endpoint.
///
/// Each entry in `children` is one immediate `$ref` source pointing at the
/// root: the key is the location of the ref, and the value is the subtree of
/// further refs pointing at *that* node, walking back along the `$ref` chain
/// all the way out to originating endpoints.
///
/// `BTreeMap` is used so rendering order is sorted rather than the order drift
/// happens to yield paths in.
///
/// For example, the chain "SubType ← Wrapper(.properties.via_a) ←
/// /a(.responses.200…/schema)" produces:
///
/// ```text
/// PathTree {
///     children: {
///         PathTreeKey {
///             base: Component(".components.schemas.Wrapper"),
///             subpath: ".properties.via_a",
///         } => PathTree {
///             children: {
///                 PathTreeKey {
///                     base: Endpoint {
///                         name: ".paths.\"/a\".get",
///                         operation_id: Some("get_a"),
///                     },
///                     subpath: ".responses.200…schema",
///                 } => PathTree { children: {} },
///             },
///         },
///     },
/// }
/// ```
#[derive(Debug, Default)]
pub(super) struct PathTree {
    pub(super) children: BTreeMap<PathTreeKey, PathTree>,
}

impl PathTree {
    /// Insert one `$ref` chain of [`PathTreeKey`]s, in leaf-first order.
    ///
    /// Each entry walks one level deeper into the tree: if a sibling at
    /// the current level already has the same key, reuse it and merge the
    /// rest of the chain into its children. Otherwise, append a new
    /// sibling.
    ///
    /// Two chains sharing only `base` (e.g., the same type referenced
    /// from multiple fields) produce separate sibling entries: each
    /// represents a distinct route from endpoint to changed component.
    pub(super) fn insert(
        &mut self,
        chain: impl IntoIterator<Item = PathTreeKey>,
    ) {
        let mut curr = self;
        for key in chain {
            curr = curr.children.entry(key).or_default();
        }
    }
}

/// A map from endpoint base (`paths/<route>/<method>`) to its
/// `operationId`. Built once per document in [`super::detect::api_compatible`]
/// and consulted during issue construction to populate
/// [`DocumentBasePath::Endpoint::operation_id`].
///
/// The map values are borrowed from the `serde_json::Value` representing the
/// OpenAPI document so that we don't allocate for the (typically large) set of op
/// ids that are never referenced by an issue; the few that are looked up get
/// cloned at the lookup site when constructing the owned `Endpoint` variant.
pub(super) type OperationIdMap<'a> = HashMap<DocumentPath, &'a str>;

/// A location within an OpenAPI document, parsed from a JSON Pointer into
/// its component segments.
///
/// For base locations, code that needs to know whether a base names a component
/// or an endpoint should use [`DocumentBasePath`] instead -- it makes that
/// distinction part of the type, and for endpoints the `operationId` is also
/// stored.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct DocumentPath {
    /// The list of segments in this path.
    ///
    /// * For the root path (`.`), this is empty.
    /// * Otherwise, there's one entry per `/`-separated component.
    ///
    /// Note that the segment is stored in unescaped form. This means:
    ///
    /// * JSON Pointer escapes have been decoded.
    /// * Whether to add quotes is determined in `write`.
    pub(super) segments: Vec<String>,
}

impl DocumentPath {
    /// Parse a JSON Pointer (or relative subpath) into segments.
    ///
    /// Accepts a leading `#` or `/`.
    ///
    /// An empty input is treated as the root path.
    pub(super) fn parse(pointer: &str) -> Self {
        let trimmed = pointer.trim_matches('#').trim_matches('/');
        if trimmed.is_empty() {
            return Self::root();
        }
        let segments =
            trimmed.split('/').map(unescape_pointer_component).collect();
        Self { segments }
    }

    pub(super) fn root() -> Self {
        Self { segments: Vec::new() }
    }

    pub(super) fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// Split into the first `n` segments and the remainder.
    ///
    /// If this path has `n` or fewer segments, the head is the whole path and
    /// the tail is [`Self::root`].
    pub(super) fn split_at(mut self, n: usize) -> (Self, Self) {
        if n >= self.segments.len() {
            return (self, Self::root());
        }
        let tail = Self { segments: self.segments.split_off(n) };
        (self, tail)
    }
}

/// Decode a JSON Pointer component (RFC 6901): `~1` → `/`, `~0` → `~`.
pub(super) fn unescape_pointer_component(component: &str) -> String {
    component.replace("~1", "/").replace("~0", "~")
}

fn to_json_pretty(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(value) => serde_json::to_string_pretty(value)
            .expect("serializing serde_json::Value should always succeed"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shorthand for `DocumentPath { segments: [...] }`.
    fn path(segments: &[&str]) -> DocumentPath {
        DocumentPath {
            segments: segments.iter().map(|&s| s.to_owned()).collect(),
        }
    }

    #[test]
    fn test_path_tree_key_parse() {
        // Each case is (input ref entry, expected base, expected subpath
        // segments).
        let ops = OperationIdMap::new();
        let cases: &[(&str, DocumentBasePath, &[&str])] = &[
            (
                "#/components/schemas/Foo/properties/x/$ref",
                DocumentBasePath::Component(path(&[
                    "components",
                    "schemas",
                    "Foo",
                ])),
                &["properties", "x"],
            ),
            (
                "#/paths/~1users/get/responses/200/content/application~1json/schema/$ref",
                DocumentBasePath::Endpoint {
                    name: path(&["paths", "/users", "get"]),
                    operation_id: None,
                },
                &["responses", "200", "content", "application/json", "schema"],
            ),
            // Without a trailing /$ref (e.g., the leaf of a path stack), the
            // whole entry is the base, and the subpath is the root.
            (
                "#/components/schemas/Foo",
                DocumentBasePath::Component(path(&[
                    "components",
                    "schemas",
                    "Foo",
                ])),
                &[],
            ),
        ];
        for (entry, want_base, want_subpath) in cases {
            let key = PathTreeKey::parse(entry, &ops);
            assert_eq!(&key.base, want_base, "base for entry {entry}");
            assert_eq!(
                key.subpath.segments.as_slice(),
                *want_subpath,
                "subpath for entry {entry}",
            );
        }
    }
}
