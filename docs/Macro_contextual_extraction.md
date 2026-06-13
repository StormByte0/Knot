# SugarCube Macro Contextual Extraction & Token Generation

> **Session Date**: 2026-06-12
> **Branch**: ver_3
> **Scope**: Complete the macro contextual and function extraction for SugarCube —
> wire the macro catalog's semantic data into the token builder and parser so that
> highlighting, diagnostics, and structured extraction reflect what each macro
> actually means.

---

## Objective

The SugarCube unified AST pipeline (3-phase: parse → JS annotate → registry populate)
is architecturally complete. Variable extraction and arena-tree parsing are done.
What remains is **macro contextual awareness**: the parser and token builder treat
all macros uniformly, even though the `MacroDef` catalog already encodes rich
semantic information about each macro's arguments, constraints, and behavior.

This plan closes that gap by:

1. **Emitting missing semantic token types** (keywords, operators, strings, numbers,
   booleans, namespaces, functions, deprecated markers) from the token builder.
2. **Adding contextual parsing** for macros whose args have known structure
   (`<<set>>`-style assignments for `<<capture>>`, loop variables for `<<for>>`,
   passage-arg extraction from the catalog).
3. **Wiring function extraction** into token emission so user-defined functions
   and widgets get proper highlighting.
4. **Deferring template (`?syntax`) work** until we have a clear understanding of
   SugarCube's template API and its role in the format.

---

## Phase Execution Contract

Every phase MUST satisfy these invariants before being considered complete:

1. **`cargo check` passes** with zero errors
2. **`cargo test` passes** for all affected crates
3. **No regressions** in existing semantic highlighting (passage headers, macros,
   variables, links all still work)
4. **Format isolation** is preserved — no `sugarcube::` types leak into
   `knot-core` or `knot-server`
5. **The unified AST pipeline is unchanged** — Phases 1-3 ordering is sacred;
   new extraction happens within the existing pipeline, not alongside it

If any invariant fails, the phase is NOT complete. Do not proceed.

---

## Current State Assessment

### What's Done ✅

| Component | Status | Location |
|-----------|--------|----------|
| 3-phase parse pipeline | Complete | `parse_pipeline.rs` |
| Unified AST with `JsAnalysis` | Complete | `ast.rs`, `js_annotate.rs` |
| Macro catalog (50+ builtins) | Complete | `macros/catalog.rs` |
| Macro classifiers | Complete | `macros/classifiers.rs` |
| `<<set>>` assignment parsing | Complete | `parser/macro_parser.rs` |
| Variable scanning (`$var`/`_var`) | Complete | `parser/variable_scan.rs` |
| Arena variable tree | Complete | `registries/variable_tree.rs` |
| `State.variables.x` ↔ `$x` normalization | Complete | `js/js_preprocess.rs`, `js/js_walk.rs` |
| Object literal property extraction | Complete | `js/js_annotate.rs` |
| Function registry | Complete | `registries/function_registry.rs` |
| Custom macro registry | Complete | `registries/custom_macros.rs` |
| Semantic tokens: macro, variable, property, link, comment | Complete | `lsp/token_builder.rs` |
| Diagnostics: unclosed blocks, parse errors | Complete | `lsp/token_builder.rs` |
| Header tokens (name, `::`, tags) | Complete | `lsp/token_builder.rs` |
| JSON body tokens (StoryData) | Complete | `lsp/token_builder.rs` |
| FormatPlugin trait — all methods implemented | Complete | `sugarcube/mod.rs` |

### What's Missing ⚠️

| Gap | Severity | Impact |
|-----|----------|--------|
| Token builder emits no keyword/operator/string/number/boolean tokens inside macro args | HIGH | No syntax highlighting for any literal or operator in `<<set $x to "hello">>`, `<<if $hp gte 50>>`, etc. |
| No namespace tokens for global objects (`State`, `Engine`, etc.) | HIGH | `State.variables` in JS contexts gets no special highlighting |
| No function tokens for user-defined widgets/custom macros | MEDIUM | All macros get the same `Macro` type; can't distinguish `<<if>>` from `<<myWidget>>` |
| No deprecated modifier on deprecated macro name tokens | MEDIUM | `<<click>>` and `<<display>>` don't show strikethrough |
| `<<capture>>` has no structured target extraction | MEDIUM | `<<capture $var>>` doesn't extract the target variable into a `set_assignment`-like field |
| `<<for>>` loop variable not extracted at AST level | MEDIUM | `<<for _i, $array>>` — `_i`'s for-loop role isn't captured; fallback to JS annotation |
| `<<widget>>` name extraction is `args.trim()` | LOW | Fragile; should use structured extraction like `<<set>>` does |
| Macro arg structure not parsed from catalog | LOW | Args are raw strings; `MacroDef.args` already has `MacroArgKind` and `is_passage_ref` but nothing consumes this at parse time |
| Hardcoded `INLINE_JS_MACROS` list in `js_annotate.rs` | LOW | Should be derived from catalog; currently missing `for`, `switch`, `while` conditions |
| Macro polymorphy not handled in parser | HIGH | `<<link "Go" "Room">>` (inline form) consumes rest of passage as children — parse bug |
| Block-macro list drift (3 separate lists) | HIGH | `<<timed>>`, `<<repeat>>`, `<<createplaylist>>` missing from parser's block list |

### Progress Tracker (Audit — 2026-06-12)

