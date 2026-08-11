// Copyright 2026 Oxide Computer Company

use crate::{
    FAILURE_EXIT_CODE, NEEDS_UPDATE_EXIT_CODE,
    apis::{ManagedApi, ManagedApis},
    compatibility::{
        ApiCompatIssue, CompatIssueLocation, CompatRenderStatus,
        FinalizedCompatDedupMap,
    },
    environment::{ErrorAccumulator, ResolvedEnv},
    resolved::{
        Fix, NonVersionProblem, Resolution, ResolutionKind, Resolved,
        VersionProblem,
    },
    validation::CheckStale,
};
use anyhow::bail;
use camino::Utf8Path;
use clap::{Args, ColorChoice};
use headers::*;
use indent_write::fmt::IndentWriter;
use owo_colors::{OwoColorize, Style};
use similar::{ChangeTag, DiffableStr, TextDiff};
use std::{
    fmt::{self, Write},
    io,
    process::ExitCode,
};

#[derive(Debug, Args)]
#[clap(next_help_heading = "Global options")]
pub struct OutputOpts {
    /// Color output
    #[clap(long, value_enum, global = true, default_value_t)]
    pub(crate) color: ColorChoice,
}

impl OutputOpts {
    /// Returns true if color should be used for the stream.
    pub(crate) fn use_color(&self, stream: supports_color::Stream) -> bool {
        match self.color {
            ColorChoice::Auto => supports_color::on_cached(stream).is_some(),
            ColorChoice::Always => true,
            ColorChoice::Never => false,
        }
    }

