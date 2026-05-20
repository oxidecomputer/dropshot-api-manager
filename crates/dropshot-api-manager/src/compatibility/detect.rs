// Copyright 2026 Oxide Computer Company

//! Detect non-trivial differences between two OpenAPI documents.
//!
//! The data types passed in and out (`ApiCompatIssue`, `PathTree`, …) live in
//! [`super::types`]; this module just bridges drift's output into them.

use super::types::{
    ApiCompatIssue, CompatIssueLocation, CompatRenderStatus, DocumentBasePath,
    DocumentPath, OperationIdMap, PathTree, PathTreeKey, SubpathChange,
    unescape_pointer_component,
};
use drift::{Change, ChangeClass, ChangeInfo, ChangePath};

impl ApiCompatIssue {
    fn new(
        blessed_spec: &serde_json::Value,
        generated_spec: &serde_json::Value,
        paths: Vec<ChangePath>,
        change_infos: Vec<ChangeInfo>,
        blessed_op_ids: &OperationIdMap<'_>,
        generated_op_ids: &OperationIdMap<'_>,
    ) -> Self {
        // Every path within a single `Change` shares the same base, so we
        // can take the first one. Drift guarantees `paths` is non-empty.
        let first = paths.first().expect("non-empty paths from drift");
        let (blessed_base_ptr, _) = first.old.base_and_subpath();
        let (generated_base_ptr, _) = first.new.base_and_subpath();

        let blessed_base = DocumentBasePath::classify(
            DocumentPath::parse(blessed_base_ptr),
            blessed_op_ids,
        );
        let generated_base = DocumentBasePath::classify(
            DocumentPath::parse(generated_base_ptr),
            generated_op_ids,
        );

        // For an added/removed endpoint, drift points the missing side at
        // the `.paths` container — fetching its JSON value there would
        // drag in *every* unrelated endpoint and produce a giant
        // uninformative diff. Skip the fetch on the `PathsRoot` side so
        // the diff renders as plain additions (or deletions) of the one
        // endpoint that actually changed.
        let blessed_value = (!blessed_base.is_paths_root())
            .then(|| get_json_value(blessed_base_ptr, blessed_spec))
            .flatten();
        let generated_value = (!generated_base.is_paths_root())
            .then(|| get_json_value(generated_base_ptr, generated_spec))
            .flatten();

        let changes =
            change_infos.into_iter().map(SubpathChange::from_info).collect();

        // Tree refs are blessed-side (see `PathTree::build`), so endpoint
        // leaves resolve their op id in the blessed map.
        let tree = PathTree::build(&paths, blessed_op_ids);

        Self {
            blessed_base,
            generated_base,
            changes,
            tree,
            blessed_value,
            generated_value,
        }
    }
}

/// Tracks compatibility issues to deduplicate them.
///
/// Call [`Self::insert`] once per `(location, issue)` pair, then
/// [`Self::finalize`] to enable lookups. The two-phase design lets anchor
/// numbers be compact: only multi-site entries get a number, and the
/// numbering is contiguous.
#[derive(Debug, Default)]
pub(crate) struct CompatDedupMap<'a> {
    // This is a `Vec` rather than a `HashMap` because `serde_json::Value`
    // doesn't implement `Hash` (floats), and the total entry count per run is
    // expected to be small.
    entries: Vec<RawEntry<'a>>,
}

#[derive(Debug)]
struct RawEntry<'a> {
    issue: &'a ApiCompatIssue,
    first_occurrence: CompatIssueLocation<'a>,
    count: usize,
}

impl<'a> CompatDedupMap<'a> {
    pub(crate) fn insert(
        &mut self,
        location: CompatIssueLocation<'a>,
        issue: &'a ApiCompatIssue,
    ) {
        if let Some(entry) =
            self.entries.iter_mut().find(|e| e.issue.is_same_change_as(issue))
        {
            entry.count += 1;
        } else {
            self.entries.push(RawEntry {
                issue,
                first_occurrence: location,
                count: 1,
            });
        }
    }

    /// Finalize the map and assign 1-indexed anchor numbers to duplicated
    /// entries.
    pub(crate) fn finalize(self) -> FinalizedCompatDedupMap<'a> {
        let mut next_anchor = 1;
        let entries = self
            .entries
            .into_iter()
            .map(|raw| {
                if raw.count > 1 {
                    let anchor = next_anchor;
                    next_anchor += 1;
                    FinalizedEntry::MultiSite {
                        issue: raw.issue,
                        first_occurrence: raw.first_occurrence,
                        anchor,
                    }
                } else {
                    FinalizedEntry::Singleton { issue: raw.issue }
                }
            })
            .collect();
        FinalizedCompatDedupMap { entries }
    }
}

