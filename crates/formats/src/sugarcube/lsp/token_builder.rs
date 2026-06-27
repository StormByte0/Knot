//! Semantic token and diagnostic building for SugarCube passages.
//!
//! This module contains functions that convert parsed AST nodes and passage
//! headers into semantic tokens and diagnostics.
//!
//! ## Passage-Relative Offsets
//!
//! All token offsets produced by this module are **passage-relative**: byte 0
//! is the `::` prefix of the passage header. This design enables incremental
//! passage updates — when a single passage is edited, only that passage's
//! token group needs to be regenerated, and the passage's document offset
//! is applied at the LSP boundary.
//!
//! The conversion pipeline is:
//! ```text
//! AST spans (body-relative, 0 = after header newline)
//!   → add body_offset_in_passage (offset of body start from passage head)
//!   → passage-relative tokens (0 = passage head `::`)
//!   → at LSP boundary: add passage_offset → document-absolute byte offset
//! ```

use std::collections::HashSet;

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity, SemanticToken, SemanticTokenModifier, SemanticTokenType};
use crate::sugarcube::ast;
use crate::sugarcube::macros::{deprecated_macros, folding_modifier_names};
use crate::sugarcube::special_passages;

/// Build semantic tokens from AST nodes.
///
/// When a node has `js_analysis`, emits tokens from its `var_ops` with
/// per-segment precision. Otherwise falls back to `var_refs` for backward
/// compat (e.g., when the annotation pass hasn't run).
///
/// `custom_macro_names` is the set of user-defined macro/widget names from
/// the custom macro registry. Macro names in this set get `Function` token
/// type instead of `Macro`, enabling distinct visual styling for user-defined
/// macros vs builtins.
///
/// **Passage-relative offsets**: All emitted tokens have `start` values
/// relative to the passage head (`::` prefix). The `body_offset_in_passage`
/// parameter is the byte offset of the body start relative to the passage
/// head (i.e., `body_document_offset - header_start`). AST body spans are relative
/// to the body start, so adding `body_offset_in_passage` converts them to
/// passage-relative offsets.
pub fn build_semantic_tokens(
    nodes: &[ast::AstNode],
    tokens: &mut Vec<SemanticToken>,
    body_offset_in_passage: usize,
    custom_macro_names: &HashSet<String>,
    body_text: &str,
) {
    build_semantic_tokens_at_depth(nodes, tokens, body_offset_in_passage, custom_macro_names, 0, body_text);
    // Filter out zero-length tokens. These can arise from:
    // - def/ndef substitution position mapping (clamped to substitution start)
    // - structured_args scanner producing empty spans for unknown macros
    // - edge cases in oxc span mapping
    // Zero-length tokens are useless noise — VS Code ignores them, but they
    // clutter debug output and can interfere with hover detection.
    tokens.retain(|t| t.length > 0);
}

