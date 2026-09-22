//! Known CSS pseudo-class/pseudo-element registry — the lint list behind
//! the unknown-pseudo and pseudo-element-placement diagnostics.
//!
//! ## Purpose
//!
//! CSS syntax validation catches *structural* mistakes; it cannot catch
//! *semantic* ones. A perfectly-parsed `a:hvoer { … }` is still wrong —
//! and worse than an unknown property, because a selector containing an
//! unknown pseudo-class makes browsers silently drop the **entire rule**
//! (selectors-4 §3.9: "an invalid selector … matches nothing"). This
//! registry is the allow-list [`unknown_pseudo_diagnostics`] checks
//! pseudo names against: unprefixed names must appear here, while
//! vendor-prefixed spellings (`:-moz-…`, `::-webkit-…`) are always
//! accepted (see [`is_lintable_pseudo_name`]).
//!
//! ## Coverage policy — generous beats strict
//!
//! Same calculus as [`super::properties`]: a false positive costs trust,
//! a false negative costs one squiggle. The lists therefore include:
//!
//! * every Selectors Level 3 + Level 4 pseudo-class (including the
//!   functional ones — `:not`, `:is`, `:where`, `:has`, `:nth-*`,
//!   `:lang`, `:dir` — judged by name, arguments are the parser's job),
//! * the HTML form-state pseudos (`:valid`, `:autofill`, …), media
//!   state pseudos (`:playing`, `:buffering`, …), fullscreen/modal/
//!   picture-in-picture, `:user-invalid`/`:user-valid`,
//! * the **legacy single-colon pseudo-element spellings** (`:before`,
//!   `:after`, `:first-line`, `:first-letter`) — they parse as
//!   pseudo-class nodes and are legal CSS, so they sit in the CLASS
//!   list on purpose (not a mistake),
//! * the `@page` margin-box pseudo-classes (`:first`, `:left`,
//!   `:right`, `:blank`, `:nth`) — the real parser emits `@page`
//!   pseudos as Selector tokens that include their leading colon,
//! * every shipped pseudo-element from the CSS snapshot plus the
//!   younger ones authors already use (`::details-content`,
//!   `::scroll-marker`, the `::view-transition-*` family,
//!   `::snippet`, `::highlight`).
//!
//! ## What the lint checks
//!
//! 1. **Unknown names** — a pseudo name that is a plain identifier
//!    but not in the matching registry (class vs element is decided
//!    by the colon count in the source: one `:` = class, `::` =
//!    element; the `@page` shape carries the colon inside the token).
//! 2. **Placement** — a pseudo-element followed by another simple
//!    selector *in the same compound* (no whitespace/combinator
//!    between, only colons). Selectors-4 §3.2.1: "Only one
//!    pseudo-element may appear per compound selector, and if
//!    present it must appear after all other simple selectors" —
//!    violating compounds are dropped whole by browsers
//!    (`div::before:hover` matches nothing). Vendor-prefixed
//!    pseudo-elements are exempt: `::-webkit-scrollbar-thumb:hover`
//!    is accepted by its vendor engine and Knot will not fight it.
//!
//! ## Why the token stream (and not the AST walk)
//!
//! The lint runs **after** [`super::parser`]'s classification, over
//! the sorted `CssToken` stream. Pseudo names are the only
//! [`CssTokenKind::Selector`] tokens whose source text either starts
//! with a colon (`@page :first`) or sits directly behind one
//! (`a:hover`, `::before`) — value-position idents are `Keyword`
//! tokens, so `.a { color:red }` cannot misfire. This shape holds
//! for every selector emission site in the parser walk (class/id/
//! type/attribute names, `:not()` argument compounds, combinator
//! gaps, keyframe `from`/`to`), and was probed empirically before
//! this module was written.
//!
//! ## Where the lint does NOT run
//!
//! Only [`super::parser::parse_css`]'s real-parser path calls it.
//! The tolerant fallback scanner ([`super::fallback`]) is skipped:
//! without an AST the scanner classifies rule-level idents by block
//! phase alone, and a degenerate top-level `color:red` (hard parse
//! failure) would present `red` as a colon-adjacent Selector — the
//! exact false-positive class this module exists to avoid. The
//! declaration-list path ([`super::fallback::parse_css_declarations`])
//! has no selectors to judge at all.
//!
//! [`unknown_pseudo_diagnostics`]: fn.unknown_pseudo_diagnostics
//! [`is_lintable_pseudo_name`]: fn.is_lintable_pseudo_name

use super::types::{CssDiagnostic, CssDiagnosticSeverity, CssToken, CssTokenKind};

