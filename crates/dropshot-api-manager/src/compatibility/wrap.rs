// Copyright 2026 Oxide Computer Company

//! Style-aware text wrapping for compatibility output.
//!
//! The [`super::display`] layer builds each "logical line" of an issue
//! header (the content after `at:` or `used by:`) as a [`Line`] of styled
//! [`Span`]s, then hands it to [`write_wrapped`] to lay out at a target
//! width.
//!
//! Wrapping is only performed at ASCII spaces between words. A "word" is a
//! contiguous run of non-space characters across one or more spans. Words wider
//! than the line extend past the boundary rather than being broken: endpoint
//! paths like `/v1/instances/{instance}/…` need to remain copyable from the
//! terminal as a single unit.
//!
//! Adapted from `wicket/src/ui/wrap.rs` in oxidecomputer/omicron.

use owo_colors::{OwoColorize, Style};
use std::{
    borrow::Cow,
    fmt::{self, Write as _},
};
use textwrap::core::display_width;

#[derive(Clone, Debug)]
pub(super) struct Span<'a> {
    content: Cow<'a, str>,
    style: Style,
}

#[derive(Debug, Default)]
pub(super) struct Line<'a> {
    spans: Vec<Span<'a>>,
}

impl<'a> Line<'a> {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Append a styled span.
    pub(super) fn push(
        &mut self,
        content: impl Into<Cow<'a, str>>,
        style: Style,
    ) -> &mut Self {
        let content = content.into();
        if !content.is_empty() {
            self.spans.push(Span { content, style });
        }
        self
    }

    /// Append an unstyled span.
    ///
    /// Shorthand for `push(content, Style::default())`.
    pub(super) fn push_plain(
        &mut self,
        content: impl Into<Cow<'a, str>>,
    ) -> &mut Self {
        self.push(content, Style::default())
    }

    /// Write every span to `f` with its style applied, with no wrapping.
    /// Useful inside tree-drawn rows where the surrounding scaffolding
    /// already constrains the layout.
    pub(super) fn write_inline(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        for span in &self.spans {
            write!(f, "{}", span.content.as_ref().style(span.style))?;
        }
        Ok(())
    }
}

/// A subsequent-line indent: the literal string emitted at the start
/// of each continuation line, paired with its visible column width.
///
/// Width is supplied by the caller rather than measured so
/// [`write_wrapped`] makes no assumption about the indent's content.
/// Plain space indents (the only kind in use today) are constructed
/// via [`Self::spaces`].
#[derive(Clone, Copy, Debug)]
pub(super) struct Indent<'a> {
    pub(super) string: &'a str,
    pub(super) width: usize,
}

impl<'a> Indent<'a> {
    /// Construct an `Indent` from a string of ASCII spaces.
    pub(super) fn spaces(string: &'a str) -> Self {
        debug_assert!(
            string.bytes().all(|b| b == b' '),
            "Indent::spaces called with non-space content: {string:?}",
        );
        Self { string, width: string.len() }
    }
}

/// A wrap-time unit: a sequence of styled body slices that move together
/// across line breaks, plus the trailing space run that follows them in
/// the source content.
///
/// `body_width` is the visible width of the body (for fit decisions);
/// `trailing_ws_width` is the width of the trailing space run, dropped
/// when this word is the last on a wrapped line.
#[derive(Debug, Default)]
struct StyledWord<'a> {
    body: Vec<(&'a str, Style)>,
    body_width: usize,
    trailing_ws_width: usize,
}

impl StyledWord<'_> {
    fn is_empty(&self) -> bool {
        self.body.is_empty() && self.trailing_ws_width == 0
    }
}

