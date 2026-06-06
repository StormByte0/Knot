# Knot ver_3 — SugarCube Rewrite Implementation Plan

## Overview

This document describes the complete implementation plan for the SugarCube format plugin rewrite on the `ver_3` branch. The rewrite replaces ~2500 lines of regex-based scanning code with a single recursive descent parser backed by an oxc JS parser, organized around a two-pass pipeline (classify → ordered parse) and format-owned registries exposed through trait methods.

### What was deleted (ver_2 → ver_3)

| Module | Lines (approx) | Reason for deletion |
|---|---|---|
| `vars/` (mod.rs, ...) | ~600 | Regex-based variable scanning — replaced by AST walk |
| `links.rs` | ~350 | Regex-based link extraction — replaced by parser |
| `validation.rs` | ~400 | Regex-based validation — replaced by AST diagnostics |
| `macro_scan.rs` | ~300 | Regex macro scanning — replaced by parser |
| `workspace/` | ~400 | Duplicate workspace module (dead code) |
| `comments.rs` | ~150 | Regex comment scanning — replaced by parser |
| `passage_tree/` | ~200 | Passage tree re-implementation — replaced by AST |
| `js_extractor.rs` | ~100 | JS snippet extraction — replaced by AST `collect_js_snippets()` |
| `blocks.rs` | ~200 | Block type detection — now inline in parser |
| `tokens.rs` | ~200 | Token scanning — replaced by semantic token builder |
| `navigation.rs` | ~200 | Navigation helpers — now served by registry trait methods |
| `tests.rs` | ~200 | Integration tests — rewritten inline |

### What was kept / ported

| Module | Notes |
|---|---|
| `lexer.rs` | Unchanged — Logos-based passage splitting, works perfectly |
| `macros.rs` | Unchanged — static macro catalog (~1200 lines of data) |
| `special_passages.rs` | Unchanged — pure data definitions |
| `variable_tree.rs` | Ported from ver_2 — side table for variable tracking |
| `custom_macros.rs` | Ported from ver_2 — widget/Macro.add registry |
| `js_preprocess.rs` | Ported from ver_2 — $var substitution for oxc |
| `js_walk.rs` | New — walks oxc AST for State.variables, Macro.add |

### What was created new

| Module | Purpose |
|---|---|
| `classifier.rs` | Tags-first classification + processing priority ordering |
| `parser.rs` | Recursive descent parser (~1000 lines) |
| `ast.rs` | AST node types + extraction helpers |
| `mod.rs` | Rewritten plugin struct + FormatPlugin impl + registry trait methods |

---

## Architecture

### Two-Parser Model

```
SugarCube source text
        |
        v
┌─────────────────────┐
│  lexer::split_passages()   │  ← Logos lexer, passage boundary detection
│  Vec<(TweeHeader, &str)>   │
└─────────┬───────────┘
          |
          v
┌─────────────────────┐
│  classifier::classify_all() │  ← Tags-first per Twee 3 spec
│  Vec<ClassifiedPassage>     │
└─────────┬───────────┘
          |
          v
┌─────────────────────┐
│  classifier::sort_for_processing() │  ← Define-before-use ordering
└─────────┬───────────┘
          |
          v
┌─────────────────────────────────────────────────────┐
│  Per-passage dispatch (by processing priority)       │
│                                                       │
│  Script  ──► oxc parse ──► js_walk ──► warm registries│
│  Widget  ──► SC parser   ──► warm widget registry     │
│  Special ──► SC parser   ──► (registries warm)        │
│  Normal  ──► SC parser   ──► (all registries avail)   │
│  Stylesheet ──► skip                                 │
│  StoryData  ──► minimal                              │
└─────────┬───────────────────────────────────────────┘
          |
          v
┌─────────────────────────────────────────────────┐
│  ParseResult { passages, tokens, diagnostics }   │
│  + Registry side effects (VariableTree, CustomMacroRegistry) │
└─────────────────────────────────────────────────┘
```

### Classification Priority (Twee 3 spec: tags override names)

1. **Core name-matched** — StoryTitle, StoryData, Start
2. **Core tag-matched** — [script], [stylesheet], [style]
3. **Format tag-matched** — [init], [widget]
4. **Format name-matched** — StoryInit, PassageHeader, PassageReady, PassageDone, etc.
5. **Regular** — user-defined passages

### Processing Order (define-before-use)