/// Known CSS pseudo-class names (lowercase, sorted — see the module docs).
///
/// Includes the legacy single-colon pseudo-element spellings and the
/// `@page` pseudo-classes; see the coverage policy. Kept sorted for
/// `binary_search`; the `class_list_is_sorted` test pins this invariant.
const KNOWN_PSEUDO_CLASSES: &[&str] = &[
    "active",
    "after",
    "any-link",
    "autofill",
    "before",
    "blank",
    "buffering",
    "checked",
    "closed",
    "current",
    "default",
    "defined",
    "dir",
    "disabled",
    "empty",
    "enabled",
    "first",
    "first-child",
    "first-letter",
    "first-of-type",
    "focus",
    "focus-visible",
    "focus-within",
    "fullscreen",
    "future",
    "has",
    "host",
    "host-context",
    "hover",
    "in-range",
    "indeterminate",
    "invalid",
    "is",
    "lang",
    "last-child",
    "last-of-type",
    "left",
    "link",
    "matches",
    "modal",
    "muted",
    "not",
    "nth",
    "nth-child",
    "nth-col",
    "nth-last-child",
    "nth-last-col",
    "nth-last-of-type",
    "nth-of-type",
    "only-child",
    "only-of-type",
    "open",
    "optional",
    "out-of-range",
    "past",
    "paused",
    "picture-in-picture",
    "placeholder-shown",
    "playing",
    "read-only",
    "read-write",
    "required",
    "right",
    "root",
    "scope",
    "seeking",
    "stalled",
    "target",
    "target-text",
    "target-within",
    "user-invalid",
    "user-valid",
    "valid",
    "visited",
    "volume-locked",
    "where",
];

/// Known CSS pseudo-element names (lowercase, sorted — see the module
/// docs). The legacy spellings live in [`KNOWN_PSEUDO_CLASSES`] because
/// they parse behind a single colon.
const KNOWN_PSEUDO_ELEMENTS: &[&str] = &[
    "after",
    "backdrop",
    "before",
    "cue",
    "cue-region",
    "details-content",
    "file-selector-button",
    "first-letter",
    "first-line",
    "grammar-error",
    "highlight",
    "marker",
    "part",
    "placeholder",
    "region",
    "scroll-marker",
    "scroll-marker-group",
    "selection",
    "slotted",
    "snippet",
    "spelling-error",
    "target-text",
    "view-transition-group",
    "view-transition-image-pair",
    "view-transition-new",
    "view-transition-old",
];

/// Case-insensitive membership test against [`KNOWN_PSEUDO_CLASSES`].
///
/// Vendor-prefixed names (`-moz-any`, `-webkit-autofill`) always pass —
/// the lint targets unprefixed standard names only.
pub fn is_known_pseudo_class(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with('-') {
        return true;
    }
    KNOWN_PSEUDO_CLASSES.binary_search(&lower.as_str()).is_ok()
}

/// Case-insensitive membership test against [`KNOWN_PSEUDO_ELEMENTS`].
///
/// Vendor-prefixed names (`-webkit-scrollbar-thumb`) always pass.
pub fn is_known_pseudo_element(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with('-') {
        return true;
    }
    KNOWN_PSEUDO_ELEMENTS.binary_search(&lower.as_str()).is_ok()
}

/// Whether `name` is a plain, unprefixed identifier the lint may judge.
///
/// Mirrors [`super::properties`]'s lintability rule: only ASCII
/// identifier bytes (`[a-zA-Z0-9-]`, first byte alphabetic or the
/// vendor hyphen). Escaped (`\61 z`), interpolated (`#{…}`) or empty
/// spellings return `false` — the lint never flags what it cannot
/// spell back to the author.
fn is_lintable_pseudo_name(name: &str) -> bool {
    let Some(&first) = name.as_bytes().first() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != b'-' {
        return false;
    }
    !name
        .bytes()
        .any(|b| !(b.is_ascii_alphanumeric() || b == b'-'))
}

/// One pseudo use recovered from the token stream.
struct PseudoUse {
    /// The pseudo NAME (no colons), as written.
    name: String,
    /// Source-relative range covering the colons AND the name — the
    /// squiggle the author sees (`:hvoer`, `::befor`).
    range: std::ops::Range<usize>,
    /// Double-colon (`::`) or single-colon (`:`).
    is_element: bool,
    /// Vendor-prefixed name (`-…`) — exempt from the placement check.
    vendor: bool,
}

