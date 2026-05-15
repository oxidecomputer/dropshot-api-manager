// Copyright 2026 Oxide Computer Company

//! Render [`ApiCompatIssue`]s as styled text for the CLI.
//!
//! The data side of compatibility checking lives in [`super::detect`]; this
//! module turns one of those issues into the labeled, tree-drawn output the
//! user sees on `cargo openapi check`.

use super::{
    types::{
        ApiCompatIssue, CompatIssueLocation, DocumentBasePath, DocumentPath,
        PathTree, PathTreeKey, SubpathChange,
    },
    wrap::{Indent, Line, write_wrapped},
};
use crate::output::Styles;
use drift::ChangeClass;
use owo_colors::{OwoColorize, Style};
use std::{
    collections::{BTreeMap, HashSet},
    fmt::{self, Write as _},
};

/// `Display` adapter for [`ApiCompatIssue`] that applies styling.
///
/// Returned by [`ApiCompatIssue::display`] and
/// [`ApiCompatIssue::display_abbreviated`].
///
/// Each issue is rendered as a single block, with the per-change severity /
/// `at:` at the top, then all the `used by:` at the bottom. Multiple changes
/// within the same issue are all used by the same set of endpoints (by the
/// structure of how OpenAPI works), so we combine those changes.
///
/// Separately, `output.rs` emits a single JSON diff per issue which is treated
/// as part of the block.
pub(crate) struct ApiCompatIssueDisplay<'a> {
    pub(super) issue: &'a ApiCompatIssue,
    pub(super) styles: &'a Styles,
    pub(super) prev: Option<CompatIssueLocation<'a>>,
    /// Total terminal width available, minus any external indent
    /// applied by the caller. `None` disables wrapping.
    pub(super) wrap_width: Option<usize>,
}

impl<'a> ApiCompatIssueDisplay<'a> {
    /// Constrain rendering to wrap at `width` visible columns. Words wider than
    /// the line (e.g., long endpoints) overflow rather than being broken, so
    /// the user can still copy it from the terminal in one piece.
    pub(crate) fn with_wrap_width(mut self, width: usize) -> Self {
        self.wrap_width = Some(width);
        self
    }
}

impl fmt::Display for ApiCompatIssueDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let groups = collect_endpoint_groups(&self.issue.tree);
        let label_width = self.block_label_width(&groups);
        // Continuation indent for wrapped rows and the leading prefix
        // for tree-drawn lines: same length as `<right-aligned label>: `
        // so columns align across the block.
        let row_indent_string = " ".repeat(label_width + 2);
        let row_indent = Indent::spaces(&row_indent_string);

        // Newlines separate sections rather than terminate them, so the
        // rendered block always ends mid-line regardless of whether a
        // `used by:` section follows. The caller supplies the trailing
        // newline (typically via `eprintln!` or `format!("{}\n", ...)`).
        //
        // The `(see <prev>)` back-reference for cross-API duplicates goes
        // on the first severity line only.
        let mut prev_marker = self.prev;
        for (i, change) in self.issue.changes.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            self.write_change_header(
                f,
                change,
                label_width,
                row_indent,
                prev_marker,
            )?;
            prev_marker = None;
        }
        if !groups.is_empty() {
            writeln!(f)?;
            self.write_used_by_section(f, &groups, label_width, row_indent)?;
        }
        Ok(())
    }
}