    /// Creates a `Styles` instance, colorized if color is enabled for the
    /// given stream.
    pub(crate) fn styles(&self, stream: supports_color::Stream) -> Styles {
        let mut styles = Styles::default();
        if self.use_color(stream) {
            styles.colorize();
        }
        styles
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Styles {
    pub(crate) bold: Style,
    pub(crate) dimmed: Style,
    pub(crate) header: Style,
    pub(crate) success_header: Style,
    pub(crate) failure: Style,
    pub(crate) failure_header: Style,
    pub(crate) warning: Style,
    pub(crate) warning_header: Style,
    pub(crate) unchanged_header: Style,
    pub(crate) filename: Style,
    pub(crate) operation_id: Style,
    pub(crate) diff_before: Style,
    pub(crate) diff_after: Style,
}

impl Styles {
    pub(crate) fn colorize(&mut self) {
        self.bold = Style::new().bold();
        self.dimmed = Style::new().dimmed();
        self.header = Style::new().purple();
        self.success_header = Style::new().green().bold();
        self.failure = Style::new().red();
        self.failure_header = Style::new().red().bold();
        self.warning = Style::new().yellow();
        self.warning_header = Style::new().yellow().bold();
        self.unchanged_header = Style::new().blue().bold();
        self.filename = Style::new().cyan();
        self.operation_id = Style::new().purple();
        self.diff_before = Style::new().red();
        self.diff_after = Style::new().green();
    }
}

// This is copied from similar's UnifiedDiff::to_writer, except with colorized
// output.
pub(crate) fn write_diff<'diff, 'old, 'new, 'bufs, T>(
    diff: &'diff TextDiff<'old, 'new, 'bufs, T>,
    path1: &Utf8Path,
    path2: &Utf8Path,
    styles: &Styles,
    context_radius: usize,
    missing_newline_hint: bool,
    out: &mut dyn io::Write,
) -> io::Result<()>
where
    'diff: 'old + 'new + 'bufs,
    T: DiffableStr + ?Sized,
{
    // The "a/" and "b/" prefixes make this feel more like a git diff. We
    // assemble the header by hand (and normalize any backslashes in the
    // path) so the output is forward-slashed regardless of host OS, which
    // is the convention every diff/patch tool expects.
    let a = format_diff_path("a", path1);
    writeln!(out, "{}", format!("--- {a}").style(styles.diff_before))?;
    let b = format_diff_path("b", path2);
    writeln!(out, "{}", format!("+++ {b}").style(styles.diff_after))?;

    let mut udiff = diff.unified_diff();
    udiff
        .context_radius(context_radius)
        .missing_newline_hint(missing_newline_hint);
    for hunk in udiff.iter_hunks() {
        for (idx, change) in hunk.iter_changes().enumerate() {
            if idx == 0 {
                writeln!(out, "{}", hunk.header())?;
            }
            let style = match change.tag() {
                ChangeTag::Delete => styles.diff_before,
                ChangeTag::Insert => styles.diff_after,
                ChangeTag::Equal => Style::new(),
            };

            write!(out, "{}", change.tag().style(style))?;
            write!(out, "{}", change.value().to_string_lossy().style(style))?;
            if !diff.newline_terminated() {
                writeln!(out)?;
            }
            if diff.newline_terminated() && change.missing_newline() {
                writeln!(
                    out,
                    "{}",
                    MissingNewlineHint(hunk.missing_newline_hint())
                )?;
            }
        }
    }

    Ok(())
}

/// Format a `prefix/path` header for unified diff output, normalizing
/// path separators to `/` so the rendered output is identical on Windows
/// and Unix (and matches the diff/patch convention).
fn format_diff_path(prefix: &str, path: &Utf8Path) -> String {
    if std::path::MAIN_SEPARATOR == '/' {
        format!("{prefix}/{path}")
    } else {
        format!("{prefix}/{}", path.as_str().replace('\\', "/"))
    }
}

pub(crate) fn display_api_doc(api: &ManagedApi, styles: &Styles) -> String {
    let mut versions = api.iter_versions_semver();
    let count = versions.len();
    let latest_version =
        versions.next_back().expect("must be at least one version");
    if api.is_versioned() {
        format!(
            "{} ({}, versioned ({} supported), latest = {})",
            api.ident().style(styles.filename),
            api.title(),
            count,
            latest_version,
        )
    } else {
        format!(
            "{} ({}, lockstep, v{})",
            api.ident().style(styles.filename),
            api.title(),
            latest_version,
        )
    }
}

pub(crate) fn display_api_doc_version(
    api: &ManagedApi,
    version: &semver::Version,
    styles: &Styles,
    resolution: &Resolution<'_>,
) -> String {
    if api.is_lockstep() {
        assert_eq!(resolution.kind(), ResolutionKind::Lockstep);
        format!(
            "{} (lockstep v{}): {}",
            api.ident().style(styles.filename),
            version,
            api.title(),
        )
    } else {
        format!(
            "{} (versioned v{} ({})): {}",
            api.ident().style(styles.filename),
            version,
            resolution.kind(),
            api.title(),
        )
    }
}

pub(crate) fn display_error(
    error: &anyhow::Error,
    failure_style: Style,
) -> impl fmt::Display + '_ {
    struct DisplayError<'a> {
        error: &'a anyhow::Error,
        failure_style: Style,
    }

    impl fmt::Display for DisplayError<'_> {
        fn fmt(&self, mut f: &mut fmt::Formatter<'_>) -> fmt::Result {
            writeln!(f, "{}", self.error.style(self.failure_style))?;

            let mut source = self.error.source();
            while let Some(curr) = source {
                write!(f, "-> ")?;
                writeln!(
                    IndentWriter::new_skip_initial("   ", &mut f),
                    "{}",
                    curr.style(self.failure_style),
                )?;
                source = curr.source();
            }

            Ok(())
        }
    }

    DisplayError { error, failure_style }
}

struct MissingNewlineHint(bool);

impl fmt::Display for MissingNewlineHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 {
            write!(f, "\n\\ No newline at end of file")?;
        }
        Ok(())
    }
}

pub fn display_load_problems(
    writer: &mut dyn io::Write,
    error_accumulator: &ErrorAccumulator,
    styles: &Styles,
) -> anyhow::Result<()> {
    for w in error_accumulator.iter_warnings() {
        writeln!(
            writer,
            "{:>HEADER_WIDTH$} {:#}",
            WARNING.style(styles.warning_header),
            w
        )?;
    }

    let mut nerrors = 0;
    for e in error_accumulator.iter_errors() {
        nerrors += 1;
        writeln!(
            writer,
            "{:>HEADER_WIDTH$} {:#}",
            FAILURE.style(styles.failure_header),
            e
        )?;
    }

    if nerrors > 0 {
        bail!(
            "bailing out after {} {} above",
            nerrors,
            plural::errors(nerrors)
        );
    }

    Ok(())
}

