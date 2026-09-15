//! CSS tokenizer (plan.md Phase 4.2 — the `knot-core::css` service).
//!
//! A hand-rolled, highlighter-grade tokenizer: never panics, never fails,
//! tolerant of malformed input (the upstream SugarCube engine is equally
//! silent about broken CSS — `styleTag` just hands the text to the browser).
//! It classifies the token families themes need (selectors, properties,
//! values, numbers, strings, comments, at-rules, functions, custom
//! properties) and reports at most the structural diagnostics that matter
//! (unclosed blocks at EOF).
//!
//! ## Design notes
//!
//! - Context tracking is a stack of block kinds: a `{` opened after a
//!   selector is a DECLARATION block; one opened after an at-rule
//!   (`@media`, `@supports`, …) is a RULE block whose children are rules
//!   again. This is what makes nested `@media { .a { … } }` classify
//!   correctly.
//! - Inside a declaration block the scanner alternates property phase
//!   (identifier before `:`) and value phase (until `;` or the block's `}`).
//! - A previous `cssparser`-based implementation was removed because it was
//!   fragile around `@media` nesting and custom properties; this scanner is
//!   deliberately simple and total. See plan.md decision D1 (match the
//!   spec-of-record, stay simple) and Phase 4.2.

use super::types::{CssDiagnostic, CssParseOutcome, CssToken, CssTokenKind};
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    /// `{` after a selector — contains `prop: value;` declarations.
    Declarations,
    /// `{` after an at-rule (`@media` …) — contains rules.
    Rules,
}

/// State of the scanner inside a declaration block.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeclPhase {
    /// Expecting a property name (or the block's `}`).
    Property,
    /// After `:` — consuming the value until `;` or `}`.
    Value,
}

/// Parse a CSS *declaration list* — `prop: value;` pairs without an
/// enclosing block, as found in `style="…"` attribute values (plan.md
/// Phase 4.2). Equivalent to [`parse_css`] but seeded as if already inside
/// a declaration block, so `color: red` classifies `color` as a Property.
pub fn parse_css_declarations(source: &str) -> CssParseOutcome {
    let mut outcome = parse_css_inner(source, Some((BlockKind::Declarations, DeclPhase::Property)));
    // A declaration list has no braces to close — drop the EOF diagnostic.
    outcome.diagnostics.clear();
    outcome
}

/// The declaration phase of the innermost block, if any.
fn decl_phase(blocks: &[(BlockKind, DeclPhase)]) -> Option<(BlockKind, DeclPhase)> {
    blocks.last().copied()
}

/// Parse CSS source text and return classified tokens + diagnostics.
///
/// Spans are source-relative. The output is total: any input produces a
/// well-formed [`CssParseOutcome`] (tolerant tokenization, no panics).
pub fn parse_css(source: &str) -> CssParseOutcome {
    parse_css_inner(source, None)
}