| Priority | Category | Parse Mode | Registry Effect |
|---|---|---|---|
| 10 | [script] | `Script` (oxc) | Warm variable + macro registries |
| 20 | [widget] | `Widget` (SC parser) | Warm widget registry |
| 30 | Named specials | `Normal` (SC parser) | Read from warm registries |
| 40 | Normal passages | `Normal` (SC parser) | Full registry access |
| 50 | Stylesheets/StoryData | `Stylesheet`/`Minimal` | None (skip) |

### SugarCube Delimiters

| Sequence | Token | Parser Handler |
|---|---|---|
| `<<` | Macro open | `parse_macro()` |
| `>>` | Macro close | `scan_macro_args()` (depth-tracked) |
| `[[` | Link open | `parse_link()` |
| `]]` | Link close | `parse_link()` (depth-tracked) |
| `$id` | Story variable | `scan_variable()` |
| `_id` | Temporary variable | `scan_variable()` (word boundary) |
| `/%` / `%/` | Twine block comment | `parse_block_comment()` |
| `/%%` / `%%/` | SugarCube block comment | `parse_block_comment()` |
| `<!--` / `-->` | HTML comment | `parse_html_comment()` |
| `$$` | Escaped dollar | Text (no variable) |

### Format-Owned Registries

The `SugarCubePlugin` struct owns two side tables:

1. **`VariableTree`** — tracks all `$var`/`_var` references across the workspace
2. **`CustomMacroRegistry`** — tracks `<<widget>>` and `Macro.add()` definitions

These are exposed through `FormatPlugin` trait methods so LSP handlers never import format-specific types:

```rust
// Handlers call these through FormatRegistry::get()
fn build_variable_tree(&self, ...) -> Vec<VariableTreeNode>
fn workspace_variable_names(&self) -> HashSet<String>
fn variable_properties(&self, var_name: &str) -> HashSet<String>
fn custom_macro_names(&self) -> Vec<String>
fn find_custom_macro(&self, name: &str) -> Option<(String, String, usize)>
fn is_custom_macro(&self, name: &str) -> bool
```

---

## Current State (3 commits on ver_3)

### Commit 1: `0a13d2d` — Purge sugarcube/, rewrite from scratch with classifier

- Deleted entire `sugarcube/` directory
- Deleted dead `workspace/` directory and `core.rs`
- Created fresh `sugarcube/` with:
  - `mod.rs` — stub plugin with trait methods wired to `macros::`
  - `lexer.rs` — restored from ver_2 (unchanged)
  - `special_passages.rs` — restored from ver_2 (pure data)
  - `macros.rs` — restored from ver_2 (~1200 lines)
  - `classifier.rs` — NEW two-pass detect+classify system
- Workspace compiles cleanly

### Commit 2: `0cdf1f6` — Phase 2 — SugarCube recursive descent parser + two-pass pipeline

- `parser.rs` — NEW recursive descent parser (~1000 lines):
  - Left-to-right delimiter scanning
  - Nested `<<>>` depth tracking
  - Block macro body parsing with close tag matching
  - Link content parsing (pipe, arrow, left-arrow, setter)
  - Block comment parsing (Twine, SugarCube, HTML)
  - Variable reference scanning (`$var`, `_var`, dot-notation)
  - Expression macro parsing (`<<=>>`, `<<->>`)
  - 18 inline unit tests
- `ast.rs` — NEW AST node types:
  - `AstNode` enum (Text, Macro, Expression, Link, Comment, Error)
  - `VarRef`, `LinkInfo`, `VarOpInfo` extraction types
  - `ParseMode` enum (Normal, Script, Widget, Interface, Stylesheet, Minimal)
  - `PassageAst` result type
  - Walker helpers (`collect_macros`, `collect_links`, `collect_errors`, `collect_js_snippets`)
- `mod.rs` updated with full `FormatPlugin::parse()` implementation using two-pass pipeline
- `build_body_blocks()`, `build_semantic_tokens()`, `build_diagnostics()` helpers
- `build_header_tokens()` and `self_classify_tag()` for header semantic tokens

### Commit 3: `72d092f` — Phase C — registries, js_preprocess, js_walk, and registry trait methods

- `variable_tree.rs` — ported from ver_2 with side table design
- `custom_macros.rs` — ported from ver_2 with widget + Macro.add support
- `js_preprocess.rs` — ported from ver_2 ($var → State_variables_X, to → =)
- `js_walk.rs` — NEW oxc AST walker:
  - `walk_script_passage()` for [script] passages
  - `walk_inline_js()` for inline JS snippets
  - `State.variables.x = value` → variable write
  - `Macro.add("name", ...)` → custom macro registration
  - `SugarCube.Macro.add(...)` → same
  - Preprocessed var detection via substitution map
