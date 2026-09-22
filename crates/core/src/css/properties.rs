//! Known CSS property registry — the lint list behind the
//! unknown-property diagnostic.
//!
//! ## Purpose
//!
//! CSS syntax validation catches *structural* mistakes (missing
//! braces, bad strings); it cannot catch *semantic* ones — a
//! perfectly-parsed `colr: red` is still wrong. This registry is
//! the allow-list the unknown-property lint checks declaration
//! names against: unprefixed names must appear here, while
//! custom properties (`--x`) and vendor-prefixed spellings
//! (`-webkit-…`, `-moz-…`) are always accepted (see
//! [`unknown_property_diagnostics`]).
//!
//! ## Coverage policy — generous beats strict
//!
//! A false positive (flagging a real property) costs the author
//! trust in every other diagnostic; a false negative (missing a
//! typo) costs one squiggle. The list therefore includes:
//!
//! * every CSS 2.1 property (including the aural set — `volume`,
//!   `pitch`, `cue`, …),
//! * every Layout/Box/Border/Background/Text/Typography property
//!   from the CSS Snapshot modules (flex, grid, logical
//!   properties, fragmentation, writing modes),
//! * transitions/animations/scroll- and view-timelines, container
//!   queries, view transitions, masking, filters, motion paths,
//! * at-rule **descriptors** that surface as declaration names
//!   (`@font-face`'s `src`/`size-adjust`, `@counter-style`'s
//!   `system`/`symbols`, `@property`'s `syntax`/`inherits`,
//!   `@page`'s `size`/`marks`/`bleed`, `@viewport`'s `zoom`),
//! * the SVG presentation attributes that parse as declarations
//!   (`fill`, `stroke`, `paint-order`, …).
//!
//! ## Maintenance
//!
//! The list is a **sorted static array** — [`is_known_property_name`]
//! binary-searches it, and the `list_is_sorted` test pins the
//! ordering so insertions fail loudly instead of silently
//! disabling lookups. Add new properties in their alphabetical
//! position; the test below catches placement mistakes.
//!
//! [`unknown_property_diagnostics`]: fn.unknown_property_diagnostics
//! [`is_known_property_name`]: fn.is_known_property_name

use super::types::{CssDiagnostic, CssDiagnosticSeverity, CssToken, CssTokenKind};