/// Walk `line`'s spans and split them into [`StyledWord`]s at ASCII-space
/// boundaries. Non-space content from consecutive spans merges into a
/// single word so style transitions within a word (e.g., `(` default →
/// `op_id` purple → `)` default) don't introduce break candidates.
fn collect_words<'a>(line: &'a Line<'a>) -> Vec<StyledWord<'a>> {
    let mut words = Vec::new();
    let mut current = StyledWord::default();
    // True once we've moved past the body of the current word into its
    // trailing whitespace run. The next non-space character commits the
    // current word and starts a new one.
    let mut in_trailing_ws = false;

    for span in &line.spans {
        let mut body_start = 0;
        let content = span.content.as_ref();
        let bytes = content.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b' ' {
                if !in_trailing_ws {
                    // Closing out the body portion of this span: commit
                    // the slice [body_start, i) to the current word body
                    // before we start counting trailing whitespace.
                    if body_start < i {
                        let slice = &content[body_start..i];
                        current.body.push((slice, span.style));
                        current.body_width += display_width(slice);
                    }
                    in_trailing_ws = true;
                }
                current.trailing_ws_width += 1;
                i += 1;
            } else {
                if in_trailing_ws {
                    // Trailing whitespace just ended — commit the current
                    // word and start a fresh one. The non-space character
                    // we just saw is the first byte of the new word's body.
                    words.push(std::mem::take(&mut current));
                    in_trailing_ws = false;
                    body_start = i;
                }
                i += 1;
            }
        }
        // Anything left over in this span goes into the current bucket:
        // body if we're not in trailing whitespace yet, otherwise the
        // trailing run has already been counted above.
        if !in_trailing_ws && body_start < bytes.len() {
            let slice = &content[body_start..];
            current.body.push((slice, span.style));
            current.body_width += display_width(slice);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Write `line` to `f`, wrapping at `width` columns.
///
/// `width` is the total visible width available; on continuation lines
/// we emit `indent.string` first and the remaining `width - indent.width`
/// columns are available for content. The first line is assumed to
/// already be positioned at column `indent.width` by the caller
/// (typically by writing a right-aligned label of the same visible
/// width), so the same content width applies to every line of the block.
///
/// Single words that overflow extend past width. The alternative would defeat
/// the user's ability to copy it from the terminal in one piece (particularly
/// for long endpoint paths).
pub(super) fn write_wrapped(
    f: &mut fmt::Formatter<'_>,
    line: &Line<'_>,
    width: usize,
    indent: Indent<'_>,
) -> fmt::Result {
    let words = collect_words(line);
    if words.is_empty() {
        return Ok(());
    }

    let content_width = width.saturating_sub(indent.width);

    // Greedy first-fit: a word joins the current line if its body still
    // fits, otherwise it starts a new line. Width is measured against
    // body alone (not body + trailing whitespace) so a trailing space
    // that would overflow doesn't force a premature break — it just
    // gets dropped at the line end.
    let mut line_ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    let mut current_width = 0;
    for (i, word) in words.iter().enumerate() {
        let cant_fit_here = current_width > 0
            && current_width + word.body_width > content_width;
        if cant_fit_here {
            line_ranges.push((start, i));
            start = i;
            current_width = 0;
        }
        current_width += word.body_width + word.trailing_ws_width;
    }
    line_ranges.push((start, words.len()));

    for (line_idx, (lo, hi)) in line_ranges.iter().copied().enumerate() {
        if line_idx > 0 {
            writeln!(f)?;
            f.write_str(indent.string)?;
        }
        let group = &words[lo..hi];
        let last_idx = group.len().saturating_sub(1);
        for (j, word) in group.iter().enumerate() {
            for (slice, style) in &word.body {
                write!(f, "{}", slice.style(*style))?;
            }
            // Drop trailing whitespace at the end of a wrapped line so we
            // don't emit dangling spaces; preserve it between words on the
            // same output line.
            if j < last_idx && word.trailing_ws_width > 0 {
                for _ in 0..word.trailing_ws_width {
                    f.write_char(' ')?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render `line` at the given `width` with an indent of `indent`
    /// ASCII spaces, returning the resulting string.
    fn render(line: &Line<'_>, width: usize, indent: &str) -> String {
        struct Adapter<'a> {
            line: &'a Line<'a>,
            width: usize,
            indent: Indent<'a>,
        }
        impl fmt::Display for Adapter<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_wrapped(f, self.line, self.width, self.indent)
            }
        }
        Adapter { line, width, indent: Indent::spaces(indent) }.to_string()
    }

    #[test]
    fn test_fit_on_one_line() {
        let mut line = Line::new();
        line.push_plain("GET ").push_plain("/short");
        assert_eq!(render(&line, 80, "    "), "GET /short");
    }

    #[test]
    fn test_break_at_whitespace() {
        let mut line = Line::new();
        line.push_plain("alpha beta");
        // Width 7: "alpha" (5) fits, "beta" (4) won't fit alongside it
        // (5+1+4=10>7) and fits fine on a line of its own, so it
        // breaks.
        assert_eq!(render(&line, 7, "  "), "alpha\n  beta");
    }

    #[test]
    fn test_adjacent_spans_form_one_word() {
        let mut line = Line::new();
        line.push_plain("AB").push_plain("CD");
        // "ABCD" is 4 chars; at width 3, a naive per-span breaker would
        // split between "AB" and "CD" — but they form one word here, so
        // the whole thing extends past the limit on a single line.
        assert_eq!(render(&line, 3, "  "), "ABCD");
    }

    #[test]
    fn test_long_word_overflows() {
        let mut line = Line::new();
        line.push_plain("a ").push_plain("loooooooooong ").push_plain("z");
        // Width 5: each word goes on its own line ("a" fits, the long
        // word overflows alone, "z" fits).
        assert_eq!(render(&line, 5, ""), "a\nloooooooooong\nz");
    }

    #[test]
    fn test_preserve_styles_across_wrap() {
        let bold = Style::new().bold();
        let mut line = Line::new();
        line.push_plain("aa bb ").push("CC", bold).push_plain("-dd");
        // Width 7 unindented: "aa bb" (5) fits on line 1; "CC-dd" (5)
        // doesn't fit alongside it (5+1+5=11>7) and starts line 2 with
        // the bold styling on CC preserved.
        let expected = format!("aa bb\n{}-dd", "CC".style(bold));
        assert_eq!(render(&line, 7, ""), expected);
    }
}