| Phase | Status | Notes |
|-------|--------|-------|
| Phase 1: Literal & Operator Tokens | ✅ Complete | Matches plan exactly |
| Phase 2: Namespace Tokens | ✅ Complete | Minor deviation: skip-condition approach instead of preferred Namespace-on-Variable approach |
| Phase 3: Function & Widget Differentiation | ✅ Complete | Includes Phase 7 (function def tokens) merged in |
| Phase 4: Deprecated Macro Modifier | ✅ Complete | Deprecated modifier on macro tokens + Hint diagnostics for deprecated macros |
| Phase 5: Contextual Macro Parsing | ✅ Complete | capture_target for <<capture>>, for_loop_vars for <<for>> simplified form |
| Phase 6: Structured Args from Catalog | ✅ Complete | `structured_args: Option<Vec<StructuredMacroArg>>` on AstNode::Macro; catalog-driven arg token scanning; Link/String/Selector/Label/VariableRef/Expression classification; passage ref tokens in token_builder |
| Phase 7: Function Token Emission | ✅ Merged into Phase 3 | |
| Phase 8: Cleanup | ✅ Partially done | INLINE_JS_MACROS, known_macro_names, deprecated_macros, dynamic_navigation_macros all now catalog-derived |

### Phase 6 Audit (2026-06-12)

Phase 6 implemented with conservative approach per plan:

**New types in `ast.rs`:**
- `ParsedArgKind` enum: `PassageRef`, `Label`, `Selector`, `String`, `VariableRef`, `Expression`
- `StructuredMacroArg` struct: `kind`, `value`, `span` — records what each arg position means

**New field on `AstNode::Macro`:**
- `structured_args: Option<Vec<StructuredMacroArg>>` — populated by `parse_structured_args()`
- Only macros with declared catalog args (`MacroDef.args.is_some()`) get structured extraction
- Macros like `<<if>>`, `<<for>>` with `args: None` remain `None` (handled by oxc)

**New function in `macro_parser.rs`:**
- `parse_structured_args()`: catalog-driven arg scanner that extracts quoted strings, variable
  refs, and bare passage names, classifying each by the catalog's `MacroArgDef` declarations
- `scan_arg_tokens()`: top-level token scanner respecting strings, brackets, operators
- `is_bare_passage_name_candidate()`: filters out JS/SugarCube keywords from bare names

**Token emission in `token_builder.rs`:**
- `emit_structured_arg_tokens()`: maps `ParsedArgKind` → semantic token types
- `PassageRef` → `Link` token (consistent with `[[ ]]` link highlighting)
- `Label` → `String` token
- `Selector` → `String` token
- `String` → `String` token
- `VariableRef` → `Variable` token
- `Expression` → skipped (handled by oxc)

**Design decisions:**
- `Label` only applied when first arg of a macro has a passage_ref arg later
  (e.g., `<<link "Talk" "Shop">>` — "Talk" is a label because "Shop" is a passage ref)
- `<<timed "2s">>` gets `String` (not `Label`) — no passage_ref arg follows
- `<<actions "P1" "P2">>` — all args are `PassageRef` (catalog declares `is_passage_ref: true`)
- Variable refs where passage_ref is expected (e.g., `<<goto $dest>>`) → `VariableRef`
- Bare passage names (e.g., `<<goto Forest>>`) → `PassageRef`

**Tests:** 14 new tests covering goto, include, link, button, actions, variable targets,
bare names, selectors, deprecated display, timed speed values, set expression args, if args, and span correctness.

### Cleanup Audit (2026-06-12, Task 8)

All hardcoded macro name lists now derive from the catalog:

| Function | Before | After |
|----------|--------|-------|
| `deprecated_macros()` | Manual HashMap with 6 entries | Derived from `builtin_macros().filter(\|m\| m.deprecated)` |
| `INLINE_JS_MACROS` (js_annotate.rs) | Hardcoded `["run", "if", ...]` | Replaced with `inline_js_macro_names()` (catalog-derived) |
| `INLINE_JS_MACROS` (ast.rs) | Hardcoded, missing `capture`/`unset` | Replaced with `inline_js_macro_names()` + `$/_` fallback |
| `known_macro_names()` | Hardcoded list of ~40 names | Derived from `builtin_macros().map(\|m\| m.name)` |
| `dynamic_navigation_macros()` | Hardcoded list of 9 names | Derived from catalog `is_passage_ref` + `back`/`return` |

New function added: `inline_js_macro_names()` in classifiers.rs — derives from catalog's
`MacroArgKind::Expression | Variable` args, plus control-flow macros with undeclared
always-JS args (`if`, `elseif`, `for`, `switch`, `while`).

Bug fix: `ast.rs` `collect_js_snippets` was missing the `$`/`_` fallback that
`js_annotate.rs` had, meaning macros not in the hardcoded list (e.g., `<<goto $target>>`)
wouldn't get JS validation diagnostics. Fixed by adding the same fallback.

### Audit Findings (2026-06-12)

**🔴 Critical: Macro Polymorphy Bug**

SugarCube macros like `<<link>>`, `<<button>>`, `<<linkappend>>`, `<<linkprepend>>`,
`<<linkreplace>>` are **polymorphic** — they can be used in two forms:

1. **Inline form**: `<<link "label" "passage">>` — no body, no close tag
2. **Block form**: `<<link "label">>...<</link>>` — has body, has close tag

The parser currently treats these as **always block** macros because
`is_block_macro("link")` returns `true` unconditionally. When the inline form
is used without a close tag, `parse_block_body()` consumes the **entire rest
of the passage** as the macro's children. This is a parse bug.