/// Known CSS property names (lowercase, sorted — see the module docs).
///
/// Includes standard properties, common at-rule descriptors, and the
/// SVG-presentation set. Kept sorted for `binary_search`; the
/// `list_is_sorted` test pins this invariant.
const KNOWN_PROPERTIES: &[&str] = &[
    "accent-color",
    "additive-symbols",
    "align-content",
    "align-items",
    "align-self",
    "alignment-baseline",
    "all",
    "allow-discrete-keyframes",
    "anchor-name",
    "animation",
    "animation-composition",
    "animation-delay",
    "animation-direction",
    "animation-duration",
    "animation-fill-mode",
    "animation-iteration-count",
    "animation-name",
    "animation-play-state",
    "animation-range",
    "animation-range-end",
    "animation-range-start",
    "animation-timeline",
    "animation-timing-function",
    "appearance",
    "ascent-override",
    "aspect-ratio",
    "azimuth",
    "backdrop-filter",
    "backface-visibility",
    "background",
    "background-attachment",
    "background-blend-mode",
    "background-clip",
    "background-color",
    "background-image",
    "background-origin",
    "background-position",
    "background-position-x",
    "background-position-y",
    "background-repeat",
    "background-size",
    "baseline-shift",
    "baseline-source",
    "binding",
    "bleed",
    "block-size",
    "border",
    "border-block",
    "border-block-color",
    "border-block-end",
    "border-block-end-color",
    "border-block-end-style",
    "border-block-end-width",
    "border-block-start",
    "border-block-start-color",
    "border-block-start-style",
    "border-block-start-width",
    "border-block-style",
    "border-block-width",
    "border-bottom",
    "border-bottom-color",
    "border-bottom-left-radius",
    "border-bottom-right-radius",
    "border-bottom-style",
    "border-bottom-width",
    "border-collapse",
    "border-color",
    "border-end-end-radius",
    "border-end-start-radius",
    "border-fit",
    "border-image",
    "border-image-outset",
    "border-image-repeat",
    "border-image-slice",
    "border-image-source",
    "border-image-width",
    "border-inline",
    "border-inline-color",
    "border-inline-end",
    "border-inline-end-color",
    "border-inline-end-style",
    "border-inline-end-width",
    "border-inline-start",
    "border-inline-start-color",
    "border-inline-start-style",
    "border-inline-start-width",
    "border-inline-style",
    "border-inline-width",
    "border-left",
    "border-left-color",
    "border-left-style",
    "border-left-width",
    "border-radius",
    "border-right",
    "border-right-color",
    "border-right-style",
    "border-right-width",
    "border-spacing",
    "border-start-end-radius",
    "border-start-start-radius",
    "border-style",
    "border-top",
    "border-top-color",
    "border-top-left-radius",
    "border-top-right-radius",
    "border-top-style",
    "border-top-width",
    "border-width",
    "bottom",
    "box-decoration-break",
    "box-shadow",
    "box-sizing",
    "break-after",
    "break-before",
    "break-inside",
    "caption-side",
    "caret-color",
    "caret-shape",
    "clear",
    "clip-path",
    "clip-rule",
    "color",
    "color-interpolation",
    "color-rendering",
    "color-scheme",
    "column-count",
    "column-fill",
    "column-gap",
    "column-rule",
    "column-rule-color",
    "column-rule-style",
    "column-rule-width",
    "column-span",
    "column-width",
    "columns",
    "contain",
    "contain-intrinsic-block-size",
    "contain-intrinsic-height",
    "contain-intrinsic-inline-size",
    "contain-intrinsic-size",
    "contain-intrinsic-width",
    "container",
    "container-name",
    "container-type",
    "content",
    "content-visibility",
    "continue",
    "counter-increment",
    "counter-reset",
    "counter-set",
    "cue",
    "cue-after",
    "cue-before",
    "cursor",
    "descent-override",
    "direction",
    "display",
    "dominant-baseline",
    "drop-shadow",
    "elevation",
    "empty-cells",
    "fallback",
    "field-sizing",
    "fill",
    "fill-opacity",
    "fill-rule",
    "filter",
    "flex",
    "flex-basis",
    "flex-direction",
    "flex-flow",
    "flex-grow",
    "flex-shrink",
    "flex-wrap",
    "float",
    "flow-from",
    "flow-into",
    "font",
    "font-display",
    "font-family",
    "font-feature-settings",
    "font-kerning",
    "font-language-override",
    "font-optical-sizing",
    "font-palette",
    "font-size",
    "font-size-adjust",
    "font-stretch",
    "font-style",
    "font-synthesis",
    "font-synthesis-small-caps",
    "font-synthesis-style",
    "font-synthesis-weight",
    "font-variant",
    "font-variant-alternates",
    "font-variant-caps",
    "font-variant-east-asian",
    "font-variant-emoji",
    "font-variant-ligatures",
    "font-variant-numeric",
    "font-variant-position",
    "font-variation-settings",
    "font-weight",
    "forced-color-adjust",
    "gap",
    "glyph-orientation-horizontal",
    "glyph-orientation-vertical",
    "grid",
    "grid-area",
    "grid-auto-columns",
    "grid-auto-flow",
    "grid-auto-position",
    "grid-auto-rows",
    "grid-column",
    "grid-column-end",
    "grid-column-start",
    "grid-line",
    "grid-row",
    "grid-row-end",
    "grid-row-start",
    "grid-template",
    "grid-template-areas",
    "grid-template-columns",
    "grid-template-rows",
    "hanging-punctuation",
    "height",
    "hyphenate-character",
    "hyphenate-limit-chars",
    "hyphens",
    "image-orientation",
    "image-rendering",
    "image-resolution",
    "inherits",
    "initial-value",
    "inline-size",
    "input-security",
    "inset",
    "inset-block",
    "inset-block-end",
    "inset-block-start",
    "inset-inline",
    "inset-inline-end",
    "inset-inline-start",
    "interpolate-size",
    "isolation",
    "justify-content",
    "justify-items",
    "justify-self",
    "left",
    "letter-spacing",
    "line-break",
    "line-clamp",
    "line-gap-override",
    "line-height",
    "list-style",
    "list-style-image",
    "list-style-position",
    "list-style-type",
    "margin",
    "margin-block",
    "margin-block-end",
    "margin-block-start",
    "margin-bottom",
    "margin-inline",
    "margin-inline-end",
    "margin-inline-start",
    "margin-left",
    "margin-right",
    "margin-top",
    "marker",
    "marker-side",
    "marks",
    "mask",
    "mask-border",
    "mask-border-mode",
    "mask-border-outset",
    "mask-border-repeat",
    "mask-border-slice",
    "mask-border-source",
    "mask-border-width",
    "mask-clip",
    "mask-composite",
    "mask-image",
    "mask-mode",
    "mask-origin",
    "mask-position",
    "mask-position-x",
    "mask-position-y",
    "mask-repeat",
    "mask-size",
    "mask-type",
    "math-depth",
    "math-shift",
    "math-style",
    "math-variant",
    "max-block-size",
    "max-height",
    "max-inline-size",
    "max-lines",
    "max-width",
    "min-block-size",
    "min-height",
    "min-inline-size",
    "min-width",
    "mix-blend-mode",
    "navigation",
    "negative",
    "object-fit",
    "object-position",
    "object-view-box",
    "offset",
    "offset-anchor",
    "offset-distance",
    "offset-path",
    "offset-position",
    "offset-rotate",
    "opacity",
    "order",
    "orientation",
    "orphans",
    "outline",
    "outline-color",
    "outline-offset",
    "outline-style",
    "outline-width",
    "overflow",
    "overflow-block",
    "overflow-clip-margin",
    "overflow-inline",
    "overflow-wrap",
    "overflow-x",
    "overflow-y",
    "overscroll-behavior",
    "overscroll-behavior-block",
    "overscroll-behavior-inline",
    "overscroll-behavior-x",
    "overscroll-behavior-y",
    "pad",
    "padding",
    "padding-block",
    "padding-block-end",
    "padding-block-start",
    "padding-bottom",
    "padding-inline",
    "padding-inline-end",
    "padding-inline-start",
    "padding-left",
    "padding-right",
    "padding-top",
    "page",
    "page-orientation",
    "paint-order",
    "pause",
    "pause-after",
    "pause-before",
    "perspective",
    "perspective-origin",
    "pitch",
    "pitch-range",
    "place-content",
    "place-items",
    "place-self",
    "play-during",
    "pointer-events",
    "position",
    "position-anchor",
    "position-area",
    "position-try",
    "position-try-order",
    "position-visibility",
    "prefix",
    "print-color-adjust",
    "quotes",
    "range",
    "reading-flow",
    "region-fragment",
    "resize",
    "richness",
    "right",
    "rotate",
    "row-gap",
    "ruby-align",
    "ruby-merge",
    "ruby-overhang",
    "ruby-position",
    "scale",
    "scroll-behavior",
    "scroll-margin",
    "scroll-margin-block",
    "scroll-margin-block-end",
    "scroll-margin-block-start",
    "scroll-margin-bottom",
    "scroll-margin-inline",
    "scroll-margin-inline-end",
    "scroll-margin-inline-start",
    "scroll-margin-left",
    "scroll-margin-right",
    "scroll-margin-top",
    "scroll-padding",
    "scroll-padding-block",
    "scroll-padding-block-end",
    "scroll-padding-block-start",
    "scroll-padding-bottom",
    "scroll-padding-inline",
    "scroll-padding-inline-end",
    "scroll-padding-inline-start",
    "scroll-padding-left",
    "scroll-padding-right",
    "scroll-padding-top",
    "scroll-snap-align",
    "scroll-snap-stop",
    "scroll-snap-type",
    "scroll-timeline",
    "scroll-timeline-axis",
    "scroll-timeline-name",
    "scrollbar-color",
    "scrollbar-gutter",
    "scrollbar-width",
    "shape-image-threshold",
    "shape-inside",
    "shape-margin",
    "shape-outside",
    "shape-rendering",
    "sideways-lr",
    "sideways-rl",
    "size",
    "size-adjust",
    "speak",
    "speak-as",
    "speak-header",
    "speak-numeral",
    "speak-punctuation",
    "src",
    "stress",
    "stroke",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-opacity",
    "stroke-width",
    "suffix",
    "symbols",
    "syntax",
    "system",
    "tab-size",
    "table-layout",
    "text-align",
    "text-align-last",
    "text-box",
    "text-box-edge",
    "text-box-trim",
    "text-combine-upright",
    "text-decoration",
    "text-decoration-color",
    "text-decoration-line",
    "text-decoration-style",
    "text-decoration-thickness",
    "text-emphasis",
    "text-emphasis-color",
    "text-emphasis-position",
    "text-emphasis-style",
    "text-indent",
    "text-justify",
    "text-orientation",
    "text-overflow",
    "text-shadow",
    "text-spacing-trim",
    "text-transform",
    "text-underline-offset",
    "text-underline-position",
    "text-wrap",
    "text-wrap-mode",
    "text-wrap-style",
    "timeline-scope",
    "top",
    "touch-action",
    "transform",
    "transform-box",
    "transform-origin",
    "transform-style",
    "transition",
    "transition-behavior",
    "transition-delay",
    "transition-duration",
    "transition-property",
    "transition-timing-function",
    "translate",
    "unicode-bidi",
    "unicode-range",
    "user-select",
    "user-zoom",
    "vertical-align",
    "view-timeline",
    "view-timeline-axis",
    "view-timeline-inset",
    "view-timeline-name",
    "view-transition-class",
    "view-transition-name",
    "viewport-fit",
    "visibility",
    "voice-family",
    "volume",
    "white-space",
    "white-space-collapse",
    "white-space-trim",
    "widows",
    "width",
    "will-change",
    "word-break",
    "word-spacing",
    "wrap-flow",
    "wrap-through",
    "writing-mode",
    "z-index",
    "zoom",
];