/// Recursive token builder that tracks block-macro nesting depth.
///
/// `depth` is the current block nesting level (0 = top-level, 1 = inside one
/// block macro, etc.). Block macros (those with `children: Some(_)`) get a
/// `BlockDepthN` modifier on both their open tag name and close tag name
/// tokens, so themes can color matching open/close pairs by nesting depth.
/// Inline macros (no children) don't get a depth modifier — they use the
/// default macro color, making them visually distinct from block macros.
///
/// `body_text` is the passage body source (the text the AST was parsed
/// from). Used to extract sub-slices for context-sensitive tokenization
/// (e.g., running the inline variable scanner over link setter expressions).
fn build_semantic_tokens_at_depth(
    nodes: &[ast::AstNode],
    tokens: &mut Vec<SemanticToken>,
    body_offset_in_passage: usize,
    custom_macro_names: &HashSet<String>,
    depth: usize,
    body_text: &str,
) {
    for node in nodes {
        match node {
            ast::AstNode::Macro { name, name_span, js_analysis, var_refs, children, definition_name_span, capture_target, for_loop_vars, structured_args, close_name_span, open_span, close_span, .. } => {
                // Modifier macros (else, elseif, case, default) are structurally
                // siblings of their parent, not children. They're in the parent's
                // children array, but they should render at the parent's depth,
                // not one level deeper. Adjust depth for these.
                let effective_depth = if is_folding_modifier(name) && depth > 0 {
                    depth - 1
                } else {
                    depth
                };

                // Determine if this is a block macro (has children → open/close pair)
                let is_block = children.is_some();

                // Macro name token — differentiate builtin vs custom/widget
                let token_type = if custom_macro_names.contains(name) {
                    SemanticTokenType::Function
                } else {
                    SemanticTokenType::Macro
                };

                // ── Modifier split: name vs delimiter ──────────────────
                //
                // The macro NAME (e.g. `link`, `set`, `if`) always renders
                // with the base `macro` / `function` color. Depth coloring
                // is ONLY applied to delimiters (`<<`, `>>`, `<</`). This
                // keeps the name visually stable — you can always spot the
                // macro identifier regardless of how deep it's nested —
                // while the delimiters around it shift color to show the
                // nesting level.
                //
                // Name modifier:
                //   - `Deprecated` if the macro is in the deprecated catalog
                //     (so themes can show strikethrough on the name)
                //   - Otherwise `None` → base `macro` color from the theme
                //
                // Delimiter modifier (depth semantics):
                //   - depth=0 (any macro, block OR inline) → None (base color)
                //   - depth=N (any macro, inside N nested blocks) → BlockDepthN
                //
                // This is consistent: a top-level `<<set>>` and a top-level
                // `<<link>>` have the SAME delimiter color (both depth 0 =
                // base). Only when you go INSIDE a block does the depth
                // modifier kick in. The depth number directly maps to "how
                // many blocks am I nested inside".
                //
                //   <<link>>              → depth 0 → None (base delimiter)
                //     <<set>>             → depth 1 → BlockDepth1
                //     <<if>>              → depth 1 → BlockDepth1
                //       <<adjustStat>>    → depth 2 → BlockDepth2
                //     <</if>>
                //   <</link>>
                //
                // Theme compatibility: the depth modifier bits are sent on
                // the wire regardless of theme. Any theme can opt into depth
                // coloring by adding `macroDelimiter.blockDepth1..6` rules to
                // its `semanticTokenColors`, or by relying on the
                // `semanticTokenScopes` fallback mappings (which map each
                // depth to a standard TextMate scope that most themes color
                // distinctly). Themes that don't define those rules fall back
                // to the base `macroDelimiter` color (all delimiters same
                // color) — still correct, just no depth variation.
                //
                // Note: our SemanticToken.modifier is Option<Modifier>, not a
                // bitset, so we can't combine Deprecated + BlockDepth on a
                // single token. The name gets Deprecated (if applicable);
                // the delimiters get depth. Both signals are still visible.
                let name_modifier = if deprecated_macros().contains_key(name.as_str()) {
                    Some(SemanticTokenModifier::Deprecated)
                } else {
                    None
                };
                let delim_modifier = SemanticTokenModifier::from_block_depth(effective_depth);
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + name_span.start,
                    length: name_span.end - name_span.start,
                    token_type,
                    modifier: name_modifier,
                });

                // ── Delimiter tokens ───────────────────────────────────
                // Emit `MacroDelimiter` tokens for `<<`, `>>`, and (for block
                // macros) `<</` and the trailing `>>`. These are intentionally
                // a separate token type from the macro name so themes can give
                // them a distinct color (similar to how `function()` colors
                // the keyword, the parens, and the args differently).
                //
                // Delimiters carry the depth modifier (NOT the name's
                // deprecated modifier) — depth coloring belongs on the
                // delimiters, not on the name.
                //
                // The close tag's `<</` is a 3-byte delimiter (the name lives
                // in `close_name_span`, between `<</` and `>>`).
                push_delimiter(tokens, body_offset_in_passage, open_span.start, 2, delim_modifier);
                if open_span.end >= 2 {
                    push_delimiter(tokens, body_offset_in_passage, open_span.end - 2, 2, delim_modifier);
                }
                if is_block {
                    if let Some(cs) = close_span {
                        // `<</` — 3 bytes at the start of the close tag
                        push_delimiter(tokens, body_offset_in_passage, cs.start, 3, delim_modifier);
                        // `>>` — 2 bytes at the end of the close tag
                        if cs.end >= 2 {
                            push_delimiter(tokens, body_offset_in_passage, cs.end - 2, 2, delim_modifier);
                        }
                    }
                }

                // For block macros: emit a token for the close tag name
                // (e.g., `if` in `<</if>>`). The close name uses the SAME
                // modifier as the open name (`name_modifier`) — NOT the
                // depth modifier. This keeps the name visually stable on
                // both open and close tags; only the delimiters show depth.
                if is_block {
                    if let Some(cn_span) = close_name_span {
                        tokens.push(SemanticToken {
                            start: body_offset_in_passage + cn_span.start,
                            length: cn_span.end - cn_span.start,
                            token_type,
                            modifier: name_modifier,
                        });
                    }
                }

                // For <<widget>> definitions: emit Function + Definition token
                // for the name being defined (e.g., "myHelper" in <<widget myHelper>>)
                if let Some(def_span) = definition_name_span {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + def_span.start,
                        length: def_span.end - def_span.start,
                        token_type: SemanticTokenType::Function,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                }
                // For <<capture>> macros: emit Variable + Definition token for
                // the capture target (e.g., "$target" in <<capture $target>>).
                // This provides AST-level capture highlighting that complements
                // the JS annotation pass.
                if let Some(ct) = capture_target {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + ct.span.start,
                        length: ct.span.end - ct.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                }
                // For <<for>> macros with simplified iteration form:
                // emit Variable + Definition for the index var (write target),
                // and Variable (no modifier) for the iterated var (read).
                if let Some(fl) = for_loop_vars {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + fl.index_var.span.start,
                        length: fl.index_var.span.end - fl.index_var.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + fl.iterated_var.span.start,
                        length: fl.iterated_var.span.end - fl.iterated_var.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: None,
                    });
                }
                // Variable references: prefer js_analysis, fallback to var_refs
                if let Some(analysis) = js_analysis {
                    for op in &analysis.var_ops {
                        emit_var_op_tokens(op, tokens, body_offset_in_passage);
                    }
                    // Emit literal tokens (strings, numbers, booleans, null)
                    emit_literal_tokens(&analysis.literal_spans, tokens, body_offset_in_passage);
                    // Emit operator tokens (SugarCube keywords + JS operators)
                    emit_operator_tokens(&analysis.operator_spans, tokens, body_offset_in_passage);
                    // Emit namespace tokens (SugarCube global objects)
                    emit_namespace_tokens(&analysis.namespace_spans, tokens, body_offset_in_passage);
                    emit_comment_tokens(&analysis.comment_spans, tokens, body_offset_in_passage);
                    // Emit comment tokens (/* */ and // inside JS expressions)
                    emit_comment_tokens(&analysis.comment_spans, tokens, body_offset_in_passage);
                    // Emit JS keyword tokens (if, for, var, function, etc.)
                    emit_keyword_tokens(&analysis.keyword_spans, tokens, body_offset_in_passage);
                    // Emit JS local variable references and definitions
                    emit_js_var_tokens(&analysis.js_var_spans, tokens, body_offset_in_passage);
                    emit_js_var_def_tokens(&analysis.js_var_def_spans, tokens, body_offset_in_passage);
                    // Emit JS method calls (.forEach, .getElementById, etc.)
                    emit_js_method_tokens(&analysis.js_method_spans, tokens, body_offset_in_passage);
                    // Emit JS property accesses (.left, .length, .innerHTML, etc.)
                    emit_js_property_tokens(&analysis.js_property_spans, tokens, body_offset_in_passage);
                    // Emit JS global object references (document, Array, Math, etc.)
                    emit_js_global_tokens(&analysis.js_global_spans, tokens, body_offset_in_passage);
                    // Emit function definition tokens from oxc analysis
                    emit_function_def_tokens(&analysis.function_defs, tokens, body_offset_in_passage);
                    // Emit function call site tokens from oxc analysis
                    emit_function_call_tokens(&analysis.function_calls, tokens, body_offset_in_passage);
                } else {
                    for vr in var_refs {
                        tokens.push(SemanticToken {
                            start: body_offset_in_passage + vr.span.start,
                            length: vr.span.end - vr.span.start,
                            token_type: SemanticTokenType::Variable,
                            modifier: if vr.is_write { Some(SemanticTokenModifier::Definition) } else { None },
                        });
                    }
                }
                // Emit tokens for structured args from catalog (Phase 6).
                // Passage references get Link tokens; labels and selectors get
                // appropriate highlighting. Variable refs in structured args
                // get Variable tokens (these complement, not replace, the
                // js_analysis/var_refs variable tokens — structured_args may
                // capture variable refs that the JS scanner doesn't classify
                // correctly for navigation macros like <<goto $dest>>).
                if let Some(sargs) = structured_args {
                    emit_structured_arg_tokens(sargs, tokens, body_offset_in_passage);
                }
                // For <<style>>/<<css>> blocks: emit CSS tokens from body text.
                // This is the direct equivalent of how <<script>> gets JS
                // tokens via js_analysis — but CSS doesn't need an annotation
                // pass, so we emit directly here.
                if name.eq_ignore_ascii_case("style") || name.eq_ignore_ascii_case("css") {
                    if let Some(ch) = children {
                        let mut css_source = String::new();
                        let mut body_start = 0usize;
                        for child in ch.iter() {
                            if let ast::AstNode::Text { content, span, .. } = child {
                                if css_source.is_empty() {
                                    body_start = span.start;
                                }
                                css_source.push_str(content);
                            }
                        }
                        if !css_source.trim().is_empty() {
                            let css_analysis = crate::sugarcube::css::analyze_css(css_source.trim());
                            for css_tok in &css_analysis.tokens {
                                tokens.push(SemanticToken {
                                    start: body_offset_in_passage + body_start + css_tok.start,
                                    length: css_tok.length,
                                    token_type: css_tok.token_type,
                                    modifier: css_tok.modifier,
                                });
                            }
                        }
                    }
                }

                // Recurse into block macro children with incremented depth.
                // Inline macros (children: None) don't recurse.
                //
                // IMPORTANT: We use `effective_depth + 1`, NOT `depth + 1`.
                // For folding modifiers (else, elseif, case, default, etc.),
                // `effective_depth` is `depth - 1` (they render at their
                // parent's level). Their children should therefore be at
                // `effective_depth + 1 = depth`, NOT `depth + 1`.
                //
                // Without this, macros nested inside `<<elseif>>` or
                // `<<case>>` would get an extra depth level they don't
                // deserve — these are segmenters, not nesting levels.
                // Example:
                //   <<if $a>>           depth=0, eff=0 → BlockDepth1
                //   <<elseif $b>>       depth=1, eff=0 → BlockDepth1 (segmenter)
                //     <<if $c>>         depth=1, eff=1 → BlockDepth2 (correct!)
                //     <</if>>
                //   <</if>>
                if let Some(ch) = children {
                    build_semantic_tokens_at_depth(ch, tokens, body_offset_in_passage, custom_macro_names, effective_depth + 1, body_text);
                }
            }
            ast::AstNode::Link {
                display_span,
                target_span,
                setter_span,
                span: _,
                ..
            } => {
                // Emit per-region semantic tokens for `[[...]]` links:
                //
                //   - `Link` token on the **target** passage name
                //     (so completion, hover, go-to-definition all trigger
                //     on the passage name, NOT on the display text)
                //   - `String` token on the **display** text (when present
                //     and distinct from the target) for visual
                //     differentiation — the editor typically renders String
                //     tokens in a different color than Link tokens
                //   - `Variable` tokens on `$var` / `_var` references
                //     inside the setter expression — the inline var
                //     scanner (`scan_inline_vars`) doesn't run inside link
                //     constructs (see its module doc), so we run it here
                //     on the setter slice to ensure variables in
                //     `[[...][$playerGold += 5]]` are highlighted.
                //
                // Previous bug: a single Link token covered the ENTIRE
                // content between `[[` and `]]` (including separators and
                // setter). This was wrong because:
                //   1. It swallowed the display text, preventing the
                //      TextMate grammar's `string.other.link.display.twee`
                //      scope from being visible.
                //   2. It swallowed the setter expression, hiding
                //      Variable tokens for `$var` inside `[[...][$var += 5]]`.
                //   3. It overlapped the separator `|` / `->` / `<-`,
                //      making token-based hover/completion trigger on
                //      separator characters.
                //
                // Per-region tokens fix all three issues and match the
                // TextMate grammar's scope decomposition.

                // Target: always present. Emit a Link token.
                let target_start = body_offset_in_passage + target_span.start;
                let target_end = body_offset_in_passage + target_span.end;
                if target_end > target_start {
                    tokens.push(SemanticToken {
                        start: target_start,
                        length: target_end - target_start,
                        token_type: SemanticTokenType::Link,
                        modifier: None,
                    });
                }

                // Display: optional. Emit a String token (different color
                // from Link, signals "this is the display text, not the
                // passage name").
                if let Some(d_span) = display_span {
                    let d_start = body_offset_in_passage + d_span.start;
                    let d_end = body_offset_in_passage + d_span.end;
                    if d_end > d_start {
                        tokens.push(SemanticToken {
                            start: d_start,
                            length: d_end - d_start,
                            token_type: SemanticTokenType::String,
                            modifier: None,
                        });
                    }
                }

                // Setter: emit Variable tokens for `$var` / `_var` refs
                // inside the setter expression. The inline var scanner
                // (`scan_inline_vars`) intentionally skips content inside
                // link constructs, so without this the variables in
                // `[[...][$playerGold += 5]]` would not be highlighted.
                //
                // We run the scanner on the setter slice text (extracted
                // from `body_text` using the absolute setter span), then
                // use the returned spans directly (they're already
                // passage-relative because we pass `s_start` as the offset).
                if let Some(s_span) = setter_span {
                    let s_start = body_offset_in_passage + s_span.start;
                    let s_end = body_offset_in_passage + s_span.end;
                    if s_end > s_start {
                        // SAFETY: `s_start` and `s_end` are passage-relative
                        // offsets that fall within the link's `span`, which
                        // was constructed from valid char-boundary offsets
                        // by `parse_link`. The slice is therefore safe.
                        // We use `get()` defensively in case of any
                        // upstream inconsistency.
                        //
                        // Note: `body_text` is body-relative (offset 0 =
                        // body start), but `s_start`/`s_end` are
                        // passage-relative (offset 0 = passage head `::`).
                        // We subtract `body_offset_in_passage` to convert
                        // back to body-relative for slicing `body_text`.
                        let body_s_start = s_start.saturating_sub(body_offset_in_passage);
                        let body_s_end = s_end.saturating_sub(body_offset_in_passage);
                        if let Some(setter_text) = body_text.get(body_s_start..body_s_end) {
                            let setter_var_refs = super::super::parser::variable_scan::scan_inline_vars(
                                setter_text,
                                s_start,
                            );
                            for vr in setter_var_refs {
                                tokens.push(SemanticToken {
                                    start: vr.span.start,
                                    length: vr.span.end - vr.span.start,
                                    token_type: SemanticTokenType::Variable,
                                    modifier: None,
                                });
                            }
                        }
                    }
                }
            }
            ast::AstNode::Expression { kind, js_analysis, var_refs, span, .. } => {
                // Emit a Macro token for the expression sigil (= or -) so
                // it's visually consistent with <<print>> getting a Macro token.
                // The sigil is at span.start + 2 (after the opening <<).
                let sigil_offset = body_offset_in_passage + span.start + 2;
                let modifier = match kind {
                    ast::ExprKind::Print => None,
                    // Silent expressions suppress output — use ControlFlow to
                    // signal that visually.
                    ast::ExprKind::Silent => Some(SemanticTokenModifier::ControlFlow),
                };
                tokens.push(SemanticToken {
                    start: sigil_offset,
                    length: 1,
                    token_type: SemanticTokenType::Macro,
                    modifier,
                });

                // ── Delimiter tokens ───────────────────────────────────
                // `<<` and `>>` around the sigil. Expressions are always
                // inline (no children), so no depth modifier — just inherit
                // the sigil's modifier (ControlFlow for `<<->>`, None for
                // `<<=>>`).
                push_delimiter(tokens, body_offset_in_passage, span.start, 2, modifier);
                if span.end >= 2 {
                    push_delimiter(tokens, body_offset_in_passage, span.end - 2, 2, modifier);
                }
                // Variable references: prefer js_analysis, fallback to var_refs
                if let Some(analysis) = js_analysis {
                    for op in &analysis.var_ops {
                        emit_var_op_tokens(op, tokens, body_offset_in_passage);
                    }
                    // Emit literal tokens (strings, numbers, booleans, null)
                    emit_literal_tokens(&analysis.literal_spans, tokens, body_offset_in_passage);
                    // Emit operator tokens (SugarCube keywords + JS operators)
                    emit_operator_tokens(&analysis.operator_spans, tokens, body_offset_in_passage);
                    // Emit namespace tokens (SugarCube global objects)
                    emit_namespace_tokens(&analysis.namespace_spans, tokens, body_offset_in_passage);
                    emit_comment_tokens(&analysis.comment_spans, tokens, body_offset_in_passage);
                    // Emit comment tokens (/* */ and // inside JS expressions)
                    emit_comment_tokens(&analysis.comment_spans, tokens, body_offset_in_passage);
                    // Emit JS keyword tokens (if, for, var, function, etc.)
                    emit_keyword_tokens(&analysis.keyword_spans, tokens, body_offset_in_passage);
                    // Emit JS local variable references and definitions
                    emit_js_var_tokens(&analysis.js_var_spans, tokens, body_offset_in_passage);
                    emit_js_var_def_tokens(&analysis.js_var_def_spans, tokens, body_offset_in_passage);
                    // Emit JS method calls (.forEach, .getElementById, etc.)
                    emit_js_method_tokens(&analysis.js_method_spans, tokens, body_offset_in_passage);
                    // Emit JS property accesses (.left, .length, .innerHTML, etc.)
                    emit_js_property_tokens(&analysis.js_property_spans, tokens, body_offset_in_passage);
                    // Emit JS global object references (document, Array, Math, etc.)
                    emit_js_global_tokens(&analysis.js_global_spans, tokens, body_offset_in_passage);
                    // Emit function definition tokens from oxc analysis
                    emit_function_def_tokens(&analysis.function_defs, tokens, body_offset_in_passage);
                    // Emit function call site tokens from oxc analysis
                    emit_function_call_tokens(&analysis.function_calls, tokens, body_offset_in_passage);
                } else {
                    for vr in var_refs {
                        tokens.push(SemanticToken {
                            start: body_offset_in_passage + vr.span.start,
                            length: vr.span.end - vr.span.start,
                            token_type: SemanticTokenType::Variable,
                            modifier: None,
                        });
                    }
                }
            }
            ast::AstNode::Comment { span, .. } => {
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start,
                    length: span.end - span.start,
                    token_type: SemanticTokenType::Comment,
                    modifier: None,
                });
            }
            ast::AstNode::Text { content, var_refs, span, is_prose, .. } => {
                // ── Collect all "special" spans (variables + templates) ────
                // These are the positions where we DON'T want to emit a Prose
                // token, because a more specific token (Variable or Function)
                // will be emitted there instead. By splitting the Prose token
                // around these positions, we avoid overlapping tokens — VS Code
                // renders each position with exactly one semantic token type.
                let mut gaps: Vec<std::ops::Range<usize>> = Vec::new();

                // Variable references
                for vr in var_refs {
                    gaps.push(vr.span.clone());
                }

                // Template invocations (?name) — scan content for ?ident.
                // The token covers the full `?name` (including the `?` sigil).
                //
                // SugarCube template names can contain hyphens (e.g.,
                // `?random-num`, `?story-name` from the testbed). The first
                // character after `?` must be an ident-start char (letter,
                // digit, `_`, `$`), but continuation characters also allow
                // `-`. This matches SugarCube's `Template.add("name", ...)`
                // which accepts any string as the name, and the `?name`
                // invocation syntax which scans the name greedily.
                let content_bytes = content.as_bytes();
                let content_len = content_bytes.len();
                let is_ident_start = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
                let is_ident_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b == b'-';
                let mut i = 0usize;
                while i < content_len {
                    if content_bytes[i] == b'?' && i + 1 < content_len && is_ident_start(content_bytes[i + 1]) {
                        let token_start = i;
                        let mut name_end = i + 1;
                        while name_end < content_len && is_ident_char(content_bytes[name_end]) {
                            name_end += 1;
                        }
                        gaps.push(span.start + token_start..span.start + name_end);
                        i = name_end;
                    } else {
                        i += 1;
                    }
                }

                // Sort gaps by start position
                gaps.sort_by_key(|g| g.start);

                // ── Emit Prose tokens for the gaps BETWEEN special spans ──
                if *is_prose {
                    let mut prose_start = span.start;
                    for gap in &gaps {
                        if gap.start > prose_start {
                            tokens.push(SemanticToken {
                                start: body_offset_in_passage + prose_start,
                                length: gap.start - prose_start,
                                token_type: SemanticTokenType::Prose,
                                modifier: None,
                            });
                        }
                        prose_start = gap.end.max(prose_start);
                    }
                    if prose_start < span.end {
                        tokens.push(SemanticToken {
                            start: body_offset_in_passage + prose_start,
                            length: span.end - prose_start,
                            token_type: SemanticTokenType::Prose,
                            modifier: None,
                        });
                    }
                }

                // ── Emit Variable tokens ──────────────────────────────────
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: None,
                    });
                }

                // ── Emit Function tokens for ?templates ───────────────────
                // Re-scan content for ?ident and emit tokens for the full ?name.
                // Uses the same is_ident_start/is_ident_char split as the gap
                // scanner above — hyphens are allowed in continuation but not
                // as the first character after `?`.
                {
                    let is_ident_start = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
                    let is_ident_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b == b'-';
                    let mut i = 0usize;
                    while i < content_len {
                        if content_bytes[i] == b'?' && i + 1 < content_len && is_ident_start(content_bytes[i + 1]) {
                            let token_start = i;
                            let mut name_end = i + 1;
                            while name_end < content_len && is_ident_char(content_bytes[name_end]) {
                                name_end += 1;
                            }
                            tokens.push(SemanticToken {
                                start: body_offset_in_passage + span.start + token_start,
                                length: name_end - token_start,
                                token_type: SemanticTokenType::Function,
                                modifier: None,
                            });
                            i = name_end;
                        } else {
                            i += 1;
                        }
                    }
                }
            }
            ast::AstNode::Error { .. } => {}
            // Inline styling: emit InlineStyle token for the class name,
            // then recurse into children for prose/variable tokens.
            ast::AstNode::InlineStyle { class_span, children, .. } => {
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + class_span.start,
                    length: class_span.end - class_span.start,
                    token_type: SemanticTokenType::InlineStyle,
                    modifier: None,
                });
                build_semantic_tokens(children, tokens, body_offset_in_passage, custom_macro_names, body_text);
            }
            // Text formatting markup: emit TextFormat token with a modifier
            // based on the formatting kind. This allows themes to style each
            // kind differently (bold→bold, underline→underline, etc.).
            // Italic is the default (no modifier) for backward compat.
            ast::AstNode::TextFormat { span, kind, .. } => {
                let modifier = match kind {
                    ast::TextFormatKind::Bold       => Some(SemanticTokenModifier::Bold),
                    ast::TextFormatKind::Italic     => None, // default — theme's `textFormat` entry
                    ast::TextFormatKind::Underline  => Some(SemanticTokenModifier::Underline),
                    ast::TextFormatKind::Strike     => Some(SemanticTokenModifier::Strikethrough),
                    ast::TextFormatKind::Sub        => Some(SemanticTokenModifier::Subscript),
                    ast::TextFormatKind::Super      => Some(SemanticTokenModifier::Superscript),
                };
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start,
                    length: span.end - span.start,
                    token_type: SemanticTokenType::TextFormat,
                    modifier,
                });
            }
            // MacroClose nodes are consumed by the tree builder and should not
            // appear in the final AST. If one slips through, skip it.
            ast::AstNode::MacroClose { .. } => {}
            // ── Block-level markup ──────────────────────────────────────────
            //
            // Phase 2: CodeBlock and InlineCode emit a single token over the
            // full span (plan.md §AD-5). Content is raw — no internal
            // highlighting, no recursion.
            ast::AstNode::CodeBlock { span, .. } => {
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start,
                    length: span.end - span.start,
                    token_type: SemanticTokenType::CodeBlock,
                    modifier: None,
                });
            }
            ast::AstNode::InlineCode { span, .. } => {
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start,
                    length: span.end - span.start,
                    token_type: SemanticTokenType::InlineCode,
                    modifier: None,
                });
            }
            // Verbatim text ("""..."""): emit NO tokens for the content.
            // The entire block renders as default prose color — no markup
            // highlighting inside. This is the fix for "content inside
            // `""" ... """` verbatim text gets highlights when it shouldn't."
            //
            // The parser already ensures the content is NOT recursively
            // parsed (no macros/variables/links inside), so we don't need
            // to worry about inner tokens — there are none. We just need
            // to NOT emit a token for the Verbatim span itself, so the
            // default prose color shows through.
            ast::AstNode::Verbatim { .. } => {}
            // Phase 3: Heading — emit a `Heading` token for the `!` run (the
            // marker), then recurse into children for prose/macro/variable
            // tokens. Content is recursively parsed (macros execute inside
            // headings per plan.md §3.5), so we must walk the children to
            // emit their tokens.
            //
            // The `!` run length = `level` (1..=6). The marker token covers
            // exactly the `!` characters, NOT the heading content — content
            // tokens are emitted by the recursive `build_semantic_tokens_at_depth`
            // call below.
            //
            // We recurse with `_at_depth` (preserving the surrounding depth)
            // so that macros inside headings get the correct `BlockDepthN`
            // modifier. This is the pattern recommended in plan.md §4.3.4
            // (avoiding the `InlineStyle` depth-reset behavior).
            ast::AstNode::Heading { level, children, span, .. } => {
                // Marker token: the `!` run at the start of the span.
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start,
                    length: *level as usize,
                    token_type: SemanticTokenType::Heading,
                    modifier: None,
                });
                // Recurse into children for content tokens (prose, macros,
                // variables, links, formatting, etc.).
                build_semantic_tokens_at_depth(
                    children,
                    tokens,
                    body_offset_in_passage,
                    custom_macro_names,
                    depth,
                    body_text,
                );
            }
            // Phase 4: HorizontalRule — single token over the `----` span.
            // No content, no recursion (HR is a void element).
            ast::AstNode::HorizontalRule { span } => {
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start,
                    length: span.end - span.start,
                    token_type: SemanticTokenType::HorizontalRule,
                    modifier: None,
                });
            }
            // Phase 4: Blockquote (line-style `>`/`>>`/etc.) — emit a
            // `Blockquote` token for the `>` run (the marker), then recurse
            // into children for prose/macro/variable tokens. Content is
            // recursively parsed (macros execute per §3.8.1).
            ast::AstNode::Blockquote { depth, children, span, .. } => {
                // Marker token: the `>` run at the start of the span.
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start,
                    length: *depth as usize,
                    token_type: SemanticTokenType::Blockquote,
                    modifier: None,
                });
                // Recurse into children for content tokens.
                build_semantic_tokens_at_depth(
                    children,
                    tokens,
                    body_offset_in_passage,
                    custom_macro_names,
                    *depth as usize,
                    body_text,
                );
            }
            // Phase 4: BlockquoteBlock (`<<<...<<<`) — emit `BlockquoteBlock`
            // tokens for the opening and closing `<<<` delimiters, then
            // recurse into children for content tokens. Content is recursively
            // parsed (macros execute per §3.8.2).
            ast::AstNode::BlockquoteBlock { children, open_span, close_span, .. } => {
                // Opening `<<<` delimiter token.
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + open_span.start,
                    length: open_span.end - open_span.start,
                    token_type: SemanticTokenType::BlockquoteBlock,
                    modifier: None,
                });
                // Recurse into children for content tokens.
                build_semantic_tokens_at_depth(
                    children,
                    tokens,
                    body_offset_in_passage,
                    custom_macro_names,
                    depth,
                    body_text,
                );
                // Closing `<<<` delimiter token (if present).
                if let Some(cs) = close_span {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + cs.start,
                        length: cs.end - cs.start,
                        token_type: SemanticTokenType::BlockquoteBlock,
                        modifier: None,
                    });
                }
            }
            // Phase 5: ListItem (`*`/`**`/`#`/`##` etc.) — emit a `ListMarker`
            // token for the marker run, then recurse into children for
            // prose/macro/variable tokens. Content is recursively parsed
            // (macros execute per §3.7).
            //
            // The marker run length = `marker.len()` (e.g. 1 for `*`, 2 for `**`).
            // The marker token covers exactly the marker characters, NOT the
            // item content — content tokens are emitted by the recursive call.
            ast::AstNode::ListItem { marker, children, span, .. } => {
                // Marker token: the `*`/`#` run at the start of the span.
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start,
                    length: marker.len(),
                    token_type: SemanticTokenType::ListMarker,
                    modifier: None,
                });
                // Recurse into children for content tokens (prose, macros,
                // variables, links, formatting, etc.).
                build_semantic_tokens_at_depth(
                    children,
                    tokens,
                    body_offset_in_passage,
                    custom_macro_names,
                    depth,
                    body_text,
                );
            }
            // Phase 6: Table — emit `Table` tokens for the opening `|` of each
            // row, then recurse into each cell's children for content tokens.
            //
            // We walk `rows` (which contains ALL rows in document order,
            // including header/footer types). For each row:
            //   1. Emit a `Table` token at `row.span.start` (the opening `|`),
            //      length 1.
            //   2. Recurse into each cell's `children` for prose/macro tokens.
            //
            // The internal `|` delimiters and closing `|` + suffix are NOT
            // tokenized — they fall in the gaps between cell content and
            // render as plain text. This is a simplification; a future pass
            // could emit `Table` tokens for every `|` delimiter.
            //
            // We use `build_semantic_tokens_at_depth` (preserving surrounding
            // depth) for cell content recursion, per §4.3.4.
            ast::AstNode::Table { rows, .. } => {
                for row in rows {
                    // Opening `|` of this row.
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + row.span.start,
                        length: 1,
                        token_type: SemanticTokenType::Table,
                        modifier: None,
                    });
                    // Recurse into each cell's children.
                    for cell in &row.cells {
                        build_semantic_tokens_at_depth(
                            &cell.children,
                            tokens,
                            body_offset_in_passage,
                            custom_macro_names,
                            depth,
                            body_text,
                        );
                    }
                }
            }
        }
    }
}