impl<'a> ApiCompatIssueDisplay<'a> {
    /// Compute the label column width for this block.
    ///
    /// The width is the longest label that will appear, across:
    ///
    /// * every change's severity word
    /// * `at`, which is always present
    /// * `used by`, if there's at least one endpoint group
    fn block_label_width(&self, groups: &[EndpointGroup<'_>]) -> usize {
        let mut max = "at".len();
        for change in &self.issue.changes {
            max = max.max(class_label(change.class).len());
        }
        if !groups.is_empty() {
            max = max.max("used by".len());
        }
        max
    }

    /// Emit one rendered `Line` of content immediately after a label,
    /// honoring [`Self::wrap_width`].
    ///
    /// The caller is expected to have already written the right-aligned
    /// label and `: ` separator (see [`write_label`]), positioning the
    /// formatter at the column where `row_indent` ends. Continuation
    /// lines, if any, align to the same column.
    fn write_row_content(
        &self,
        f: &mut fmt::Formatter<'_>,
        row_indent: Indent<'_>,
        content: &Line<'_>,
    ) -> fmt::Result {
        match self.wrap_width {
            Some(width) => write_wrapped(f, content, width, row_indent),
            None => content.write_inline(f),
        }
    }

    /// Render one change's severity and `at:` lines (without a trailing newline
    /// after `at:`).
    fn write_change_header(
        &self,
        f: &mut fmt::Formatter<'_>,
        change: &'a SubpathChange,
        label_width: usize,
        row_indent: Indent<'_>,
        prev_marker: Option<CompatIssueLocation<'_>>,
    ) -> fmt::Result {
        let styles = self.styles;

        // Severity line: `<severity>: <message>`, with an optional
        // `(see <prev>)` suffix when this is the first severity line of
        // an abbreviated duplicate. The `(see ...)` doubles as the
        // version anchor — for the first-occurrence rendering, the
        // surrounding cargo Failure header already names the version, so
        // we don't repeat it here.
        let severity = class_label(change.class);
        write_label(
            f,
            severity,
            class_style(change.class, styles),
            label_width,
        )?;
        let mut severity_line = Line::new();
        severity_line.push_plain(change.message.as_str());
        if let Some(prev) = prev_marker {
            severity_line
                .push_plain(" ")
                .push(format!("(see {prev})"), styles.warning);
        }
        self.write_row_content(f, row_indent, &severity_line)?;
        writeln!(f)?;

        write_label(f, "at", styles.dimmed, label_width)?;
        let mut at_line = Line::new();
        self.append_at_content(&mut at_line, change);
        self.write_row_content(f, row_indent, &at_line)
    }

    /// Render `used by:` lines and their intermediate trees, without a leading
    /// or trailing newline.
    fn write_used_by_section(
        &self,
        f: &mut fmt::Formatter<'_>,
        groups: &[EndpointGroup<'a>],
        label_width: usize,
        row_indent: Indent<'_>,
    ) -> fmt::Result {
        let styles = self.styles;
        // This map is used to elide shared subtrees with `(*)` back-references,
        // similar to `cargo tree`.
        //
        // Note that the same `base` with a different `subpath` is not deduped
        // at that level, though it's likely to be deduped at the level above.
        let mut seen = HashSet::new();
        for (i, group) in groups.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write_label(f, "used by", styles.dimmed, label_width)?;
            let mut head = Line::new();
            append_humanized_base(&mut head, group.endpoint_base, styles);
            self.write_row_content(f, row_indent, &head)?;
            if !group.intermediates.children.is_empty() {
                let mut prefix = row_indent.string.to_string();
                self.write_intermediate_tree(
                    f,
                    &group.intermediates,
                    &mut prefix,
                    &mut seen,
                )?;
            }
        }
        Ok(())
    }

    /// Recursively render an [`IntermediateTree`] using `cargo tree`-style
    /// `├─` / `└─` connectors with `│  ` continuations down the levels.
    ///
    /// `prefix` carries the indentation from ancestor levels (the spaces
    /// to skip past the `used by:` label, plus a `│  ` for each unfinished
    /// ancestor branch). It's mutated in place for convenience but is
    /// always restored to its incoming value before this function returns.
    ///
    /// `seen` records [`PathTreeKey`]s already expanded earlier in the
    /// rendering. A repeated key is rendered with a trailing `(*)` and its
    /// subtree is suppressed — the reader can find the full chain at the
    /// first occurrence.
    ///
    /// We don't try and wrap lines within the tree. That's because handling
    /// such lines is quite complex: a line within the tree can be prefixed by
    /// any number of `│ ` , and applying a wrap would require passing in the
    /// right prefix. Endpoint paths inside the tree don't really happen in
    /// practice, so we just let these lines overflow. Something worth
    /// revisiting in the future, though!
    fn write_intermediate_tree(
        &self,
        f: &mut fmt::Formatter<'_>,
        tree: &'a IntermediateTree<'a>,
        prefix: &mut String,
        seen: &mut HashSet<&'a PathTreeKey>,
    ) -> fmt::Result {
        let styles = self.styles;
        let last = tree.children.len().saturating_sub(1);
        for (i, (&key, child)) in tree.children.iter().enumerate() {
            let is_last = i == last;
            let connector = if is_last { "└─ " } else { "├─ " };
            let child_indent = if is_last { "   " } else { "│  " };

            writeln!(f)?;
            // Dim the whole tree-drawing portion (prefix bars + connector)
            // so the chain reads as quieter scaffolding around the bolded
            // type names. Plain spaces in the prefix render the same dim
            // or not, so styling the whole string is fine.
            write!(
                f,
                "{}{}",
                prefix.as_str().style(styles.dimmed),
                connector.style(styles.dimmed),
            )?;
            // Type name stays bolded as the row's anchor; field path
            // renders at default weight so the tree stays quieter than
            // the `at:` line above.
            let mut row = Line::new();
            append_humanized_at_pair(
                &mut row,
                &key.base,
                &key.subpath,
                styles,
                Style::default(),
            );
            row.write_inline(f)?;

            // If this exact key was already expanded earlier in the
            // block, mark `(*)` and skip the subtree.
            if !seen.insert(key) {
                write!(f, " {}", "(*)".style(styles.dimmed))?;
                continue;
            }

            let saved = prefix.len();
            prefix.push_str(child_indent);
            self.write_intermediate_tree(f, child, prefix, seen)?;
            prefix.truncate(saved);
        }
        Ok(())
    }

    /// Build the content of the `at:` line for one [`SubpathChange`].
    ///
    /// * If the blessed and generated locations match exactly, render once.
    /// * For pure additions (old subpath is root, base unchanged) or
    ///   removals (new subpath is root, base unchanged), render only the
    ///   side that names a real location — the message text already
    ///   conveys whether something was added or removed.
    /// * If exactly one side classifies as
    ///   [`DocumentBasePath::PathsRoot`] (which is drift's view of the
    ///   `.paths` container when the endpoint itself doesn't exist on
    ///   that side), render only the recognized side. Reporting it as a
    ///   rename from `.paths` to `GET /system/info` is misleading: it's
    ///   really an add or remove of the endpoint, with the parent
    ///   container being a noisy artifact of drift's path stack.
    /// * Otherwise (rename, type change with rename, etc.) render both
    ///   forms separated by ` -> ` so the rename is explicit.
    fn append_at_content(
        &self,
        line: &mut Line<'a>,
        change: &'a SubpathChange,
    ) {
        let issue = self.issue;
        let styles = self.styles;
        let same_base = issue.blessed_base == issue.generated_base;
        let same_subpath = change.old_subpath == change.new_subpath;

        // The `at:` line is the primary location of the change, so we
        // bold both the type name and the field path within it. The
        // tree-drawn continuation lines below render the field path with
        // a quieter field style; see `write_intermediate_tree`.
        let field_style = styles.bold;
        if same_base && same_subpath {
            append_humanized_at_pair(
                line,
                &issue.blessed_base,
                &change.old_subpath,
                styles,
                field_style,
            );
            return;
        }
        if same_base && change.old_subpath.is_root() {
            append_humanized_at_pair(
                line,
                &issue.generated_base,
                &change.new_subpath,
                styles,
                field_style,
            );
            return;
        }
        if same_base && change.new_subpath.is_root() {
            append_humanized_at_pair(
                line,
                &issue.blessed_base,
                &change.old_subpath,
                styles,
                field_style,
            );
            return;
        }

        // Pair on `(blessed_is_paths_root, generated_is_paths_root)` so the
        // four cases are explicit.
        match (
            issue.blessed_base.is_paths_root(),
            issue.generated_base.is_paths_root(),
        ) {
            // Endpoint added: drop the `.paths` side and render only the
            // new endpoint.
            (true, false) => append_humanized_at_pair(
                line,
                &issue.generated_base,
                &change.new_subpath,
                styles,
                field_style,
            ),
            // Endpoint removed: drop the `.paths` side and render only the old
            // endpoint.
            (false, true) => append_humanized_at_pair(
                line,
                &issue.blessed_base,
                &change.old_subpath,
                styles,
                field_style,
            ),
            // `(false, false)` is a real rename. `(true, true)` should not
            // ordinarily happen, but handle it in case it does anyway. In both
            // cases, render both sides with ` -> `.
            (true, true) | (false, false) => {
                append_humanized_at_pair(
                    line,
                    &issue.blessed_base,
                    &change.old_subpath,
                    styles,
                    field_style,
                );
                line.push_plain(" -> ");
                append_humanized_at_pair(
                    line,
                    &issue.generated_base,
                    &change.new_subpath,
                    styles,
                    field_style,
                );
            }
        }
    }
}

/// Write a right-aligned label followed by `: `.
///
/// Uses manual whitespace padding (rather than the `{:>n}` width specifier)
/// so that ANSI escape sequences in the styled label don't throw the
/// alignment off.
fn write_label(
    f: &mut fmt::Formatter<'_>,
    label: &str,
    label_style: Style,
    max_width: usize,
) -> fmt::Result {
    let pad = max_width.saturating_sub(label.len());
    for _ in 0..pad {
        f.write_char(' ')?;
    }
    write!(f, "{}: ", label.style(label_style))
}

fn class_label(class: ChangeClass) -> &'static str {
    match class {
        ChangeClass::BackwardIncompatible => "backward-incompatible",
        ChangeClass::ForwardIncompatible => "forward-incompatible",
        ChangeClass::Incompatible => "incompatible",
        // Trivial changes are filtered out in `api_compatible`, so this
        // branch is only reachable if a future drift release classifies
        // something here that we then forget to filter; show it neutrally
        // rather than panic.
        ChangeClass::Trivial => "trivial",
        ChangeClass::Unhandled => "change",
    }
}

