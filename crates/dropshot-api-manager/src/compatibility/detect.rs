// Copyright 2026 Oxide Computer Company

//! Detect non-trivial differences between two OpenAPI documents.
//!
//! The data types passed in and out (`ApiCompatIssue`, `PathTree`, …) live in
//! [`super::types`]; this module just bridges drift's output into them.

use super::types::{
    ApiCompatIssue, DocumentBasePath, DocumentPath, OperationIdMap, PathTree,
    PathTreeKey, SubpathChange, unescape_pointer_component,
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

pub fn api_compatible(
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
    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