/// Build semantic tokens from a script passage's `JsAnalysis`.
///
/// Script passages contain pure JS (no SugarCube syntax). Their analysis is
/// stored on `PassageAst::script_js_analysis` rather than on AST nodes. This
/// function emits all token types (variables, literals, operators, namespaces,
/// function defs, function calls) from that analysis.
pub fn build_script_passage_tokens(
    analysis: &ast::JsAnalysis,
    tokens: &mut Vec<SemanticToken>,
    body_offset_in_passage: usize,
) {
    for op in &analysis.var_ops {
        emit_var_op_tokens(op, tokens, body_offset_in_passage);
    }
    emit_literal_tokens(&analysis.literal_spans, tokens, body_offset_in_passage);
    emit_operator_tokens(&analysis.operator_spans, tokens, body_offset_in_passage);
    emit_namespace_tokens(&analysis.namespace_spans, tokens, body_offset_in_passage);
    emit_comment_tokens(&analysis.comment_spans, tokens, body_offset_in_passage);
    emit_keyword_tokens(&analysis.keyword_spans, tokens, body_offset_in_passage);
    // JS local variables, methods, properties, and globals
    emit_js_var_tokens(&analysis.js_var_spans, tokens, body_offset_in_passage);
    emit_js_var_def_tokens(&analysis.js_var_def_spans, tokens, body_offset_in_passage);
    emit_js_method_tokens(&analysis.js_method_spans, tokens, body_offset_in_passage);
    emit_js_property_tokens(&analysis.js_property_spans, tokens, body_offset_in_passage);
    emit_js_global_tokens(&analysis.js_global_spans, tokens, body_offset_in_passage);
    emit_function_def_tokens(&analysis.function_defs, tokens, body_offset_in_passage);
    emit_function_call_tokens(&analysis.function_calls, tokens, body_offset_in_passage);
}