/// Pick the severity color for a [`ChangeClass`].
///
/// Hard incompatibilities are red (the surface either is or is about to be
/// broken). Forward-incompatible is amber: newer clients can't talk to
/// older servers, but the deployed surface still works. Other changes get
/// a neutral warning style.
fn class_style(class: ChangeClass, styles: &Styles) -> Style {
    match class {
        ChangeClass::BackwardIncompatible | ChangeClass::Incompatible => {
            styles.failure_header
        }
        ChangeClass::ForwardIncompatible => styles.warning_header,
        ChangeClass::Unhandled | ChangeClass::Trivial => styles.warning,
    }
}

/// One originating endpoint, plus the tree of `$ref` chains that link it back
/// to the changed component.
#[derive(Debug)]
struct EndpointGroup<'a> {
    /// Base of the originating endpoint.
    endpoint_base: &'a DocumentBasePath,
    /// The tree of `$ref` chains leading from `endpoint_base` back toward the
    /// changed component.
    intermediates: IntermediateTree<'a>,
}

/// Tree of `$ref` intermediate sources, oriented endpoint-first.
///
/// Structurally identical to [`PathTree`], but a different type to keep
/// the orientation explicit: in [`PathTree`] the root is the changed
/// component and the leaves are endpoints; here the (implicit) root is
/// an endpoint and the deepest nodes are the components closest to the
/// changed type.
///
/// Borrows [`PathTreeKey`]s from a [`PathTree`] rather than owning them
/// so the rebuild at display time is cheap.
#[derive(Debug, Default)]
struct IntermediateTree<'a> {
    children: BTreeMap<&'a PathTreeKey, IntermediateTree<'a>>,
}

/// Enumerate distinct endpoints through `tree`, grouping all chains originating
/// from the same endpoint into an [`EndpointGroup`]. Within each group, chains
/// that share a prefix are also merged into an `IntermediateTree`. (This
/// effectively inverts the `PathTree`, as desired by display output.)
fn collect_endpoint_groups(tree: &PathTree) -> Vec<EndpointGroup<'_>> {
    let mut groups: BTreeMap<_, IntermediateTree<'_>> = BTreeMap::new();
    let mut ancestors = Vec::new();
    for (key, child) in &tree.children {
        walk_and_collect(key, child, &mut ancestors, &mut groups);
    }
    groups
        .into_iter()
        .map(|(endpoint_base, intermediates)| EndpointGroup {
            endpoint_base,
            intermediates,
        })
        .collect()
}

/// Walk the `node` rooted at `key`, and at each endpoint leaf, insert the
/// accumulated `ancestors` into that endpoint's [`IntermediateTree`].
fn walk_and_collect<'a>(
    key: &'a PathTreeKey,
    node: &'a PathTree,
    ancestors: &mut Vec<&'a PathTreeKey>,
    groups: &mut BTreeMap<&'a DocumentBasePath, IntermediateTree<'a>>,
) {
    if node.children.is_empty() {
        // This is a leaf node in the `PathTree`. This means that `key` is the
        // originating endpoint.
        //
        // `ancestors` covers the rest of the tree. Walk it in reverse,
        // descending through `IntermediateTree` children, so the resulting
        // subtree is endpoint-first.
        let mut curr = groups.entry(&key.base).or_default();
        for &ancestor in ancestors.iter().rev() {
            curr = curr.children.entry(ancestor).or_default();
        }
        return;
    }
    ancestors.push(key);
    for (child_key, child) in &node.children {
        walk_and_collect(child_key, child, ancestors, groups);
    }
    ancestors.pop();
}