/// Summarize the results of checking all supported API versions, plus other
/// problems found during resolution
pub fn display_resolution(
    writer: &mut dyn io::Write,
    env: &ResolvedEnv,
    apis: &ManagedApis,
    resolved: &Resolved,
    styles: &Styles,
) -> anyhow::Result<CheckResult> {
    let total = resolved.nexpected_documents();

    writeln!(
        writer,
        "{:>HEADER_WIDTH$} {} OpenAPI {}...",
        CHECKING.style(styles.success_header),
        total.style(styles.bold),
        plural::documents(total),
    )?;

    let mut num_fresh = 0;
    let mut num_stale = 0;
    let mut num_failed = 0;
    let mut num_non_version_problems = 0;

    let dedup = resolved.build_compat_dedup_map();

    // Print problems associated with a supported API version
    // (i.e., one of the expected OpenAPI documents).
    for api in apis.iter_apis() {
        let ident = api.ident();

        for version in api.iter_versions_semver() {
            let resolution = resolved
                .resolution_for_api_version(ident, version)
                .expect("resolution for all supported API versions");
            if resolution.has_errors() {
                num_failed += 1;
            } else if resolution.has_problems() {
                num_stale += 1;
            } else {
                num_fresh += 1;
            }
            summarize_one(
                writer, env, api, version, resolution, styles, &dedup,
            )?;
        }

        if !api.is_versioned() {
            continue;
        }

        if let Some(symlink_problem) = resolved.symlink_problem(ident) {
            if symlink_problem.is_fixable() {
                num_non_version_problems += 1;
                writeln!(
                    writer,
                    "{:>HEADER_WIDTH$} {} \"latest\" symlink",
                    STALE.style(styles.warning_header),
                    ident.style(styles.filename),
                )?;
                display_non_version_problems(
                    writer,
                    std::iter::once(symlink_problem),
                    styles,
                )?;
            } else {
                num_failed += 1;
                writeln!(
                    writer,
                    "{:>HEADER_WIDTH$} {} \"latest\" symlink",
                    FAILURE.style(styles.failure_header),
                    ident.style(styles.filename),
                )?;
                display_non_version_problems(
                    writer,
                    std::iter::once(symlink_problem),
                    styles,
                )?;
            }
        } else {
            num_fresh += 1;
            writeln!(
                writer,
                "{:>HEADER_WIDTH$} {} \"latest\" symlink",
                FRESH.style(styles.success_header),
                ident.style(styles.filename),
            )?;
        }
    }

    // Print problems not associated with any supported version, if any.
    let orphaned_and_unparseable: Vec<_> =
        resolved.orphaned_and_unparseable().collect();
    num_non_version_problems += if !orphaned_and_unparseable.is_empty() {
        writeln!(
            writer,
            "\n{:>HEADER_WIDTH$} problems not associated with a specific \
             supported API version:",
            "Other".style(styles.warning_header),
        )?;

        let (fixable, unfixable): (
            Vec<&NonVersionProblem>,
            Vec<&NonVersionProblem>,
        ) = orphaned_and_unparseable.iter().partition(|p| p.is_fixable());
        num_failed += unfixable.len();
        display_non_version_problems(writer, orphaned_and_unparseable, styles)?;
        fixable.len()
    } else {
        0
    };

    // Print informational notes, if any.
    for n in resolved.notes() {
        let initial_indent =
            format!("{:>HEADER_WIDTH$} ", "Note".style(styles.warning_header));
        let more_indent = " ".repeat(HEADER_WIDTH + " ".len());
        writeln!(
            writer,
            "\n{}\n",
            textwrap::fill(
                &n.to_string(),
                textwrap::Options::new(term_width())
                    .initial_indent(&initial_indent)
                    .subsequent_indent(&more_indent)
            )
        )?;
    }

    // Print a summary line.
    let status_header = if num_failed > 0 {
        FAILURE.style(styles.failure_header)
    } else if num_stale > 0 || num_non_version_problems > 0 {
        STALE.style(styles.warning_header)
    } else {
        SUCCESS.style(styles.success_header)
    };

    writeln!(writer, "{:>HEADER_WIDTH$}", SEPARATOR)?;
    writeln!(
        writer,
        "{:>HEADER_WIDTH$} {} {} checked: {} fresh, {} stale, {} failed, \
         {} other {}",
        status_header,
        total.style(styles.bold),
        plural::documents(total),
        num_fresh.style(styles.bold),
        num_stale.style(styles.bold),
        num_failed.style(styles.bold),
        num_non_version_problems.style(styles.bold),
        plural::problems(num_non_version_problems),
    )?;
    if num_failed > 0 {
        writeln!(
            writer,
            "{:>HEADER_WIDTH$} (fix failures, then run {} to update)",
            "",
            format!("{} generate", env.command).style(styles.bold)
        )?;
        Ok(CheckResult::Failures)
    } else if num_stale > 0 || num_non_version_problems > 0 {
        writeln!(
            writer,
            "{:>HEADER_WIDTH$} (run {} to update)",
            "",
            format!("{} generate", env.command).style(styles.bold)
        )?;
        Ok(CheckResult::NeedsUpdate)
    } else {
        Ok(CheckResult::Success)
    }
}