/// Push a `MacroDelimiter` token for `<<`, `>>`, or `<</`.
///
/// `start` is the body-relative byte offset of the delimiter, `len` is its
/// length in bytes (2 for `<<`/`>>`, 3 for `<</`). `modifier` is whatever
/// modifier the corresponding macro name token received — this lets depth
/// coloring and the deprecated flag propagate to delimiters automatically.
///
/// See `build_semantic_tokens_at_depth` for the full rationale.
fn push_delimiter(
    tokens: &mut Vec<SemanticToken>,
    body_offset_in_passage: usize,
    start: usize,
    len: usize,
    modifier: Option<SemanticTokenModifier>,
) {
    tokens.push(SemanticToken {
        start: body_offset_in_passage + start,
        length: len,
        token_type: SemanticTokenType::MacroDelimiter,
        modifier,
    });
}

/// Emit semantic tokens for a single `AnalyzedVarOp`.
///
/// Uses `segment_spans` for per-token highlighting when available,
/// giving exact span precision for each variable/property token.
fn emit_var_op_tokens(op: &ast::AnalyzedVarOp, tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    let modifier = if op.access_kind.is_write() {
        Some(SemanticTokenModifier::Definition)
    } else {
        None
    };

    if !op.segment_spans.is_empty() {
        for (i, seg_span) in op.segment_spans.iter().enumerate() {
            let token_type = if i == 0 {
                SemanticTokenType::Variable  // $foo — the root variable
            } else {
                SemanticTokenType::Property  // .bar, .baz — property access
            };
            tokens.push(SemanticToken {
                start: body_offset_in_passage + seg_span.start,
                length: seg_span.end - seg_span.start,
                token_type,
                modifier,
            });
        }
    } else {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + op.span.start,
            length: op.span.end - op.span.start,
            token_type: SemanticTokenType::Variable,
            modifier,
        });
    }
}