/// Append a (`base`, `subpath`) pair to `line` in the form used by `at:`
/// and the tree-drawn continuation lines.
///
/// `field_style` is forwarded to [`append_humanized_subpath`] for components
/// and controls how the subpath portion is styled.
fn append_humanized_at_pair<'a>(
    line: &mut Line<'a>,
    base: &'a DocumentBasePath,
    subpath: &'a DocumentPath,
    styles: &Styles,
    field_style: Style,
) {
    append_humanized_base(line, base, styles);
    if subpath.is_root() {
        return;
    }
    match base {
        DocumentBasePath::Component(_) => {
            append_humanized_subpath(line, subpath, field_style);
        }
        DocumentBasePath::Endpoint { .. } => {
            // We call `append_jq_path` and not `append_humanized_subpath`
            // here. This is because segments like `properties` are not a
            // structural part of endpoints (only of components).
            line.push_plain(" (");
            append_jq_path(line, subpath, Style::default());
            line.push_plain(")");
        }
        DocumentBasePath::PathsRoot | DocumentBasePath::Other(_) => {
            append_jq_path(line, subpath, Style::default());
        }
    }
}

/// Append `base` to `line` in humanized form by dispatching on its variant:
///
/// * For [`DocumentBasePath::Component`], the bolded schema name.
/// * For [`DocumentBasePath::Endpoint`], a green-bold `<METHOD>` followed by
///   `<route>`. If an `operation_id` is provided, it is appended in
///   parentheses.
/// * For [`DocumentBasePath::PathsRoot`], the literal `.paths`. In practice
///   this is dropped in [`ApiCompatIssueDisplay::append_at_content`] before
///   reaching here, so this is purely defensive.
/// * For [`DocumentBasePath::Other`], falls back to full jq notation.
fn append_humanized_base<'a>(
    line: &mut Line<'a>,
    base: &'a DocumentBasePath,
    styles: &Styles,
) {
    match base {
        DocumentBasePath::Component(path) => {
            line.push(component_name(path), styles.bold);
        }
        DocumentBasePath::Endpoint { name, operation_id } => {
            append_endpoint_method_route(line, name, styles);
            append_operation_id(line, operation_id.as_deref(), styles);
        }
        DocumentBasePath::PathsRoot => {
            line.push(".paths", styles.bold);
        }
        DocumentBasePath::Other(path) => {
            append_jq_path(line, path, styles.bold);
        }
    }
}

/// Append `METHOD /route` for an endpoint base path.
///
/// `name` is assumed to match the `paths/<route>/<method>` shape that
/// [`DocumentBasePath::classify`] enforces on `Endpoint` variants.
fn append_endpoint_method_route<'a>(
    line: &mut Line<'a>,
    name: &'a DocumentPath,
    styles: &Styles,
) {
    // We expect this to be `paths/<route>/<method>`:
    //
    // * segments[1] is the unescaped route,
    // * segments[2] is the HTTP method.
    let route = name.segments[1].as_str();
    let method = name.segments[2].to_uppercase();
    line.push(method, styles.success_header)
        .push_plain(" ")
        .push(route, styles.filename);
}

/// Append ` (<op_id>)` to `line` in the operation-id style, or no-op if
/// `op_id` is `None`. A leading space is included so callers can call
/// this unconditionally. The trio is split across three spans only for
/// styling — no internal whitespace means a wrap can only occur before
/// the leading space, not inside `(op_id)`.
fn append_operation_id<'a>(
    line: &mut Line<'a>,
    op_id: Option<&'a str>,
    styles: &Styles,
) {
    let Some(op_id) = op_id else { return };
    line.push_plain(" (").push(op_id, styles.operation_id).push_plain(")");
}

/// Append a component's subpath in OpenAPI-aware form.
///
/// With OpenAPI, paths can contain both structural keywords (`properties`,
/// `items`, etc.) and field names. The structural pieces do not add much value,
/// so we skip over them:
///
/// * `.properties.<name>` just becomes `.<name>`. The `.properties` marker
///   just says that the next segment is a field name.
/// * `.items` (the schema `items` keyword, marking the element type of
///   an array) becomes `[]`. (This makes `.foo.items` render as `.foo[]`.
///
/// Other keywords (`allOf.0`, `additionalProperties`, etc.) are
/// rendered literally.
fn append_humanized_subpath<'a>(
    line: &mut Line<'a>,
    subpath: &'a DocumentPath,
    style: Style,
) {
    let mut next_is_field_name = false;
    for s in &subpath.segments {
        if next_is_field_name {
            append_jq_segment(line, s, style);
            next_is_field_name = false;
            continue;
        }
        match s.as_str() {
            "properties" => next_is_field_name = true,
            "items" => {
                line.push("[]", style);
            }
            other => append_jq_segment(line, other, style),
        }
    }
}

/// Append `path` to `line` in plain jq notation, applying `style` to every
/// segment.
///
/// The root (empty) path renders as `.`, while non-empty paths render as
/// `.seg1.seg2…`.
fn append_jq_path<'a>(
    line: &mut Line<'a>,
    path: &'a DocumentPath,
    style: Style,
) {
    if path.is_root() {
        line.push(".", style);
        return;
    }
    for s in &path.segments {
        append_jq_segment(line, s, style);
    }
}

/// Append one jq segment to `line` starting with a leading `.`.
///
/// The segment text is quoted if it contains characters ambiguous as a bare jq
/// identifier.
fn append_jq_segment<'a>(line: &mut Line<'a>, segment: &'a str, style: Style) {
    line.push(".", style);
    if segment.contains(['/', '~']) {
        line.push(format!("\"{segment}\""), style);
    } else {
        line.push(segment, style);
    }
}