/// Recover the pseudo uses from a classified token stream.
///
/// Two shapes (see the module docs):
/// - the token text starts with colons (the `@page :first` shape —
///   the parser includes the sigil in the span), and
/// - the byte(s) before the token span are colons (`a:hover`,
///   `p::before` — the span covers the name only; the range is
///   extended back over the colon run for the diagnostic).
fn collect_pseudo_uses(tokens: &[CssToken], source: &str) -> Vec<PseudoUse> {
    let bytes = source.as_bytes();
    let mut uses = Vec::new();
    for token in tokens {
        if token.kind != CssTokenKind::Selector {
            continue;
        }
        let start = token.span.start.min(source.len());
        let end = token.span.end.min(source.len());
        if start >= end || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
            continue; // degenerate span — nothing to judge
        }
        let text = &source[start..end];
        if text.starts_with(':') {
            // @page shape — colons inside the token.
            let name_start = start + text.len() - text.trim_start_matches(':').len();
            let name = &text[name_start - start..];
            if name.is_empty() {
                continue;
            }
            uses.push(PseudoUse {
                name: name.to_string(),
                range: start..end,
                is_element: text.starts_with("::"),
                vendor: name.starts_with('-'),
            });
        } else {
            // Name-only shape — count the colon run before the span.
            let mut colon_start = start;
            while colon_start > 0 && bytes[colon_start - 1] == b':' {
                colon_start -= 1;
            }
            if colon_start == start {
                continue; // not colon-adjacent — a plain selector part
            }
            uses.push(PseudoUse {
                name: text.to_string(),
                range: colon_start..end,
                is_element: start - colon_start >= 2,
                vendor: text.starts_with('-'),
            });
        }
    }
    uses
}

