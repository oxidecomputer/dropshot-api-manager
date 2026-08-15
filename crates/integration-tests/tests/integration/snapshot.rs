// Copyright 2026 Oxide Computer Company

use camino::Utf8PathBuf;
use integration_tests::TestEnvironment;

/// Asserts that the rendered output matches the snapshot in
/// `tests/output/integration/<snapshot_name>`.
pub fn assert_render_snapshot(
    env: &TestEnvironment,
    snapshot_name: &str,
    rendered: &str,
) {
    // The "Loading local OpenAPI documents from <abs_dir>" line embeds the
    // tempdir path, which varies per run. The path is rendered with `{:?}`
    // (Debug), which on Windows escapes backslashes — so we have to match
    // the Debug-formatted form (which includes surrounding quotes), not
    // the raw `as_str()` form, otherwise the replacement silently no-ops
    // on Windows. The placeholder restores the quotes so the line still
    // reads naturally.
    let documents_dir_debug = format!("{:?}", env.documents_dir());
    let normalized =
        rendered.replace(&documents_dir_debug, "\"<documents dir>\"");
    // Diff headers for stale files print the raw (unquoted) absolute path with
    // forward slashes (see format_diff_path), so normalize that form as well.
    // (This must run after the Debug-form replacement above so it isn't
    // half-replaced.)
    let documents_dir_forward = env.documents_dir().as_str().replace('\\', "/");
    let normalized =
        normalized.replace(&documents_dir_forward, "<documents dir>");

    let snapshot_path = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/output/integration")
        .join(snapshot_name);
    expectorate::assert_contents(snapshot_path, &normalized);
}
