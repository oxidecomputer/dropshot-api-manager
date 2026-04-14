// Copyright 2026 Oxide Computer Company

//! Determine if one OpenAPI document is a subset of another

use drift::{Change, ChangeClass};
use std::{collections::BTreeMap, fmt};

/// A compatibility error between two OpenAPI documents, indexed by the blessed
/// and generated paths.
#[derive(Debug)]
pub struct ApiCompatIssue {
    // Blessed and generated pointers in JSON Pointer format (e.g.
    // "#/paths/~1thing3/get")
    blessed_pointer: String,
    generated_pointer: String,
    data: CompatIssueData,
}

impl ApiCompatIssue {
    fn best_pointer(&self) -> ApiCompatPointer<'_> {
        ApiCompatPointer::best_pointer(
            &self.blessed_pointer,
            &self.generated_pointer,
        )
    }

    pub(crate) fn blessed_json(&self) -> String {
        to_json_pretty(self.data.blessed_value.as_ref())
    }

    pub(crate) fn generated_json(&self) -> String {
        to_json_pretty(self.data.generated_value.as_ref())
    }
}

impl fmt::Display for ApiCompatIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.best_pointer() {
            ApiCompatPointer::Same(p)
            | ApiCompatPointer::Blessed(p)
            | ApiCompatPointer::Generated(p) => {
                write!(f, "at {}:", json_pointer_to_jq(p))?;
            }
            ApiCompatPointer::Rename { blessed_pointer, generated_pointer } => {
                write!(
                    f,
                    "at {} -> {}:",
                    json_pointer_to_jq(blessed_pointer),
                    json_pointer_to_jq(generated_pointer),
                )?;
            }
        }

        if self.data.changes.len() == 1 {
            let Change {
                message,
                old_path: _,
                new_path: _,
                comparison: _,
                class,
                details: _,
            } = &self.data.changes[0];
            write!(f, " {}change: {}", change_class_str(class), message)?;
        } else {
            writeln!(f)?;
            for error in &self.data.changes {
                let Change {
                    message,
                    old_path: _,
                    new_path: _,
                    comparison: _,
                    class,
                    details: _,
                } = error;
                writeln!(
                    f,
                    "- {}change: {}",
                    change_class_str(class),
                    message
                )?;
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
struct CompatIssueData {
    blessed_value: Option<serde_json::Value>,
    generated_value: Option<serde_json::Value>,
    changes: Vec<Change>,
}

impl CompatIssueData {
    fn new(
        blessed_spec: &serde_json::Value,
        blessed_pointer: &str,
        generated_spec: &serde_json::Value,
        generated_pointer: &str,
    ) -> Self {
        let (blessed_value, generated_value) =
            match ApiCompatPointer::best_pointer(
                blessed_pointer,
                generated_pointer,
            ) {
                ApiCompatPointer::Same(pointer) => (
                    get_json_value(pointer, blessed_spec),
                    get_json_value(pointer, generated_spec),
                ),
                ApiCompatPointer::Blessed(pointer) => {
                    // If the blessed version is the best, then it means that
                    // the generated version isn't relevant to this
                    // determination (typically because the generated version
                    // removed a path or schema that the blessed version has).
                    // In that case, store only the blessed value.
                    (get_json_value(pointer, blessed_spec), None)
                }
                ApiCompatPointer::Generated(pointer) => {
                    // Same logic as above, but for the generated version.
                    (None, get_json_value(pointer, generated_spec))
                }
                ApiCompatPointer::Rename {
                    blessed_pointer,
                    generated_pointer,
                } => (
                    get_json_value(blessed_pointer, blessed_spec),
                    get_json_value(generated_pointer, generated_spec),
                ),
            };

        Self { blessed_value, generated_value, changes: Vec::new() }
    }
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

fn to_json_pretty(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(value) => serde_json::to_string_pretty(value)
            .expect("serializing serde_json::Value should always succeed"),
        None => String::new(),
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ApiCompatPointer<'a> {
    Same(&'a str),
    Blessed(&'a str),
    Generated(&'a str),
    Rename { blessed_pointer: &'a str, generated_pointer: &'a str },
}

impl<'a> ApiCompatPointer<'a> {
    fn best_pointer(
        blessed_pointer: &'a str,
        generated_pointer: &'a str,
    ) -> Self {
        if blessed_pointer == generated_pointer {
            return ApiCompatPointer::Same(blessed_pointer);
        }

        // If one of the pointers (transformed into a prefix-free string by
        // adding a trailing `/`) is a parent of the other, return the child.
        if let Some(suffix) = blessed_pointer.strip_prefix(generated_pointer)
            && suffix.starts_with('/')
        {
            return ApiCompatPointer::Blessed(blessed_pointer);
        }
        if let Some(suffix) = generated_pointer.strip_prefix(blessed_pointer)
            && suffix.starts_with('/')
        {
            return ApiCompatPointer::Generated(generated_pointer);
        }

        // Neither pointer is a parent of the other, so we need to treat this as
        // a rename.
        ApiCompatPointer::Rename { blessed_pointer, generated_pointer }
    }
}

fn json_pointer_to_jq(pointer: &str) -> String {
    let mut out = String::new();
    // Strip the leading `#` and/or `/`.
    let pointer = pointer.trim_matches('#').trim_matches('/');

    // For each component (split by slash):
    for component in pointer.split('/') {
        // Produce a leading `.`.
        out.push('.');

        // If there are any escapes (~s), then unescape and quote the string.
        if component.contains('~') {
            let component = unescape_pointer_component(component);
            out.push('"');
            out.push_str(&component);
            out.push('"');
        } else {
            out.push_str(component);
        }
    }

    out
}

fn unescape_pointer_component(component: &str) -> String {
    component.replace("~1", "/").replace("~0", "~")
}

/// Escape a string for use as a JSON Pointer component (RFC 6901).
fn escape_json_pointer(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// Normalize old-format websocket responses in the blessed spec to match the
/// new format used by the generated spec.
///
/// Dropshot 0.17 changed how websocket endpoints are represented in OpenAPI
/// (see oxidecomputer/dropshot#1554):
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
    let Some(blessed_paths) =
        blessed.pointer_mut("/paths").and_then(|v| v.as_object_mut())
    else {
        return;
    };

    // Collect the paths and methods that need updating. We need a two-pass
    // approach because we can't borrow generated while mutating blessed.
    let updates: Vec<(String, String, serde_json::Value)> = blessed_paths
        .iter()
        .flat_map(|(path, item)| {
            let item = item.as_object()?;
            Some(item.iter().filter_map(move |(method, operation)| {
                let op = operation.as_object()?;
                // Only process websocket operations.
                if !op.contains_key("x-dropshot-websocket") {
                    return None;
                }
                // Only process old-format responses (has "default" key).
                let responses = op.get("responses")?.as_object()?;
                if !responses.contains_key("default") {
                    return None;
                }
                // Look up the corresponding operation in the generated spec
                // and grab its responses.
                let gen_responses = generated
                    .pointer(&format!(
                        "/paths/{}/{}",
                        escape_json_pointer(path),
                        method
                    ))?
                    .as_object()?
                    .get("responses")?
                    .clone();
                Some((path.clone(), method.clone(), gen_responses))
            }))
        })
        .flatten()
        .collect();

    // Apply the updates.
    for (path, method, new_responses) in updates {
        if let Some(op) = blessed
            .pointer_mut(&format!(
                "/paths/{}/{}",
                escape_json_pointer(&path),
                &method
            ))
            .and_then(|v| v.as_object_mut())
        {
            op.insert("responses".to_string(), new_responses);
        }
    }
}

pub fn api_compatible(
    blessed: &serde_json::Value,
    generated: &serde_json::Value,
) -> anyhow::Result<Vec<ApiCompatIssue>> {
    // Normalize old-format websocket responses in the blessed spec before
    // comparison. Dropshot 0.17 changed how websocket endpoints are
    // represented: from a `default` response with `*/*` content to explicit
    // `101`/`4XX`/`5XX` responses. This is purely a spec-generation change,
    // not a wire-format change.
    let generated = generated.clone();
    let changes =
        drift::compare_with_normalizer(blessed, &generated, |blessed| {
            normalize_old_websocket_responses(blessed, &generated)
        })?;
    let changes = changes
        .into_iter()
        .filter_map(|change| match change.class {
            ChangeClass::BackwardIncompatible
            | ChangeClass::ForwardIncompatible
            | ChangeClass::Incompatible
            | ChangeClass::Unhandled => Some(change),
            ChangeClass::Trivial => None,
        })
        .fold(
            // BTreeMap of (blessed_pointer, generated_pointer) => data
            BTreeMap::<(String, String), CompatIssueData>::new(),
            |mut acc, change| {
                let blessed_pointer = change.old_path.iter().next().unwrap();
                let generated_pointer = change.new_path.iter().next().unwrap();
                acc.entry((
                    blessed_pointer.to_owned(),
                    generated_pointer.to_owned(),
                ))
                .or_insert_with(|| {
                    CompatIssueData::new(
                        blessed,
                        blessed_pointer,
                        &generated,
                        generated_pointer,
                    )
                })
                .changes
                .push(change);
                acc
            },
        );
    Ok(changes
        .into_iter()
        .map(|((blessed_pointer, generated_pointer), data)| ApiCompatIssue {
            blessed_pointer,
            generated_pointer,
            data,
        })
        .collect())
}

pub fn change_class_str(class: &ChangeClass) -> &'static str {
    match class {
        // Add spaces to the end of everything so "unhandled" can return an
        // empty string.
        ChangeClass::BackwardIncompatible => "backward-incompatible ",
        ChangeClass::ForwardIncompatible => "forward-incompatible ",
        ChangeClass::Incompatible => "incompatible ",
        // For unhandled changes, just say "change" in the error message (so
        // nothing here).
        ChangeClass::Unhandled => "",
        ChangeClass::Trivial => "trivial ",
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_best_pointer() {
        let cases = vec![
            // Same pointers.
            (
                "#/paths/~1users/get",
                "#/paths/~1users/get",
                ApiCompatPointer::Same("#/paths/~1users/get"),
            ),
            // Blessed pointer is child of generated pointer.
            (
                "#/paths/~1users/get/responses",
                "#/paths/~1users/get",
                ApiCompatPointer::Blessed("#/paths/~1users/get/responses"),
            ),
            (
                "#/paths/~1users/get/responses/200",
                "#/paths/~1users/get",
                ApiCompatPointer::Blessed("#/paths/~1users/get/responses/200"),
            ),
            // Generated pointer is child of blessed pointer.
            (
                "#/paths/~1users/get",
                "#/paths/~1users/get/responses",
                ApiCompatPointer::Generated("#/paths/~1users/get/responses"),
            ),
            (
                "#/paths/~1users/get",
                "#/paths/~1users/get/responses/200/content",
                ApiCompatPointer::Generated(
                    "#/paths/~1users/get/responses/200/content",
                ),
            ),
            // Neither is parent of the other (rename case).
            (
                "#/paths/~1users/get",
                "#/paths/~1accounts/get",
                ApiCompatPointer::Rename {
                    blessed_pointer: "#/paths/~1users/get",
                    generated_pointer: "#/paths/~1accounts/get",
                },
            ),
            (
                "#/paths/~1users/post/requestBody",
                "#/paths/~1users/put/requestBody",
                ApiCompatPointer::Rename {
                    blessed_pointer: "#/paths/~1users/post/requestBody",
                    generated_pointer: "#/paths/~1users/put/requestBody",
                },
            ),
            // Edge case: similar paths but not parent-child.
            (
                "#/paths/~1user",
                "#/paths/~1users",
                ApiCompatPointer::Rename {
                    blessed_pointer: "#/paths/~1user",
                    generated_pointer: "#/paths/~1users",
                },
            ),
        ];

        for (blessed_pointer, generated_pointer, expected) in cases {
            eprintln!("testing {blessed_pointer} -> {generated_pointer}");
            let actual = ApiCompatPointer::best_pointer(
                blessed_pointer,
                generated_pointer,
            );
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn test_json_pointer_to_jq() {
        let cases = vec![
            // Basic path with no escapes.
            ("#/paths/users", ".paths.users"),
            // Path with tilde escape.
            ("#/paths/~0users", r#".paths."~users""#),
            // Path with slash escape.
            ("#/paths/~1users", r#".paths."/users""#),
            // Path with both escapes.
            ("#/paths/~0users~1get", r#".paths."~users/get""#),
            // Complex path with multiple segments.
            (
                "#/paths/~1users/get/responses/200",
                r#".paths."/users".get.responses.200"#,
            ),
            // Path without leading #.
            ("/paths/users", ".paths.users"),
            // Empty path.
            ("", "."),
            // Just #.
            ("#", "."),
            // Path with multiple slashes to escape.
            ("#/paths/~1api~1v1~1users", r#".paths."/api/v1/users""#),
            // Path with mixed escapes.
            (
                "#/components/schemas/User~0Name~1Field",
                r#".components.schemas."User~Name/Field""#,
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(
                json_pointer_to_jq(input),
                expected,
                "for input: {input}",
            );
        }
    }

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
}