/// Lint a classified token stream for selector mistakes: unknown
/// pseudo-class/pseudo-element names and misplaced pseudo-elements.
///
/// Returns Warning-severity [`CssDiagnostic`]s in the same
/// source-relative coordinate system as the tokens (the caller's
/// shifting contract applies unchanged):
///
/// - `Unknown CSS pseudo-class: `:name`` / `Unknown CSS pseudo-element:
///   `::name`` — unknown unprefixed name;
/// - `CSS pseudo-element must be the last part of the selector: `::name``
///   — a non-vendor pseudo-element followed by another simple selector
///   of the same compound (browsers drop the whole rule).
///
/// Skipped (never flagged): vendor-prefixed names, non-identifier
/// spellings (escapes, interpolation), and every token that is not
/// colon-adjacent (class names, tag names, keyframe selectors, …).
pub fn unknown_pseudo_diagnostics(tokens: &[CssToken], source: &str) -> Vec<CssDiagnostic> {
    let mut out = Vec::new();
    let uses = collect_pseudo_uses(tokens, source);

    // Unknown names.
    for u in &uses {
        if !is_lintable_pseudo_name(&u.name) {
            continue;
        }
        let known = if u.is_element {
            is_known_pseudo_element(&u.name)
        } else {
            is_known_pseudo_class(&u.name)
        };
        if !known {
            let sigil = if u.is_element { "::" } else { ":" };
            out.push(CssDiagnostic {
                message: format!(
                    "Unknown CSS pseudo-{}: `{sigil}{}` — browsers drop the whole rule",
                    if u.is_element { "element" } else { "class" },
                    u.name
                ),
                range: u.range.clone(),
                severity: CssDiagnosticSeverity::Warning,
            });
        }
    }

    // Placement: a pseudo-element followed by another simple selector of
    // the same compound (the gap between the two tokens is only colons).
    // `collect_pseudo_uses` returns stream order, so uses[i+1] is the
    // next Selector token after uses[i].
    for window in uses.windows(2) {
        let (elem, next) = (&window[0], &window[1]);
        if !elem.is_element || elem.vendor {
            continue;
        }
        let gap = &source[elem.range.end.min(source.len())..next.range.start.min(source.len())];
        if !gap.bytes().all(|b| b == b':') {
            continue; // whitespace/combinator between — different compounds
        }
        out.push(CssDiagnostic {
            message: format!(
                "CSS pseudo-element must be the last part of the selector: `::{}` — as written, browsers drop the whole rule",
                elem.name
            ),
            range: elem.range.clone(),
            severity: CssDiagnosticSeverity::Warning,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector_token(span: std::ops::Range<usize>) -> CssToken {
        CssToken {
            kind: CssTokenKind::Selector,
            span,
        }
    }

    #[test]
    fn class_list_is_sorted() {
        assert!(KNOWN_PSEUDO_CLASSES.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn element_list_is_sorted() {
        assert!(KNOWN_PSEUDO_ELEMENTS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn standard_names_are_known() {
        for name in [
            "hover",
            "first-child",
            "nth-last-of-type",
            "placeholder-shown",
            "focus-visible",
            "user-invalid",
            "before", // legacy single-colon pseudo-element
            "first",  // @page
            "playing",
        ] {
            assert!(is_known_pseudo_class(name), "{name} must be known");
        }
        for name in [
            "before",
            "selection",
            "file-selector-button",
            "details-content",
            "view-transition-image-pair",
            "scroll-marker",
        ] {
            assert!(is_known_pseudo_element(name), "{name} must be known");
        }
    }

    #[test]
    fn casing_and_vendor_pass() {
        assert!(is_known_pseudo_class("HOVER"));
        assert!(is_known_pseudo_class("Hover"));
        assert!(is_known_pseudo_class("-moz-any"));
        assert!(is_known_pseudo_element("-webkit-scrollbar-thumb"));
    }

    #[test]
    fn typos_are_unknown() {
        assert!(!is_known_pseudo_class("hvoer"));
        assert!(!is_known_pseudo_class("foucs"));
        assert!(!is_known_pseudo_element("befor"));
        assert!(!is_known_pseudo_element("selectoin"));
    }

    #[test]
    fn lint_flags_unknown_class_and_element() {
        // "a:hvoer { color: red }"
        let src = "a:hvoer { color: red }";
        let tokens = [selector_token(0..1), selector_token(2..7)];
        let diags = unknown_pseudo_diagnostics(&tokens, src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].range, 1..7, "covers `:hvoer`");
        assert!(
            diags[0].message.contains("pseudo-class"),
            "{}",
            diags[0].message
        );
        assert!(diags[0].message.contains("hvoer"));
        assert_eq!(diags[0].severity, CssDiagnosticSeverity::Warning);

        // "p::befor { }"
        let src = "p::befor { }";
        let tokens = [selector_token(0..1), selector_token(3..8)];
        let diags = unknown_pseudo_diagnostics(&tokens, src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].range, 1..8, "covers `::befor`");
        assert!(
            diags[0].message.contains("pseudo-element"),
            "{}",
            diags[0].message
        );
    }

    #[test]
    fn lint_accepts_known_vendor_and_page_shapes() {
        // ":-moz-any(.a) { }" — vendor passes; "@page :first { }" — the
        // token itself carries the colon (class, known).
        let src = ":-moz-any(.a) { }";
        let tokens = [selector_token(1..9)];
        assert!(unknown_pseudo_diagnostics(&tokens, src).is_empty());

        let src = "@page :first { }";
        let tokens = [selector_token(6..12)];
        assert!(unknown_pseudo_diagnostics(&tokens, src).is_empty());
    }

    #[test]
    fn lint_ignores_plain_selector_parts() {
        // `.a { color:red }` — `red` is a Keyword in the real stream and
        // even a stray Selector token here is NOT colon-adjacent (space);
        // `color` is preceded by `{ `.
        let src = ".a b, .c { }";
        let tokens = [
            selector_token(0..2),
            selector_token(3..4),
            selector_token(6..8),
        ];
        assert!(unknown_pseudo_diagnostics(&tokens, src).is_empty());
    }

    #[test]
    fn lint_flags_pseudo_element_not_last() {
        // "div::before:hover { color: red }" — element followed by class.
        let src = "div::before:hover { color: red }";
        let tokens = [
            selector_token(0..3),
            selector_token(5..11),
            selector_token(12..17),
        ];
        let diags = unknown_pseudo_diagnostics(&tokens, src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0].message.contains("last part"),
            "{}",
            diags[0].message
        );
        assert_eq!(diags[0].range, 3..11, "covers `::before`");
    }

    #[test]
    fn lint_accepts_class_then_element_and_descendant() {
        // "div:hover::before { }" — valid order.
        let src = "div:hover::before { }";
        let tokens = [
            selector_token(0..3),
            selector_token(4..9),
            selector_token(11..17),
        ];
        assert!(unknown_pseudo_diagnostics(&tokens, src).is_empty());

        // "div::before :hover { }" — descendant combinator (space):
        // different compounds, valid.
        let src = "div::before :hover { }";
        let tokens = [
            selector_token(0..3),
            selector_token(5..11),
            selector_token(13..18),
        ];
        assert!(unknown_pseudo_diagnostics(&tokens, src).is_empty());
    }

    #[test]
    fn lint_exempts_vendor_elements_from_placement() {
        // "::-webkit-scrollbar-thumb:hover { }" — accepted by its vendor
        // engine; Knot stays silent (generous policy).
        let src = "::-webkit-scrollbar-thumb:hover { }";
        let tokens = [selector_token(2..25), selector_token(26..31)];
        assert!(unknown_pseudo_diagnostics(&tokens, src).is_empty());
    }

    #[test]
    fn non_identifier_names_are_never_judged() {
        // Escaped / degenerate spellings pass untouched.
        let src = ":\\61 z { }";
        let tokens = [selector_token(1..7)];
        assert!(unknown_pseudo_diagnostics(&tokens, src).is_empty());
    }
}