/// Emit semantic tokens for literal spans found by oxc.
///
/// Maps `LiteralKind` to the appropriate `SemanticTokenType` and pushes
/// a token for each literal. The spans are passage-body-relative (already
/// mapped through the preprocessor), so we just add `body_offset_in_passage`.
fn emit_literal_tokens(literals: &[ast::LiteralSpan], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for lit in literals {
        let token_type = match lit.kind {
            ast::LiteralKind::String  => SemanticTokenType::String,
            ast::LiteralKind::Number  => SemanticTokenType::Number,
            ast::LiteralKind::Boolean => SemanticTokenType::Boolean,
            ast::LiteralKind::Null    => SemanticTokenType::Keyword,
        };
        tokens.push(SemanticToken {
            start: body_offset_in_passage + lit.span.start,
            length: lit.span.end - lit.span.start,
            token_type,
            modifier: None,
        });
    }
}

/// Emit semantic tokens for operator spans found by oxc and the preprocessor.
///
/// SugarCube keyword operators (`to`, `eq`, `and`, etc.) are classified as
/// comparison/assignment/logical operators by `OperatorKind`. Standard JS
/// operators that weren't substituted are also emitted here.
///
/// All operators emit the same token type (`Operator`) with no modifiers —
/// they should render in a single unified color. Previously, `Logical`
/// operators received a `ControlFlow` modifier which caused themes to render
/// them in a separate color from other operators. The user-facing decision is
/// that operators should be visually unified regardless of category; theme
/// authors who want per-category styling can still distinguish via the
/// `OperatorKind` data exposed in the AST (should we ever re-introduce
/// modifiers behind a setting).
fn emit_operator_tokens(operators: &[ast::OperatorSpan], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for op in operators {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + op.span.start,
            length: op.span.end - op.span.start,
            token_type: SemanticTokenType::Operator,
            modifier: None,
        });
    }
}

/// Emit semantic tokens for namespace spans found by oxc.
///
/// SugarCube global objects like `State`, `Engine`, `Story`, `Config`, etc.
/// get `Namespace` tokens. Properties accessed on them get `Property` tokens.
/// This provides distinct visual styling for global object references that
/// aren't `$variable` accesses.
///
/// **Deduplication note**: `State.variables.x` patterns are NOT emitted here
/// because they're already covered by `AnalyzedVarOp` (which emits `Variable`
/// + `Property` tokens). Only non-variable-access patterns on globals
/// (e.g., `State.turns`, `Engine.play()`, `Config.debug`) are emitted.
fn emit_namespace_tokens(namespaces: &[ast::NamespaceSpan], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for ns in namespaces {
        // Namespace token for the global object name
        tokens.push(SemanticToken {
            start: body_offset_in_passage + ns.span.start,
            length: ns.span.end - ns.span.start,
            token_type: SemanticTokenType::Namespace,
            modifier: None,
        });
        // Property tokens for each property accessed on the global
        for prop in &ns.property_spans {
            tokens.push(SemanticToken {
                start: body_offset_in_passage + prop.span.start,
                length: prop.span.end - prop.span.start,
                token_type: SemanticTokenType::Property,
                modifier: None,
            });
        }
    }
}

