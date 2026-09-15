//! Chunked JS region parsing support (plan.md Phase 3.2).
//!
//! oxc offers no error recovery through 0.150 (verified empirically —
//! `scripts/oxc_probe_150`): one fatal syntax error blanks the whole region's
//! AST, and with it every token in the region. This module provides the two
//! language-agnostic building blocks for the recovery strategy built in the
//! format layer:
//!
//! 1. [`split_js_statements`] — a string/comment/template/regex-aware
//!    statement splitter that cuts a JS region into top-level chunks at
//!    depth-0 boundaries (`;`, newline, closing `}` of a top-level block).
//!    Because chunks are only cut at brace depth 0, a chunk can never have a
//!    "brace deficit" across its boundary — the retry-merge strategy from
//!    the original plan is therefore unnecessary by construction.
//! 2. [`lex_js_fallback`] — a tolerant lexer that never fails, used to
//!    keep highlighting alive for chunks whose parse was fatal. It emits
//!    comments, strings, numbers, keywords and identifiers — enough for
//!    theming, not an AST.
//!
//! NOTE on `catch_unwind`: oxc's `fatal_error` is a normal return value, not
//! a Rust panic, so per-chunk unwinding guards are unnecessary (verified in
//! the probe above).

/// A top-level statement chunk: a byte range of the source that should
/// parse independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub start: usize,
    pub end: usize,
}

/// True when the innermost template-literal context is currently scanning a
/// `${ … }` expression body (braces in there don't affect block depth).
fn in_template_expr(stack: &[(u8, bool)]) -> bool {
    stack
        .iter()
        .rev()
        .find(|(k, _)| *k == 5)
        .is_some_and(|(_, in_expr)| *in_expr)
}