- Registry trait methods added to `FormatPlugin` impl:
  - `build_variable_tree()`, `workspace_variable_names()`, `variable_properties()`
  - `custom_macro_names()`, `find_custom_macro()`, `is_custom_macro()`
- Full workspace compiles (cargo check passes)

---

## Remaining Work

### Phase D: Inline JS Validation via oxc

**Status**: Not started  
**Files to modify**: `mod.rs`, possibly new `js_validate.rs`  
**Estimated size**: ~200 lines

The parser already produces `PassageAst` nodes that contain JS snippets (from `<<set>>`, `<<run>>`, `<<script>>`, `<<=>>` blocks). The `ast::collect_js_snippets()` helper extracts these. The next step is to validate them with oxc and produce diagnostics.

**Tasks**:
1. After the SC parser produces a `PassageAst` for a normal/widget passage, call `collect_js_snippets()` on the AST
2. For each snippet, preprocess with `js_preprocess::preprocess_for_oxc()`
3. Parse with `knot_core::oxc::parse_js()`
4. On `JsParseOutcome::Error`, convert JS diagnostics to `FormatDiagnostic` with byte-offset mapping via `PreprocessedJs::map_range_to_original()`
5. Add the diagnostics to the parse result

**Key concern**: Position mapping. The JS snippet offsets are relative to the passage body. They need to be shifted by `body_offset` and then mapped through the preprocessor's substitution table to get back to original SugarCube source positions.

### Phase E: find_macro_at_position + scan_line_for_macro_events

**Status**: Not started  
**Files to modify**: `mod.rs` (add trait method impls)  
**Estimated size**: ~150 lines

These two `FormatPlugin` trait methods are needed by the completion, hover, signature-help, and folding-range handlers. They are currently returning `None`/empty in the default impl.

**Tasks**:
1. Implement `find_macro_at_position(line, byte_pos)`:
   - Scan `line` for `<<name ...>>` and `<</name>>` patterns
   - Handle nested delimiters and string contexts
   - Return `MacroAtPosition { name, full_range, name_range, is_unclosed }`
2. Implement `scan_line_for_macro_events(line, line_idx)`:
   - Detect `<<name>>` (open) and `<</name>>` (close) on a single line
   - Return `Vec<MacroBlockEvent>` for the folding-range handler
   - Handle `<<elseif>>`, `<<else>>` as modifier events

### Phase F: build_var_string_map + resolve_dynamic_navigation_links

**Status**: Not started  
**Files to modify**: `mod.rs`  
**Estimated size**: ~200 lines

These power the dynamic navigation resolution in the graph handler. Currently returning empty defaults.

**Tasks**:
1. Implement `build_var_string_map(workspace)`:
   - Walk all passages in the workspace
   - For each `<<set $var to "literal">>` or `<<set $var to 'literal'>>`, record `$var → "literal"`
   - Return `HashMap<String, Vec<String>>`
2. Implement `resolve_dynamic_navigation_links(passage, var_string_map)`:
   - For each `<<goto $var>>`, `<<include $var>>`, `<<link $var>>`, `<<button $var>>` in the passage
   - Look up `$var` in the var_string_map
   - Return `Vec<ResolvedNavLink>` with the resolved passage names

### Phase G: extract_passage_variable_refs + build_object_property_map + build_shape_aware_property_map

**Status**: Not started (may need trait additions)  
**Files to modify**: `mod.rs`, possibly `plugin.rs` (trait)  
**Estimated size**: ~300 lines

These power the variable tracker UI panel and dot-notation completion. They need the format plugin to extract per-passage variable references with line numbers and build property shape maps.

**Tasks**:
1. Implement `extract_passage_variable_refs()` (may need trait method addition):
   - For a given passage, return `Vec<PassageVarRef>` with variable name, read/write, line number
   - Use the `SourceTextProvider` to compute line numbers from byte offsets
2. Implement `build_object_property_map()`:
   - Return `HashMap<String, HashSet<String>>` mapping variable name → known properties
   - Delegates to `VariableTree::property_map()`
3. Implement `build_shape_aware_property_map()`:
   - Return `HashMap<String, PropertyMapEntry>` with kind inference (Object, Array, Scalar)
   - Infer from assignment patterns: `<<set $x to {}>>` → Object, `<<set $x to []>>` → Array

### Phase H: Incremental Re-parse Optimization

