//! Core Twee link scanner — `[[...]]` forms shared by every format plugin.
//!
//! The base Twine engine defines three link spellings that every story
//! format recognizes regardless of its own markup:
//!
//! ```text
//! [[Target]]              — simple
//! [[Display->Target]]     — right arrow
//! [[Display|Target]]      — pipe
//! ```
//!
//! Historically each plugin extracted these with its own copy of the same
//! three regexes (`\[\[([^\]|>-]+?)\]\]` and friends), scanning raw
//! passage bodies directly — a violation of the project rule that
//! operations run on span-carrying parse results. This module replaces
//! them with a single hand-rolled byte scanner in the house style
//! (compare `knot_core::css::fallback` and the SugarCube `link_parser`):
//! one pass over the body, emitting span-carrying [`CoreLink`] records
//! that callers slice, trim, and filter per their format's needs.
//!
//! ## Semantics (pinned by tests, mirroring the previous regexes)
//!
//! The scanner reproduces the combined behavior of the three regexes that
//! used to live in `twine_core.rs` / `snowman` / `chapbook` / `harlowe`:
//!
//! * A link **site** is a `[[` in the body. All forms that match at a
//!   site end at the **first `]]` at or after the site** — a `]` that is
//!   not immediately followed by another `]` terminates every form at
//!   that site (the regex char classes all excluded `]`).
//! * **Arrow**: the *leftmost* `->` separator that leaves a non-empty
//!   display before it and a non-empty target after it (up to the closing
//!   `]]`). Both sides may contain any byte except `]` (including `[`,
//!   `|`, `-`, `>` — the regex display/target classes were `[^\]]`).
//! * **Pipe**: the *leftmost* `|` separator under the same constraints.
//! * **Simple**: the whole content (between `[[` and the closing `]]`)
//!   is non-empty and contains none of `]`, `|`, `>`, `-`.
//! * Arrow and pipe are **independent**: content like `[[a|b->c]]`
//!   matches BOTH forms (the arrow reading is display `a|b`, target `c`;
//!   the pipe reading is display `a`, target `b->c`). The previous regex
//!   runs collected both interpretations, and the scanner preserves that
//!   quirk for behavior parity. (The SugarCube AST parser instead applies
//!   pipe-first precedence — plugins migrating to full AST parsing should
//!   reuse `sugarcube::parser::link_parser` rather than this scanner.)
//! * A simple match never coexists with an arrow/pipe match at the same
//!   site (their separator bytes are all excluded from simple content),
//!   which is how the old "skip simple when covered by an arrow/pipe
//!   span" dedupe is expressed here: simple is only tried when neither
//!   matched.
//! * After any match at a site the scan resumes **after the closing
//!   `]]`**, so `[[` sequences inside a matched link (e.g. the target of
//!   `[[a->b [[c]]`) do not produce their own links — the same
//!   non-overlapping progression `captures_iter` had.
//! * When a site cannot match, the scan advances one byte (so overlapping
//!   sites like the two `[[` in `[[[a]]` are each considered — the
//!   leftmost one wins).
//!
//! ## Coordinate system
//!
//! All spans are **byte offsets into the scanned body**. Callers that
//! scan a passage body embedded in a document shift by the body's offset
//! themselves (the same contract as `knot_core::css` and `knot_core::html`).

use std::ops::Range;

/// Which of the three core Twee link forms a match is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreLinkForm {
    /// `[[Target]]` — no display part.
    Simple,
    /// `[[Display->Target]]`.
    Arrow,
    /// `[[Display|Target]]`.
    Pipe,
}

/// One scanned `[[...]]` link with its source extents.
///
/// Spans are relative to the text passed to [`scan_core_links`]. For the
/// simple form `display_span` is `None` and `target_span` covers the whole
/// content; for the arrow/pipe forms both parts are present. `span` always
/// covers the full `[[...]]` spelling.
#[derive(Debug, Clone)]
pub struct CoreLink {
    /// Which form matched.
    pub form: CoreLinkForm,
    /// The display part (`[[Display->Target]]`), if the form has one.
    pub display_span: Option<Range<usize>>,
    /// The target part (always present; for the simple form it is the
    /// entire content between `[[` and `]]`).
    pub target_span: Range<usize>,
    /// The full `[[...]]` extent.
    pub span: Range<usize>,
}