/// Emit semantic tokens for function definitions found by oxc.
///
/// Function declarations and named function expressions in JS contexts
/// (inside `<<run>>`, `<<script>>`, etc.) get `Function` tokens with
/// the `Definition` modifier. This covers patterns like:
/// - `function myHelper() { ... }` → Function token on "myHelper"
/// - `var calculateScore = function() { ... }` → Function token on "calculateScore"
/// - `const add = () => { ... }` → Function token on "add"
fn emit_function_def_tokens(function_defs: &[ast::FunctionDefInfo], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for func_def in function_defs {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + func_def.name_offset,
            length: func_def.name.len(),
            token_type: SemanticTokenType::Function,
            modifier: Some(SemanticTokenModifier::Definition),
        });
    }
}


fn emit_comment_tokens(comments: &[ast::CommentSpan], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for c in comments { tokens.push(SemanticToken { start: body_offset_in_passage + c.span.start, length: c.span.end - c.span.start, token_type: SemanticTokenType::Comment, modifier: None }); }
}

/// Emit semantic tokens for JS keywords found by oxc.
///
/// Covers statement-level keywords (`if`, `for`, `while`, `return`, `try`,
/// `catch`, `finally`, `function`), declaration keywords (`var`, `let`,
/// `const`), and expression-level keywords (`new`, `typeof`, `instanceof`,
/// `delete`, `void`, `in`). Each gets a `Keyword` token so themes can
/// color JS keywords distinctly from SugarCube macro names.
fn emit_keyword_tokens(keywords: &[ast::KeywordSpan], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for kw in keywords {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + kw.span.start,
            length: kw.span.end - kw.span.start,
            token_type: SemanticTokenType::Keyword,
            modifier: None,
        });
    }
}

/// Emit semantic tokens for function call sites found by oxc.
///
/// When an identifier that was preprocessed from a SugarCube `$var` or `_var`
/// is used as a function call target (e.g., `_myHelper()`), it should be
/// classified as a function call, not a variable reference. This function
/// emits `Function` tokens for those call sites.
fn emit_function_call_tokens(function_calls: &[ast::FunctionCallInfo], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for call in function_calls {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + call.span.start,
            length: call.span.end - call.span.start,
            token_type: SemanticTokenType::Function,
            modifier: None,
        });
    }
}

/// Emit semantic tokens for plain JS local variable references.
///
/// These are identifiers that are NOT SugarCube variables (`$var`/`_var`),
/// NOT properties, and NOT function calls — just plain JS locals like
/// `el`, `g`, `profile`, `vm`, `html`. Emitted as `Variable` tokens.
fn emit_js_var_tokens(spans: &[std::ops::Range<usize>], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for span in spans {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + span.start,
            length: span.end - span.start,
            token_type: SemanticTokenType::Variable,
            modifier: None,
        });
    }
}

/// Emit semantic tokens for JS local variable declarations.
///
/// These are the binding names in `var x = ...`, `let x = ...`, `const x = ...`,
/// and function parameters. Emitted as `Variable` tokens with the `Definition`
/// modifier so themes can bold them.
fn emit_js_var_def_tokens(spans: &[std::ops::Range<usize>], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for span in spans {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + span.start,
            length: span.end - span.start,
            token_type: SemanticTokenType::Variable,
            modifier: Some(SemanticTokenModifier::Definition),
        });
    }
}

/// Emit semantic tokens for JS method call names.
///
/// These are the property names in `expr.method(...)` patterns, e.g.
/// `.forEach`, `.getElementById`, `.isArray`, `.filter`. Emitted as
/// `Function` tokens so they're visually distinct from property accesses.
fn emit_js_method_tokens(spans: &[std::ops::Range<usize>], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for span in spans {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + span.start,
            length: span.end - span.start,
            token_type: SemanticTokenType::Function,
            modifier: None,
        });
    }
}

/// Emit semantic tokens for JS property access names.
///
/// These are the property names in `expr.prop` patterns (not followed by `(`),
/// e.g. `.left`, `.length`, `.innerHTML`, `.showIf`. Emitted as `Property`
/// tokens so themes can color them distinctly from variables.
fn emit_js_property_tokens(spans: &[std::ops::Range<usize>], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for span in spans {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + span.start,
            length: span.end - span.start,
            token_type: SemanticTokenType::Property,
            modifier: None,
        });
    }
}

/// Emit semantic tokens for JS global object references.
///
/// These are identifiers matching known JS globals (`document`, `Array`,
/// `Math`, `JSON`, `console`, etc.). Emitted as `Namespace` tokens.
fn emit_js_global_tokens(spans: &[std::ops::Range<usize>], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for span in spans {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + span.start,
            length: span.end - span.start,
            token_type: SemanticTokenType::Namespace,
            modifier: None,
        });
    }
}

/// Emit semantic tokens for structured macro arguments (Phase 6).
///
/// Each `StructuredMacroArg` gets a token type based on its `ParsedArgKind`:
///
/// - `PassageRef` → `Link` token (same as `[[ ]]` links, enabling consistent
///   passage name highlighting and go-to-definition)
/// - `Label` → `String` token (display text in link/button macros)
/// - `Selector` → `String` token (CSS selectors)
/// - `String` → `String` token (generic string values like speed "2s")
/// - `VariableRef` → `Variable` token (variable used as passage target, etc.)
/// - `Expression` → no token (JS expressions are handled by oxc)
///
/// **Deduplication**: `VariableRef` tokens from structured_args may overlap
/// with tokens from `js_analysis`/`var_refs`. The token builder emits both
/// because structured_args captures the *semantic role* (e.g., `$dest` as a
/// dynamic passage target) while var_ops captures the *JS semantics* (read
/// vs write). The editor typically uses the last-emitted token for a position,
/// so this is acceptable — the user sees the correct highlighting either way.
fn emit_structured_arg_tokens(sargs: &[ast::StructuredMacroArg], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for sarg in sargs {
        let token_type = match sarg.kind {
            ast::ParsedArgKind::PassageRef => SemanticTokenType::Link,
            ast::ParsedArgKind::Label => SemanticTokenType::String,
            ast::ParsedArgKind::Selector => SemanticTokenType::String,
            ast::ParsedArgKind::String => SemanticTokenType::String,
            ast::ParsedArgKind::VariableRef => SemanticTokenType::Variable,
            ast::ParsedArgKind::Expression => continue, // Handled by oxc
            ast::ParsedArgKind::Keyword => SemanticTokenType::Keyword,
            ast::ParsedArgKind::LinkMarkup => SemanticTokenType::Link,
            ast::ParsedArgKind::ImageMarkup => SemanticTokenType::Link,
            ast::ParsedArgKind::Number => SemanticTokenType::Number,
        };
        tokens.push(SemanticToken {
            start: body_offset_in_passage + sarg.span.start,
            length: sarg.span.end - sarg.span.start,
            token_type,
            modifier: None,
        });
    }
}