**Status**: Not started  
**Files to modify**: `mod.rs`  
**Estimated size**: ~100 lines

The current `parse()` method clears all registries for the file and re-parses from scratch. For large files, this is wasteful. The `parse_passage()` method exists for incremental updates but needs registry integration.

**Tasks**:
1. In `parse_passage()`, after parsing the single passage, update the registries:
   - Remove old entries for this passage from VariableTree and CustomMacroRegistry
   - Add new entries from the fresh AST
2. Ensure `remove_file()` is called for full-file re-parse but NOT for single-passage re-parse
3. Test that registry state is consistent after incremental updates

### Phase I: Integration Testing + Server Handler Wiring

**Status**: Not started  
**Files to modify**: `integration_tests.rs`, server handlers  
**Estimated size**: ~500 lines

The format plugin is structurally complete after Phases D-H. This phase ensures end-to-end correctness.

**Tasks**:
1. Write integration tests in `formats/src/integration_tests.rs`:
   - Full file parse → verify passages, links, variables, tokens, diagnostics
   - Two-pass ordering → verify scripts parsed before normal passages
   - Registry population → verify VariableTree and CustomMacroRegistry after parse
   - Edge cases: empty files, CRLF, unclosed macros, nested block macros
2. Verify server handlers work with the new trait methods:
   - `completion.rs` — uses `workspace_variable_names()`, `custom_macro_names()`, `find_macro()`
   - `hover.rs` — uses `find_macro()`, `find_custom_macro()`, `global_hover_text()`
   - `variables.rs` — uses `build_variable_tree()`, `extract_passage_variable_refs()`
   - `graph.rs` — uses `build_var_string_map()`, `resolve_dynamic_navigation_links()`
   - `semantic.rs` — uses parse result tokens directly
   - `diagnostics.rs` — uses parse result diagnostics + JS validation diagnostics

### Phase J: Cleanup + Documentation

**Status**: Not started  
**Files to modify**: All sugarcube modules  
**Estimated size**: Minor edits

**Tasks**:
1. Remove any `TODO` / `FIXME` comments left from the rewrite
2. Ensure all `#[cfg(test)]` modules have adequate coverage
3. Verify `cargo clippy` passes with no warnings
4. Verify `cargo test` passes for the entire workspace
5. Update module-level doc comments if any are stale

---

## Dependency Graph

```
Phase A (COMPLETE) ─── Purge + classifier + lexer + macros + special_passages
    |
Phase B (COMPLETE) ─── Parser + AST + two-pass pipeline
    |
Phase C (COMPLETE) ─── Registries + js_preprocess + js_walk + trait methods
    |
    ├── Phase D ─── Inline JS validation via oxc
    ├── Phase E ─── find_macro_at_position + scan_line_for_macro_events
    ├── Phase F ─── Dynamic navigation resolution
    └── Phase G ─── Variable refs + property maps
         |
    Phase H ─── Incremental re-parse optimization (depends on G)
         |
    Phase I ─── Integration testing + handler wiring (depends on D, E, F, G)
         |
    Phase J ─── Cleanup + documentation (depends on I)
```

Phases D, E, F, and G are **independent** and can be worked on in parallel.

---

## File Map — ver_3 sugarcube/

```
sugarcube/
├── mod.rs              (~850 lines)  Plugin struct + FormatPlugin impl + registry trait methods
│                                     + build_body_blocks + build_semantic_tokens + build_diagnostics
│                                     + build_header_tokens + self_classify_tag
├── lexer.rs            (~127 lines)  Logos-based passage splitting + position_from_header
├── classifier.rs       (~503 lines)  Tags-first classification + processing priority + tests
├── parser.rs           (~1038 lines) Recursive descent parser + variable scanning + tests
├── ast.rs              (~415 lines)  AST node types + extraction helpers + JsSnippet
├── macros.rs           (~1200 lines) Static macro catalog (data only, unchanged from ver_2)
├── special_passages.rs (~150 lines)  Special passage definitions (data only, unchanged)
├── variable_tree.rs    (~470 lines)  Variable side table + tree building + tests
├── custom_macros.rs    (~213 lines)  Custom macro registry + tests
├── js_preprocess.rs    (~332 lines)  $var substitution + to→= + position mapping + tests
└── js_walk.rs          (~723 lines)  Oxc AST walker + State.variables + Macro.add + tests
```

**Total**: ~5021 lines (vs ~4000 lines of regex code deleted, net +1000 but with proper AST structure, test coverage, and registry integration)

---

## Key Design Decisions