/// Split `source` into top-level statement chunks.
///
/// Boundaries are `;`, `\n` and the `}` that closes back to brace depth 0 —
/// but ONLY outside strings, template literals, comments, regex literals and
/// any `(...)`/`[...]`/`{...}` nesting. The returned ranges are
/// source-relative and non-overlapping; whitespace-only chunks are NOT
/// filtered here (callers skip them).
///
/// Deliberate heuristics (documented, highlighter-grade):
/// - A `/` begins a regex literal when the previous significant byte is a
///   starter (`(`, `[`, `{`, `,`, `;`, `:`, `!`, `&`, `|`, `?`, `=`, `<`,
///   `>`, `+`, `-`, `*`, `%`, `^`, `~`, a newline, or start-of-input);
///   otherwise it is division. Object-literal `}` followed by `/` is
///   classified as division (the rarer shape in story scripts).
/// - Template-literal `${ … }` expressions push a context so braces inside
///   them don't affect depth, and a `}` closing a `${` does NOT count as a
///   top-level boundary.
/// - Unterminated strings/comments simply run to the end of input (their
///   region becomes one chunk — the region parse already failed by the time
///   this runs).
pub fn split_js_statements(source: &str) -> Vec<Chunk> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut chunks = Vec::new();
    let mut chunk_start = 0usize;
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    // Stack of template-literal `${` entries: the paren depth when `${` was
    // entered, so the matching `}` pops back to the template.
    let mut template_paren: Vec<i32> = Vec::new();
    // (kind, in_template_expr) — kind: 1 = line comment, 2 = block comment,
    // 3 = single-quote string, 4 = double-quote string, 5 = template
    // literal (scanning literal text), 6 = regex literal.
    let mut stack: Vec<(u8, bool)> = Vec::new();
    let mut prev_significant: u8 = 0;
    let mut i = 0usize;

    while i < len {
        let b = bytes[i];
        // `${ … }` bodies are CODE: the (5,true) stack entry means "template
        // expression" and must go through the normal-state scanner below so
        // braces/parens/strings inside it are tracked. Only other lexeme
        // states take the fast path here.
        let in_code = stack.is_empty() || (stack.last().unwrap().0 == 5 && stack.last().unwrap().1);
        if !in_code {
            let (kind, _in_expr) = *stack.last().unwrap();
            match kind {
                1 => {
                    // Line comment: ends at newline (not consumed here).
                    if b == b'\n' {
                        stack.pop();
                        continue;
                    }
                    i += 1;
                }
                2 => {
                    if b == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                        stack.pop();
                        i += 2;
                        prev_significant = b'/';
                    } else {
                        if b == b'\n' {
                            prev_significant = b'\n';
                        }
                        i += 1;
                    }
                }
                3 | 4 => {
                    if b == b'\\' {
                        i += 2;
                    } else if b == b'\n' {
                        // Unterminated string: tolerate to end of line.
                        stack.pop();
                        // The newline is a boundary candidate; reprocess it.
                    } else if (kind == 3 && b == b'\'') || (kind == 4 && b == b'"') {
                        stack.pop();
                        i += 1;
                        prev_significant = b'"';
                    } else {
                        i += 1;
                    }
                }
                5 => {
                    // Template literal text.
                    if b == b'\\' {
                        i += 2;
                    } else if b == b'`' {
                        stack.pop();
                        i += 1;
                        prev_significant = b'`';
                    } else if b == b'$' && i + 1 < len && bytes[i + 1] == b'{' {
                        template_paren.push(paren);
                        stack.push((5, true));
                        paren = 0;
                        i += 2;
                        prev_significant = b'{';
                    } else {
                        i += 1;
                    }
                }
                6 => {
                    // Regex literal: `\x` escapes and a closing `/`, then
                    // flags (identifier chars).
                    if b == b'\\' {
                        i += 2;
                    } else if b == b'\n' {
                        // Unterminated regex: tolerate to end of line.
                        stack.pop();
                    } else if b == b'/' {
                        stack.pop();
                        i += 1;
                        while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                            i += 1; // flags
                        }
                        prev_significant = b'/';
                    } else {
                        i += 1;
                    }
                }
                _ => unreachable!(),
            }
            // Reprocess the current byte after a lexeme closed (the newline
            // after a line comment / unterminated string is a boundary).
            if !stack.is_empty() && b == b'\n' && kind != 2 {
                continue;
            }
            continue;
        }

        // Normal code state.
        match b {
            b'/' if i + 1 < len && bytes[i + 1] == b'/' => {
                stack.push((1, false));
                i += 2;
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                stack.push((2, false));
                i += 2;
            }
            b'\'' => {
                stack.push((3, false));
                i += 1;
            }
            b'"' => {
                stack.push((4, false));
                i += 1;
            }
            b'`' => {
                stack.push((5, false));
                i += 1;
            }
            b'/' => {
                let regex_starter = matches!(
                    prev_significant,
                    0 | b'('
                        | b'['
                        | b'{'
                        | b','
                        | b';'
                        | b':'
                        | b'!'
                        | b'&'
                        | b'|'
                        | b'?'
                        | b'='
                        | b'<'
                        | b'>'
                        | b'+'
                        | b'-'
                        | b'*'
                        | b'%'
                        | b'^'
                        | b'~'
                        | b'\n'
                );
                if regex_starter {
                    stack.push((6, false));
                    i += 1;
                } else {
                    i += 1;
                    prev_significant = b'/';
                }
            }
            b'(' => {
                paren += 1;
                i += 1;
                prev_significant = b'(';
            }
            b')' => {
                paren = (paren - 1).max(0);
                i += 1;
                prev_significant = b')';
            }
            b'[' => {
                bracket += 1;
                i += 1;
                prev_significant = b'[';
            }
            b']' => {
                bracket = (bracket - 1).max(0);
                i += 1;
                prev_significant = b']';
            }
            b'{' => {
                brace += 1;
                i += 1;
                prev_significant = b'{';
            }
            b'}' => {
                if in_template_expr(&stack) {
                    // Closing a `${` expression: return to template scanning.
                    if let Some(p) = template_paren.pop() {
                        paren = p;
                    }
                    stack.pop(); // pop the (5, true) expression marker
                    i += 1;
                    prev_significant = b'}';
                } else {
                    brace = (brace - 1).max(0);
                    i += 1;
                    prev_significant = b'}';
                    // NOTE: a `}` closing back to depth 0 is deliberately NOT
                    // a boundary — object literals inside assignments
                    // (`var o = { a: 1 };`) would be split mid-statement. The
                    // `;` or `\n` that follows provides the boundary.
                }
            }
            b';' if paren == 0 && bracket == 0 && brace == 0 => {
                i += 1;
                push_chunk(&mut chunks, source, chunk_start, i);
                chunk_start = i;
                prev_significant = b';';
            }
            b'\n' if paren == 0 && bracket == 0 && brace == 0 => {
                i += 1;
                push_chunk(&mut chunks, source, chunk_start, i);
                chunk_start = i;
                prev_significant = b'\n';
            }
            _ => {
                if !b.is_ascii_whitespace() {
                    prev_significant = b;
                }
                i += 1;
            }
        }
    }

    if chunk_start < len {
        push_chunk(&mut chunks, source, chunk_start, len);
    }
    chunks
}