/// The tokenizer proper. `seed_block` seeds the block stack (used by
/// [`parse_css_declarations`] so bare declaration lists classify correctly).
fn parse_css_inner(source: &str, seed_block: Option<(BlockKind, DeclPhase)>) -> CssParseOutcome {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut tokens: Vec<CssToken> = Vec::new();
    let mut diagnostics: Vec<CssDiagnostic> = Vec::new();

    // Blocks: (kind, phase) — phase only meaningful for Declarations.
    let mut blocks: Vec<(BlockKind, DeclPhase)> = Vec::new();
    if let Some(seed) = seed_block {
        blocks.push(seed);
    }
    let mut i = 0usize;

    while i < len {
        let b = bytes[i];
        match b {
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                let start = i;
                i += 2;
                while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(len);
                tokens.push(CssToken {
                    kind: CssTokenKind::Comment,
                    span: start..i,
                });
            }
            b'"' | b'\'' => {
                let start = i;
                let quote = b;
                i += 1;
                while i < len && bytes[i] != quote && bytes[i] != b'\n' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i < len && bytes[i] == quote {
                    i += 1;
                }
                tokens.push(CssToken {
                    kind: CssTokenKind::String,
                    span: start..i,
                });
            }
            b'@' if is_ident_start(b) || b == b'@' => {
                // At-rule: `@` + identifier (prelude handled by the generic
                // scanner — at-rule arguments classify as keywords/strings).
                let start = i;
                i += 1;
                while i < len && is_ident_byte(bytes[i]) {
                    i += 1;
                }
                tokens.push(CssToken {
                    kind: CssTokenKind::AtRule,
                    span: start..i,
                });
            }
            b'{' => {
                // A block opened while the previous significant token was an
                // at-rule is a RULE block; otherwise declarations.
                let prev_at_rule = tokens
                    .iter()
                    .rev()
                    .find(|t| t.kind != CssTokenKind::Whitespace)
                    .is_some_and(|t| t.kind == CssTokenKind::AtRule);
                blocks.push((
                    if prev_at_rule {
                        BlockKind::Rules
                    } else {
                        BlockKind::Declarations
                    },
                    DeclPhase::Property,
                ));
                tokens.push(CssToken {
                    kind: CssTokenKind::Punctuation,
                    span: i..i + 1,
                });
                i += 1;
            }
            b'}' => {
                blocks.pop();
                tokens.push(CssToken {
                    kind: CssTokenKind::Punctuation,
                    span: i..i + 1,
                });
                i += 1;
            }
            b';' => {
                // Ends a declaration value (or an at-rule prelude).
                if let Some((BlockKind::Declarations, p)) = blocks.last_mut() {
                    *p = DeclPhase::Property;
                }
                tokens.push(CssToken {
                    kind: CssTokenKind::Punctuation,
                    span: i..i + 1,
                });
                i += 1;
            }
            b':' => {
                // Property/value separator (or pseudo-class prefix — both
                // tokenize as punctuation).
                if let Some((BlockKind::Declarations, p)) = blocks.last_mut()
                    && *p == DeclPhase::Property
                {
                    *p = DeclPhase::Value;
                }
                tokens.push(CssToken {
                    kind: CssTokenKind::Punctuation,
                    span: i..i + 1,
                });
                i += 1;
            }
            b'!' => {
                // `!important` — keyword (value phase only; harmless at top
                // level).
                let start = i;
                i += 1;
                while i < len && is_ident_byte(bytes[i]) {
                    i += 1;
                }
                tokens.push(CssToken {
                    kind: CssTokenKind::Keyword,
                    span: start..i,
                });
            }
            b'#' => {
                // Hex color (value phase) or an ID selector prefix (rule
                // context) — consume the following ident/number run.
                let start = i;
                i += 1;
                while i < len
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
                {
                    i += 1;
                }
                let kind = if matches!(
                    decl_phase(&blocks),
                    Some((BlockKind::Declarations, DeclPhase::Value))
                ) {
                    CssTokenKind::Number
                } else {
                    CssTokenKind::Selector
                };
                tokens.push(CssToken {
                    kind,
                    span: start..i,
                });
            }
            b'.' if i + 1 < len && is_ident_byte(bytes[i + 1]) => {
                // Class selector `.name` — one Selector token (parity with
                // the `#name` ID-selector handling).
                let start = i;
                i += 1;
                while i < len && is_ident_byte(bytes[i]) {
                    i += 1;
                }
                tokens.push(CssToken {
                    kind: CssTokenKind::Selector,
                    span: start..i,
                });
            }
            b'>' | b'+' | b'~' | b',' | b'[' | b']' | b'(' | b')' | b'*' | b'=' | b'|' | b'^'
            | b'$' | b'.' => {
                tokens.push(CssToken {
                    kind: CssTokenKind::Punctuation,
                    span: i..i + 1,
                });
                i += 1;
            }
            b'-' | b'0'..=b'9' => {
                // Numbers (with optional sign/units) vs custom properties
                // (`--x`) vs hyphenated identifiers — decided by lookahead.
                if b == b'-'
                    && i + 1 < len
                    && bytes[i + 1] == b'-'
                    && !(matches!(
                        decl_phase(&blocks),
                        Some((BlockKind::Declarations, DeclPhase::Property))
                    ) || decl_phase(&blocks).is_none())
                {
                    // `--custom-property` in a value context → Variable.
                    let start = i;
                    i += 2;
                    while i < len && is_ident_byte(bytes[i]) {
                        i += 1;
                    }
                    tokens.push(CssToken {
                        kind: CssTokenKind::Variable,
                        span: start..i,
                    });
                } else if b == b'-'
                    && i + 1 < len
                    && bytes[i + 1].is_ascii_digit()
                    && matches!(
                        decl_phase(&blocks),
                        Some((BlockKind::Declarations, DeclPhase::Value))
                    )
                {
                    let start = i;
                    i += 1;
                    i = consume_number(bytes, i);
                    tokens.push(CssToken {
                        kind: CssTokenKind::Number,
                        span: start..i,
                    });
                } else if b.is_ascii_digit()
                    && (matches!(
                        decl_phase(&blocks),
                        Some((BlockKind::Declarations, DeclPhase::Value))
                    ) || blocks.is_empty())
                {
                    let start = i;
                    i = consume_number(bytes, i);
                    tokens.push(CssToken {
                        kind: CssTokenKind::Number,
                        span: start..i,
                    });
                } else {
                    // Hyphen-starting identifier (selector like `.a-b` or
                    // property like `-webkit-x`): fall through to ident
                    // classification.
                    let start = i;
                    while i < len && is_ident_byte(bytes[i]) {
                        i += 1;
                    }
                    tokens.push(classify_ident(source, start..i, decl_phase(&blocks)));
                }
            }
            _ if is_ident_start(b) => {
                let start = i;
                while i < len && is_ident_byte(bytes[i]) {
                    i += 1;
                }
                let span = start..i;
                tokens.push(classify_ident(source, span, decl_phase(&blocks)));
            }
            b' ' | b'\t' | b'\n' | b'\r' => {
                // Whitespace is not emitted (the downstream mapping skips
                // `Whitespace` tokens anyway).
                i += 1;
            }
            _ => {
                tokens.push(CssToken {
                    kind: CssTokenKind::Punctuation,
                    span: i..i + 1,
                });
                i += 1;
            }
        }
    }

    // Structural diagnostics: unclosed blocks at EOF (at most one summary —
    // upstream is silent about broken CSS, so keep it minimal and honest).
    if !blocks.is_empty() {
        diagnostics.push(CssDiagnostic {
            message: format!(
                "Unclosed CSS block{} — missing {} closing brace(s) at end of input",
                if blocks.len() > 1 { "s" } else { "" },
                blocks.len()
            ),
            range: len.saturating_sub(1)..len,
        });
    }

    CssParseOutcome {
        tokens,
        diagnostics,
    }
}