/// Lookup-phase dedup map. Returned by [`CompatDedupMap::finalize`].
#[derive(Debug)]
pub(crate) struct FinalizedCompatDedupMap<'a> {
    entries: Vec<FinalizedEntry<'a>>,
}

#[derive(Debug)]
enum FinalizedEntry<'a> {
    Singleton {
        issue: &'a ApiCompatIssue,
    },
    MultiSite {
        issue: &'a ApiCompatIssue,
        first_occurrence: CompatIssueLocation<'a>,
        anchor: usize,
    },
}

impl<'a> FinalizedEntry<'a> {
    fn issue(&self) -> &'a ApiCompatIssue {
        match self {
            Self::Singleton { issue } | Self::MultiSite { issue, .. } => issue,
        }
    }
}

impl FinalizedCompatDedupMap<'_> {
    /// Returns how `issue` at `current` should be rendered.
    ///
    /// Panics if `issue` was never inserted.
    pub(crate) fn status_for(
        &self,
        issue: &ApiCompatIssue,
        current: CompatIssueLocation<'_>,
    ) -> CompatRenderStatus {
        let entry = self
            .entries
            .iter()
            .find(|e| e.issue().is_same_change_as(issue))
            .expect("every issue passed to status_for was inserted");
        match entry {
            FinalizedEntry::Singleton { .. } => {
                CompatRenderStatus::FirstOccurrence { anchor: None }
            }
            FinalizedEntry::MultiSite { first_occurrence, anchor, .. } => {
                if *first_occurrence == current {
                    CompatRenderStatus::FirstOccurrence {
                        anchor: Some(*anchor),
                    }
                } else {
                    CompatRenderStatus::Duplicate { anchor: *anchor }
                }
            }
        }
    }
}

impl SubpathChange {
    fn from_info(info: ChangeInfo) -> Self {
        Self {
            class: info.class,
            message: info.message,
            old_subpath: DocumentPath::parse(&info.old_subpath),
            new_subpath: DocumentPath::parse(&info.new_subpath),
        }
    }
}

impl PathTree {
    fn build(paths: &[ChangePath], op_ids: &OperationIdMap<'_>) -> Self {
        // For each `ChangePath`, `old.iter()` iterates over the reference stack
        // starting at the leaf (the directly-affected schema), through any
        // `$ref` chains, and terminating at the originating endpoint. For
        // example, a change at `SubType` might really be:
        //
        //     [0] #/components/schemas/SubType                 <- leaf
        //     [1] #/components/schemas/Wrapper/.../$ref        <- ref source
        //     [2] #/paths/~1hello/get/.../$ref                 <- endpoint
        //
        // The first entry [0] is the changed schema itself. This is identical
        // across every path in this `Change` and already shown in the issue
        // header above the path tree, so we skip over that. We do need to show
        // the remaining entries, though.
        //
        // We read the old (blessed) side rather than the new (generated) side
        // because in case of renames, both sides have the same chain shape, but
        // only the blessed names match the `blessed_base` in the header.
        let mut tree = PathTree::default();
        for path in paths {
            let ref_chain = path
                .old
                .iter()
                .skip(1)
                .map(|entry| PathTreeKey::parse(entry, op_ids));
            tree.insert(ref_chain);
        }
        tree
    }
}

/// Walk `doc.paths.<route>.<method>` and collect each operation's
/// `operationId` into a map keyed by its `paths/<route>/<method>` base.
///
/// Endpoints without an `operationId` (or with a non-string value) are simply
/// omitted. We consider that to be okay because the operation ID isn't
/// load-bearing internally and is just used for user-friendly output.
fn extract_operation_ids(doc: &serde_json::Value) -> OperationIdMap<'_> {
    let mut out = OperationIdMap::new();
    let Some(paths) = doc.pointer("/paths").and_then(|v| v.as_object()) else {
        return out;
    };
    for (route, item) in paths {
        let Some(item) = item.as_object() else { continue };
        for (method, op) in item {
            let Some(op) = op.as_object() else { continue };
            let Some(op_id) = op.get("operationId").and_then(|v| v.as_str())
            else {
                continue;
            };
            let base = DocumentPath {
                segments: vec![
                    "paths".to_string(),
                    route.clone(),
                    method.clone(),
                ],
            };
            out.insert(base, op_id);
        }
    }
    out
}

