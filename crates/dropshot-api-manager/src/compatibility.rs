// Copyright 2025 Oxide Computer Company

//! Determine if one OpenAPI spec is a subset of another

use std::{collections::BTreeMap, fmt};

use drift::{Change, ChangeClass};

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
        let last_component = pointer.split('/').last().unwrap_or("");
        surround_with_map(&last_component, v)
    })
}

fn surround_with_map(
    pointer: &str,
    value: &serde_json::Value,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let last_component = pointer.split('/').last().unwrap_or("");
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
        if let Some(suffix) = blessed_pointer.strip_prefix(&generated_pointer) {
            if suffix.starts_with('/') {
                return ApiCompatPointer::Blessed(blessed_pointer);
            }
        }
        if let Some(suffix) = generated_pointer.strip_prefix(&blessed_pointer) {
            if suffix.starts_with('/') {
                return ApiCompatPointer::Generated(generated_pointer);
            }
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

pub fn api_compatible(
    blessed: &serde_json::Value,
    generated: &serde_json::Value,
) -> anyhow::Result<Vec<ApiCompatIssue>> {
    let changes = drift::compare(blessed, generated)?;
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
                        generated,
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
}