/// Classify an identifier token by context: selectors at rule level,
/// properties in declaration phase, keywords/functions/variables in value
/// phase.
fn classify_ident(
    source: &str,
    span: Range<usize>,
    phase: Option<(BlockKind, DeclPhase)>,
) -> CssToken {
    // Function call: `name(` — peek at the raw source after the ident.
    let is_call = source.as_bytes()[span.end..]
        .first()
        .is_some_and(|&b| b == b'(');

    let kind = match phase {
        Some((BlockKind::Declarations, DeclPhase::Property)) => CssTokenKind::Property,
        Some((BlockKind::Declarations, DeclPhase::Value)) => {
            if is_call {
                CssTokenKind::Function
            } else {
                CssTokenKind::Keyword
            }
        }
        // Rule level / top level / inside at-rule prelude: selectors (and
        // function calls like :is(...) keep Selector — highlighter-grade).
        _ => CssTokenKind::Selector,
    };
    CssToken { kind, span }
}

/// Consume a numeric literal including decimals, exponents, and a unit
/// suffix (`px`, `em`, `%`, `rem`, `vh`, …).
fn consume_number(bytes: &[u8], mut i: usize) -> usize {
    let len = bytes.len();
    if i < len && (bytes[i] == b'-' || bytes[i] == b'+') {
        i += 1;
    }
    while i < len && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    // Exponent.
    if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if j < len && (bytes[j] == b'-' || bytes[j] == b'+') {
            j += 1;
        }
        if j < len && bytes[j].is_ascii_digit() {
            i = j;
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    // Unit suffix (`%`, `px`, `em`, …).
    if i < len && (bytes[i] == b'%' || is_ident_start(bytes[i])) {
        i += 1;
        while i < len && is_ident_byte(bytes[i]) {
            i += 1;
        }
    }
    i
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b == b'-' || b.is_ascii_alphabetic()
}

fn is_ident_byte(b: u8) -> bool {
    b == b'_' || b == b'-' || b.is_ascii_alphanumeric() || b >= 0x80
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(CssTokenKind, String)> {
        parse_css(src)
            .tokens
            .into_iter()
            .map(|t| (t.kind, src[t.span.start..t.span.end].to_string()))
            .collect()
    }

    #[test]
    fn classifies_rule_shape() {
        let out = kinds(".hud { color: red; }");
        assert!(out.contains(&(CssTokenKind::Selector, ".hud".into())));
        assert!(out.contains(&(CssTokenKind::Property, "color".into())));
        assert!(out.contains(&(CssTokenKind::Keyword, "red".into())));
    }

    #[test]
    fn classifies_numbers_units_and_functions() {
        let out = kinds(".a { margin: -1.5em; left: calc(100% - 2px); }");
        assert!(out.contains(&(CssTokenKind::Number, "-1.5em".into())));
        assert!(out.contains(&(CssTokenKind::Function, "calc".into())));
        assert!(out.contains(&(CssTokenKind::Number, "100%".into())));
        assert!(out.contains(&(CssTokenKind::Number, "2px".into())));
    }

    #[test]
    fn custom_properties_are_variables() {
        let out = kinds(".a { color: var(--main); --x: 1; }");
        assert!(out.contains(&(CssTokenKind::Function, "var".into())));
        assert!(out.contains(&(CssTokenKind::Variable, "--main".into())));
        assert!(out.contains(&(CssTokenKind::Property, "--x".into())));
    }

    #[test]
    fn at_rules_and_comments() {
        let out = kinds("/* hi */\n@media (min-width: 400px) { .a { color: blue; } }");
        assert!(out.contains(&(CssTokenKind::Comment, "/* hi */".into())));
        assert!(out.contains(&(CssTokenKind::AtRule, "@media".into())));
        // Inside the nested rule block, `.a` is still a selector and `color`
        // still a property (the @media block is a RULE block).
        assert!(out.contains(&(CssTokenKind::Selector, ".a".into())));
        assert!(out.contains(&(CssTokenKind::Property, "color".into())));
    }

    #[test]
    fn hex_colors_are_numbers_in_values() {
        let out = kinds(".a { color: #ff00aa; }");
        assert!(out.contains(&(CssTokenKind::Number, "#ff00aa".into())));
    }

    #[test]
    fn unclosed_block_is_reported_once() {
        let outcome = parse_css(".a { color: red;");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert!(outcome.diagnostics[0].message.contains("Unclosed"));
    }

    #[test]
    fn never_panics_on_garbage() {
        for src in ["", "{", "}}}", "\"unterminated", "@media {", ".a { -- } }"] {
            let _ = parse_css(src);
        }
    }
}