fn get_json_value(
    pointer: &str,
    spec: &serde_json::Value,
) -> Option<serde_json::Value> {
    // serde_json's JSON Pointer implementation does not accept
    // leading `#`, so strip that.
    let pointer = pointer.trim_start_matches('#');

    spec.pointer(pointer).map(|v| {
        // Add a map around the value, with the key being the last
        // component of the pointer.
        let last_component = pointer.split('/').next_back().unwrap_or("");
        surround_with_map(last_component, v)
    })
}

fn surround_with_map(
    last_component: &str,
    value: &serde_json::Value,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(unescape_pointer_component(last_component), value.clone());
    serde_json::Value::Object(map)
}

/// Escape a string for use as a JSON Pointer component (RFC 6901).
fn escape_json_pointer(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// Normalize old-format websocket responses in the blessed spec to match the
/// new format used by the generated spec.
///
/// Dropshot 0.17 changed how websocket endpoints are represented in OpenAPI
/// (see https://github.com/oxidecomputer/dropshot/pull/1554):
///
/// Old format (0.16 and earlier):
/// ```json
/// "responses": {
///     "default": { "description": "", "content": { "*/*": { "schema": {} } } }
/// }
/// ```
///
/// New format (0.17):
/// ```json
/// "responses": {
///     "101": { "description": "Negotiating protocol upgrade ..." },
///     "4XX": { "$ref": "#/components/responses/Error" },
///     "5XX": { "$ref": "#/components/responses/Error" }
/// }
/// ```
///
/// This function detects operations with the `x-dropshot-websocket` extension
/// that still have the old response format and replaces their responses with
/// those from the corresponding operation in the generated spec. This is safe
/// because the wire format did not change — only the OpenAPI representation
/// did.
fn normalize_old_websocket_responses(
    blessed: &mut serde_json::Value,
    generated: &serde_json::Value,
) {
    // Exact JSON for the old and new websocket response formats. We only
    // normalize when both sides match exactly, to avoid accidentally
    // papering over a real incompatibility.
    let old_ws_responses = serde_json::json!({
        "default": {
            "description": "",
            "content": {
                "*/*": { "schema": {} }
            }
        }
    });
    let new_ws_responses = serde_json::json!({
        "101": {
            "description":
                "Negotiating protocol upgrade from HTTP/1.1 to WebSocket"
        },
        "4XX": {
            "$ref": "#/components/responses/Error"
        },
        "5XX": {
            "$ref": "#/components/responses/Error"
        }
    });

    let Some(blessed_paths) =
        blessed.pointer_mut("/paths").and_then(|v| v.as_object_mut())
    else {
        return;
    };

    for (path, item) in blessed_paths.iter_mut() {
        let Some(item) = item.as_object_mut() else { continue };
        for (method, operation) in item.iter_mut() {
            let Some(op) = operation.as_object_mut() else {
                continue;
            };
            if !op.contains_key("x-dropshot-websocket") {
                continue;
            }
            if op.get("responses") != Some(&old_ws_responses) {
                continue;
            }
            let Some(gen_op) = generated
                .pointer(&format!(
                    "/paths/{}/{}",
                    escape_json_pointer(path),
                    method,
                ))
                .and_then(|v| v.as_object())
            else {
                continue;
            };
            if !gen_op.contains_key("x-dropshot-websocket") {
                continue;
            }
            if gen_op.get("responses") != Some(&new_ws_responses) {
                continue;
            }
            op.insert("responses".to_string(), new_ws_responses.clone());
        }
    }
}

pub(crate) fn api_compatible(
    blessed: &serde_json::Value,
    generated: &serde_json::Value,
) -> anyhow::Result<Vec<ApiCompatIssue>> {
    let mut blessed = blessed.clone();

    // Normalize old-format websocket responses in the blessed spec before
    // comparison. Dropshot 0.17 changed how websocket endpoints are
    // represented: from a `default` response with `*/*` content to explicit
    // `101`/`4XX`/`5XX` responses. This is purely a spec-generation change,
    // not a wire-format change.
    normalize_old_websocket_responses(&mut blessed, generated);

    // Build the per-spec op-id maps once. Each issue consults them through
    // `DocumentBasePath::classify` to populate the `operation_id` field on
    // its endpoint variants.
    let blessed_op_ids = extract_operation_ids(&blessed);
    let generated_op_ids = extract_operation_ids(generated);

    let changes = drift::compare(&blessed, generated)?;
    let mut issues = Vec::new();
    for Change { paths, changes: change_infos } in changes {
        // Filter out trivial changes; if nothing non-trivial remains, skip
        // the issue entirely.
        let non_trivial: Vec<_> = change_infos
            .into_iter()
            .filter(|c| match c.class {
                ChangeClass::BackwardIncompatible
                | ChangeClass::ForwardIncompatible
                | ChangeClass::Incompatible
                | ChangeClass::Unhandled => true,
                ChangeClass::Trivial => false,
            })
            .collect();
        if non_trivial.is_empty() {
            continue;
        }
        issues.push(ApiCompatIssue::new(
            &blessed,
            generated,
            paths,
            non_trivial,
            &blessed_op_ids,
            &generated_op_ids,
        ));
    }
    // Sort by base to ensure a deterministic iteration order, independent of
    // whatever drift returns. The JSON values are derived from the base, and
    // `serde_json::Value` isn't `Ord`, so we don't include them in the key.
    issues.sort_by(|a, b| {
        (&a.blessed_base, &a.generated_base)
            .cmp(&(&b.blessed_base, &b.generated_base))
    });
    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropshot_api_manager_types::ApiIdent;
    use std::collections::BTreeSet;

    #[test]
    fn test_normalize_old_websocket_responses() {
        // Old format: default response with */* schema.
        let mut blessed = serde_json::json!({
            "paths": {
                "/subscribe": {
                    "get": {
                        "operationId": "subscribe",
                        "responses": {
                            "default": {
                                "description": "",
                                "content": {
                                    "*/*": { "schema": {} }
                                }
                            }
                        },
                        "x-dropshot-websocket": {}
                    }
                },
                "/health": {
                    "get": {
                        "operationId": "health_check",
                        "responses": {
                            "200": {
                                "description": "OK"
                            }
                        }
                    }
                }
            }
        });

        // New format: 101/4XX/5XX responses.
        let generated = serde_json::json!({
            "paths": {
                "/subscribe": {
                    "get": {
                        "operationId": "subscribe",
                        "responses": {
                            "101": {
                                "description": "Negotiating protocol upgrade from HTTP/1.1 to WebSocket"
                            },
                            "4XX": {
                                "$ref": "#/components/responses/Error"
                            },
                            "5XX": {
                                "$ref": "#/components/responses/Error"
                            }
                        },
                        "x-dropshot-websocket": {}
                    }
                },
                "/health": {
                    "get": {
                        "operationId": "health_check",
                        "responses": {
                            "200": {
                                "description": "OK"
                            }
                        }
                    }
                }
            }
        });

        let original_blessed = blessed.clone();
        normalize_old_websocket_responses(&mut blessed, &generated);

        // The websocket operation should have been updated.
        assert_eq!(
            blessed.pointer("/paths/~1subscribe/get/responses"),
            generated.pointer("/paths/~1subscribe/get/responses"),
            "websocket responses should be normalized to new format",
        );

        // The non-websocket operation should be unchanged.
        assert_eq!(
            blessed.pointer("/paths/~1health/get/responses"),
            original_blessed.pointer("/paths/~1health/get/responses"),
            "non-websocket responses should not be modified",
        );
    }

    #[test]
    fn test_normalize_already_new_format_is_noop() {
        // Both blessed and generated have the new format — normalization
        // should be a no-op.
        let mut spec = serde_json::json!({
            "paths": {
                "/subscribe": {
                    "get": {
                        "operationId": "subscribe",
                        "responses": {
                            "101": {
                                "description": "Negotiating protocol upgrade"
                            },
                            "4XX": { "$ref": "#/components/responses/Error" },
                            "5XX": { "$ref": "#/components/responses/Error" }
                        },
                        "x-dropshot-websocket": {}
                    }
                }
            }
        });

        let original = spec.clone();
        normalize_old_websocket_responses(&mut spec, &original);
        assert_eq!(spec, original);
    }

    #[test]
    fn test_normalize_no_websocket_endpoints_is_noop() {
        let mut spec = serde_json::json!({
            "paths": {
                "/health": {
                    "get": {
                        "operationId": "health",
                        "responses": { "200": { "description": "OK" } }
                    }
                }
            }
        });

        let original = spec.clone();
        normalize_old_websocket_responses(&mut spec, &original);
        assert_eq!(spec, original);
    }

    #[test]
    fn test_normalize_missing_generated_path_leaves_blessed_unchanged() {
        // If the generated spec doesn't have the websocket path, the blessed
        // spec should be left unchanged.
        let mut blessed = serde_json::json!({
            "paths": {
                "/subscribe": {
                    "get": {
                        "operationId": "subscribe",
                        "responses": {
                            "default": {
                                "description": "",
                                "content": { "*/*": { "schema": {} } }
                            }
                        },
                        "x-dropshot-websocket": {}
                    }
                }
            }
        });

        let generated = serde_json::json!({
            "paths": {}
        });

        let original = blessed.clone();
        normalize_old_websocket_responses(&mut blessed, &generated);
        assert_eq!(blessed, original);
    }

    #[test]
    fn test_api_compatible_old_ws_format() {
        // Old-format blessed spec should be compatible with new-format
        // generated spec after normalization.
        let blessed = serde_json::json!({
            "openapi": "3.0.3",
            "info": { "title": "Test", "version": "1.0.0" },
            "paths": {
                "/subscribe": {
                    "get": {
                        "operationId": "subscribe",
                        "responses": {
                            "default": {
                                "description": "",
                                "content": {
                                    "*/*": { "schema": {} }
                                }
                            }
                        },
                        "x-dropshot-websocket": {}
                    }
                }
            },
            "components": {
                "responses": {
                    "Error": {
                        "description": "Error",
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/Error"
                                }
                            }
                        }
                    }
                },
                "schemas": {
                    "Error": {
                        "description": "Error information from a response.",
                        "type": "object",
                        "properties": {
                            "message": { "type": "string" },
                            "request_id": { "type": "string" }
                        },
                        "required": ["message", "request_id"]
                    }
                }
            }
        });

        let generated = serde_json::json!({
            "openapi": "3.0.3",
            "info": { "title": "Test", "version": "1.0.0" },
            "paths": {
                "/subscribe": {
                    "get": {
                        "operationId": "subscribe",
                        "responses": {
                            "101": {
                                "description": "Negotiating protocol upgrade from HTTP/1.1 to WebSocket"
                            },
                            "4XX": {
                                "$ref": "#/components/responses/Error"
                            },
                            "5XX": {
                                "$ref": "#/components/responses/Error"
                            }
                        },
                        "x-dropshot-websocket": {}
                    }
                }
            },
            "components": {
                "responses": {
                    "Error": {
                        "description": "Error",
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/Error"
                                }
                            }
                        }
                    }
                },
                "schemas": {
                    "Error": {
                        "description": "Error information from a response.",
                        "type": "object",
                        "properties": {
                            "message": { "type": "string" },
                            "request_id": { "type": "string" }
                        },
                        "required": ["message", "request_id"]
                    }
                }
            }
        });

        let issues = api_compatible(&blessed, &generated).unwrap();
        assert!(
            issues.is_empty(),
            "old ws format should be compatible after normalization, \
             but got: {issues:?}",
        );
    }

    #[test]
    fn test_normalize_ws_to_http_still_detects_incompatibility() {
        // If a blessed websocket endpoint is replaced by a normal HTTP
        // endpoint at the same path/method, normalization should not
        // mask the change. The normalizer copies responses from the
        // generated operation, but drift still detects the removal of
        // the `default` response (the generated operation lacks one).
        let blessed = serde_json::json!({
            "openapi": "3.0.3",
            "info": { "title": "Test", "version": "1.0.0" },
            "paths": {
                "/subscribe": {
                    "get": {
                        "operationId": "subscribe",
                        "responses": {
                            "default": {
                                "description": "",
                                "content": {
                                    "*/*": { "schema": {} }
                                }
                            }
                        },
                        "x-dropshot-websocket": {}
                    }
                }
            },
            "components": {
                "responses": {
                    "Error": {
                        "description": "Error",
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/Error"
                                }
                            }
                        }
                    }
                },
                "schemas": {
                    "Error": {
                        "description": "Error information from a response.",
                        "type": "object",
                        "properties": {
                            "message": { "type": "string" },
                            "request_id": { "type": "string" }
                        },
                        "required": ["message", "request_id"]
                    }
                }
            }
        });

        // Same path/method, but now a normal HTTP endpoint (no
        // x-dropshot-websocket), with a regular 200 response.
        let generated = serde_json::json!({
            "openapi": "3.0.3",
            "info": { "title": "Test", "version": "1.0.0" },
            "paths": {
                "/subscribe": {
                    "get": {
                        "operationId": "subscribe",
                        "responses": {
                            "200": {
                                "description": "OK",
                                "content": {
                                    "application/json": {
                                        "schema": { "type": "string" }
                                    }
                                }
                            },
                            "4XX": {
                                "$ref": "#/components/responses/Error"
                            },
                            "5XX": {
                                "$ref": "#/components/responses/Error"
                            }
                        }
                    }
                }
            },
            "components": {
                "responses": {
                    "Error": {
                        "description": "Error",
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/Error"
                                }
                            }
                        }
                    }
                },
                "schemas": {
                    "Error": {
                        "description": "Error information from a response.",
                        "type": "object",
                        "properties": {
                            "message": { "type": "string" },
                            "request_id": { "type": "string" }
                        },
                        "required": ["message", "request_id"]
                    }
                }
            }
        });

        let issues = api_compatible(&blessed, &generated).unwrap();
        assert!(
            !issues.is_empty(),
            "websocket-to-HTTP change should be detected as incompatible",
        );
    }

    /// Shorthand for a component-shaped [`DocumentBasePath`] from a JSON
    /// Pointer string.
    fn component_base(p: &str) -> DocumentBasePath {
        DocumentBasePath::Component(DocumentPath::parse(p))
    }

    /// Build an `ApiCompatIssue` directly.
    fn synthetic_issue(
        base: &str,
        message: &str,
        blessed_value: serde_json::Value,
        generated_value: serde_json::Value,
    ) -> ApiCompatIssue {
        ApiCompatIssue {
            blessed_base: component_base(base),
            generated_base: component_base(base),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::Incompatible,
                message: message.into(),
                old_subpath: DocumentPath::parse("properties/value"),
                new_subpath: DocumentPath::parse("properties/value"),
            }]),
            // Leave this empty since it's ignored by the dedup logic.
            tree: PathTree::default(),
            blessed_value: Some(blessed_value),
            generated_value: Some(generated_value),
        }
    }

    /// Owns the data a [`CompatIssueLocation`] borrows from, so a test can
    /// hold it alive while passing the location around.
    struct OwnedLoc {
        api: ApiIdent,
        version: semver::Version,
    }

    impl OwnedLoc {
        fn new(api: &str, version: &str) -> Self {
            Self {
                api: ApiIdent::from(api.to_string()),
                version: version.parse().unwrap(),
            }
        }

        fn as_loc(&self) -> CompatIssueLocation<'_> {
            CompatIssueLocation { api: &self.api, version: &self.version }
        }
    }

    #[track_caller]
    fn assert_status(
        dedup: &FinalizedCompatDedupMap<'_>,
        issue: &ApiCompatIssue,
        current: CompatIssueLocation<'_>,
        expected: CompatRenderStatus,
    ) {
        let actual = dedup.status_for(issue, current);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_dedup_basic() {
        let issue_a = synthetic_issue(
            "#/components/schemas/Error",
            "schema types changed",
            serde_json::json!({"Error": {"type": "string"}}),
            serde_json::json!({"Error": {"type": "integer"}}),
        );
        let issue_b = synthetic_issue(
            "#/components/schemas/Error",
            "schema types changed",
            serde_json::json!({"Error": {"type": "string"}}),
            serde_json::json!({"Error": {"type": "integer"}}),
        );

        let foo = OwnedLoc::new("foo", "1.0.0");
        let bar = OwnedLoc::new("bar", "1.0.0");
        let mut dedup = CompatDedupMap::default();
        dedup.insert(foo.as_loc(), &issue_a);
        dedup.insert(bar.as_loc(), &issue_b);
        let dedup = dedup.finalize();

        assert_status(
            &dedup,
            &issue_a,
            foo.as_loc(),
            CompatRenderStatus::FirstOccurrence { anchor: Some(1) },
        );
        assert_status(
            &dedup,
            &issue_b,
            bar.as_loc(),
            CompatRenderStatus::Duplicate { anchor: 1 },
        );
    }

    /// Two issues with the same name and message but different underlying
    /// values are not duplicates: an `Error` schema in one API may be a
    /// completely different type from `Error` in another.
    #[test]
    fn test_dedup_distinguishes_by_value() {
        let issue_a = synthetic_issue(
            "#/components/schemas/Error",
            "schema types changed",
            serde_json::json!({"Error": {"type": "string"}}),
            serde_json::json!({"Error": {"type": "integer"}}),
        );
        // Same name, same change message, different concrete schema.
        let issue_b = synthetic_issue(
            "#/components/schemas/Error",
            "schema types changed",
            serde_json::json!({"Error": {"type": "object"}}),
            serde_json::json!({"Error": {"type": "array"}}),
        );

        let foo = OwnedLoc::new("foo", "1.0.0");
        let bar = OwnedLoc::new("bar", "1.0.0");
        let mut dedup = CompatDedupMap::default();
        dedup.insert(foo.as_loc(), &issue_a);
        dedup.insert(bar.as_loc(), &issue_b);
        let dedup = dedup.finalize();

        // Both issues are reported by exactly one (api, version), so finalize
        // didn't assign them anchors.
        assert_status(
            &dedup,
            &issue_a,
            foo.as_loc(),
            CompatRenderStatus::FirstOccurrence { anchor: None },
        );
        assert_status(
            &dedup,
            &issue_b,
            bar.as_loc(),
            CompatRenderStatus::FirstOccurrence { anchor: None },
        );
    }

    /// The same issue reported under multiple versions of the same API
    /// dedups the second version, not the first.
    #[test]
    fn test_dedup_across_versions_of_same_api() {
        let issue = synthetic_issue(
            "#/components/schemas/Error",
            "schema types changed",
            serde_json::json!({"Error": {"type": "string"}}),
            serde_json::json!({"Error": {"type": "integer"}}),
        );

        let v1 = OwnedLoc::new("foo", "1.0.0");
        let v2 = OwnedLoc::new("foo", "2.0.0");
        let mut dedup = CompatDedupMap::default();
        dedup.insert(v1.as_loc(), &issue);
        dedup.insert(v2.as_loc(), &issue);
        let dedup = dedup.finalize();

        assert_status(
            &dedup,
            &issue,
            v1.as_loc(),
            CompatRenderStatus::FirstOccurrence { anchor: Some(1) },
        );
        assert_status(
            &dedup,
            &issue,
            v2.as_loc(),
            CompatRenderStatus::Duplicate { anchor: 1 },
        );
    }

    #[test]
    fn test_dedup_asymmetric_versions_across_apis() {
        let issue = synthetic_issue(
            "#/components/schemas/Error",
            "schema types changed",
            serde_json::json!({"Error": {"type": "string"}}),
            serde_json::json!({"Error": {"type": "integer"}}),
        );

        let a_v1 = OwnedLoc::new("api_a", "1.0.0");
        let a_v2 = OwnedLoc::new("api_a", "2.0.0");
        let a_v3 = OwnedLoc::new("api_a", "3.0.0");
        let b_v2 = OwnedLoc::new("api_b", "2.0.0");

        let mut dedup = CompatDedupMap::default();
        dedup.insert(a_v1.as_loc(), &issue);
        dedup.insert(a_v2.as_loc(), &issue);
        dedup.insert(a_v3.as_loc(), &issue);
        dedup.insert(b_v2.as_loc(), &issue);
        let dedup = dedup.finalize();

        // a@v1 is the canonical occurrence; everyone else is a duplicate
        // pointing at anchor 1.
        assert_status(
            &dedup,
            &issue,
            a_v1.as_loc(),
            CompatRenderStatus::FirstOccurrence { anchor: Some(1) },
        );
        for loc in [&a_v2, &a_v3, &b_v2] {
            assert_status(
                &dedup,
                &issue,
                loc.as_loc(),
                CompatRenderStatus::Duplicate { anchor: 1 },
            );
        }
    }

    #[test]
    fn test_dedup_ignores_tree() {
        let ops = OperationIdMap::new();
        let mut tree_a = PathTree::default();
        tree_a.insert([PathTreeKey::parse(
            "#/components/schemas/Wrapper/properties/a/$ref",
            &ops,
        )]);
        let mut tree_b = PathTree::default();
        tree_b.insert([PathTreeKey::parse(
            "#/components/schemas/Wrapper/properties/b/$ref",
            &ops,
        )]);

        let make_issue = |tree: PathTree| ApiCompatIssue {
            blessed_base: component_base("#/components/schemas/SubType"),
            generated_base: component_base("#/components/schemas/SubType"),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::Incompatible,
                message: "schema types changed".into(),
                old_subpath: DocumentPath::parse("properties/value"),
                new_subpath: DocumentPath::parse("properties/value"),
            }]),
            tree,
            blessed_value: Some(
                serde_json::json!({"SubType": {"type": "string"}}),
            ),
            generated_value: Some(
                serde_json::json!({"SubType": {"type": "integer"}}),
            ),
        };

        let issue_a = make_issue(tree_a);
        let issue_b = make_issue(tree_b);

        assert!(
            issue_a.is_same_change_as(&issue_b),
            "issues identical except for tree should dedup",
        );
    }

    #[test]
    fn test_anchor_numbering_skips_single_occurrence() {
        let multi_a = synthetic_issue(
            "#/components/schemas/MultiA",
            "schema types changed",
            serde_json::json!({"MultiA": {"type": "string"}}),
            serde_json::json!({"MultiA": {"type": "integer"}}),
        );
        let solo = synthetic_issue(
            "#/components/schemas/Solo",
            "schema types changed",
            serde_json::json!({"Solo": {"type": "string"}}),
            serde_json::json!({"Solo": {"type": "integer"}}),
        );
        let multi_b = synthetic_issue(
            "#/components/schemas/MultiB",
            "schema types changed",
            serde_json::json!({"MultiB": {"type": "string"}}),
            serde_json::json!({"MultiB": {"type": "integer"}}),
        );

        let foo = OwnedLoc::new("foo", "1.0.0");
        let bar = OwnedLoc::new("bar", "1.0.0");
        let mut dedup = CompatDedupMap::default();
        // Insert order: multi_a (twice), solo (once), multi_b (twice). Solo
        // sits between the multis in insert order, so a non-compact scheme
        // would assign it anchor 2 and skip to 3 for multi_b.
        dedup.insert(foo.as_loc(), &multi_a);
        dedup.insert(bar.as_loc(), &multi_a);
        dedup.insert(foo.as_loc(), &solo);
        dedup.insert(foo.as_loc(), &multi_b);
        dedup.insert(bar.as_loc(), &multi_b);
        let dedup = dedup.finalize();

        assert_status(
            &dedup,
            &multi_a,
            foo.as_loc(),
            CompatRenderStatus::FirstOccurrence { anchor: Some(1) },
        );
        assert_status(
            &dedup,
            &solo,
            foo.as_loc(),
            CompatRenderStatus::FirstOccurrence { anchor: None },
        );
        assert_status(
            &dedup,
            &multi_b,
            foo.as_loc(),
            // multi_b must be anchor 2, not 3 — the solo issue between the
            // two multis takes no slot in the visible numbering.
            CompatRenderStatus::FirstOccurrence { anchor: Some(2) },
        );
    }

    /// Two issues at the same base with the same change set but reported in
    /// different orders should dedup. The `changes` field is a `BTreeSet`,
    /// so the comparison is order-independent regardless of what order
    /// drift happened to emit the inner changes in.
    #[test]
    fn test_dedup_change_order_independent() {
        fn make_change(message: &str, subpath: &str) -> SubpathChange {
            SubpathChange {
                class: ChangeClass::Incompatible,
                message: message.into(),
                old_subpath: DocumentPath::parse(subpath),
                new_subpath: DocumentPath::parse(subpath),
            }
        }

        fn issue_with_changes(
            changes: impl IntoIterator<Item = SubpathChange>,
        ) -> ApiCompatIssue {
            ApiCompatIssue {
                blessed_base: component_base("#/components/schemas/User"),
                generated_base: component_base("#/components/schemas/User"),
                changes: changes.into_iter().collect(),
                tree: PathTree::default(),
                blessed_value: Some(serde_json::json!({"User": {}})),
                generated_value: Some(serde_json::json!({"User": {}})),
            }
        }

        let a_changes = [
            make_change("a changed", "properties/a"),
            make_change("b changed", "properties/b"),
        ];
        let b_changes = [
            make_change("b changed", "properties/b"),
            make_change("a changed", "properties/a"),
        ];

        let issue_a = issue_with_changes(a_changes);
        let issue_b = issue_with_changes(b_changes);

        assert!(
            issue_a.is_same_change_as(&issue_b),
            "issues with same change set in different order should dedup",
        );

        let foo = OwnedLoc::new("foo", "1.0.0");
        let bar = OwnedLoc::new("bar", "1.0.0");
        let mut dedup = CompatDedupMap::default();
        dedup.insert(foo.as_loc(), &issue_a);
        dedup.insert(bar.as_loc(), &issue_b);
        let dedup = dedup.finalize();
        assert_status(
            &dedup,
            &issue_b,
            bar.as_loc(),
            CompatRenderStatus::Duplicate { anchor: 1 },
        );
    }

    #[test]
    #[should_panic(expected = "every issue passed to status_for was inserted")]
    fn test_status_for_panics_on_uninserted() {
        let issue = synthetic_issue(
            "#/components/schemas/Error",
            "schema types changed",
            serde_json::json!({"Error": {"type": "string"}}),
            serde_json::json!({"Error": {"type": "integer"}}),
        );
        let foo = OwnedLoc::new("foo", "1.0.0");
        let dedup = CompatDedupMap::default().finalize();
        let _ = dedup.status_for(&issue, foo.as_loc());
    }
}