/// Extract the schema/component name from a known-component path
/// (`components/<kind>/<name>`). The shape is enforced by
/// [`DocumentBasePath::classify`] on the `Component` variant.
fn component_name(base: &DocumentPath) -> &str {
    &base.segments[2]
}

#[cfg(test)]
mod tests {
    use super::{
        super::types::{
            ApiCompatIssue, CompatIssueLocation, DocumentBasePath,
            DocumentPath, OperationIdMap, PathTree, PathTreeKey, SubpathChange,
        },
        *,
    };
    use crate::output::Styles;
    use camino::Utf8PathBuf;
    use drift::ChangeClass;
    use dropshot_api_manager_types::ApiIdent;
    use owo_colors::OwoColorize;
    use std::collections::BTreeSet;

    /// Build a small [`OperationIdMap`] from `(endpoint_base, op_id)` pairs.
    /// Used by synthetic-issue tests that bypass `api_compatible` and so
    /// don't have a spec to extract op ids from.
    fn op_ids<'a>(entries: &[(&str, &'a str)]) -> OperationIdMap<'a> {
        entries
            .iter()
            .map(|(base, op_id)| (DocumentPath::parse(base), *op_id))
            .collect()
    }

    /// Shorthand for `DocumentBasePath::Component(DocumentPath::parse(p))`.
    fn component(p: &str) -> DocumentBasePath {
        DocumentBasePath::Component(DocumentPath::parse(p))
    }

    fn output_path(name: &str) -> camino::Utf8PathBuf {
        Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/output/compatibility")
            .join(name)
    }

    /// Render `issues` with `styles`, joining multiple issues with a blank
    /// line and ensuring a trailing newline.
    fn render_with_styles(
        issues: &[ApiCompatIssue],
        styles: &Styles,
    ) -> String {
        if issues.is_empty() {
            return "(no issues)\n".to_string();
        }
        let mut out = issues
            .iter()
            .map(|i| i.display(styles).to_string())
            .collect::<Vec<_>>()
            .join("\n\n");
        out.push('\n');
        out
    }

    /// Run `api_compatible` on `(blessed, generated)` and snapshot both the
    /// plain (`<name>.txt`) and colorized (`<name>.ansi`) renderings of the
    /// resulting issues.
    fn assert_render_snapshots(
        name: &str,
        blessed: &serde_json::Value,
        generated: &serde_json::Value,
    ) {
        let issues = super::super::detect::api_compatible(blessed, generated)
            .expect("api_compatible should not fail on well-formed input");
        let mut colorized = Styles::default();
        colorized.colorize();
        expectorate::assert_contents(
            output_path(&format!("{name}.txt")),
            &render_with_styles(&issues, &Styles::default()),
        );
        expectorate::assert_contents(
            output_path(&format!("{name}.ansi")),
            &render_with_styles(&issues, &colorized),
        );
    }

    /// Snapshot the plain (`<name>.txt`) and colorized (`<name>.ansi`)
    /// renderings of a synthetic [`ApiCompatIssue`].
    fn assert_issue_snapshots(name: &str, issue: &ApiCompatIssue) {
        let mut colorized = Styles::default();
        colorized.colorize();
        expectorate::assert_contents(
            output_path(&format!("{name}.txt")),
            &format!("{}\n", issue.display(&Styles::default())),
        );
        expectorate::assert_contents(
            output_path(&format!("{name}.ansi")),
            &format!("{}\n", issue.display(&colorized)),
        );
    }

    /// Render `path` unstyled into a `String` using [`append_jq_path`].
    fn jq_string(path: &DocumentPath) -> String {
        struct Render<'a>(&'a DocumentPath);
        impl fmt::Display for Render<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut line = Line::new();
                append_jq_path(&mut line, self.0, Style::default());
                line.write_inline(f)
            }
        }
        Render(path).to_string()
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

    #[test]
    fn test_jq_path_display() {
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
            // Empty path (root).
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
            // Relative subpath (no `#`/leading `/`).
            ("properties/value", ".properties.value"),
            (
                "responses/200/content/application~1json",
                r#".responses.200.content."application/json""#,
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(
                jq_string(&DocumentPath::parse(input)),
                expected,
                "for input: {input}",
            );
        }
    }

    /// Build a minimal OpenAPI 3.0 document with a single GET endpoint
    /// whose 200 response schema is a `$ref` to the named schema, and
    /// the supplied `components.schemas` map.
    fn doc_with_ref_schema(
        target_ref: &str,
        schemas: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "openapi": "3.0.3",
            "info": { "title": "T", "version": "1.0.0" },
            "paths": {
                "/hello/{name}": {
                    "get": {
                        "operationId": "hello",
                        "parameters": [{
                            "name": "name", "in": "path",
                            "required": true,
                            "schema": { "type": "string" }
                        }],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": { "$ref": target_ref }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "components": { "schemas": schemas }
        })
    }

    /// An endpoint-level change (a new required parameter) reaches no schema
    /// component, so the rendered output should not include a reference tree.
    #[test]
    fn test_render_endpoint_only_change() {
        let blessed = serde_json::json!({
            "openapi": "3.0.3",
            "info": { "title": "T", "version": "1.0.0" },
            "paths": {
                "/users/{id}": {
                    "get": {
                        "operationId": "get_user",
                        "parameters": [{
                            "name": "id", "in": "path",
                            "required": true,
                            "schema": { "type": "string" }
                        }],
                        "responses": {
                            "200": { "description": "ok" }
                        }
                    }
                }
            },
            "components": { "schemas": {} }
        });

        let mut generated = blessed.clone();

        // This is a new required query parameter, which means it is
        // backward-incompatible.
        let params = generated
            .pointer_mut("/paths/~1users~1{id}/get/parameters")
            .and_then(|v| v.as_array_mut())
            .unwrap();
        params.push(serde_json::json!({
            "name": "token", "in": "query",
            "required": true,
            "schema": { "type": "string" }
        }));

        assert_render_snapshots("endpoint_only_change", &blessed, &generated);
    }

    /// Tree with 2 siblings at depth 1, each with 2 siblings at depth 2.
    #[test]
    fn test_render_multi_sibling_tree() {
        let ops = op_ids(&[
            ("#/paths/~1a/get", "list_a"),
            ("#/paths/~1b/get", "list_b"),
        ]);
        let make_chain = |wrapper_field: &str, route: &str| {
            vec![
                PathTreeKey::parse(
                    &format!(
                        "#/components/schemas/Wrapper/properties/{wrapper_field}/$ref"
                    ),
                    &ops,
                ),
                PathTreeKey::parse(
                    &format!(
                        "#/paths/~1{route}/get/responses/200/content/application~1json/schema/$ref"
                    ),
                    &ops,
                ),
            ]
        };
        let mut tree = PathTree::default();
        tree.insert(make_chain("via_a", "a"));
        tree.insert(make_chain("via_a", "b"));
        tree.insert(make_chain("via_b", "a"));
        tree.insert(make_chain("via_b", "b"));

        let issue = ApiCompatIssue {
            blessed_base: component("#/components/schemas/SubType"),
            generated_base: component("#/components/schemas/SubType"),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::Incompatible,
                message: "schema types changed".into(),
                old_subpath: DocumentPath::parse("properties/value"),
                new_subpath: DocumentPath::parse("properties/value"),
            }]),
            tree,
            blessed_value: None,
            generated_value: None,
        };
        assert_issue_snapshots("multi_sibling_tree", &issue);
    }

    /// Tree under a single endpoint with shared prefixes that branch at
    /// deeper levels.
    ///
    /// In this test:
    ///
    /// * `/a`'s response schema refers to `Wrapper`
    /// * `Wrapper` has two fields that each refer to an `Inner`
    /// * `Inner` has three fields (`x`, `y`, `z`) that all refer to `SubType`
    ///
    /// So three chains reach the change site:
    ///
    /// 1. endpoint → Wrapper.via_a → Inner.x → SubType
    /// 2. endpoint → Wrapper.via_a → Inner.y → SubType
    /// 3. endpoint → Wrapper.via_b → Inner.z → SubType
    ///
    /// The first two share `Wrapper.via_a` and fan out at `Inner`; the
    /// third is its own branch.
    #[test]
    fn test_render_deep_branching_tree() {
        // `tree.insert` expects entries in `JsonPathStack::iter().skip(1)`
        // order: closest to the changed component first (drift's
        // most-recent-ref is at the head), origin endpoint last. So
        // `Inner` precedes `Wrapper` even though the endpoint visits
        // `Wrapper` first when following refs forward.
        let ops = op_ids(&[("#/paths/~1a/get", "list_a")]);
        let make_chain = |wrapper_field: &str, inner_field: &str| {
            vec![
                PathTreeKey::parse(
                    &format!(
                        "#/components/schemas/Inner/properties/{inner_field}/$ref"
                    ),
                    &ops,
                ),
                PathTreeKey::parse(
                    &format!(
                        "#/components/schemas/Wrapper/properties/{wrapper_field}/$ref"
                    ),
                    &ops,
                ),
                PathTreeKey::parse(
                    "#/paths/~1a/get/responses/200/content/application~1json/schema/$ref",
                    &ops,
                ),
            ]
        };
        let mut tree = PathTree::default();
        tree.insert(make_chain("via_a", "x"));
        tree.insert(make_chain("via_a", "y"));
        tree.insert(make_chain("via_b", "z"));

        let issue = ApiCompatIssue {
            blessed_base: component("#/components/schemas/SubType"),
            generated_base: component("#/components/schemas/SubType"),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::Incompatible,
                message: "schema types changed".into(),
                old_subpath: DocumentPath::parse("properties/value"),
                new_subpath: DocumentPath::parse("properties/value"),
            }]),
            tree,
            blessed_value: None,
            generated_value: None,
        };
        assert_issue_snapshots("deep_branching_tree", &issue);
    }

    #[test]
    fn test_render_renamed_base() {
        let issue = ApiCompatIssue {
            blessed_base: component("#/components/schemas/OldName"),
            generated_base: component("#/components/schemas/NewName"),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::Incompatible,
                message: "schema types changed".into(),
                old_subpath: DocumentPath::parse("properties/value"),
                new_subpath: DocumentPath::parse("properties/value"),
            }]),
            tree: PathTree::default(),
            blessed_value: None,
            generated_value: None,
        };
        assert_issue_snapshots("renamed_base", &issue);
    }

    /// An added endpoint: drift reports the change with `Other(.paths)`
    /// on the blessed side and the full `Endpoint(...)` on the generated
    /// side. The `at:` line should show only the recognized side rather
    /// than render the asymmetric pair as a rename from `.paths`.
    #[test]
    fn test_render_endpoint_added() {
        let ops = op_ids(&[("#/paths/~1system~1info/get", "get_system_info")]);
        let issue = ApiCompatIssue {
            blessed_base: DocumentBasePath::classify(
                DocumentPath::parse("/paths"),
                &OperationIdMap::new(),
            ),
            generated_base: DocumentBasePath::classify(
                DocumentPath::parse("/paths/~1system~1info/get"),
                &ops,
            ),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::ForwardIncompatible,
                message: "The operation get_system_info was added".into(),
                old_subpath: DocumentPath::root(),
                new_subpath: DocumentPath::root(),
            }]),
            tree: PathTree::default(),
            blessed_value: None,
            generated_value: None,
        };
        assert_issue_snapshots("endpoint_added", &issue);
    }

    /// A subpath rename within a fixed base — the field name differs
    /// between blessed and generated. The `at:` line surfaces the rename
    /// with the `->` arrow.
    #[test]
    fn test_render_subpath_renamed() {
        let issue = ApiCompatIssue {
            blessed_base: component("#/components/schemas/Foo"),
            generated_base: component("#/components/schemas/Foo"),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::Incompatible,
                message: "field renamed".into(),
                old_subpath: DocumentPath::parse("properties/old_name"),
                new_subpath: DocumentPath::parse("properties/new_name"),
            }]),
            tree: PathTree::default(),
            blessed_value: None,
            generated_value: None,
        };
        assert_issue_snapshots("subpath_renamed", &issue);
    }

    /// An [`ChangeClass::Unhandled`] change renders with the neutral
    /// `change:` keyword rather than a severity color.
    #[test]
    fn test_render_unhandled_class() {
        let issue = ApiCompatIssue {
            blessed_base: component("#/components/schemas/Foo"),
            generated_base: component("#/components/schemas/Foo"),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::Unhandled,
                message: "array maxItems changed".into(),
                old_subpath: DocumentPath::parse("items"),
                new_subpath: DocumentPath::parse("items"),
            }]),
            tree: PathTree::default(),
            blessed_value: None,
            generated_value: None,
        };
        assert_issue_snapshots("unhandled_class", &issue);
    }

    /// Multiple non-trivial changes within the same component render as
    /// separate self-contained blocks separated by a blank line.
    #[test]
    fn test_render_multiple_changes_in_one_component() {
        let blessed = doc_with_ref_schema(
            "#/components/schemas/Wrapper",
            serde_json::json!({
                "Wrapper": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "string" },
                        "b": { "type": "integer" }
                    },
                    "required": ["a", "b"]
                }
            }),
        );

        let mut generated = blessed.clone();
        // Flip both field types — two backward-incompatible changes at
        // the same schema base.
        *generated
            .pointer_mut("/components/schemas/Wrapper/properties/a/type")
            .unwrap() = serde_json::Value::String("integer".into());
        *generated
            .pointer_mut("/components/schemas/Wrapper/properties/b/type")
            .unwrap() = serde_json::Value::String("string".into());

        assert_render_snapshots(
            "multiple_changes_in_one_component",
            &blessed,
            &generated,
        );
    }

    /// Abbreviated rendering of an issue with multiple [`SubpathChange`]s
    /// — the `(see foo v1.0.0)` back-reference must appear *only* on the
    /// first severity line. The grouping logic stacks both severities at
    /// the top of one block, sharing the `exposed:` tree below; the
    /// pointer to the first occurrence is a property of the issue, not of
    /// any one change within it, so repeating it on every line would just
    /// be noise.
    #[test]
    fn test_render_abbreviated_multi_change() {
        let ops = op_ids(&[("#/paths/~1bar1/get", "get_bar1")]);
        let make_chain = |route: &str| {
            vec![
                PathTreeKey::parse(
                    "#/components/schemas/Wrapper/properties/value/$ref",
                    &ops,
                ),
                PathTreeKey::parse(
                    &format!(
                        "#/paths/~1{route}/get/responses/200/content/application~1json/schema/$ref"
                    ),
                    &ops,
                ),
            ]
        };
        let mut tree = PathTree::default();
        tree.insert(make_chain("bar1"));

        let issue = ApiCompatIssue {
            blessed_base: component("#/components/schemas/SubType"),
            generated_base: component("#/components/schemas/SubType"),
            changes: BTreeSet::from([
                SubpathChange {
                    class: ChangeClass::Incompatible,
                    message: "schema kind changed".into(),
                    old_subpath: DocumentPath::parse("properties/value"),
                    new_subpath: DocumentPath::parse("properties/value"),
                },
                SubpathChange {
                    class: ChangeClass::Unhandled,
                    message: "object properties changed".into(),
                    old_subpath: DocumentPath::root(),
                    new_subpath: DocumentPath::root(),
                },
            ]),
            tree,
            blessed_value: None,
            generated_value: None,
        };

        let prev = OwnedLoc::new("foo", "1.0.0");
        let mut colorized = Styles::default();
        colorized.colorize();
        expectorate::assert_contents(
            output_path("abbreviated_multi_change.txt"),
            &format!(
                "{}\n",
                issue.display_abbreviated(&Styles::default(), prev.as_loc()),
            ),
        );
        expectorate::assert_contents(
            output_path("abbreviated_multi_change.ansi"),
            &format!(
                "{}\n",
                issue.display_abbreviated(&colorized, prev.as_loc()),
            ),
        );
    }

    /// Snapshot the abbreviated rendering used for duplicate issues. The
    /// header and the per-API tree should both appear; the back-reference
    /// `(see foo v1.0.0)` should be on the header line.
    #[test]
    fn test_render_abbreviated() {
        let ops = op_ids(&[
            ("#/paths/~1bar1/get", "get_bar1"),
            ("#/paths/~1bar2/get", "get_bar2"),
        ]);
        let make_chain = |route: &str| {
            vec![
                PathTreeKey::parse(
                    "#/components/schemas/Wrapper/properties/value/$ref",
                    &ops,
                ),
                PathTreeKey::parse(
                    &format!(
                        "#/paths/~1{route}/get/responses/200/content/application~1json/schema/$ref"
                    ),
                    &ops,
                ),
            ]
        };
        let mut tree = PathTree::default();
        tree.insert(make_chain("bar1"));
        tree.insert(make_chain("bar2"));

        let issue = ApiCompatIssue {
            blessed_base: component("#/components/schemas/SubType"),
            generated_base: component("#/components/schemas/SubType"),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::Incompatible,
                message: "schema types changed".into(),
                old_subpath: DocumentPath::parse("properties/value"),
                new_subpath: DocumentPath::parse("properties/value"),
            }]),
            tree,
            blessed_value: None,
            generated_value: None,
        };

        let prev = OwnedLoc::new("foo", "1.0.0");
        let mut colorized = Styles::default();
        colorized.colorize();
        expectorate::assert_contents(
            output_path("abbreviated.txt"),
            &format!(
                "{}\n",
                issue.display_abbreviated(&Styles::default(), prev.as_loc()),
            ),
        );
        expectorate::assert_contents(
            output_path("abbreviated.ansi"),
            &format!(
                "{}\n",
                issue.display_abbreviated(&colorized, prev.as_loc()),
            ),
        );
    }

    #[test]
    fn test_render_wrapped_long_endpoint() {
        // A long, realistic Oxide-style endpoint name with multiple path
        // parameters that would normally exceed terminal width when prefixed by
        // `used by: GET `.
        let endpoint = "#/paths/~1v1~1instances~1{instance}\
                        ~1network-interfaces~1{network_interface}~1multicast/post";
        let ops = op_ids(&[(endpoint, "instance_network_multicast_attach")]);
        let make_chain = || {
            vec![
                PathTreeKey::parse(
                    "#/components/schemas/MulticastGroupMember\
                     /properties/identity/$ref",
                    &ops,
                ),
                PathTreeKey::parse(
                    &format!(
                        "{}/responses/200/content/application~1json/schema/$ref",
                        endpoint,
                    ),
                    &ops,
                ),
            ]
        };
        let mut tree = PathTree::default();
        tree.insert(make_chain());

        let issue = ApiCompatIssue {
            blessed_base: component(
                "#/components/schemas/MulticastGroupIdentity",
            ),
            generated_base: component(
                "#/components/schemas/MulticastGroupIdentity",
            ),
            changes: BTreeSet::from([SubpathChange {
                class: ChangeClass::Incompatible,
                message: "schema types changed".into(),
                old_subpath: DocumentPath::parse("properties/id"),
                new_subpath: DocumentPath::parse("properties/id"),
            }]),
            tree,
            blessed_value: None,
            generated_value: None,
        };

        // Pin the width so the test doesn't depend on the developer's
        // terminal size. 80 is a typical width where the `used by:`
        // endpoint overflows but the rest of the block doesn't.
        let mut colorized = Styles::default();
        colorized.colorize();
        expectorate::assert_contents(
            output_path("wrapped_long_endpoint.txt"),
            &format!(
                "{}\n",
                issue.display(&Styles::default()).with_wrap_width(80),
            ),
        );
        expectorate::assert_contents(
            output_path("wrapped_long_endpoint.ansi"),
            &format!("{}\n", issue.display(&colorized).with_wrap_width(80)),
        );
    }

    /// Snapshot the issue in its surrounding render context: a cargo-style
    /// `Failure` header, the second-tier `error:` keyword, and the issue
    /// body indented so its leftmost label aligns with where `error`
    /// begins. This validates the design intent that the verb column
    /// "extends downward" from the cargo headers through `error:` into
    /// the per-issue labels.
    #[test]
    fn test_render_in_context() {
        // Mirror the indents picked by `display_resolution_problems` /
        // `display_compat_issue` in `output.rs` so this test exercises the
        // same alignment math.
        const HEADER_WIDTH: usize = 12;
        let issue_indent = " ".repeat(HEADER_WIDTH - "error".len());

        let ops = op_ids(&[(
            "#/paths/~1v1~1system~1audit-log/get",
            "audit_log_list",
        )]);
        let make_chain = |route: &str| {
            vec![
                PathTreeKey::parse(
                    "#/components/schemas/AuditLogEntryResultsPage\
                     /properties/items/items/$ref",
                    &ops,
                ),
                PathTreeKey::parse(
                    &format!(
                        "#/paths/~1v1~1system~1{route}/get/responses/200/content\
                         /application~1json/schema/$ref"
                    ),
                    &ops,
                ),
            ]
        };
        let mut tree = PathTree::default();
        tree.insert(make_chain("audit-log"));

        let issue = ApiCompatIssue {
            blessed_base: component("#/components/schemas/AuditLogEntry"),
            generated_base: component("#/components/schemas/AuditLogEntry"),
            changes: BTreeSet::from([
                SubpathChange {
                    class: ChangeClass::Incompatible,
                    message: "schema kind changed".into(),
                    old_subpath: DocumentPath::parse("properties/auth_method"),
                    new_subpath: DocumentPath::parse("properties/auth_method"),
                },
                SubpathChange {
                    class: ChangeClass::Unhandled,
                    message: "object properties changed".into(),
                    old_subpath: DocumentPath::root(),
                    new_subpath: DocumentPath::root(),
                },
            ]),
            tree,
            blessed_value: None,
            generated_value: None,
        };

        let render = |styles: &Styles| -> String {
            let mut out = String::new();
            // The full preceding cargo lines aren't relevant to alignment —
            // just the Failure header that immediately precedes `error:`.
            writeln!(
                &mut out,
                "{:>HEADER_WIDTH$} nexus (versioned v2025112000.0.0 \
                 (blessed)): Oxide Region API",
                "Failure".style(styles.failure_header),
            )
            .unwrap();
            writeln!(
                &mut out,
                "{:>HEADER_WIDTH$}: OpenAPI document is incompatible with \
                 the blessed upstream",
                "error".style(styles.failure_header),
            )
            .unwrap();
            writeln!(&mut out).unwrap();
            // Indent the issue body.
            let mut indented = String::new();
            write!(
                indent_write::fmt::IndentWriter::new(
                    &issue_indent,
                    &mut indented,
                ),
                "{}",
                issue.display(styles),
            )
            .unwrap();
            out.push_str(&indented);
            out.push('\n');
            out
        };

        let mut colorized = Styles::default();
        colorized.colorize();
        expectorate::assert_contents(
            output_path("in_context.txt"),
            &render(&Styles::default()),
        );
        expectorate::assert_contents(
            output_path("in_context.ansi"),
            &render(&colorized),
        );
    }
}