/// The result of a check operation.
///
/// Returned by the `check_apis_up_to_date` function.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckResult {
    /// The APIs are up-to-date.
    Success,
    /// The APIs need to be updated.
    NeedsUpdate,
    /// There were validation errors or other problems.
    Failures,
}

impl CheckResult {
    /// Returns the exit code corresponding to the check result.
    pub fn to_exit_code(self) -> ExitCode {
        match self {
            CheckResult::Success => ExitCode::SUCCESS,
            CheckResult::NeedsUpdate => NEEDS_UPDATE_EXIT_CODE.into(),
            CheckResult::Failures => FAILURE_EXIT_CODE.into(),
        }
    }
}

/// Summarize the "check" status of one supported API version
fn summarize_one(
    writer: &mut dyn io::Write,
    env: &ResolvedEnv,
    api: &ManagedApi,
    version: &semver::Version,
    resolution: &Resolution<'_>,
    styles: &Styles,
    dedup: &FinalizedCompatDedupMap<'_>,
) -> io::Result<()> {
    let problems: Vec<_> = resolution.problems().collect();
    if problems.is_empty() {
        // Success case: file is up-to-date.
        writeln!(
            writer,
            "{:>HEADER_WIDTH$} {}",
            FRESH.style(styles.success_header),
            display_api_doc_version(api, version, styles, resolution),
        )?;
    } else {
        // There were one or more problems, some of which may be unfixable.
        writeln!(
            writer,
            "{:>HEADER_WIDTH$} {}",
            if resolution.has_errors() {
                FAILURE.style(styles.failure_header)
            } else {
                assert!(resolution.has_problems());
                STALE.style(styles.warning_header)
            },
            display_api_doc_version(api, version, styles, resolution),
        )?;

        let compat_ctx = CompatDisplayContext {
            dedup,
            current: CompatIssueLocation { api: api.ident(), version },
        };
        display_version_problems(writer, env, problems, styles, compat_ctx)?;
    }
    Ok(())
}

pub(crate) struct CompatDisplayContext<'a> {
    pub(crate) dedup: &'a FinalizedCompatDedupMap<'a>,
    pub(crate) current: CompatIssueLocation<'a>,
}