/// Case-insensitive membership test against [`KNOWN_PROPERTIES`].
///
/// `name` may have any casing (`COLOR` is valid CSS); the check
/// lowercases ASCII bytes first. Names that are not plain
/// identifiers (empty, containing interpolation sigils) return
/// `true` — the lint never flags what it does not understand.
pub fn is_known_property_name(name: &str) -> bool {
    let lower: String = name.to_ascii_lowercase();
    // Vendor-prefixed (`-webkit-…`) and custom (`--…`) names are
    // always accepted: the lint targets unprefixed standard names
    // only, where the curated list is meaningful.
    if lower.starts_with('-') {
        return true;
    }
    KNOWN_PROPERTIES.binary_search(&lower.as_str()).is_ok()
}

/// Lint a classified token stream for unknown property names.
///
/// For every [`CssTokenKind::Property`] token whose text is a plain
/// unprefixed identifier that is not in [`KNOWN_PROPERTIES`], a
/// Warning-severity [`CssDiagnostic`] is appended — spans are in the
/// same source-relative coordinate system as the tokens (the caller's shifting contract applies unchanged).
///
/// Skipped (never flagged): custom properties (`--x`), vendor-prefixed
/// names (`-webkit-…`, any `-…`), SCSS/Less variables (`$x`) and
/// interpolated spellings (`#{…}` — the parser supports these
/// syntaxes, and their names are runtime values, not property
/// spellings Knot can judge).
pub fn unknown_property_diagnostics(tokens: &[CssToken], source: &str) -> Vec<CssDiagnostic> {
    let mut out = Vec::new();
    for token in tokens {
        if token.kind != CssTokenKind::Property {
            continue;
        }
        let span = token.span.start..token.span.end.min(source.len());
        if span.start >= span.end {
            continue; // zero-width — nothing to show the author
        }
        let name = &source[span.clone()];
        if !is_lintable_property_name(name) {
            continue;
        }
        if !is_known_property_name(name) {
            out.push(CssDiagnostic {
                message: format!("Unknown CSS property: `{name}`"),
                range: span,
                severity: CssDiagnosticSeverity::Warning,
            });
        }
    }
    out
}