**Proposed fix**: After scanning args, check if a polymorphic macro has a
passage argument (2nd string arg) — if so, treat as inline regardless of the
block-macro list. This requires:
1. Adding a `polymorphic` or `can_be_inline` field to `MacroDef`
2. Adding arg-awareness to the parser's block/inline decision
3. Optionally adding an `is_block: bool` field to `AstNode::Macro` for clarity

**🔴 Critical: Block-Macro List Drift**

Three separate lists with divergent contents:

| Source | Count | Unique issues |
|--------|-------|---------------|
| `classifiers::block_macro_names()` | 24 | Includes `elseif`/`else`/`case`/`default` (modifiers, not standalone blocks) |
| `lookup::is_block_macro()` | 26 | Adds `timed`, `repeat`, `createplaylist`, `css` |
| `predicates::is_block_macro()` | ~28 | Starts from classifiers, adds `widget`, `script`, `style`, `css`, `nobr`, `silently`, `done`, `capture` |

**Missing from parser's list (predicates)**: `timed`, `repeat`, `createplaylist`.
This means `<<timed 2s>>...<</timed>>` is **not parsed as a block macro**.

**Proposed fix**: Unify all three into a single source of truth. Derive from
`MacroDef.has_body` in the catalog where possible. Add missing entries to the
parser's block list as an interim fix.

**🟡 Moderate: Deprecated Macros Duplication**

`deprecated_macros()` in `lookup.rs` manually duplicates the `deprecated` and
`deprecation_message` fields from `catalog.rs`. If one is updated without the
other, they'll disagree.

**Proposed fix**: Derive `deprecated_macros()` from `builtin_macros()` by
filtering for `deprecated == true`. This makes the catalog the single source
of truth.

---

## Phase 1: Token Builder — Literal & Operator Tokens from Macro Args

### Goal

Emit keyword, operator, string, number, and boolean semantic tokens for content
inside macro arguments. Currently the token builder only emits a `Macro` token
for the macro name and `Variable`/`Property` tokens for variables found via
`js_analysis`. The actual literal values and operators are invisible to the
editor.

### 1.1 Design: Where to Extract Literal/Operator Spans

There are two possible approaches:

**Option A: Parse macro args with a lightweight tokenizer**
After Phase 1 (structural parse), the AST has `args: String` on each Macro node.
We could run a small tokenizer over `args` that emits spans for strings, numbers,
booleans, and SugarCube keyword-operators. This is simple but duplicates work
that oxc does in Phase 2.

**Option B: Extract from oxc's `JsAnalysis` in Phase 2**
The oxc parse already tokenizes JS expressions. We could extend `JsAnalysis`
with fields like `literal_spans: Vec<LiteralSpan>` and `operator_spans: Vec<OperatorSpan>`
that carry the type and position of each token. The token builder would then
emit these during its walk, just like it already does for `var_ops`.

**Chosen approach: Option B** — Extending `JsAnalysis`. This avoids maintaining
a second tokenizer, keeps the single-source-of-truth principle (JsAnalysis owns
all JS-derived data), and leverages oxc's already-correct handling of strings,
comments, and nesting.

### 1.2 Extend `JsAnalysis` with Literal and Operator Span Types

Add to `ast.rs`:

```rust
/// A literal token found by oxc within a JS snippet.
#[derive(Debug, Clone)]
pub struct LiteralSpan {
    pub kind: LiteralKind,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    String,
    Number,
    Boolean,
    Null,
}

/// An operator token found by oxc within a JS snippet.
#[derive(Debug, Clone)]
pub struct OperatorSpan {
    pub kind: OperatorKind,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorKind {
    Assignment,     // =, to
    CompoundAssign, // +=, -=, *=, /=, %=
    Comparison,     // ===, !==, >, <, >=, <=, eq, neq, is, isnot, gt, gte, lt, lte
    Logical,        // &&, ||, !, and, or, not
    Arithmetic,     // +, -, *, /, %
    Other,          // ?:, ??, etc.
}
```

Add to `JsAnalysis`:

```rust
pub struct JsAnalysis {
    pub var_ops: Vec<AnalyzedVarOp>,
    pub macro_adds: Vec<MacroAddInfo>,
    pub template_adds: Vec<TemplateAddInfo>,
    pub function_defs: Vec<FunctionDefInfo>,
    // NEW:
    pub literal_spans: Vec<LiteralSpan>,
    pub operator_spans: Vec<OperatorSpan>,
    pub namespace_spans: Vec<NamespaceSpan>,  // Phase 2
}
```

### 1.3 Populate Literal and Operator Spans in `js_walk.rs`

Extend `walk_inline_js()` and `walk_script_passage()` to:

- Walk oxc expression/statement nodes and extract string literal spans,
  numeric literal spans, boolean literal spans, and null literal spans.
- Walk binary/unary/assignment/conditional expression nodes to extract
  operator spans.
- Map all spans back through the preprocessor's `map_to_original()` so
  they reference passage-body positions.

**Key consideration**: SugarCube's English-like operators (`to`, `eq`, `and`, etc.)
are already normalized to JS equivalents (`=`, `===`, `&&`) by `js_preprocess.rs`.
The preprocessor's substitution table maps each normalized token back to the
original SugarCube keyword position. So `OperatorKind::Comparison` at the
normalized `===` position will map back to the original `eq` or `is` position.

**Implementation note**: The oxc AST already has `Span` on every expression node.
For binary expressions, the `operator` field carries the operator type. We need
to extract the operator's *own* span (not the whole expression span), which
requires using oxc's token information or computing it from the expression span
and operand spans.