/// Print a formatted list of per-(api, version) [`VersionProblem`]s to
/// `writer`, including any compatibility-issue diffs and fix descriptions.
///
/// `compat_ctx` enables [`VersionProblem::BlessedVersionBroken`] rendering to
/// abbreviate compatibility issues that have already been shown elsewhere.
pub(crate) fn display_version_problems<'a, T>(
    writer: &mut dyn io::Write,
    env: &ResolvedEnv,
    problems: T,
    styles: &Styles,
    compat_ctx: CompatDisplayContext<'_>,
) -> io::Result<()>
where
    T: IntoIterator<Item = &'a VersionProblem<'a>>,
{
    for p in problems.into_iter() {
        write_problem_header(writer, p, p.is_fixable(), styles)?;

        // Indent for compat-issue bodies. The issue's longest label sits
        // at this column so its leftmost edge lines up with where `error`
        // begins (HEADER_WIDTH minus the 5 chars of "error"), preserving
        // the verb column the eye is already tracking down from the
        // cargo headers.
        let issue_indent = " ".repeat(HEADER_WIDTH - "error".len());

        // Each issue gets a leading blank (emitted by `display_compat_issue`,
        // separating it from the problem header or the previous diff) and a
        // single trailing blank here (separating the last issue from the next
        // problem). An issue already reported elsewhere is rendered in
        // abbreviated form, pointing at the canonical occurrence.
        let issues = p.compatibility_issues();
        for issue in issues {
            let status = compat_ctx.dedup.status_for(issue, compat_ctx.current);
            display_compat_issue(
                &mut *writer,
                issue,
                &issue_indent,
                styles,
                status,
            )?;
        }
        if !issues.is_empty() {
            writeln!(writer)?;
        }

        // For BlessedLatestVersionBytewiseMismatch, show a diff between blessed
        // and generated versions even though there's no fix.
        if let VersionProblem::BlessedLatestVersionBytewiseMismatch {
            blessed,
            generated,
        } = p
        {
            let diff =
                TextDiff::from_lines(blessed.contents(), generated.contents());
            let path1 =
                env.openapi_abs_dir().join(blessed.doc_file_name().path());
            let path2 =
                env.openapi_abs_dir().join(generated.doc_file_name().path());
            let indent = " ".repeat(HEADER_WIDTH + 1);
            write_diff(
                &diff,
                &path1,
                &path2,
                styles,
                // context_radius: show enough context to understand the changes.
                3,
                /* missing_newline_hint */ true,
                &mut indent_write::io::IndentWriter::new(&indent, &mut *writer),
            )?;
        }

        let Some(fix) = p.fix() else {
            continue;
        };

        write_fix_summary(writer, &fix, styles)?;

        // When possible, print a useful diff of changes.
        let do_diff = match p {
            VersionProblem::LockstepStale { found, generated } => {
                let diff = TextDiff::from_lines(
                    found.contents(),
                    generated.contents(),
                );
                let path1 =
                    env.openapi_abs_dir().join(found.doc_file_name().path());
                let path2 = env
                    .openapi_abs_dir()
                    .join(generated.doc_file_name().path());
                Some((diff, path1, path2))
            }
            VersionProblem::ExtraFileStale {
                check_stale:
                    CheckStale::Modified { full_path, actual, expected },
                ..
            } => {
                let diff = TextDiff::from_lines(actual, expected);
                Some((diff, full_path.clone(), full_path.clone()))
            }
            VersionProblem::LocalVersionStale { doc_files, generated }
                if doc_files.len() == 1 =>
            {
                let diff = TextDiff::from_lines(
                    doc_files[0].contents(),
                    generated.contents(),
                );
                let path1 = env
                    .openapi_abs_dir()
                    .join(doc_files[0].doc_file_name().path());
                let path2 = env
                    .openapi_abs_dir()
                    .join(generated.doc_file_name().path());
                Some((diff, path1, path2))
            }
            _ => None,
        };

        if let Some((diff, path1, path2)) = do_diff {
            let indent = " ".repeat(HEADER_WIDTH + 1);
            write_diff(
                &diff,
                &path1,
                &path2,
                styles,
                // context_radius: here, a small radius is sufficient to show
                // differences.
                3,
                /* missing_newline_hint */ true,
                // Add an indent to align diff with the status message.
                &mut indent_write::io::IndentWriter::new(&indent, &mut *writer),
            )?;
            writeln!(writer)?;
        }
    }
    Ok(())
}