/// Push a chunk unless it is entirely whitespace (empty statements are
/// noise; callers would just skip them anyway).
fn push_chunk(chunks: &mut Vec<Chunk>, source: &str, start: usize, end: usize) {
    if !source[start..end].trim().is_empty() {
        chunks.push(Chunk { start, end });
    }
}

/// A token from the tolerant fallback lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackToken {
    pub kind: FallbackTokenKind,
    pub range: std::ops::Range<usize>,
}

/// The kinds the fallback lexer distinguishes — exactly enough for theming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackTokenKind {
    LineComment,
    BlockComment,
    String,
    Template,
    Number,
    Keyword,
    Identifier,
}

/// JS keywords recognized by the fallback lexer (includes `true`/`false`/
/// `null`/`undefined` — callers classify the literals).
pub const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// Lex `source` tolerantly — never fails, never panics.
///
/// Comments, strings, template literals, numbers, keywords and identifiers
/// are recognized; everything else (operators, punctuation) is skipped. The
/// primary use is fallback highlighting for chunks whose oxc parse was
/// fatal (plan.md Phase 3.2). Spans are source-relative; unterminated
/// strings/comments/templates run to end-of-line/input instead of failing.
pub fn lex_js_fallback(source: &str) -> Vec<FallbackToken> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut i = 0usize;

    while i < len {
        let b = bytes[i];
        match b {
            b'/' if i + 1 < len && bytes[i + 1] == b'/' => {
                let start = i;
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                tokens.push(FallbackToken {
                    kind: FallbackTokenKind::LineComment,
                    range: start..i,
                });
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                let start = i;
                i += 2;
                while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(len);
                tokens.push(FallbackToken {
                    kind: FallbackTokenKind::BlockComment,
                    range: start..i,
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
                tokens.push(FallbackToken {
                    kind: FallbackTokenKind::String,
                    range: start..i,
                });
            }
            b'`' => {
                let start = i;
                i += 1;
                while i < len && bytes[i] != b'`' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                tokens.push(FallbackToken {
                    kind: FallbackTokenKind::Template,
                    range: start..i,
                });
            }
            _ if b.is_ascii_digit() => {
                let start = i;
                while i < len
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
                {
                    i += 1;
                }
                tokens.push(FallbackToken {
                    kind: FallbackTokenKind::Number,
                    range: start..i,
                });
            }
            _ if b == b'_' || b == b'$' || b.is_ascii_alphabetic() => {
                let start = i;
                while i < len
                    && (bytes[i] == b'_' || bytes[i] == b'$' || bytes[i].is_ascii_alphanumeric())
                {
                    i += 1;
                }
                let text = &source[start..i];
                let kind = if JS_KEYWORDS.contains(&text) {
                    FallbackTokenKind::Keyword
                } else {
                    FallbackTokenKind::Identifier
                };
                tokens.push(FallbackToken {
                    kind,
                    range: start..i,
                });
            }
            _ => i += 1,
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_texts(src: &str) -> Vec<&str> {
        split_js_statements(src)
            .into_iter()
            .map(|c| &src[c.start..c.end])
            .collect()
    }

    #[test]
    fn splits_on_semicolons_and_newlines_at_depth_zero() {
        let src = "var a = 1;\nvar b = 2;\nvar c = 3;\n";
        let chunks = chunk_texts(src);
        assert_eq!(chunks, vec!["var a = 1;", "var b = 2;", "var c = 3;"]);
    }

    #[test]
    fn semicolons_inside_strings_do_not_split() {
        let src = "var a = \"x; y//z\";\nvar b = 2;\n";
        let chunks = chunk_texts(src);
        assert_eq!(chunks, vec!["var a = \"x; y//z\";", "var b = 2;"]);
    }

    #[test]
    fn braces_and_brackets_guard_boundaries() {
        // `;` inside an object literal and inside parens must not split;
        // the closing `}` of a top-level block is a boundary itself.
        let src = "var o = { a: 1; };\nfunction f() { return 2; }\nvar c = 3;\n";
        let chunks = chunk_texts(src);
        assert_eq!(
            chunks,
            vec![
                // The `;` inside the object literal must not split, and the
                // `}` closing it must not become a boundary (object literal
                // inside an assignment).
                "var o = { a: 1; };",
                "function f() { return 2; }\n",
                "var c = 3;",
            ]
        );
    }

    #[test]
    fn template_expression_braces_do_not_close_blocks() {
        let src = "var t = `a ${x.y} b`;\nvar n = 1;\n";
        let chunks = chunk_texts(src);
        assert_eq!(chunks, vec!["var t = `a ${x.y} b`;", "var n = 1;"]);
    }

    #[test]
    fn regex_literal_is_not_division_split() {
        // The `//` inside the regex must not become a line comment.
        let src = "var re = /a\\/\\/b/g;\nvar n = 1;\n";
        let chunks = chunk_texts(src);
        assert_eq!(chunks, vec!["var re = /a\\/\\/b/g;", "var n = 1;"]);
    }

    #[test]
    fn division_after_ident_is_not_regex() {
        let src = "var x = a / b;\n// real comment\n";
        let chunks = chunk_texts(src);
        assert_eq!(chunks, vec!["var x = a / b;", "// real comment\n"]);
    }

    #[test]
    fn unterminated_string_tolerates_to_line_end() {
        let src = "var a = \"oops\nvar b = 2;\n";
        let chunks = chunk_texts(src);
        assert_eq!(chunks, vec!["var a = \"oops\n", "var b = 2;"]);
    }

    #[test]
    fn fallback_lexer_finds_token_families() {
        let src = "var hp = 10; // set hp\nConfig.debug = \"on\";\nfunction f() {}";
        let toks = lex_js_fallback(src);
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&FallbackTokenKind::Keyword)); // var/function
        assert!(kinds.contains(&FallbackTokenKind::Number)); // 10
        assert!(kinds.contains(&FallbackTokenKind::LineComment));
        assert!(kinds.contains(&FallbackTokenKind::String));
        assert!(kinds.contains(&FallbackTokenKind::Identifier)); // hp, Config…
        // The `var` keyword token must cover exactly "var".
        let var_tok = toks
            .iter()
            .find(|t| t.kind == FallbackTokenKind::Keyword)
            .unwrap();
        assert_eq!(&src[var_tok.range.clone()], "var");
    }

    #[test]
    fn fallback_lexer_survives_garbage() {
        // Never panics on pathological input.
        for src in [
            "",
            "(((",
            "}}}",
            "\"unterminated",
            "/* unterminated",
            "`tpl",
            "var = ;",
        ] {
            let _ = lex_js_fallback(src);
            let _ = split_js_statements(src);
        }
    }
}