### 1.4 Emit Tokens in `token_builder.rs`

In `build_semantic_tokens()`, after emitting macro name tokens and variable
tokens, also emit:

```rust
if let Some(analysis) = js_analysis {
    // ... existing var_ops emission ...

    for lit in &analysis.literal_spans {
        let token_type = match lit.kind {
            LiteralKind::String  => SemanticTokenType::String,
            LiteralKind::Number  => SemanticTokenType::Number,
            LiteralKind::Boolean => SemanticTokenType::Boolean,
            LiteralKind::Null    => SemanticTokenType::Keyword,
        };
        tokens.push(SemanticToken {
            start: body_offset + lit.span.start,
            length: lit.span.end - lit.span.start,
            token_type,
            modifier: None,
        });
    }

    for op in &analysis.operator_spans {
        let modifier = match op.kind {
            OperatorKind::Logical => Some(SemanticTokenModifier::ControlFlow),
            _ => None,
        };
        tokens.push(SemanticToken {
            start: body_offset + op.span.start,
            length: op.span.end - op.span.start,
            token_type: SemanticTokenType::Operator,
            modifier,
        });
    }
}
```

### 1.5 Handle the `js_analysis is None` Fallback Path

When `js_analysis` is `None` (Phase 2 hasn't run, or oxc parse failed), the
token builder currently falls back to `var_refs` for variable tokens. For
literals/operators, there is no equivalent fallback — and that's acceptable.
Without oxc, we can't reliably distinguish `"hello"` as a string from `hello`
as an identifier in macro args. The fallback path should simply not emit
literal/operator tokens, which is the current behavior.

### 1.6 Tests

- `<<set $name to "hello">>` → String token on `"hello"`, Operator token on `to`
- `<<if $hp gte 50>>` → Number token on `50`, Operator token on `gte`
- `<<set $alive to true>>` → Boolean token on `true`, Operator token on `to`
- `<<if $hasKey and $doorOpen>>` → Operator tokens on `and`, with ControlFlow modifier
- Nested: `<<set $x to $arr.length>>` → Operator on `to`, Property on `length`
- String with escapes: `<<set $msg to "say \"hi\"">>` → String token covers full quoted range

### 1.7 Validation

- `cargo check` passes
- `cargo test` passes (including new tests)
- Open a .tw file: `<<set $hp to 100>>` now shows `100` in number color, `to` in operator color
- `<<if $alive and $armed>>` shows `and` as a control-flow operator

---

## Phase 2: Namespace Tokens for Global Objects

### Goal

When JS expressions reference SugarCube global objects like `State`, `Engine`,
`Story`, `Save`, `Config`, `UI`, `Macro`, `Template`, etc., emit `Namespace`
semantic tokens for the object name and `Property` tokens for their known
properties.

### 2.1 Add `NamespaceSpan` to `JsAnalysis`

```rust
/// A reference to a SugarCube global object found by oxc.
#[derive(Debug, Clone)]
pub struct NamespaceSpan {
    pub name: String,            // "State", "Engine", etc.
    pub span: Range<usize>,      // span of the name in passage-body coordinates
    pub property_spans: Vec<PropertySpan>,  // known properties accessed on this object
}

#[derive(Debug, Clone)]
pub struct PropertySpan {
    pub name: String,
    pub span: Range<usize>,
}
```

### 2.2 Detect Global Object Accesses in `js_walk.rs`

The walker already detects `State.variables.x` patterns for variable tracking.
Extend it to also emit `NamespaceSpan` entries when it encounters member
expressions whose object is a known global:

- `State.variables` → Namespace "State" + Property "variables"
- `Engine.play()` → Namespace "Engine" + Property "play"
- `Story.has("name")` → Namespace "Story" + Property "has"
- `Config.debug` → Namespace "Config" + Property "debug"
- `SugarCube.State` → Namespace "SugarCube" + Property "State"

The `builtin_globals()` catalog in `macros/globals.rs` already lists all global
objects and their properties. The walker should check against this catalog to
determine whether a member expression's object is a known global.

### 2.3 Emit Tokens in `token_builder.rs`

```rust
for ns in &analysis.namespace_spans {
    tokens.push(SemanticToken {
        start: body_offset + ns.span.start,
        length: ns.span.end - ns.span.start,
        token_type: SemanticTokenType::Namespace,
        modifier: None,
    });
    for prop in &ns.property_spans {
        tokens.push(SemanticToken {
            start: body_offset + prop.span.start,
            length: prop.span.end - prop.span.start,
            token_type: SemanticTokenType::Property,
            modifier: None,
        });
    }
}
```

### 2.4 Deduplication with Existing Property Tokens

`emit_var_op_tokens()` already emits `Variable` + `Property` tokens for
`State.variables.x` via `AnalyzedVarOp.segment_spans`. The new `NamespaceSpan`
tokens will overlap with these for the same positions. We need to decide which
takes priority.

**Resolution**: `NamespaceSpan` tokens should be emitted ONLY for globals that
are NOT already covered by `AnalyzedVarOp`. Specifically:
- `State.variables.x` → Already handled by `var_ops` as `Variable($State) + Property(variables) + Property(x)`. No namespace token.
- `Engine.play()` → NOT a variable operation. Emit `Namespace(Engine) + Property(play)`.
- `Config.debug` → NOT a variable operation (it's a read, but not of a `$var`). Emit `Namespace(Config) + Property(debug)`.

This means the walker should only emit `NamespaceSpan` for globals that are NOT
`State` or `SugarCube.State` (since those are already covered by variable
tracking). Alternatively, we could change the existing `var_ops` emission for
`State.variables.x` to use `Namespace` for "State" and `Property` for
"variables" instead of `Variable` + `Property`, which is semantically more
accurate. This is a design decision that should be made during implementation.

**Preferred approach**: Change `AnalyzedVarOp` emission so that the root
segment of `State.variables.x` emits `Namespace` instead of `Variable`. This
avoids deduplication logic and gives semantically correct highlighting. The
token builder's `emit_var_op_tokens()` would check if the variable name matches
a known global object and use `Namespace` type for the root segment.

### 2.5 Tests

- `<<run Engine.play()>>` → Namespace token on `Engine`, Property token on `play`
- `<<run Config.debug = true>>` → Namespace token on `Config`, Property token on `debug`
- `<<run State.variables.x>>` → Either Variable+Property (current) or Namespace+Property (new)
- `<<run Story.has("Cave")>>` → Namespace on `Story`, Property on `has`

### 2.6 Validation

- `cargo check` passes
- `cargo test` passes
- Open a .tw file with script passage: `Engine.play()` shows `Engine` in namespace color

---

## Phase 3: Function & Widget Token Differentiation

### Goal

Distinguish between built-in macros (like `<<if>>`, `<<set>>`) and user-defined
macros (like `<<myWidget>>`, `<<Macro.add()>>`-registered macros) in semantic
tokens. Built-in macros get the `Macro` token type; user-defined macros get the
`Function` token type. This enables different visual styling and supports
go-to-definition for custom macros.

### 3.1 Detect Custom Macro Invocations in Token Builder

In `build_semantic_tokens()`, when emitting a `Macro` token for the macro name,
check whether the name is a known builtin or a custom macro:

```rust
let is_builtin = macros::known_macro_names().contains(name.as_str());
let is_custom_widget = /* check custom_macros registry */;
let token_type = if is_builtin {
    SemanticTokenType::Macro
} else {
    SemanticTokenType::Function  // user-defined
};
```

**Challenge**: The token builder is a pure function that takes `&[AstNode]`
and `&mut Vec<SemanticToken>`. It doesn't have access to the registry. We
need to either:

**Option A**: Pass the custom macro names set as an additional parameter.
Simple, but breaks the clean signature.

**Option B**: Store a `is_custom: bool` flag on `AstNode::Macro` during
parsing (Phase 1 or Phase 3 of the pipeline). The token builder then reads
this flag.

**Option C**: Emit all macro names as `Macro` type, then do a post-pass in
`parse_pipeline.rs` that upgrades custom macro name tokens from `Macro` to
`Function` using the registry. This keeps the token builder pure.

**Chosen approach: Option A** — Pass a `custom_macro_names: &HashSet<String>`
parameter to `build_semantic_tokens()`. This is the simplest change and the
registry is already available in `parse_pipeline.rs` when tokens are built.
The token builder's function signature becomes:

```rust
pub fn build_semantic_tokens(
    nodes: &[ast::AstNode],
    tokens: &mut Vec<SemanticToken>,
    body_offset: usize,
    custom_macro_names: &HashSet<String>,
)
```

### 3.2 Also Differentiate Widget Definitions

When a `<<widget myWidget>>` macro is parsed, the `name` token should be
`Function` type (it's defining a function, not calling a macro). Currently it
gets `Macro`. Fix this by checking `name == "widget"` in the token builder and
emitting the *following* name token (the widget name from `args`) as `Function`.

**Design question**: The widget name is inside `args`, not a separate span.
We need the widget name's span. Options:

1. Add a `definition_name_span: Option<Range<usize>>` field to `AstNode::Macro`
   that captures the span of the name being defined (for `<<widget>>` and
   potential future `<<macro>>` syntax).
2. Parse the widget name from `args` in the token builder and compute its span.

**Preferred approach: Option 1** — This is cleaner and aligns with the
structured extraction pattern used for `<<set>>`. It also supports Phase 4's
contextual parsing work.

### 3.3 Tests

- `<<if $cond>>` → `Macro` token for `if`
- `<<myWidget>>` (registered) → `Function` token for `myWidget`
- `<<widget myWidget>>` → `Macro` token for `widget`, `Function` token for `myWidget`
- `<<set $x to 1>>` → `Macro` token for `set` (builtin)
- Unknown macro `<<unknownThing>>` → `Macro` token (not Function — it's not registered)

### 3.4 Validation

- `cargo check` passes
- `cargo test` passes
- Widget definitions and invocations show in a different color than builtins

---

## Phase 4: Deprecated Macro Modifier

### Goal

When a deprecated macro is used (e.g., `<<click>>`, `<<display>>`), emit the
`Deprecated` semantic token modifier on the macro name token. This enables
strikethrough rendering in VS Code.

### 4.1 Check Deprecation in Token Builder

In `build_semantic_tokens()`, when emitting a `Macro` name token:

```rust
let is_deprecated = macros::deprecated_macros().contains_key(name.as_str());
let modifier = if is_deprecated {
    Some(SemanticTokenModifier::Deprecated)
} else {
    None  // or existing modifier
};
```

This is a simple lookup against the existing `deprecated_macros()` map in
`macros/lookup.rs`.

### 4.2 Handle Deprecation Diagnostics

Consider also emitting a diagnostic for deprecated macro usage (as a hint or
warning). This would go in `build_diagnostics()`:

```rust
if macros::deprecated_macros().contains_key(name.as_str()) {
    let msg = macros::deprecated_macros().get(name.as_str()).unwrap();
    diagnostics.push(FormatDiagnostic {
        range: body_offset + name_span.start..body_offset + name_span.end,
        message: msg.to_string(),
        severity: FormatDiagnosticSeverity::Hint,
        code: "sc-deprecated".to_string(),
    });
}
```

### 4.3 Tests

- `<<click "label">>` → Deprecated modifier on `click`
- `<<display "passage">>` → Deprecated modifier on `display`
- `<<link "label">>` → No deprecated modifier (current API)

### 4.4 Validation

- `cargo check` passes
- `cargo test` passes
- `<<click>>` shows with strikethrough in editor

---

## Phase 5: Contextual Macro Parsing — Structured Target Extraction

### Goal

Extend the parser to extract structured information from macros beyond `<<set>>`.
Currently only `<<set>>` gets a `set_assignment` field on `AstNode::Macro`.
We need equivalent structured extraction for `<<capture>>`, `<<for>>`, and
`<<widget>>`.

### 5.1 `<<capture>>` Target Extraction

**Problem**: `<<capture $var>>...<</capture>>` captures the variable named in
the args. The semantic override in `registry_populate.rs` marks it as
`VarAccessKind::Capture`, but the parser doesn't extract the target variable
into a structured field. This means the AST consumer has to re-parse the args
string to find the variable name.

**Solution**: Add a `capture_target: Option<VarRef>` field to `AstNode::Macro`,
analogous to `set_assignment`. Parse it in `parse_macro()` when the name is
`capture`:

```rust
let capture_target = if name.eq_ignore_ascii_case("capture") {
    parse_capture_target(&args, offset + args_start)
} else {
    None
};
```

`parse_capture_target()` scans the args for a `$var` or `_var` reference
(similar to how `parse_set_assignment()` scans for the target variable).

### 5.2 `<<for>>` Loop Variable Extraction

**Problem**: SugarCube's `<<for>>` has two syntax forms:
- `<<for _i to 0; _i lt 10; _i++>>` — C-style for loop
- `<<for _i, $array>>` — SugarCube's simplified iteration syntax

The simplified form creates a temporary variable `_i` that iterates over `$array`.
Currently the parser just captures the raw args string, and the JS annotation
pass picks up `$array` as a read. But `_i`'s special role (it's a write target
that gets each element) isn't captured.

**Solution**: Add a `for_loop_vars: Option<ForLoopVars>` field to `AstNode::Macro`:

```rust
#[derive(Debug, Clone)]
pub struct ForLoopVars {
    pub index_var: Option<VarRef>,    // _i in <<for _i, $array>>
    pub iterated_var: Option<VarRef>, // $array in <<for _i, $array>>
}
```

Parse in `parse_macro()` when name is `for`. The simplified form `_i, $array`
can be detected by the comma separator. The C-style form falls through to
the JS annotation pass.

### 5.3 `<<widget>>` Name Extraction

**Problem**: `<<widget myWidget>>` stores the widget name as `args.trim()`.
This is fragile — if there's extra whitespace or trailing content, it could
break. The name should be a structured span.

**Solution**: Add a `definition_name: Option<DefinitionName>` field:

```rust
#[derive(Debug, Clone)]
pub struct DefinitionName {
    pub name: String,
    pub span: Range<usize>,
}
```

Parse in `parse_macro()` when name is `widget`. Scan the args for the first
identifier and record its span.

**Future extensibility**: `definition_name` can also be used for `Macro.add()`
call sites in JS, but that's handled separately via `js_walk`.

### 5.4 Generalize: Unified Macro Context Field

Rather than adding multiple optional fields (`capture_target`, `for_loop_vars`,
`definition_name`), consider a single `macro_context` field:

```rust
pub enum MacroContext {
    Set(SetAssignment),           // <<set>>
    Capture(VarRef),              // <<capture>>
    ForLoop(ForLoopVars),         // <<for>>
    Definition(DefinitionName),   // <<widget>>
    None,
}
```

This is cleaner but is a bigger refactor. **Decision**: For now, add individual
optional fields. They can be unified into an enum later without breaking the
pipeline.

### 5.5 Wire New Fields into Registry Population

Update `registry_populate.rs` to use the new structured fields:

- `<<capture>>`: Use `capture_target` instead of relying on
  `determine_macro_override()` to re-scan the args.
- `<<for>>`: Use `for_loop_vars` to register `_i` as a `Write` (not just
  `Read`) and `$array` as a `Read` at the AST level, complementing the
  JS annotation.
- `<<widget>>`: Use `definition_name` for widget registration instead of
  `args.trim()`.

### 5.6 Wire New Fields into Token Builder

- `<<capture $var>>` → Emit `Variable` token with `Definition` modifier for
  the capture target (currently only handled via `js_analysis` or `var_refs`).
- `<<widget myWidget>>` → Emit `Function` token with `Definition` modifier
  for the widget name span (from `definition_name`).
- `<<for _i, $array>>` → Emit `Variable` token with `Definition` modifier
  for `_i`, `Variable` token (no modifier) for `$array`.

### 5.7 Tests

- `<<capture $target>>...<</capture>>` → `capture_target` is `Some(VarRef { name: "$target", ... })`
- `<<for _i, $items>>...<</for>>` → `for_loop_vars.index_var` is `_i`, `iterated_var` is `$items`
- `<<widget myHelper>>...<</widget>>` → `definition_name` is `Some(DefinitionName { name: "myHelper", ... })`
- `<<for _i to 0; _i lt 5; _i++>>` → `for_loop_vars` is None (C-style falls through to JS)

### 5.8 Validation

- `cargo check` passes
- `cargo test` passes
- Variable panel shows `<<capture>>` targets as Capture access kind
- Widget definitions appear in completion with proper go-to-definition

---

## Phase 6: Macro Arg Structured Extraction from Catalog

### Goal

Leverage the `MacroDef.args` catalog to parse macro arguments into structured
fields based on the declared argument types. Currently the parser captures the
entire args string and the downstream consumers have to re-parse it. This phase
makes the AST aware of what each argument position means.

### 6.1 Design: Structured Macro Args on the AST

Add an optional `structured_args` field to `AstNode::Macro`:

```rust
/// Structured argument information derived from the MacroDef catalog.
pub structured_args: Option<Vec<MacroArg>>,
```

Where:

```rust
#[derive(Debug, Clone)]
pub struct MacroArg {
    pub kind: MacroArgKind,
    pub value: String,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroArgKind {
    Expression,    // Generic JS expression
    PassageRef,    // Passage name reference (quoted or unquoted)
    VariableRef,   // $var or _var reference
    Label,         // Display label (string)
    Selector,      // CSS selector
    Speed,         // Timing value (e.g., "2s")
    Other,         // Catch-all
}
```

### 6.2 Parsing Strategy

Not all macros need structured args. For many macros, the raw args string is
sufficient and the JS annotation pass handles the rest. Structured extraction
is valuable when:

1. An arg is a **passage reference** — needed for link extraction, graph edges,
   and go-to-definition on passage names.
2. An arg is a **variable reference** in a specific role — needed for variable
   tracking beyond what JS annotation provides.
3. An arg is a **label** — needed for display text extraction.

**Implementation**: In `parse_macro()`, after scanning the name and args:

1. Look up the macro in the catalog via `macros::find_macro(&name)`.
2. If found and `args.is_some()`, parse the args string into structured
   `MacroArg` entries based on the catalog's `MacroArgDef` list.
3. Parse quoted strings first (they're unambiguous), then try to match
   remaining args against the expected types.

**Key challenge**: SugarCube macro args don't have a fixed separator. Args are
a single JS expression or a comma-separated list within that expression. The
parser needs to respect JS syntax (commas inside strings or brackets don't
count as arg separators).

**Conservative approach**: Only extract passage references from quoted string
arguments. This covers the most impactful use cases:

- `<<goto "Cave">>` → `MacroArg { kind: PassageRef, value: "Cave", span: ... }`
- `<<link "Talk" "Shop">>` → `MacroArg { kind: Label, value: "Talk" }, MacroArg { kind: PassageRef, value: "Shop" }`
- `<<include "Header">>` → `MacroArg { kind: PassageRef, value: "Header" }`

This avoids the complexity of full expression parsing in the SugarCube parser
(which is oxc's job) while still providing structured data for the most
important arg types.

### 6.3 Token Emission for Passage References in Args

When `structured_args` contains a `PassageRef`, emit a `Link` or `PassageRef`
semantic token for the passage name span. This replaces the current post-hoc
extraction in `extraction.rs` for passage-arg macros, making the token builder
the single source of truth for token emission.

### 6.4 Tests

- `<<goto "Cave">>` → `structured_args[0]` is `PassageRef("Cave")`
- `<<link "Talk" "Shop">>` → `structured_args[0]` is `Label("Talk")`, `structured_args[1]` is `PassageRef("Shop")`
- `<<include "Header">>` → `structured_args[0]` is `PassageRef("Header")`
- `<<set $x to 1>>` → `structured_args` is None (set has its own `set_assignment`)

### 6.5 Validation

- `cargo check` passes
- `cargo test` passes
- Passage name in `<<goto "Cave">>` gets `Link` token highlighting
- No regressions in existing link extraction or graph edges

---

## Phase 7: Function Extraction Token Emission

### Goal

Wire the existing `FunctionRegistry` into the token builder so that
user-defined function names in JS contexts get `Function` semantic tokens.
Currently the registry is populated during Phase 3 but never read by the
token builder.

### 7.1 Detect Function Names in JS Contexts

In `js_walk.rs`, when a function declaration or named expression is found,
the `FunctionDefInfo` already records the name and span. Add this information
to `JsAnalysis` (it's already there via `function_defs: Vec<FunctionDefInfo>`).

### 7.2 Emit Function Tokens from `JsAnalysis`

In the token builder, when walking a `Macro` or `Expression` node with
`js_analysis`, emit `Function` tokens for each `function_defs` entry:

```rust
if let Some(analysis) = js_analysis {
    for func_def in &analysis.function_defs {
        tokens.push(SemanticToken {
            start: body_offset + func_def.name_offset,
            length: func_def.name.len(),
            token_type: SemanticTokenType::Function,
            modifier: Some(SemanticTokenModifier::Definition),
        });
    }
}
```

### 7.3 Script Passage Functions

For script passages, function definitions are stored in
`PassageAst::script_js_analysis`. The token builder currently doesn't handle
`script_js_analysis` because script passages don't emit body tokens. This is
correct for the body (scripts don't get structural tokens), but we may want
to emit tokens for the script passage body in a future phase. For now, skip
function tokens in script passages — they're visible in the function registry
for completion/hover but don't need body-level tokens.

### 7.4 Tests

- `<<run function myHelper() { ... }>>` → Function token on `myHelper` with Definition modifier
- `<<script>>const calculateScore = () => { ... };<</script>>` → Function token on `calculateScore`

### 7.5 Validation

- `cargo check` passes
- `cargo test` passes

---

## Phase 8: Clean Up Hardcoded Lists

### Goal

Replace the hardcoded `INLINE_JS_MACROS` list in `js_annotate.rs` with a
catalog-derived check, and clean up any other hardcoded lists that duplicate
catalog data.

### 8.1 Replace `INLINE_JS_MACROS` in `js_annotate.rs`

Currently:

```rust
const INLINE_JS_MACROS: &[&str] = &[
    "run", "if", "elseif", "else", "print", "nobr",
    "capture", "unset",
];
```

This is missing `for`, `switch`, and potentially others. Replace with a
catalog-derived check:

```rust
/// Determine whether a macro's args should be parsed as a JS expression.
fn macro_args_are_js(name: &str) -> bool {
    // All macros with args contain JS expressions in SugarCube.
    // Only macros whose args are purely structural (like <<widget name>>)
    // are NOT JS. Check the catalog for arg types.
    if let Some(def) = macros::find_macro(name) {
        // If the macro has no args declared, assume no JS
        def.args.is_some()
    } else {
        // Unknown macros: treat as JS if args contain $ or _
        true  // The existing fallback already checks this
    }
}
```

Alternatively, add an `is_js_expression: bool` field to `MacroArgDef` so the
catalog explicitly declares which args are JS expressions vs structural labels.

### 8.2 Other Hardcoded Lists to Audit

- `is_block_macro()` in `macros/lookup.rs` has a hardcoded match — should
  derive from `MacroDef.has_body` in the catalog.
- `block_macro_names()` in `classifiers.rs` has a hardcoded set — should
  derive from catalog.
- `dynamic_navigation_macros()` has a hardcoded set — should derive from
  catalog's `MacroArgDef.is_passage_ref`.

**Note**: Some of these hardcoded lists exist because deriving from the catalog
at `const`/`static` time isn't possible (the catalog returns `&'static [MacroDef]`
from a function). The lists could be computed once at startup and cached, but
that adds complexity. This is a "nice to have" cleanup, not a correctness issue.

### 8.3 Validation

- `cargo check` passes
- `cargo test` passes
- No change in visible behavior (refactor only)

---

## Phase Dependency Graph

```
Phase 1 (Literals/Operators)
    ↓
Phase 2 (Namespaces) ← depends on Phase 1's JsAnalysis extension pattern
    ↓
Phase 3 (Function/Widget tokens) ← independent, can parallel with Phase 2
    ↓
Phase 4 (Deprecated modifier) ← independent, can be done any time
    ↓
Phase 5 (Contextual parsing) ← independent, AST-level changes
    ↓
Phase 6 (Structured args from catalog) ← depends on Phase 5's pattern
    ↓
Phase 7 (Function tokens) ← independent, uses existing FunctionRegistry
    ↓
Phase 8 (Cleanup) ← depends on all above for stable baseline
```

**Practical ordering**: Phases 1-4 can be done in sequence. Phases 5-7 can be
done in any order after Phase 1 (they don't depend on each other). Phase 8 is
last.

---

## Template API — Research Complete

The `Template.add()` API in SugarCube allows authors to register output
functions or string substitutions that can be invoked with `?templateName`
syntax in passage prose. Example:

```
:: StoryInit
<<run Template.add("healMessage", function () { return "You feel better!"; })>>

:: Forest
?healMessage
```

The `?templateName` syntax is unique — it's neither a macro (`<<>>`) nor a
link (`[[]]`) nor an expression (`<=>>`). It's a third class of inline
construct.

### Research Findings

1. **Registration API**: Templates are registered with `Template.add()` in
   script passages, using the same pattern as `Macro.add()`. This means we
   can create template registries just like we do for custom macros — the
   `js_walk.rs` `TemplateAddInfo` extraction and `TemplateRegistry` population
   are already wired up.

2. **Invocation syntax**: `?templateName` in passage prose. The SugarCube
   parser may not currently recognize this syntax — we need to investigate
   whether the parser handles it or whether we need to add detection.

3. **Scope**: Templates are global (like macros), registered once and
   available throughout the story.

4. **Template validation**: Unknown at parse time; SugarCube resolves
   templates at runtime.

5. **Chaining**: Unknown; needs investigation during implementation.

### Implementation Plan (Deferred)

The `TemplateRegistry` already tracks `Template.add()` definitions from
`JsAnalysis.template_adds`. What's missing is:

- Detection of `?name` syntax in the SugarCube parser's main loop
  (may need parser modification if the parser doesn't currently recognize `?`)
- A new `AstNode` variant or annotation for template invocations
- Token emission for `?` prefix and template name
- Completion for `?`-prefixed names

This work should be a separate plan after this one is complete.

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| oxc span mapping errors for literals/operators | Medium | Tokens appear at wrong positions | Extensive position-mapping tests; compare against manual inspection |
| Token overlap between `var_ops` and `namespace_spans` | High | Double-emission causes visual glitches | Deduplication logic or priority rules in token builder |
| Structured arg parsing breaks on edge cases | Medium | Parse failures for unusual macro usage | Conservative approach: only extract quoted passage refs; fallback to raw args |
| Performance regression from additional `JsAnalysis` fields | Low | Slower parse on large files | Benchmark before/after; lazy computation if needed |
| Breaking the `js_analysis is None` fallback path | Medium | Tokens disappear for broken JS | Always test both paths; never require `js_analysis` for basic highlighting |