/// Print a formatted list of [`NonVersionProblem`]s to `writer`.
///
/// None of these variants have associated diffs, so this is just a header +
/// fix.
pub fn display_non_version_problems<'a, T>(
    writer: &mut dyn io::Write,
    problems: T,
    styles: &Styles,
) -> io::Result<()>
where
    T: IntoIterator<Item = &'a NonVersionProblem<'a>>,
{
    for p in problems.into_iter() {
        write_problem_header(writer, p, p.is_fixable(), styles)?;
        if let Some(fix) = p.fix() {
            write_fix_summary(writer, &fix, styles)?;
        }
    }
    Ok(())
}

/// Write the `problem:` / `error:` header line for a single problem, wrapping
/// the error chain to the terminal width.
///
/// The keyword is the second-tier verb: it continues the cargo-style verb
/// column from the surrounding `Fresh` / `Failure` headers. It's right-aligned
/// to the same width so the colons stack vertically as the eye scans down.
fn write_problem_header(
    writer: &mut dyn io::Write,
    error: &dyn std::error::Error,
    is_fixable: bool,
    styles: &Styles,
) -> io::Result<()> {
    let first_indent = format!(
        "{:>HEADER_WIDTH$}: ",
        if is_fixable {
            "problem".style(styles.warning_header)
        } else {
            "error".style(styles.failure_header)
        }
    );
    // Continuation indent for wrapped error text. Aligns with the
    // post-keyword content (HEADER_WIDTH + ": ".len()).
    let more_indent = " ".repeat(HEADER_WIDTH + 2);
    writeln!(
        writer,
        "{}",
        textwrap::fill(
            &InlineErrorChain::new(error).to_string(),
            textwrap::Options::new(term_width())
                .initial_indent(&first_indent)
                .subsequent_indent(&more_indent)
        )
    )
}

/// Write the `fix:` line(s) for a single fix, splitting multi-step fixes into
/// separate `will ...` lines that share the column structure of
/// [`write_problem_header`].
fn write_fix_summary(
    writer: &mut dyn io::Write,
    fix: &Fix<'_>,
    styles: &Styles,
) -> io::Result<()> {
    let first_indent =
        format!("{:>HEADER_WIDTH$}: ", "fix".style(styles.warning_header));
    let more_indent = " ".repeat(HEADER_WIDTH + 2);
    let fix_str = fix.to_string();
    for s in fix_str.trim_end().split("\n") {
        writeln!(
            writer,
            "{}",
            textwrap::fill(
                &format!("will {}", s),
                textwrap::Options::new(term_width())
                    .initial_indent(&first_indent)
                    .subsequent_indent(&more_indent)
            )
        )?;
    }
    Ok(())
}

/// Render one compatibility issue under a problem to `writer`.
///
/// `body_indent` is the column the issue body starts at (each rendered
/// line gets this prefix). The labels right-align within an issue's own
/// colon column, so a single `body_indent` is sufficient — no separate
/// initial/continuation indents are needed.
fn display_compat_issue(
    writer: &mut dyn io::Write,
    issue: &ApiCompatIssue,
    body_indent: &str,
    styles: &Styles,
    status: CompatRenderStatus,
) -> io::Result<()> {
    // A blank line separates this issue from the previous problem header
    // (or, in the full form, from the previous issue's JSON diff which
    // already ends in a newline).
    writeln!(writer)?;

    // Wrap at terminal width minus the body indent. (`display_width` matches
    // what `wrap.rs` uses for its own indent.)
    let wrap_width =
        term_width().saturating_sub(textwrap::core::display_width(body_indent));

    // Indent every line of the rendered block. `IndentWriter` prefixes the
    // first line as well, so we don't need a separate initial-indent string.
    let mut buf = String::new();
    write!(
        IndentWriter::new(body_indent, &mut buf),
        "{}",
        issue.display(styles, status).with_wrap_width(wrap_width),
    )
    .expect("writing to a String never fails");
    writeln!(writer, "{buf}")?;

    match status {
        CompatRenderStatus::FirstOccurrence { .. } => {
            // Full form: print the textual diff between the blessed and
            // generated values for this base.
            let blessed_json = issue.blessed_json();
            let generated_json = issue.generated_json();

            let diff = TextDiff::from_lines(&blessed_json, &generated_json);
            write_diff(
                &diff,
                "blessed".as_ref(),
                "generated".as_ref(),
                styles,
                // context_radius: use a large radius to ensure that most
                // of the schema is printed out.
                8,
                /* missing_newline_hint */ false,
                // Align diff with the issue body.
                &mut indent_write::io::IndentWriter::new(body_indent, writer),
            )
        }
        CompatRenderStatus::Duplicate { .. } => {
            // Abbreviated form: no JSON diff, since it was already shown
            // at the first occurrence.
            Ok(())
        }
    }
}
/// Adapter for [`Error`]s that provides a [`std::fmt::Display`] implementation
/// that print the full chain of error sources, separated by `: `.
pub struct InlineErrorChain<'a>(&'a dyn std::error::Error);