/// Build diagnostics from AST error nodes and deprecated macro usage.
///
/// Emits:
/// - Error diagnostics for parse errors and unclosed block macros
/// - Hint diagnostics for deprecated macro usage (with deprecation message)
///
/// All diagnostic ranges are **passage-relative**: byte 0 is the `::` prefix
/// of the passage header. The `body_offset_in_passage` parameter converts
/// AST body spans (which are relative to body start) to passage-relative
/// offsets. The LSP boundary adds `passage_offset` to produce
/// document-absolute ranges.
///
/// `custom_macros` is the workspace's custom macro registry. It's consulted
/// when a macro isn't found in the builtin catalog — if a custom macro has
/// `BodyRequirement::Never` (inline), no "unclosed" diagnostic is emitted
/// even if the tree builder gave it children. Without this, every inline
/// custom macro (e.g., `Macro.add("emojify", { handler() {...} })` used as
/// `<<emojify "x">>`) would produce a false "Unclosed block macro" error.
pub fn build_diagnostics(
    nodes: &[ast::AstNode],
    diagnostics: &mut Vec<FormatDiagnostic>,
    body_offset_in_passage: usize,
    custom_macros: &crate::sugarcube::registries::CustomMacroRegistry,
) {
    let dep_macros = deprecated_macros();
    for node in nodes {
        if let ast::AstNode::Error { message, span } = node {
            diagnostics.push(FormatDiagnostic {
                range: body_offset_in_passage + span.start..body_offset_in_passage + span.end,
                message: message.clone(),
                severity: FormatDiagnosticSeverity::Error,
                code: "sc-parse".to_string(),
            });
        }
        if let ast::AstNode::Macro { children, name, name_span, close_span, .. } = node {
            // Unclosed block macro diagnostic.
            //
            // Only emit for macros with BodyRequirement::Required that have
            // children but no close tag. Macros with BodyRequirement::Optional
            // (e.g. <<case>>, <<default>>) are allowed to omit the close tag —
            // the tree builder handles this without error.
            //
            // Custom macros (defined via Macro.add() or <<widget>>) are looked
            // up in the custom_macros registry. Their BodyRequirement is
            // derived from the `tags` field of the Macro.add() config (or the
            // `container` keyword for widgets). Previously, all custom macros
            // fell back to `Required`, causing false "Unclosed block macro"
            // errors for every inline custom macro.
            if children.is_some() && close_span.is_none() {
                let body_req = lookup_body_requirement(name, custom_macros);
                if body_req == crate::types::BodyRequirement::Required {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset_in_passage + name_span.start..body_offset_in_passage + name_span.end,
                        message: format!("Unclosed block macro: <<{}>>", name),
                        severity: FormatDiagnosticSeverity::Error,
                        code: "sc-unclosed".to_string(),
                    });
                }
            }
            // Deprecated macro usage diagnostic
            if let Some(msg) = dep_macros.get(name.as_str()) {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset_in_passage + name_span.start..body_offset_in_passage + name_span.end,
                    message: (*msg).to_string(),
                    severity: FormatDiagnosticSeverity::Hint,
                    code: "sc-deprecated".to_string(),
                });
            }
            if let Some(ch) = children {
                build_diagnostics(ch, diagnostics, body_offset_in_passage, custom_macros);
            }
        }
    }
}

/// Look up a macro's `BodyRequirement` from the builtin catalog, falling
/// back to the custom macro registry.
///
/// For builtin macros: uses `macros::find_macro(name)` → `MacroDef.body`.
/// For custom macros: uses `custom_macros.get(name)` → `CustomMacro.body`.
/// For completely unknown macros: returns `BodyRequirement::Optional` —
/// matching the tree builder's behavior for unknown macros. This means
/// macros that Knot has never seen (not builtin, not custom) won't produce
/// false "unclosed" errors.
fn lookup_body_requirement(
    name: &str,
    custom_macros: &crate::sugarcube::registries::CustomMacroRegistry,
) -> crate::types::BodyRequirement {
    // Check builtin catalog first.
    if let Some(def) = crate::sugarcube::macros::find_macro(name) {
        return def.body;
    }
    // Check custom macro registry.
    if let Some(custom) = custom_macros.get(name) {
        return custom.body;
    }
    // Unknown macro — match the tree builder's default of `Optional`.
    // This avoids false "unclosed" errors for macros that Knot doesn't
    // know about yet (e.g., from a format plugin that hasn't loaded).
    crate::types::BodyRequirement::Optional
}

/// Find a tag name's byte offset within `[...]` bracket blocks.
///
/// Scans only inside `[...]` blocks in `tags_raw`, avoiding false matches
/// on the passage name portion. Returns the byte offset relative to the
/// start of `tags_raw` (which the caller adjusts by `name_start` to get
/// a document-absolute offset).
///
/// For example, given `tags_raw = "DarkForest [dark scary]"` and tag `"dark"`,
/// this returns `Some(12)` (pointing to "dark" inside the brackets), NOT `Some(0)`
/// (which would incorrectly point to "Dark" in "DarkForest").
fn find_tag_in_brackets(tags_raw: &str, tag: &str) -> Option<usize> {
    let bytes = tags_raw.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Find the next '['
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }

        // Found '[' — find matching ']'
        let bracket_start = i;
        let mut j = i + 1;
        while j < len && bytes[j] != b']' {
            j += 1;
        }

        if j >= len {
            // Unclosed bracket — no more blocks to search
            break;
        }

        // We have a complete `[...]` block. Search for the tag
        // as a whole word within this block's interior.
        let interior = &tags_raw[bracket_start + 1..j];
        if let Some(pos_in_interior) = find_word_in_text(interior, tag) {
            return Some(bracket_start + 1 + pos_in_interior);
        }

        // Move past this bracket block
        i = j + 1;
    }

    None
}

/// Find a whole-word occurrence of `word` in `text`, returning the byte offset.
///
/// A "whole word" means the match is bounded by non-alphanumeric characters
/// (or the start/end of text). This prevents "dark" from matching "darkness".
fn find_word_in_text(text: &str, word: &str) -> Option<usize> {
    let word_bytes = word.as_bytes();
    let word_len = word_bytes.len();
    let text_bytes = text.as_bytes();
    let text_len = text_bytes.len();

    if word_len == 0 || word_len > text_len {
        return None;
    }

    let mut search_start = 0;
    while search_start <= text_len - word_len {
        // Find the next occurrence of the word text
        if let Some(pos) = text[search_start..].find(word) {
            let abs_pos = search_start + pos;

            // Check that the character before the match is a word boundary
            let before_ok = if abs_pos == 0 {
                true
            } else {
                let prev = text_bytes[abs_pos - 1];
                !prev.is_ascii_alphanumeric() && prev != b'_'
            };

            // Check that the character after the match is a word boundary
            let after_pos = abs_pos + word_len;
            let after_ok = if after_pos >= text_len {
                true
            } else {
                let next = text_bytes[after_pos];
                !next.is_ascii_alphanumeric() && next != b'_'
            };

            if before_ok && after_ok {
                return Some(abs_pos);
            }

            // Not a whole-word match — advance past this occurrence
            search_start = abs_pos + 1;
        } else {
            break;
        }
    }

    None
}

/// Build header tokens for a passage.
///
/// Produces semantic tokens for the `::` prefix, the passage name, and
/// each tag within `[...]` blocks. All byte offsets are **passage-relative**:
/// byte 0 is the `::` prefix of the passage header. The `TweeHeader` fields
/// `header_start` and `name_start` are document-absolute, so we subtract
/// `header_start` to produce passage-relative offsets.
pub fn build_header_tokens(header: &crate::header::TweeHeader, is_special: bool) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // All header offsets are relative to the passage head (:: prefix).
    // The TweeHeader stores document-absolute offsets, so we subtract
    // header_start to make them passage-relative.
    let head = header.header_start;

    // :: prefix token — always at passage-relative offset 0
    let header_type = if is_special {
        SemanticTokenType::SpecialPassageHeader
    } else {
        SemanticTokenType::PassageHeader
    };
    tokens.push(SemanticToken {
        start: 0, // :: is always at the very start of the passage
        length: 2, // ::
        token_type: header_type,
        modifier: None,
    });

    // Passage name token
    let name_type = if is_special {
        SemanticTokenType::SpecialPassage
    } else {
        SemanticTokenType::PassageName
    };
    let name_len = header.name.len();
    tokens.push(SemanticToken {
        start: header.name_start - head,
        length: name_len,
        token_type: name_type,
        modifier: None,
    });

    // Tag tokens — only the tag names, with appropriate modifiers.
    //
    // We search for each tag inside `[...]` bracket blocks within
    // `tags_raw`, NOT by doing a simple `find()` on the entire string.
    // A naive `find()` could match a tag name that appears as a
    // substring of the passage name (e.g., tag "dark" matching inside
    // "DarkForest" in `tags_raw = "DarkForest [dark]"`).
    //
    // `tags_raw[0]` aligns with `name_start` in the document, so
    // `name_start + tag_pos - head` gives the passage-relative offset.
    for tag in &header.tags {
        if let Some(tag_pos) = find_tag_in_brackets(&header.tags_raw, tag) {
            let modifier = self_classify_tag(tag);
            tokens.push(SemanticToken {
                start: header.name_start - head + tag_pos,
                length: tag.len(),
                token_type: SemanticTokenType::Tag,
                modifier,
            });
        }
    }

    tokens
}

/// Classify a tag and return the appropriate semantic token modifier.
pub fn self_classify_tag(tag: &str) -> Option<SemanticTokenModifier> {
    // Core tags: [script], [stylesheet], [style]
    for def in knot_core::passage::twine_core_special_passages() {
        if def.match_strategy == knot_core::passage::MatchStrategy::Tag
            && tag.eq_ignore_ascii_case(&def.name)
        {
            return Some(SemanticTokenModifier::TwineCore);
        }
    }
    // Legacy core tags
    for def in knot_core::passage::legacy_core_special_passages() {
        if def.match_strategy == knot_core::passage::MatchStrategy::Tag
            && tag.eq_ignore_ascii_case(&def.name)
        {
            return Some(SemanticTokenModifier::TwineCore);
        }
    }
    // Format-specific tags: [init], [widget]
    for def in special_passages::tag_matched_special_passages() {
        if tag.eq_ignore_ascii_case(&def.name) {
            return Some(SemanticTokenModifier::StoryFormat);
        }
    }
    None
}

