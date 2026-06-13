# SugarCube Parser & Prose Token — Issues and Fix Plan

> Audited against: [SugarCube 2 Documentation](https://www.motoslave.net/sugarcube/2/docs/)
> Date: 2026-06-13
> Branch: `ver_3`

---

## Issue 1 — `<<done>>` misclassified as prose-rendering

**Severity:** High (spec incoherence, produces wrong tokens)
**Files:** `crates/formats/src/sugarcube/parser/tree_builder.rs`

### Problem

`<<done>>` "executes code after the passage is fully rendered" — its body is imperative code, not narrative. The function `is_prose_rendering_macro()` at line 357 currently treats every macro as prose-rendering except `silently`, `script`, `style`, and `css`. This means text inside `<<done>>` incorrectly gets `is_prose = true` and emits `Prose` semantic tokens.

### Expected behavior

Text inside `<<done>>` should be marked `is_prose = false`, same as `<<silently>>`.

### Fix

Add `"done"` to the exclusion list in `is_prose_rendering_macro()`:

```rust
fn is_prose_rendering_macro(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !matches!(lower.as_str(), "silently" | "script" | "style" | "css" | "done")
}
```

Add a test `prose_inside_done_is_not_prose` in `core.rs`.

---

## Issue 2 — `into` keyword operator missing from normalization and `<<set>>` parser

**Severity:** High (valid SugarCube syntax silently falls through to opaque JS)
**Files:** `crates/formats/src/sugarcube/macros/operators.rs`, `crates/formats/src/sugarcube/parser/macro_parser.rs`

### Problem

SugarCube supports `<<set 100 into $hp>>` as a reverse-assignment syntax (value on the left, variable on the right). The `operators.rs` normalization table does not include `into`, and the `<<set>>` assignment parser in `macro_parser.rs` does not recognize it. The entire expression falls through to oxc as an opaque JS expression, losing structured assignment semantics.

### Expected behavior

`<<set 100 into $hp>>` should be parsed as a structured `SetAssignment` with a new `SetOperator::Into` variant, where the target variable is on the right side and the expression is on the left.

### Fix

1. **`ast.rs`** — Add `Into` variant to `SetOperator` enum with doc comment.
2. **`operators.rs`** — Add `into` → `=` mapping in the normalization table.
3. **`macro_parser.rs`** — In `parse_set_assignment()`, add a branch that detects the `into` keyword and parses the reverse-assignment form: expression on the left, variable on the right. Handle the boundary check (same as `to` keyword — must not match inside words like `intogether`).
4. **`token_builder.rs`** — Handle `SetOperator::Into` in the set-assignment token emission (emit `Variable` + `Definition` for the target on the right side).
5. Add tests: `set_into_assignment`, `set_into_keyword_boundary`.

---

## Issue 3 — Image links `[[img[url][passage]]` misparsed as setter links

**Severity:** High (produces wrong link target)
**Files:** `crates/formats/src/sugarcube/parser/link_parser.rs`

### Problem

The link parser uses `trimmed.rfind("][")` to detect setter syntax (`[[target][$var to value]]`). For image links like `[[img[image_url][passage_name]]`, this `][` sits between the image URL and the passage name. The current code:

- `inner_before` = `img[image_url` (with trailing `]` stripped)
- `inner_after` = `passage_name` (with leading `[` stripped)
- Since `passage_name` doesn't start with `$` or `_`, `setter_var` is `None`
- The link target becomes `img[image_url` instead of `passage_name`

The `img[` prefix is never recognized, and the passage name is lost.

### Expected behavior

`[[img[http://example.com/pic.jpg][Forest]]` should produce a link with `target = "Forest"`, an image URL extracted separately, and `LinkKind::Image` (new variant).

### Fix

1. **`ast.rs`** — Add `Image` variant to `LinkKind` enum. Add `image_url: Option<String>` field to `AstNode::Link`.
2. **`link_parser.rs`** — In `parse_link_content()`, check for the `img[` prefix *before* the setter `][` detection:
   - If content starts with `img[`, extract the image URL between `img[` and the next `]`.
   - The remaining content after `][` is the passage target (or display|target with optional pipe/arrow).
   - Return `LinkKind::Image` with `image_url` populated.
3. **`extraction.rs`** — Handle image links in link extraction (they produce `LinkSource::PassageLink` with the passage target).
4. Add tests: `image_link_simple`, `image_link_with_display`.

---

## Issue 4 — `<<print>>` vs `<<=>>` semantic identity but structural divergence

**Severity:** Medium (downstream complexity)
**Files:** `crates/formats/src/sugarcube/ast.rs`, `crates/formats/src/sugarcube/lsp/token_builder.rs`

### Problem

SugarCube treats `<<print expr>>` and `<<=>>expr>>` as semantically identical. The parser puts them in different AST node types: `<<print>>` → `AstNode::Macro`, `<<=>>` → `AstNode::Expression { kind: Print }`. Downstream consumers (token builder, JS annotation pass, extraction) must handle both paths for what is the same operation. This creates maintenance burden and potential for divergent behavior.

### Expected behavior

Both should produce consistent token emission and downstream handling. The structural difference can remain (Expression is a lighter-weight node), but the token builder should emit equivalent tokens.

### Fix (minimal — normalize at token level)

1. **`token_builder.rs`** — When handling `AstNode::Expression`, emit a `Macro` token for the sigil span (`=` or `-`) so it's visually consistent with `<<print>>` getting a `Macro` token for "print". Use the `open_span` to determine the sigil position and length.
2. **`token_builder.rs`** — Use `ExprKind` to differentiate: `Print` gets no modifier, `Silent` gets `ControlFlow` modifier (signal suppressed output).
3. Add tests verifying `<<print>>` and `<<=>>` emit equivalent variable tokens.

### Alternative (larger — normalize at AST level)

Refactor `<<print>>` to also produce `AstNode::Expression { kind: Print }` instead of `AstNode::Macro`. This would unify the downstream path but requires changes to the JS annotation pass and extraction. Recommend the minimal fix first.

---

## Issue 5 — `@@class;text@@` inline styling not parsed

**Severity:** Medium (commonly used markup with no semantic support)
**Files:** `crates/formats/src/sugarcube/parser/core.rs`, `crates/formats/src/sugarcube/ast.rs`, `crates/formats/src/plugin.rs`, `crates/formats/src/sugarcube/lsp/token_builder.rs`

### Problem

SugarCube's `@@class;text@@` inline styling markup (produces `<span class="class">text</span>`) is not recognized. The `@@` sequence falls through to plain text in `core.rs:129-132`. Similarly, `@class;text@` (SugarCube 2.37+ shorthand) is not recognized.

### Expected behavior

- `@@.highlight;important text@@` → `InlineStyle` AST node with class=".highlight", body="important text"
- Token emission: `InlineStyle` token for the class name, `Prose` tokens for the body text

### Fix

1. **`plugin.rs`** — Add `InlineStyle` variant to `SemanticTokenType` with doc: "SugarCube inline styling markup (@@class;text@@)".
2. **`ast.rs`** — Add `InlineStyle { class: String, class_span: Range<usize>, children: Vec<AstNode>, span: Range<usize> }` variant to `AstNode`.
3. **`core.rs`** — Add `b'@'` branch: if `bytes[i+1] == b'@'`, parse `@@class;text@@` (double-at). If `bytes[i+1]` is an ident char, parse `@class;text@` (single-at). Extract the class name up to `;`, then recursively parse the body content between the delimiters. Flush text before the delimiter start.
4. **`token_builder.rs`** — Handle `InlineStyle`: emit `InlineStyle` token for the class span, recurse into children for prose/variable tokens.
5. **`lifecycle.rs`** / **`semantic.rs`** — Register `InlineStyle` in the token legend and mapping.
6. **`package.json`** — Add scope mapping for `inlineStyle`.
7. Add tests: `inline_style_double_at`, `inline_style_single_at`, `inline_style_with_variable`.

---

## Issue 6 — Text formatting markup not parsed (`''bold''`, `//italic//`, etc.)

**Severity:** Low (TextMate grammar can cover this; `//` ambiguity complicates parsing)
**Files:** `crates/formats/src/sugarcube/parser/core.rs`, `crates/formats/src/sugarcube/ast.rs`, `crates/formats/src/plugin.rs`

### Problem

SugarCube's built-in text formatting markup is not recognized by the parser:

| Markup | Produces | Current behavior |
|---|---|---|
| `''bold''` | `<strong>` | Plain text |
| `//italic//` | `<em>` | Consumed as comment if heuristic matches, else plain text |
| `__underline__` | `<u>` | Plain text |
| `==strike==` | `<s>` | Plain text |
| `~~sub~~` | `<sub>` | Plain text |
| `^^super^^` | `<sup>` | Plain text |

The `//italic//` case specifically conflicts with the `//` line-comment heuristic in `core.rs:64-93`. The current heuristic requires a space after `//` (or line-start position), which means `//italic//` without a space won't be consumed as a comment — but `// italic text` at line start would be, and in SugarCube that could be intended as italic formatting.

### Expected behavior

Text formatting markup should be recognized and given appropriate semantic tokens. The `//` comment heuristic should be aware of the closing `//` pattern that indicates italic markup rather than a comment.

### Fix

1. **`plugin.rs`** — Add `TextFormat` variant to `SemanticTokenType` with doc: "SugarCube text formatting markup (bold, italic, underline, etc.)".
2. **`ast.rs`** — Add `TextFormat { kind: TextFormatKind, content: String, span: Range<usize> }` variant to `AstNode`. Add `TextFormatKind` enum: `Bold`, `Italic`, `Underline`, `Strike`, `Sub`, `Super`.
3. **`core.rs`** — Add delimiter branches for each formatting pair. For `//`, modify the existing comment heuristic to check for a closing `//` on the same line before treating it as a comment. Priority: `//text//` (italic) takes precedence over `// comment` when a matching closing `//` exists.
4. **`token_builder.rs`** — Emit `TextFormat` token for the delimiter + content span.
5. **`lifecycle.rs`** / **`semantic.rs`** — Register `TextFormat` in token legend and mapping.
6. **`package.json`** — Add scope mapping for `textFormat`.
7. Add tests for each formatting type.

### Caveat

Text formatting markup is a good candidate for TextMate grammar handling instead of semantic tokens, since it's purely presentational and doesn't affect code intelligence. Recommend evaluating whether TextMate scopes are sufficient before implementing parser-level support.

---

## Issue 7 — HTML tags in passages not parsed

**Severity:** Low (data-passage extraction works; full HTML parsing is out of scope for an LSP)
**Files:** `crates/formats/src/sugarcube/parser/core.rs`, `crates/formats/src/plugin.rs`

### Problem

SugarCube allows raw HTML in passages (`<img>`, `<audio>`, `<video>`, `<html>...</html>` blocks, etc.). Currently only `<!--` comments and `data-passage` attribute extraction are handled. HTML tags get no semantic highlighting.

### Expected behavior

At minimum, `<img>`, `<audio>`, `<video>` media tags should get a semantic token so themes can distinguish them from prose. The `<html>...</html>` block should optionally be treated as a raw-body context (no SugarCube syntax parsing inside).

### Fix

1. **`plugin.rs`** — Add `HtmlTag` variant to `SemanticTokenType`.
2. **`core.rs`** — Add `<` branch (after `<!--` and `<<` are ruled out) that scans for HTML tag names. Emit an `HtmlTag` AST node for recognized media/structural tags. For `<html>`, switch to raw-body mode until `</html>`.
3. **`ast.rs`** — Add `HtmlTag { tag_name: String, span: Range<usize> }` variant. Optionally add `HtmlBlock { tag_name, content, span }` for raw-body HTML blocks.
4. **`token_builder.rs`** — Emit `HtmlTag` token.
5. Register in lifecycle/semantic/package.json.
6. Add tests.

### Caveat

Full HTML parsing is a significant scope increase. A pragmatic first step is to only recognize `<html>...</html>` as a raw-body block (same as `<<script>>`) and add `HtmlTag` tokens for the few tags that affect story navigation (like `<img data-passage="...">`). General HTML tag highlighting can be deferred to TextMate grammars.

---

## Issue 8 — `<<=>>` / `<<->>` sigils get no semantic token

**Severity:** Low (visual gap, no functional impact)
**Files:** `crates/formats/src/sugarcube/lsp/token_builder.rs`

### Problem

The `Expression` node handler in `token_builder.rs` only emits tokens for the expression content (variables, literals, operators). The `=` or `-` sigil itself is invisible in semantic highlighting. Combined with no differentiation between `ExprKind::Print` and `ExprKind::Silent`, both `<<=>>` and `<<->>` look identical in the editor.

### Fix

1. **`token_builder.rs`** — In the `AstNode::Expression` match arm, emit a `Macro` token for the sigil span (1 byte for `=` or `-`, located right after `<<`). Derive the span from the expression's existing span data.
2. **`token_builder.rs`** — Use `kind` field to differentiate: `Print` → no modifier, `Silent` → `ControlFlow` modifier.
3. Add test verifying sigil token emission.

---

## Implementation Plan

### Phase 1 — Quick fixes (Issues 1, 8)

These are one-line or few-line changes with no architectural impact.

| Step | Issue | File(s) | Estimate |
|---|---|---|---|
| 1.1 | #1 | `tree_builder.rs` — add `"done"` to exclusion list | 1 line |
| 1.2 | #1 | `core.rs` — add `prose_inside_done_is_not_prose` test | ~15 lines |
| 1.3 | #8 | `token_builder.rs` — emit `Macro` token for expression sigil | ~10 lines |
| 1.4 | #8 | `token_builder.rs` — differentiate `Print` vs `Silent` via modifier | ~3 lines |
| 1.5 | #8 | `core.rs` — add test for expression sigil token | ~20 lines |
| 1.6 | — | Run `cargo test` to verify | — |

### Phase 2 — Spec alignment (Issues 2, 3)

These fix genuine spec incoherences that produce wrong output.

| Step | Issue | File(s) | Estimate |
|---|---|---|---|
| 2.1 | #2 | `ast.rs` — add `SetOperator::Into` | 2 lines |
| 2.2 | #2 | `operators.rs` — add `into` → `=` mapping | 1 line |
| 2.3 | #2 | `macro_parser.rs` — add `into` branch in `parse_set_assignment()` | ~30 lines |
| 2.4 | #2 | `token_builder.rs` — handle `SetOperator::Into` in token emission | ~5 lines |
| 2.5 | #2 | `core.rs` — add tests for `into` assignment | ~25 lines |
| 2.6 | #3 | `ast.rs` — add `LinkKind::Image`, `image_url` field on Link | ~5 lines |
| 2.7 | #3 | `link_parser.rs` — add `img[` prefix detection before setter branch | ~30 lines |
| 2.8 | #3 | `extraction.rs` — handle image links | ~5 lines |
| 2.9 | #3 | `core.rs` — add tests for image links | ~20 lines |
| 2.10 | — | Run `cargo test` to verify | — |

### Phase 3 — New token types (Issues 4, 5)

These add new AST nodes and semantic token types.

| Step | Issue | File(s) | Estimate |
|---|---|---|---|
| 3.1 | #4 | `token_builder.rs` — normalize `<<print>>` and `<<=>>` token emission | ~15 lines |
| 3.2 | #4 | Add test for equivalence | ~20 lines |
| 3.3 | #5 | `plugin.rs` — add `InlineStyle` token type | 3 lines |
| 3.4 | #5 | `ast.rs` — add `InlineStyle` AST variant | ~10 lines |
| 3.5 | #5 | `core.rs` — add `@@` and `@` delimiter branches | ~50 lines |
| 3.6 | #5 | `token_builder.rs` — handle `InlineStyle` tokens | ~15 lines |
| 3.7 | #5 | `lifecycle.rs` / `semantic.rs` — register `InlineStyle` | 4 lines |
| 3.8 | #5 | `package.json` — add scope mapping | 2 lines |
| 3.9 | #5 | `core.rs` — add tests | ~30 lines |
| 3.10 | — | Run `cargo test` to verify | — |

### Phase 4 — Lower priority (Issues 6, 7)

These require more design work and may be partially handled by TextMate grammars.

| Step | Issue | File(s) | Estimate |
|---|---|---|---|
| 4.1 | #6 | Evaluate TextMate grammar approach for text formatting | Design |
| 4.2 | #6 | If parser-level: add `TextFormat` token type, AST variant, core.rs branches | ~80 lines |
| 4.3 | #6 | Resolve `//` comment vs `//italic//` ambiguity | ~20 lines |
| 4.4 | #7 | Add `<html>...</html>` raw-body block handling | ~30 lines |
| 4.5 | #7 | Optionally add `HtmlTag` token type for media tags | ~40 lines |

---

## Worklog

| Date | Phase | Step | Status | Notes |
|---|---|---|---|---|
| 2026-06-13 | — | Audit | ✅ Complete | Full parser audit against SugarCube 2 spec. 8 issues identified. |
| 2026-06-13 | — | Prose token | ✅ Complete | `is_prose` flag + `propagate_prose_context()` + `SemanticTokenType::Prose`. 461 tests pass. |
| 2026-06-13 | 1.1 | `<<done>>` fix | ✅ Complete | Added `"done"` to `is_prose_rendering_macro()` exclusion list in `tree_builder.rs`. |
| 2026-06-13 | 1.2 | `<<done>>` test | ✅ Complete | Added `prose_inside_done_is_not_prose` test in `core.rs`. |
| 2026-06-13 | 1.3 | Expression sigil token | ✅ Complete | `token_builder.rs` emits `Macro` token for `=`/`-` sigil in Expression nodes. |
| 2026-06-13 | 1.4 | Print vs Silent differentiation | ✅ Complete | `<<->>` sigil gets `ControlFlow` modifier; `<<=>>` gets no modifier. |
| 2026-06-13 | 1.5 | Expression sigil tests | ✅ Complete | `expression_sigil_emits_macro_token` + `silent_expression_sigil_has_control_flow_modifier`. |
| 2026-06-13 | 1.6 | Phase 1 verification | ✅ Complete | 464 tests pass (3 new). |
| 2026-06-13 | 2.1 | `SetOperator::Into` | ✅ Complete | Added `Into` variant to `SetOperator` enum in `ast.rs`. |
| 2026-06-13 | 2.2 | `into` normalization | ✅ Complete | Added `into → =` mapping in `operators.rs` + `assignment_operators()`. |
| 2026-06-13 | 2.3 | `into` parsing | ✅ Complete | `parse_set_assignment()` now handles reverse-assignment form. Added `find_into_keyword()` + `scan_set_target()` helpers in `macro_parser.rs`. |
| 2026-06-13 | 2.4 | `Into` token emission | ✅ Complete | `registry_populate.rs` maps `SetOperator::Into → VarAccessKind::Write`. JS annotation pass reuses existing `sa.expression` pipeline. |
| 2026-06-13 | 2.5 | `into` tests | ✅ Complete | `set_into_assignment`, `set_into_keyword_boundary`, `set_into_with_expression`. |
| 2026-06-13 | 2.6 | `LinkKind::Image` + `image_url` | ✅ Complete | Added `Image` variant to `LinkKind`, `image_url: Option<String>` field on `AstNode::Link` in `ast.rs`. |
| 2026-06-13 | 2.7 | Image link parsing | ✅ Complete | `link_parser.rs` detects `img[` prefix before setter `][` branch. Returns `LinkKind::Image` with `image_url` populated. |
| 2026-06-13 | 2.8 | Image link extraction | ✅ Complete | `extraction.rs` uses `..` pattern — already handles new fields. |
| 2026-06-13 | 2.9 | Image link tests | ✅ Complete | `image_link_simple`, `image_link_with_display`. |
| 2026-06-13 | 2.10 | Phase 2 verification | ✅ Complete | 469 tests pass (5 new since Phase 1). |
| 2026-06-13 | 3.1 | `<<print>>`/`<<=>>` normalization | ✅ Complete | Both emit `Macro` token for name/sigil + `Variable` tokens for content. Issue 8 sigil token already unifies them. |
| 2026-06-13 | 3.2 | Equivalence test | ✅ Complete | `print_and_expression_emit_equivalent_variable_tokens` verifies same Variable token count. |
| 2026-06-13 | 3.3 | `InlineStyle` token type | ✅ Complete | Added `SemanticTokenType::InlineStyle` in `plugin.rs`. |
| 2026-06-13 | 3.4 | `InlineStyle` AST variant | ✅ Complete | Added `InlineStyle { class, class_span, children, span }` to `AstNode` in `ast.rs`. |
| 2026-06-13 | 3.5 | `@@` / `@` parsing | ✅ Complete | `core.rs` adds `b'@'` branch for both `@@class;text@@` and `@class;text@`. Supports `.`/`#`-prefixed classes. Added `parse_inline_style()` + `find_class_and_body_start()`. |
| 2026-06-13 | 3.6 | `InlineStyle` token emission | ✅ Complete | `token_builder.rs` emits `InlineStyle` token for class span, recurses into children. |
| 2026-06-13 | 3.7 | `InlineStyle` LSP registration | ✅ Complete | `lifecycle.rs` (index 19), `semantic.rs` (`ST_INLINE_STYLE`), mapping added. |
| 2026-06-13 | 3.8 | `InlineStyle` scope mapping | ✅ Complete | `package.json` — 5 language IDs updated with `"inlineStyle": ["entity.other.attribute-name.class.twee"]`. |
| 2026-06-13 | 3.9 | Inline style tests | ✅ Complete | `inline_style_double_at`, `inline_style_single_at`, `inline_style_with_variable`, `inline_style_emits_token`. |
| 2026-06-13 | 3.10 | Phase 3 verification | ✅ Complete | 474 tests pass (5 new since Phase 2). Also added `InlineStyle` to `passage_build.rs`. |
| 2026-06-13 | 4.1 | TextMate evaluation | ✅ Complete | Decided to implement parser-level support since `//italic//` ambiguity requires parser awareness anyway. |
| 2026-06-13 | 4.2 | `TextFormat` token type + AST | ✅ Complete | Added `SemanticTokenType::TextFormat` in `plugin.rs`, `TextFormatKind` enum + `TextFormat { kind, content, span }` AST variant in `ast.rs`. |
| 2026-06-13 | 4.3 | `//` comment vs italic fix | ✅ Complete | `core.rs` now checks for closing `//` on the same line before treating `//` as a comment. If found, it's italic formatting. All 6 format types parsed: `''`, `//`, `__`, `==`, `~~`, `^^`. |
| 2026-06-13 | 4.4 | HTML `<html>` block | ⏳ Deferred | Not implemented — requires significant scope increase. Recommend TextMate grammar or future PR. |
| 2026-06-13 | 4.5 | `HtmlTag` token type | ⏳ Deferred | Not implemented — deferred per plan. |
| 2026-06-13 | — | `TextFormat` LSP registration | ✅ Complete | `lifecycle.rs` (index 20), `semantic.rs` (`ST_TEXT_FORMAT`), mapping, scope mapping in `package.json`. |
| 2026-06-13 | — | `TextFormat` tests | ✅ Complete | `text_format_bold`, `text_format_italic`, `text_format_strike`, `text_format_emits_token`. |
| 2026-06-13 | — | **Final verification** | ✅ Complete | **478 tests pass** (17 new since start). All phases complete except Issue 7 (HTML tags) — deferred. |