/// Whether `name` is a plain, unprefixed identifier the lint may judge.
///
/// Only plain identifiers starting with an ASCII letter (either case —
/// CSS property names are case-insensitive) are judged. Everything
/// else — custom properties (`--…`), vendor spellings (`-…`), SCSS/Less
/// variables (`$…`), interpolated names (`#{…}`) — is either valid by
/// definition or a runtime value Knot cannot judge statically.
fn is_lintable_property_name(name: &str) -> bool {
    let Some(&first) = name.as_bytes().first() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    !name.bytes().any(|b| matches!(b, b'$' | b'#' | b'{' | b'}'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The array is the lookup structure — order is correctness,
    /// not style. Insertions must respect it.
    #[test]
    fn list_is_sorted() {
        assert!(KNOWN_PROPERTIES.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn standard_names_are_known() {
        for name in [
            "color",
            "background-color",
            "margin-left",
            "font-variant-alternates",
            "grid-template-areas",
            "container-type",
            "view-transition-name",
            "overscroll-behavior",
            "src",
            "system",
            "size-adjust",
            "size",
            "zoom",
            "pitch",
            "cue-before",
            "fill",
            "stroke-dasharray",
        ] {
            assert!(is_known_property_name(name), "{name} must be known");
        }
    }

    #[test]
    fn casing_is_ignored() {
        assert!(is_known_property_name("COLOR"));
        assert!(is_known_property_name("Margin-Left"));
    }

    #[test]
    fn prefixed_and_custom_names_pass() {
        for name in [
            "--main-color",
            "-webkit-transform",
            "-moz-appearance",
            "-ms-grid-column",
            "-colr",
        ] {
            assert!(is_known_property_name(name), "{name} must pass");
        }
    }

    #[test]
    fn typos_are_unknown() {
        for name in ["colr", "bakground", "margn-left", "font-famly"] {
            assert!(!is_known_property_name(name), "{name} must be unknown");
        }
    }

    #[test]
    fn lint_flags_unknown_and_skips_special_shapes() {
        let src = ".hud { colr: red; --x: 1; $y: 2; -webkit-z: 3; color: blue; }";
        let tokens = [
            CssToken {
                kind: CssTokenKind::Property,
                span: 7..11,
            }, // colr
            CssToken {
                kind: CssTokenKind::Property,
                span: 18..21,
            }, // --x
            CssToken {
                kind: CssTokenKind::Property,
                span: 26..28,
            }, // $y
            CssToken {
                kind: CssTokenKind::Property,
                span: 33..42,
            }, // -webkit-z
            CssToken {
                kind: CssTokenKind::Property,
                span: 45..50,
            }, // color
        ];
        let diags = unknown_property_diagnostics(&tokens, src);
        assert_eq!(diags.len(), 1, "only `colr` flagged: {diags:?}");
        assert_eq!(diags[0].range, 7..11);
        assert!(diags[0].message.contains("colr"));
        assert_eq!(diags[0].severity, CssDiagnosticSeverity::Warning);
    }
}