/// Build semantic tokens for a JSON body (used for StoryData passages).
pub fn build_json_body_tokens(body: &str, body_offset_in_passage: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut pos = 0usize;

    while pos < len {
        match bytes[pos] {
            b'"' => {
                let start = pos;
                pos += 1;
                let mut is_escaped = false;
                while pos < len {
                    if is_escaped {
                        is_escaped = false;
                        pos += 1;
                        continue;
                    }
                    if bytes[pos] == b'\\' {
                        is_escaped = true;
                        pos += 1;
                        continue;
                    }
                    if bytes[pos] == b'"' {
                        pos += 1;
                        break;
                    }
                    pos += 1;
                }
                let end = pos;

                let mut lookahead = pos;
                while lookahead < len && bytes[lookahead] == b' ' || lookahead < len && bytes[lookahead] == b'\t' || lookahead < len && bytes[lookahead] == b'\n' || lookahead < len && bytes[lookahead] == b'\r' {
                    lookahead += 1;
                }
                let is_property_name = lookahead < len && bytes[lookahead] == b':';

                if is_property_name {
                    let content_start = start + 1;
                    let content_end = end.saturating_sub(1);
                    if content_start < content_end {
                        tokens.push(SemanticToken {
                            start: body_offset_in_passage + content_start,
                            length: content_end - content_start,
                            token_type: SemanticTokenType::Property,
                            modifier: Some(SemanticTokenModifier::TwineCore),
                        });
                    }
                } else {
                    let content_start = start + 1;
                    let content_end = end.saturating_sub(1);
                    if content_start < content_end {
                        tokens.push(SemanticToken {
                            start: body_offset_in_passage + content_start,
                            length: content_end - content_start,
                            token_type: SemanticTokenType::String,
                            modifier: None,
                        });
                    }
                }
            }
            b'0'..=b'9' | b'-' => {
                let start = pos;
                if bytes[pos] == b'-' {
                    pos += 1;
                }
                while pos < len && (bytes[pos].is_ascii_digit() || bytes[pos] == b'.' || bytes[pos] == b'e' || bytes[pos] == b'E' || bytes[pos] == b'+' || bytes[pos] == b'-') {
                    pos += 1;
                }
                if pos == start + 1 && bytes[start] == b'-' {
                    continue;
                }
                let end = pos;
                if end > start {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + start,
                        length: end - start,
                        token_type: SemanticTokenType::Number,
                        modifier: None,
                    });
                }
            }
            b't' => {
                if body[pos..].starts_with("true") {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + pos,
                        length: 4,
                        token_type: SemanticTokenType::Boolean,
                        modifier: None,
                    });
                    pos += 4;
                    continue;
                }
                pos += 1;
            }
            b'f' => {
                if body[pos..].starts_with("false") {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + pos,
                        length: 5,
                        token_type: SemanticTokenType::Boolean,
                        modifier: None,
                    });
                    pos += 5;
                    continue;
                }
                pos += 1;
            }
            b'n' => {
                if body[pos..].starts_with("null") {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + pos,
                        length: 4,
                        token_type: SemanticTokenType::Keyword,
                        modifier: None,
                    });
                    pos += 4;
                    continue;
                }
                pos += 1;
            }
            _ => {
                pos += 1;
            }
        }
    }

    tokens
}

/// Check if a macro name is a folding modifier (else, elseif, case, default).
/// These are structural siblings of their parent, not nested children —
/// they should render at the parent's depth level.
fn is_folding_modifier(name: &str) -> bool {
    folding_modifier_names().contains(name)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::parser::parse_passage_body;
    use crate::plugin::SemanticTokenType;

    /// Build semantic tokens for a body string and return all tokens
    /// as `(type_name, text)` pairs.
    fn all_token_texts(body: &str) -> Vec<(&'static str, String)> {
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &std::collections::HashSet::new(), body);

        tokens.iter()
            .map(|t| {
                let type_name = match t.token_type {
                    SemanticTokenType::Link => "Link",
                    SemanticTokenType::String => "String",
                    SemanticTokenType::Variable => "Variable",
                    SemanticTokenType::Macro => "Macro",
                    SemanticTokenType::PassageName => "PassageName",
                    SemanticTokenType::PassageHeader => "PassageHeader",
                    SemanticTokenType::Prose => "Prose",
                    _ => "Other",
                };
                let text = body[t.start.min(body.len())..(t.start + t.length).min(body.len())].to_string();
                (type_name, text)
            })
            .filter(|(tn, _)| matches!(*tn, "Link" | "String" | "Variable"))
            .collect()
    }

    /// Regression test: `[[Target]]` simple link should produce ONE Link
    /// token covering just the target passage name.
    #[test]
    fn link_token_simple_link_covers_target_only() {
        let tokens = all_token_texts("Go [[Forest]] now");
        assert_eq!(tokens, vec![("Link", "Forest".to_string())]);
    }

    /// Regression test: `[[Display|Target]]` pipe link should produce TWO
    /// tokens — a `String` on the display, a `Link` on the target.
    ///
    /// Previous bug #1: a single Link token covered "Retur" (first 5 bytes
    /// of "Return to start") because the span used `target.len()` from
    /// `span.start + 2`.
    /// Previous bug #2: a single Link token covered the ENTIRE content
    /// "Return to start|Start", swallowing the display and separator.
    #[test]
    fn link_token_pipe_link_decomposes_into_display_and_target() {
        let tokens = all_token_texts("[[Return to start|Start]]");
        assert_eq!(tokens, vec![
            ("Link", "Start".to_string()),
            ("String", "Return to start".to_string()),
        ]);
    }

    #[test]
    fn link_token_arrow_right_decomposes() {
        let tokens = all_token_texts("[[Display->Target]]");
        assert_eq!(tokens, vec![
            ("Link", "Target".to_string()),
            ("String", "Display".to_string()),
        ]);
    }

    #[test]
    fn link_token_arrow_left_decomposes() {
        let tokens = all_token_texts("[[Target<-Display]]");
        assert_eq!(tokens, vec![
            ("Link", "Target".to_string()),
            ("String", "Display".to_string()),
        ]);
    }

    /// Regression test: setter links `[[Display|Target][$var += 5]]` should
    /// produce THREE tokens — `String` on display, `Link` on target,
    /// `Variable` on the setter variable `$var`. The setter expression
    /// content (operators, numbers) does NOT get a token — those bytes
    /// are not highlighted by any semantic token type.
    #[test]
    fn link_token_setter_link_emits_variable_for_setter_var() {
        let tokens = all_token_texts("[[Link with setter|Time][$playerGold += 5]]");
        assert_eq!(tokens, vec![
            ("Link", "Time".to_string()),
            ("String", "Link with setter".to_string()),
            ("Variable", "$playerGold".to_string()),
        ]);
    }

    /// Regression test: image links `[[img[url][Target]]` should produce
    /// ONE Link token on the target. The image URL is not currently
    /// tokenized (no dedicated ImageUrl token type).
    #[test]
    fn link_token_image_link_covers_target_only() {
        let tokens = all_token_texts("[[img[http://example.com/pic.jpg][Forest]]");
        assert_eq!(tokens, vec![("Link", "Forest".to_string())]);
    }

    /// Image link with display: `[[img[url][Display|Target]]` should
    /// produce `String` on the display, `Link` on the target.
    #[test]
    fn link_token_image_link_with_display_decomposes() {
        let tokens = all_token_texts("[[img[http://example.com/pic.jpg][Dark Forest|Forest]]");
        assert_eq!(tokens, vec![
            ("Link", "Forest".to_string()),
            ("String", "Dark Forest".to_string()),
        ]);
    }

    /// Regression test: links with multi-byte UTF-8 characters should not
    /// panic and should produce correct per-region tokens.
    #[test]
    fn link_token_multibyte_utf8_decomposes_correctly() {
        let tokens = all_token_texts("[[Café—naïve|Target]]");
        assert_eq!(tokens, vec![
            ("Link", "Target".to_string()),
            ("String", "Café—naïve".to_string()),
        ]);
    }
}