impl CoreLink {
    /// The display text, if this form has one. `None` for the simple form
    /// regardless of content.
    pub fn display<'a>(&self, body: &'a str) -> Option<&'a str> {
        self.display_span.as_ref().map(|d| &body[d.clone()])
    }

    /// The target text (untrimmed — callers trim per their format's rules).
    pub fn target<'a>(&self, body: &'a str) -> &'a str {
        &body[self.target_span.clone()]
    }
}

/// Scan `body` for core Twee links, returning matches in document order.
///
/// Regex-free and total: never panics on any input. The scan works on
/// bytes but only takes boundaries at ASCII `[[`/`]]`/`->`/`|`, so UTF-8
/// content inside links is carried through the spans untouched.
pub fn scan_core_links(body: &str) -> Vec<CoreLink> {
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut links = Vec::new();
    let mut i = 0usize;

    while i + 1 < len {
        // A site starts at `[[`.
        if bytes[i] != b'[' || bytes[i + 1] != b'[' {
            i += 1;
            continue;
        }
        let site = i;

        // Closing: the FIRST `]` at/after site+2 must be immediately
        // followed by another `]`. Any other shape (a lone `]`, or no
        // `]` at all) means no form can match at this site.
        let mut close = None;
        let mut j = site + 2;
        while j < len {
            if bytes[j] == b']' {
                if j + 1 < len && bytes[j + 1] == b']' {
                    close = Some(j);
                }
                break;
            }
            j += 1;
        }
        let Some(close) = close else {
            // No `]]` anywhere after the site — no form can match.
            i = site + 1;
            continue;
        };

        // Content region: between `[[` and the closing `]]`.
        let content_start = site + 2;
        let content_end = close;
        let end = close + 2;

        // Arrow: the leftmost `->` whose split leaves both sides
        // non-empty. Display ≥ 1 byte ⇒ separator starts at
        // content_start + 1 at the earliest; target = [sep+2, close)
        // non-empty ⇒ sep ≤ close - 3.
        let arrow_sep =
            find_separator(bytes, content_start + 1, close.saturating_sub(2), |a, b| {
                a == b'-' && b == b'>'
            });

        // Pipe: same shape for a single-byte separator — target =
        // [sep+1, close) non-empty ⇒ sep ≤ close - 2.
        let pipe_sep = find_separator(bytes, content_start + 1, close.saturating_sub(1), |a, _| {
            a == b'|'
        });

        let mut matched = false;
        if let Some(sep) = arrow_sep {
            links.push(CoreLink {
                form: CoreLinkForm::Arrow,
                display_span: Some(content_start..sep),
                target_span: sep + 2..content_end,
                span: site..end,
            });
            matched = true;
        }
        if let Some(sep) = pipe_sep {
            links.push(CoreLink {
                form: CoreLinkForm::Pipe,
                display_span: Some(content_start..sep),
                target_span: sep + 1..content_end,
                span: site..end,
            });
            matched = true;
        }
        if !matched {
            // Simple: whole content non-empty and free of `]`, `|`, `>`,
            // `-`. (An arrow/pipe separator inside the content would
            // already have matched above — those bytes are excluded
            // here, which is why simple never coexists with them at one
            // site.)
            let clean = content_start < content_end
                && bytes[content_start..content_end]
                    .iter()
                    .all(|&b| !matches!(b, b']' | b'|' | b'>' | b'-'));
            if clean {
                links.push(CoreLink {
                    form: CoreLinkForm::Simple,
                    display_span: None,
                    target_span: content_start..content_end,
                    span: site..end,
                });
                matched = true;
            }
        }

        // Advance: past the whole match when one was emitted (skipping
        // any `[[` inside it — the old captures_iter non-overlap), else
        // one byte so overlapping `[[` sites are each considered.
        if matched {
            i = end;
        } else {
            i = site + 1;
        }
    }

    links
}