impl<'a> InlineErrorChain<'a> {
    pub fn new(error: &'a dyn std::error::Error) -> Self {
        Self(error)
    }
}

impl fmt::Display for InlineErrorChain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;
        let mut cause = self.0.source();
        while let Some(source) = cause {
            write!(f, ": {source}")?;
            cause = source.source();
        }
        Ok(())
    }
}

/// Returns the wrap width to use for terminal output.
///
/// Honors the `OPENAPI_MGR_TERM_WIDTH` environment variable as an override.
/// Otherwise falls back to [`textwrap::termwidth`], which queries the terminal
/// connected to stdout, or returns 80 when stdout isn't a tty.
///
/// The override exists for snapshot determinism. Under `cargo nextest run` by
/// default, stdout is captured, so `termwidth` returns 80 and snapshots are
/// deterministic. Under `cargo nextest run --no-capture` (or `cargo test`),
/// however, stdout may be the developer's tty, and width is wherever the window
/// happens to be sized. Setting `OPENAPI_MGR_TERM_WIDTH=80` explicitly, as we
/// do in our tests, ensures that snapshots are deterministic in this scenario
/// as well.
pub(crate) fn term_width() -> usize {
    match std::env::var("OPENAPI_MGR_TERM_WIDTH") {
        Ok(s) => s.parse().unwrap_or_else(|err| {
            panic!("OPENAPI_MGR_TERM_WIDTH={s:?} is not a valid width: {err}")
        }),
        Err(_) => textwrap::termwidth(),
    }
}

/// Output headers.
pub(crate) mod headers {
    // Same width as Cargo's output.
    pub(crate) const HEADER_WIDTH: usize = 12;

    pub(crate) static SEPARATOR: &str = "-------";

    pub(crate) static CHECKING: &str = "Checking";
    pub(crate) static GENERATING: &str = "Generating";

    pub(crate) static FRESH: &str = "Fresh";
    pub(crate) static STALE: &str = "Stale";

    pub(crate) static UNCHANGED: &str = "Unchanged";

    pub(crate) static SUCCESS: &str = "Success";
    pub(crate) static FAILURE: &str = "Failure";
    pub(crate) static WARNING: &str = "Warning";
}

pub(crate) mod plural {
    pub(crate) fn files(count: usize) -> &'static str {
        if count == 1 { "file" } else { "files" }
    }

    pub(crate) fn changes(count: usize) -> &'static str {
        if count == 1 { "change" } else { "changes" }
    }

    pub(crate) fn documents(count: usize) -> &'static str {
        if count == 1 { "document" } else { "documents" }
    }

    pub(crate) fn errors(count: usize) -> &'static str {
        if count == 1 { "error" } else { "errors" }
    }

    pub(crate) fn paths(count: usize) -> &'static str {
        if count == 1 { "path" } else { "paths" }
    }

    pub(crate) fn problems(count: usize) -> &'static str {
        if count == 1 { "problem" } else { "problems" }
    }

    pub(crate) fn schemas(count: usize) -> &'static str {
        if count == 1 { "schema" } else { "schemas" }
    }
}