### 1. Why format-owned registries (not shared containers)?

The `VariableTree` and `CustomMacroRegistry` live inside `SugarCubePlugin`, not in a shared workspace-level container. This is because:

- **Different formats have different variable models.** SugarCube has `$var`/`_var` with `State.variables.*` semantics. Harlowe has different scoping. A shared container would need format-specific branches.
- **Trait methods are the abstraction.** Handlers call `workspace_variable_names()` and get back a `HashSet<String>`. They never know it came from a `VariableTree`. This is cleaner than a shared container that needs format-specific downcasting.
- **Memory isolation.** Each format plugin owns its own state. When the active format changes (SugarCube → Harlowe), the old registries are dropped automatically.

### 2. Why two-pass (classify then ordered parse)?

The two-pass design separates two concerns that were tangled in the old single-pass parser:

- **Classification** is a pure function of `(name, tags)`. It determines WHAT a passage is.
- **Processing order** is a function of classification results. It determines WHEN a passage is parsed.

Without separation, the old code tried to do both at once, leading to the 900-line `parse()` monster with dual-path logic.

### 3. Why recursive descent (not regex)?

The old approach used ~2500 lines of regex patterns across 8+ modules. This was fragile, hard to maintain, and couldn't handle nesting correctly. The recursive descent parser:

- Handles `<<if>><<if>><</if>><</if>>` nesting naturally via depth tracking
- Produces an AST that all downstream consumers (tokens, diagnostics, links, vars) walk uniformly
- Is ~1000 lines instead of ~2500 lines spread across 8 files
- Has no regex dependency for runtime scanning (regex only in the Logos lexer for passage boundary detection, which is correct)

### 4. Why oxc for JS (not regex)?

SugarCube's `State.variables.x = value` and `Macro.add("name", ...)` patterns are JavaScript. Regex-based extraction of these patterns is fragile (can't handle computed property access, aliasing, nested expressions). Oxc gives us:

- A real AST that we can walk structurally
- Built-in error recovery for incomplete JS during live editing
- Position mapping back to original source via the preprocessor's substitution table
- Future extensibility for more complex JS analysis (type inference, control flow)

---

## Testing Strategy

Each module has inline `#[cfg(test)]` tests. The current test coverage:

| Module | Tests | Coverage |
|---|---|---|
| `classifier.rs` | 6 | Tags-first priority, processing order, edge cases |
| `parser.rs` | 18 | Text, links, macros, variables, comments, nesting, errors |
| `variable_tree.rs` | 5 | Record, retrieve, properties, seeded, remove_file |
| `custom_macros.rs` | 4 | Widget, Macro.add, completion, remove_file |
| `js_preprocess.rs` | 6 | Variable substitution, property paths, to→=, position mapping |
| `js_walk.rs` | 3 | State.variables write, Macro.add, inline JS |

**Target**: Phase I should bring total test count to 60+ with integration tests covering full-file parsing scenarios.

---

## Migration Guide: Old → New

For any developer familiar with the ver_2 codebase:

| Old (ver_2) | New (ver_3) | Notes |
|---|---|---|
| `sugarcube::parse()` (900 lines) | `sugarcube::SugarCubePlugin::parse()` (~60 lines) + `parser::parse_passage_body()` | Pipeline is now: split → classify → sort → dispatch |
| `vars/mod.rs::scan_variables()` | `parser::scan_inline_vars()` + `parser::extract_var_ops_from_ast()` | Variables extracted from AST, not regex |
| `links.rs::extract_links()` | `parser::extract_links_from_ast()` | Links extracted from AST, not regex |
| `validation.rs` | `build_diagnostics()` in `mod.rs` | Parse errors from AST Error nodes + unclosed blocks |
| `macro_scan.rs` | `parser::parse_macro()` | Macros parsed by recursive descent, not regex |
| `comments.rs` | `parser::parse_block_comment()` + `parser::parse_html_comment()` | Comments parsed inline by the parser |
| `passage_tree/` | `ast.rs` | New AST node types replace old passage tree |
| `js_extractor.rs` | `ast::collect_js_snippets()` | JS extraction from AST nodes |
| `workspace/` | Deleted | Dead duplicate code |
| `blocks.rs` | Inline in `parser::is_block_macro()` | Block detection via macro catalog lookup |
| `tokens.rs` | `build_semantic_tokens()` in `mod.rs` | Tokens built from AST walk |
| `navigation.rs` | Registry trait methods | Navigation data served from VariableTree/CustomMacroRegistry |