/// Find the leftmost position `p` in `[from, to)` such that the byte pair
/// starting at `p` satisfies `pred`. `to` is exclusive; the caller
/// derives it from the closing-brace position so the split leaves a
/// non-empty target side. ASCII separators never occur inside multi-byte
/// UTF-8 sequences, so byte-wise scanning is slice-safe.
fn find_separator(
    bytes: &[u8],
    from: usize,
    to: usize,
    pred: impl Fn(u8, u8) -> bool,
) -> Option<usize> {
    let mut p = from;
    while p < to {
        if let Some(&next) = bytes.get(p + 1)
            && pred(bytes[p], next)
        {
            return Some(p);
        }
        p += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (form, display, target, full span) for compact assertions.
    fn collected(src: &str) -> Vec<(CoreLinkForm, Option<String>, String, Range<usize>)> {
        scan_core_links(src)
            .into_iter()
            .map(|l| {
                (
                    l.form,
                    l.display(src).map(str::to_string),
                    l.target(src).to_string(),
                    l.span,
                )
            })
            .collect()
    }

    /// Helper for expected tuples: `(form, display, target, start, end)`.
    fn expected(
        form: CoreLinkForm,
        display: Option<&str>,
        target: &str,
        span: Range<usize>,
    ) -> (CoreLinkForm, Option<String>, String, Range<usize>) {
        (form, display.map(str::to_string), target.to_string(), span)
    }

    #[test]
    fn simple_arrow_pipe_basics() {
        let out = collected("Go to [[Forest]] or [[Cave->Dark Woods]] or [[Cave|Woods]]");
        assert_eq!(out.len(), 3, "{out:?}");
        assert_eq!(
            out[0],
            expected(CoreLinkForm::Simple, None, "Forest", 6..16),
            "{out:?}"
        );
        assert_eq!(
            out[1],
            expected(CoreLinkForm::Arrow, Some("Cave"), "Dark Woods", 20..40),
            "{out:?}"
        );
        assert_eq!(
            out[2],
            expected(CoreLinkForm::Pipe, Some("Cave"), "Woods", 44..58),
            "{out:?}"
        );
    }

    #[test]
    fn arrow_takes_first_separator_target_runs_to_close() {
        // Display stops at the FIRST `->`; the target may contain `->`,
        // `|`, and `[` (the regex classes only excluded `]`).
        let out = collected("[[a->b->c]]");
        assert_eq!(
            out,
            vec![expected(CoreLinkForm::Arrow, Some("a"), "b->c", 0..11)]
        );
    }

    #[test]
    fn arrow_and_pipe_both_match_ambiguous_content() {
        // Behavior parity with the old regex runs: both interpretations
        // were collected. The scanner preserves the quirk.
        let out = collected("[[a|b->c]]");
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(
            out[0],
            expected(CoreLinkForm::Arrow, Some("a|b"), "c", 0..10)
        );
        assert_eq!(
            out[1],
            expected(CoreLinkForm::Pipe, Some("a"), "b->c", 0..10)
        );
    }

    #[test]
    fn embedded_site_inside_a_match_is_skipped() {
        // The arrow consumes through the first `]]`; the `[[c]]` inside
        // its target produced no separate simple link (the old simple-run
        // dedupe check — span coverage — and the non-overlap progression
        // agree on this).
        let out = collected("[[a->b [[c]]");
        assert_eq!(
            out,
            vec![expected(CoreLinkForm::Arrow, Some("a"), "b [[c", 0..12)]
        );
    }

    #[test]
    fn overlapping_sites_leftmost_wins() {
        // `[[[a]]` contains sites 0 and 1; site 0 matches (content `[a`
        // is clean for the simple form), consuming the whole spelling —
        // the same result the regex's leftmost non-overlapping match gave.
        let out = collected("[[[a]]");
        assert_eq!(out, vec![expected(CoreLinkForm::Simple, None, "[a", 0..6)]);
    }

    #[test]
    fn lone_bracket_kills_every_form_at_the_site() {
        // The first `]` after the site is not followed by `]` — no form
        // can cross it (all the old char classes excluded `]`).
        let out = collected("[[a]x->b]]");
        assert!(out.is_empty(), "{out:?}");
        // But a later site still matches.
        let out = collected("[[a]x->b]] [[ok]]");
        assert_eq!(
            out,
            vec![expected(CoreLinkForm::Simple, None, "ok", 11..17)]
        );
    }

    #[test]
    fn empty_display_or_target_fails_that_form() {
        // `[[->a]]`: the only `->` sits right after `[[` — display empty.
        let out = collected("[[->a]]");
        assert!(out.is_empty(), "{out:?}");
        // `[[a->]]`: target empty.
        let out = collected("[[a->]]");
        assert!(out.is_empty(), "{out:?}");
        // `[[a->->b]]`: the FIRST `->` leaves a non-empty target (`->b`),
        // so it wins.
        let out = collected("[[a->->b]]");
        assert_eq!(
            out,
            vec![expected(CoreLinkForm::Arrow, Some("a"), "->b", 0..10)]
        );
        // The display-grows-through-the-separator case: display may
        // contain `-` and `>` (only `]` was excluded), so `[[->->b]]`
        // matches with display `->` at the SECOND separator.
        let out = collected("[[->->b]]");
        assert_eq!(
            out,
            vec![expected(CoreLinkForm::Arrow, Some("->"), "b", 0..9)]
        );
        // Pipe equivalents.
        assert!(collected("[[|a]]").is_empty());
        assert!(collected("[[a|]]").is_empty());
    }

    #[test]
    fn one_char_target_on_pipe_boundary() {
        // The pipe bound is one byte tighter than the arrow bound: a
        // single-byte target must still match.
        let out = collected("[[a|b]]");
        assert_eq!(
            out,
            vec![expected(CoreLinkForm::Pipe, Some("a"), "b", 0..7)]
        );
        let out = collected("[[a->b]]");
        assert_eq!(
            out,
            vec![expected(CoreLinkForm::Arrow, Some("a"), "b", 0..8)]
        );
    }

    #[test]
    fn simple_content_excludes_pipe_arrow_chars() {
        // `-`, `>`, `|` are not valid in the simple form's content —
        // `[[a-b]]` produced no link under the regexes either.
        assert!(collected("[[a-b]]").is_empty());
        assert!(collected("[[a>b]]").is_empty());
        // `[` IS allowed (only `]`, `|`, `>`, `-` were excluded).
        let out = collected("[[a[b]]");
        assert_eq!(out, vec![expected(CoreLinkForm::Simple, None, "a[b", 0..7)]);
    }

    #[test]
    fn unterminated_link_is_not_a_link() {
        assert!(collected("[[Forest").is_empty());
        assert_eq!(collected("[[Forest]] and [[Cave").len(), 1);
    }

    #[test]
    fn multiple_links_in_document_order() {
        let out = collected("[[a]] middle [[b|c]] tail [[d->e]]");
        assert_eq!(
            out,
            vec![
                expected(CoreLinkForm::Simple, None, "a", 0..5),
                expected(CoreLinkForm::Pipe, Some("b"), "c", 13..20),
                expected(CoreLinkForm::Arrow, Some("d"), "e", 26..34),
            ]
        );
    }

    #[test]
    fn adjacent_and_nested_closers() {
        // The first `]]` closes; the rest is ordinary text to rescan.
        let out = collected("[[a]]]]");
        assert_eq!(out, vec![expected(CoreLinkForm::Simple, None, "a", 0..5)]);
        // `[[a]]b]]` — one link; the stray `b]]` is not a site.
        let out = collected("[[a]]b]]");
        assert_eq!(out, vec![expected(CoreLinkForm::Simple, None, "a", 0..5)]);
    }

    #[test]
    fn never_panics_on_edge_inputs() {
        for src in [
            "",
            "[",
            "[]",
            "[[",
            "[[]",
            "[[]]",
            "[[[[[[[[",
            "]]]]]]]]",
            "[[a",
            "[[|",
            "[[->",
            "[[-",
            "日本語[[passage]]テキスト",
            "[[日本語]]",
            "[[a->日本語]]",
        ] {
            let _ = scan_core_links(src);
        }
        // Unicode content round-trips through the spans.
        let out = collected("[[日本語->English]]");
        assert_eq!(
            out,
            vec![expected(
                CoreLinkForm::Arrow,
                Some("日本語"),
                "English",
                0..22
            )]
        );
        let out = collected("[[日本語]]");
        assert_eq!(
            out,
            vec![expected(CoreLinkForm::Simple, None, "日本語", 0..13)]
        );
    }
}
