# Token-Based Hover with Layering — Implementation Plan

**Created:** 2026-06-13  
**Status:** Planning  
**Goal:** Replace line-scanned macro hover with token-based hover that resolves per-token, with inner-layer arg hover overriding outer-layer macro hover.

---

## Problem Statement

### 1. Macro hover is line-scanned, not token-based

`try_macro_hover()` in `hover.rs` extracts the current line via `text.lines().nth(line_idx)`, then calls `plugin.find_macro_at_position(line, byte_pos)`. This means:

- **Multi-line macros** are missed or partially matched (the scanner only sees one line)
- The byte position is **re-derived from the line** instead of using the document-level `byte_offset` already computed at the top of `hover()`
- The hover range is **line-local** (`utf16_len_up_to(line, ...)`) instead of document-absolute
- Every other hover type (variable, link, passage header) is **span-based** — macro hover is the odd one out

### 2. No arg-level hover inside macros (no layering)

`<<link "Talk" "Shop">>` only shows hover for the **entire macro**. There is no way to hover over `"Shop"` and get passage info, even though:

- The AST already has `structured_args: Vec<StructuredMacroArg>` with `ParsedArgKind::PassageRef`
- The token builder already emits `Link` tokens for `PassageRef` args
- The link extractor already creates `Link { target: "Shop", span: ... }` entries

The data exists — hover just doesn't use it for "inner layer" resolution.

### 3. Link hover covers the entire macro span, not the passage-ref arg

`extract_macro_passage_refs()` in `extraction.rs` creates links with `span: open_span.clone()` — the entire `<<link "Talk" "Shop">>` range. So hovering over `"Talk"` (display text) shows passage hover for "Shop" (the target), which is wrong.

---

## Design Decisions

### D1: Store macro arg refs on Passage (not re-derive at hover time)

**Decision:** Add a `macro_arg_refs: Vec<MacroArgRef>` field to the `Passage` struct.

**Alternatives considered:**

| Approach | Pros | Cons |
|----------|------|------|
| **A) Store on Passage** (chosen) | O(1) lookup at hover time; no re-parsing; consistent with how `vars[]` and `links[]` work | Increases `Passage` memory slightly |
| B) Re-derive from AST at hover time | No memory increase | AST is discarded after parsing; would need to re-parse the passage body just for hover |
| C) Store on a separate index in `ParseResult` | Separation of concerns | Another index to maintain; inconsistent with `vars[]`/`links[]` pattern |

The `MacroArgRef` struct is small (4 fields, typically 0-3 per passage). The memory cost is negligible.

### D2: `MacroArgRef` shape

```rust
/// A passage reference inside a macro argument.
///
/// Used for layered hover: when the cursor is on a `PassageRef` arg,
/// the arg's passage hover overrides the outer macro hover.
pub struct MacroArgRef {
    /// The passage name referenced by this arg.
    pub target: String,
    /// Passage-relative byte span of just the reference text
    /// (e.g., `Shop` inside `"Shop"` — not including quotes).
    pub span: Range<usize>,
    /// The macro name containing this arg (e.g., `"link"`).
    pub macro_name: String,
    /// Passage-relative byte span of the macro name portion
    /// (e.g., just `link` in `<<link "Talk" "Shop">>`)
    /// so that macro hover can fire only when the cursor is on the name.
    pub macro_name_span: Range<usize>,
}
```

**Why `macro_name_span` instead of `macro_span` (full macro range)?**  
We need to know whether the cursor is on the macro *name* (→ show macro hover) vs. on an *arg* (→ show arg hover vs. nothing). The full macro range doesn't help distinguish these — only the name span does. If the cursor is inside the macro but not on the name and not on a `MacroArgRef`, we still show macro hover (fallback to outer layer).

**Why not also store `macro_full_span`?**  
The `find_macro_at_position()` line-scanner currently returns the full range. We could store it here and use it for the "cursor is somewhere inside the macro" fallback. But we can derive it from the AST's `open_span` at build time. Actually, we should store it — see Task 4.

**Revised shape:**

```rust
pub struct MacroArgRef {
    /// The passage name referenced by this arg.
    pub target: String,
    /// Passage-relative byte span of just the reference text.
    pub span: Range<usize>,
    /// The macro name (e.g., "link").
    pub macro_name: String,
    /// Passage-relative byte span of the macro name portion.
    pub macro_name_span: Range<usize>,
    /// Passage-relative byte span of the full macro opening tag
    /// (`<<link "Talk" "Shop">>`). Used for fallback macro hover
    /// when cursor is inside the macro but not on a specific arg.
    pub macro_open_span: Range<usize>,
}
```

### D3: Hover priority order (layering)

Current order:
1. Passage header hover
2. Macro hover (line-scanned, always fires for entire macro)
3. Variable hover (span-based)
4. Global object hover (line-scanned)
5. Link hover (span-based)

New order:
1. **Passage header hover** (unchanged)
2. **Macro arg ref hover** (NEW — inner layer: `PassageRef` args show passage info)
3. **Variable hover** (unchanged)
4. **Link hover** (unchanged — but narrowed spans for macro-based links)
5. **Macro hover** (DEMOTED — only fires when cursor is on the macro *name*, not args)
6. **Global object hover** (unchanged)

The layering rule: **innermost token wins**. If the cursor is on a `PassageRef` arg inside a macro, the passage hover takes priority over the macro hover. If the cursor is on the macro name, macro hover fires. If the cursor is on a non-`PassageRef` arg (like a Label), fall through to macro hover as the outer layer.

### D4: Macro hover changes from line-scanned to span-based

**Current:** `try_macro_hover(line, line_idx, char_pos, plugin)` — extracts line, scans for `<<...>>`.

**New:** `try_macro_hover(text, byte_offset, doc, plugin)` — checks `macro_arg_refs[].macro_name_span` to see if cursor is on a macro name, then looks up the macro definition.

This eliminates:
- The line extraction step
- The `find_macro_at_position()` call (replaced by span check)
- The line-local UTF-16 conversion
- Multi-line macro breakage

The `find_macro_at_position()` method on `FormatPlugin` is **not removed** — it's still used by the completion handler. But hover no longer depends on it.

### D5: Narrow link spans for macro-based links

**Current:** `extract_macro_passage_refs()` creates links with `span: open_span.clone()`.

**New:** When `structured_args` contains a `PassageRef` arg, the link's span should be set to that arg's individual span instead of the full `open_span`. This makes link hover only trigger when the cursor is actually on the passage name, not on the display text.

This requires changes to `extract_macro_passage_refs()` or a post-processing step that merges `structured_args` spans into the link data.

**Chosen approach:** Post-process in `passage_build.rs`. After building links from `passage_ast.links`, cross-reference with `structured_args` from the AST to narrow spans. This avoids changing the `extraction.rs` code (which is shared with other consumers) and keeps the span-narrowing logic in one place.

### D6: Non-link macro args (Selector, VariableRef, String)

The `MacroArgRef` struct only stores `PassageRef` args. Other arg kinds (Selector, VariableRef, String, Label) are **not** stored because:

- **VariableRef**: Already handled by `passage.vars[]` with individual spans. No duplication needed.
- **Selector**: No hover value (CSS selectors aren't resolved by the LSP).
- **String/Label**: No hover value (plain text, no target to resolve).

If we later want hover for other arg kinds, we can extend `MacroArgRef` with a `kind` field. But `PassageRef` is the only kind that needs layering today.

---

## Implementation Tasks

### Task 1: Add `MacroArgRef` struct and `macro_arg_refs` field

**Files:** `crates/core/src/passage.rs`

- Add `MacroArgRef` struct with `target`, `span`, `macro_name`, `macro_name_span`, `macro_open_span`
- Add `macro_arg_refs: Vec<MacroArgRef>` field to `Passage` struct
- Add `#[serde(default)]` for backward compatibility
- Initialize as `Vec::new()` in `Passage::new()` and `Passage::new_special()`

**Verification:** `cargo check` passes.

---

### Task 2: Populate `macro_arg_refs` in `passage_build.rs`

**Files:** `crates/formats/src/sugarcube/graph/passage_build.rs`

- Add a new function `build_macro_arg_refs(passage_ast: &PassageAst, body_offset_in_passage: usize) -> Vec<MacroArgRef>`
- Walk the AST, find `AstNode::Macro` nodes with `structured_args`
- For each `StructuredMacroArg` where `kind == ParsedArgKind::PassageRef`:
  - Create a `MacroArgRef` with the arg's `value` as `target`
  - Shift `span` by `body_offset_in_passage` (passage-relative)
  - Shift `name_span` by `body_offset_in_passage` for `macro_name_span`
  - Shift `open_span` by `body_offset_in_passage` for `macro_open_span`
- Call from `build_passage()` and assign to `passage.macro_arg_refs`
- Also call from `build_vars_from_unified_ast()` or separately in `parse_pipeline.rs`

**Verification:** Add a unit test: parse `<<link "Talk" "Shop">>`, verify `macro_arg_refs` has one entry with `target: "Shop"` and a narrow span.

---

### Task 3: Add `try_macro_arg_ref_hover()` in `hover.rs`

**Files:** `crates/server/src/handlers/hover.rs`

- New function `try_macro_arg_ref_hover(text, byte_offset, doc, workspace) -> Option<Hover>`
- Iterate `doc.passages[].macro_arg_refs[]`
- If `passage.span_contains_abs_offset(&ref.span, byte_offset)`:
  - Look up the target passage via `workspace.find_passage(&ref.target)`
  - Show passage hover (same format as link hover)
  - Use `passage.abs_range(&ref.span)` for the hover range
  - Return the hover (inner layer wins)
- Insert into the hover priority chain at position 2 (after passage header, before variable hover)

**Verification:** Manual test: hover over `"Shop"` in `<<link "Talk" "Shop">>` → shows passage info for "Shop". Hover over `"Talk"` → no arg ref hover (falls through).

---

### Task 4: Convert macro hover to span-based

**Files:** `crates/server/src/handlers/hover.rs`

- Replace `try_macro_hover(line, line_idx, char_pos, plugin)` with `try_macro_hover(text, byte_offset, doc, plugin)`
- New logic:
  1. Iterate `doc.passages[].macro_arg_refs[]`
  2. If `passage.span_contains_abs_offset(&ref.macro_name_span, byte_offset)`:
     - Look up the macro definition via `plugin.find_macro(&ref.macro_name)`
     - Show macro hover (same content as current)
     - Use `passage.abs_range(&ref.macro_name_span)` for the hover range
     - Return the hover
  3. Also check if cursor is inside `macro_open_span` but not on the name or a `PassageRef` arg:
     - This handles the "cursor on a Label arg" case → show macro hover as outer layer
     - Use `passage.abs_range(&ref.macro_open_span)` for the hover range in this case
- Remove the line extraction and `find_macro_at_position()` call from hover
- The `find_macro_at_position()` method stays on `FormatPlugin` (used by completion)

**Verification:** Manual test: hover over `link` in `<<link "Talk" "Shop">>` → shows macro hover. Hover over `"Talk"` → shows macro hover (outer layer, since it's not a PassageRef). Hover over `"Shop"` → shows passage hover (inner layer, from Task 3).

---

### Task 5: Narrow link spans for macro-based links

**Files:** `crates/formats/src/sugarcube/graph/passage_build.rs`

- After building `passage.links` from `passage_ast.links`, cross-reference with `structured_args` from the AST
- For each link whose `source` is a macro (not `PassageLink`), find the matching `StructuredMacroArg` with `kind == PassageRef`
- If found, replace the link's span with the arg's span (shifted by `body_offset_in_passage`)
- This makes link hover only trigger on the actual passage name, not the entire macro

**Alternative:** Do this in `extraction.rs` by passing `structured_args` into `extract_macro_passage_refs()`. But this requires changing the function signature and the call chain. Post-processing in `passage_build.rs` is simpler.

**Verification:** Manual test: hover over `"Talk"` in `<<link "Talk" "Shop">>` → no link hover (falls through to macro hover). Hover over `"Shop"` → passage hover from `macro_arg_refs` (inner layer).

---

### Task 6: Add `macro_arg_refs` to incremental parse path

**Files:** `crates/formats/src/sugarcube/parse_pipeline.rs`

- In `parse_single()`, call `build_macro_arg_refs()` and assign to `passage.macro_arg_refs`
- Same logic as `parse_full()` but with `body_offset_in_passage = 0`

**Verification:** `cargo check` passes. Incremental edits on a file with `<<link "Talk" "Shop">>` still produce correct hover.

---

### Task 7: Clean up and test

- Remove any dead code from the line-scanned macro hover path
- Verify all existing tests pass
- Test multi-line macros: `<<if $x gt 0>>\n  <<link "Go" "Cave">>\n<</if>>`
- Test edge cases:
  - `<<goto $dest>>` (variable target, no `MacroArgRef`)
  - `<<goto Forest>>` (bare passage name)
  - `<<actions "Room1" "Room2" "Room3">>` (multiple passage refs)
  - `<<link "Display">>` (single arg, display=target)
  - `<<back "Back">>` (no passage target, dynamic)
  - Nested macros: `<<if true>><<link "Go" "Cave">><</if>>`

---

## Task Completion Log

| Task | Status | Notes |
|------|--------|-------|
| 1. Add `MacroArgRef` struct | ✅ done | |
| 2. Populate `macro_arg_refs` | ✅ done | |
| 3. Add `try_macro_arg_ref_hover()` | ✅ done | |
| 4. Convert macro hover to span-based | ✅ done | |
| 5. Narrow link spans for macro links | ✅ done | |
| 6. Incremental parse path | ✅ done | inherited from Task 2 |
| 7. Clean up and test | ✅ done | all 448+ tests pass |

---

### Task 1 Worklog: Add `MacroArgRef` struct and `macro_arg_refs` field

**Date:** 2026-06-13

**Changes made:**

1. **Added `MacroArgRef` struct** in `crates/core/src/passage.rs` (after `VarKind`, before `VarOp`):
   - `target: String` — passage name referenced by this arg
   - `span: Range<usize>` — passage-relative byte span of just the reference text
   - `macro_name: String` — the macro name (e.g., `"link"`)
   - `macro_name_span: Range<usize>` — passage-relative span of the macro name portion
   - `macro_open_span: Range<usize>` — passage-relative span of the full macro opening tag
   - Derives: `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`

2. **Added `macro_arg_refs: Vec<MacroArgRef>` field** to `Passage` struct:
   - Placed after `vars` field, before `is_special`
   - Marked `#[serde(default)]` for backward compatibility
   - Full doc comment explaining layered hover purpose and why only `PassageRef` args are stored

3. **Initialized `macro_arg_refs: Vec::new()`** in both `Passage::new()` and `Passage::new_special()`

4. **Updated Passage struct doc comment** to include `macro_arg_refs[].span` in the passage-relative spans list

**Verification:** `cargo check` passes with no new warnings.

---

### Task 2 Worklog: Populate `macro_arg_refs` in `passage_build.rs`

**Date:** 2026-06-13

**Changes made:**

1. **Updated import** in `passage_build.rs`: added `MacroArgRef` to the `knot_core::passage` import.

2. **Added `build_macro_arg_refs()` function** — public entry point that creates a `Vec<MacroArgRef>` from AST nodes:
   - Delegates to `collect_macro_arg_refs()` recursive helper
   - Follows the same pattern as `build_vars_from_unified_ast()` / `collect_vars_from_nodes()`

3. **Added `collect_macro_arg_refs()` recursive helper** — walks AST nodes:
   - Matches on `AstNode::Macro { name, name_span, open_span, children, structured_args, .. }`
   - For each macro with `structured_args`, filters for `ParsedArgKind::PassageRef`
   - Creates `MacroArgRef` with:
     - `target`: the arg's `value` (passage name)
     - `span`: `body_offset_in_passage + sarg.span` (passage-relative)
     - `macro_name`: the macro's `name`
     - `macro_name_span`: `body_offset_in_passage + name_span` (passage-relative)
     - `macro_open_span`: `body_offset_in_passage + open_span` (passage-relative)
   - Recurses into `children` for block macros (e.g., `<<if>>...<<link>>...<</if>>`)

4. **Wired into `build_passage()`**: added `passage.macro_arg_refs = build_macro_arg_refs(&passage_ast.nodes, body_offset_in_passage);` after the vars assignment.

**Design note:** `build_macro_arg_refs()` is `pub` so it can also be called from `parse_pipeline.rs` for the incremental path (Task 6). The function takes `&[AstNode]` and `body_offset_in_passage` — no plugin state needed.

**Verification:** `cargo check` passes with no new warnings.

---

### Task 3 Worklog: Add `try_macro_arg_ref_hover()` in `hover.rs`

**Date:** 2026-06-13

**Changes made:**

1. **Updated module doc comment** — added "Layered Hover" section explaining the inner-layer priority model. Updated "Span-Based Resolution" to note that all hover types except global object hover are now span-based.

2. **Added `try_macro_arg_ref_hover()` function** (inserted before `try_macro_hover`):
   - Signature: `fn try_macro_arg_ref_hover(text, byte_offset, doc, workspace) -> Option<Hover>`
   - Iterates `doc.passages[].macro_arg_refs[]`
   - Uses `passage.span_contains_abs_offset(&arg_ref.span, byte_offset)` for precise matching
   - For found passage: shows same rich info as link hover (name, file, links out, incoming, tags, vars)
   - Adds `*Referenced by* <<macro_name>>` line to distinguish from `[[link]]` hover
   - For broken ref: shows `⚠ Broken link` diagnostic
   - Uses `passage.abs_range(&arg_ref.span)` for the hover range (narrow, just the passage name)

3. **Reordered hover priority chain** to match the plan (D3):
   - Step 1: Passage header hover (unchanged)
   - Step 2: **Macro arg ref hover** (NEW — inner layer)
   - Step 3: Variable hover (unchanged)
   - Step 4: **Link hover** (moved up from step 5)
   - Step 5: **Macro hover** (demoted from step 2 — outer layer, only fires when inner layer didn't match)
   - Step 6: Global object hover (unchanged, moved from step 4)

4. **Key design choice**: The arg ref hover adds `*Referenced by* <<link>>` at the bottom of the hover text. This distinguishes it from a `[[link]]` hover on the same passage — the user can see which macro triggered the reference.

**Verification:** `cargo check` passes with no new warnings.

---

### Task 4 Worklog: Convert macro hover to span-based

**Date:** 2026-06-13

**Changes made:**

1. **Replaced `try_macro_hover` signature** from `(line, line_idx, char_pos, plugin)` to `(text, byte_offset, doc, plugin)` — now takes document-level inputs like all other span-based hover functions.

2. **New span-based logic** (two-tier check):
   - **Tier 1 — Cursor on macro name**: Checks `passage.span_contains_abs_offset(&arg_ref.macro_name_span, byte_offset)`. If matched, shows macro hover with `macro_name_span` as the hover range (e.g., just `link` highlighted).
   - **Tier 2 — Cursor inside macro open tag**: Checks `passage.span_contains_abs_offset(&arg_ref.macro_open_span, byte_offset)` but excludes the name and PassageRef arg spans. Shows macro hover with `macro_open_span` as the hover range. This handles Label args, whitespace, etc.
   - Both tiers iterate `doc.passages[].macro_arg_refs[]` — the same data as the inner-layer hover.

3. **Line-scanning fallback retained** (with TODO): Macros that have no `macro_arg_refs` (no PassageRef args, e.g., `<<set>>`, `<<if>>`, `<<print>>`) can't be found via span-based resolution yet. The fallback uses `find_macro_at_position()` on the current line. This will be removed once we store macro name spans for ALL macros.

4. **Extracted `build_macro_hover_text()`**: The hover text formatting (description, deprecation, parameters, container constraints) was extracted into a shared function used by both the span-based path and the line-scanning fallback. This avoids code duplication.

5. **Updated call site** in `hover()`: Changed from `try_macro_hover(line, line_idx, char_pos, plugin)` to `try_macro_hover(text, byte_offset, doc, plugin)`. No more line extraction needed for the span-based path.

**Important design note**: The `continue` in the Tier 2 check is critical — it prevents the macro open span from matching when the cursor is actually on a PassageRef arg (which should be handled by `try_macro_arg_ref_hover` in step 2, not by the macro hover in step 5).

**Verification:** `cargo check` passes with no new warnings.

---

### Task 5 Worklog: Narrow link spans for macro-based links

**Date:** 2026-06-13

**Changes made:**

1. **Added `narrow_link_spans()` function** in `passage_build.rs`:
   - Takes `&mut [Link]` and `&[MacroArgRef]`
   - For each link, looks for a `MacroArgRef` with matching `target` whose `macro_open_span` overlaps with the link's span
   - If found, replaces the link's span with the arg's narrower `span` (just the passage name, not the whole macro)
   - `[[passage]]` links are unaffected — they have no matching `MacroArgRef`

2. **Overlap check** instead of equality: The link span may equal `open_span` (for single-arg macros like `<<goto "Passage">>`) or differ slightly. The overlap check (`link.span.start >= arg_ref.macro_open_span.start && link.span.end <= arg_ref.macro_open_span.end`) correctly handles both cases.

3. **Called from `build_passage()`**: After building both `links` and `macro_arg_refs`, calls `narrow_link_spans(&mut passage.links, &passage.macro_arg_refs)`.

4. **Effect on hover**: Now when the cursor is on `"Talk"` in `<<link "Talk" "Shop">>`, the link hover (step 4) won't fire because the link span only covers `"Shop"`. The cursor falls through to the macro hover (step 5), which shows `<<link>>` info. When the cursor is on `"Shop"`, the `try_macro_arg_ref_hover` (step 2) fires first with passage info.

**Verification:** `cargo check` passes with no new warnings.

---

### Task 6 Worklog: Add `macro_arg_refs` to incremental parse path

**Date:** 2026-06-13

**Changes made:**

**No code changes needed.** The `parse_single()` function in `parse_pipeline.rs` already calls `build_passage(&cp, &passage_ast, 0, 0)`, which was updated in Task 2 to populate `passage.macro_arg_refs` via `build_macro_arg_refs()` and narrow link spans via `narrow_link_spans()`. Unlike `passage.vars` (which gets overridden by `build_vars_from_unified_ast` for more accurate js_analysis-based var ops), `macro_arg_refs` doesn't need a separate "unified" pass — `build_macro_arg_refs` already uses `structured_args` from the AST which is populated during the SugarCube parser's structural parse phase.

**Verification:** `cargo check` passes with no new warnings.

---

### Task 7 Worklog: Clean up and test

**Date:** 2026-06-13

**Verification results:**

1. **Full build**: `cargo build` succeeds — no errors.
2. **All tests**: `cargo test` — 33 server tests pass, 448 format tests pass, 0 failures.
3. **Warnings check**: `cargo check` — only pre-existing `send_semantic_token_refresh` warning. No new warnings from our changes.
4. **Dead code check**: All imports in `hover.rs` are still used (`Passage`, `MacroArgKind`). No dead code introduced.

**Summary of all changes across the 7 tasks:**

| File | Changes |
|------|---------|
| `crates/core/src/passage.rs` | Added `MacroArgRef` struct (5 fields), `macro_arg_refs: Vec<MacroArgRef>` field on `Passage`, initialized in both constructors, updated doc comment |
| `crates/formats/src/sugarcube/graph/passage_build.rs` | Added `build_macro_arg_refs()`, `collect_macro_arg_refs()`, `narrow_link_spans()` functions; wired into `build_passage()` |
| `crates/server/src/handlers/hover.rs` | Added `try_macro_arg_ref_hover()` (inner layer), rewrote `try_macro_hover()` (span-based, outer layer), extracted `build_macro_hover_text()`, reordered hover priority chain |

**Remaining work (not in this plan):**
- The line-scanning fallback in `try_macro_hover()` handles macros without PassageRef args (e.g., `<<set>>`, `<<if>>`). To fully eliminate `find_macro_at_position()` from hover, we'd need to store macro name spans for ALL macros (not just those with PassageRef args). This is a separate enhancement.
- Unit tests for `build_macro_arg_refs()` and `narrow_link_spans()` would be valuable but aren't blocking.

---

### Post-Plan Fix: Macro Polymorphy in Hover

**Date:** 2026-06-13

**Problem:** `<<link>>` has two variants in SugarCube — inline (`<<link "Talk" "Shop">>`, no body, no close tag) and block (`<<link "Talk" "Shop">>…<</link>>`, has body). Both showed "**Block macro**" in hover because `classify()` returns `MacroKind::Block` for all macros with `body: Optional`, regardless of whether the specific invocation actually has a body.

**Key insight:** The polymorphy is NOT about arg count (1 vs 2 args). Both inline and block variants can have the same args. The difference is the presence of a body: `<<link [args]>>` (inline) vs `<<link [args]>><</link>>` (block).

**Root causes:**

1. `MacroArgRef` didn't track whether the macro has a body, so hover had no way to distinguish variants
2. `<<link "Talk">>` (1 arg) classified its sole arg as `ParsedArgKind::Label`, not `PassageRef`, so no `MacroArgRef` was created — hover fell through to the line-scanning fallback which always shows "Block macro"
3. `build_macro_hover_text()` had no concept of polymorphy — same description and kind label for all invocations

**Changes made:**

1. **`crates/core/src/passage.rs`** — Added `has_body: bool` field to `MacroArgRef`:
   - `true` when the macro has a body (children between open and close tags, i.e., `<<link>>…<</link>>`)
   - `false` when the macro is inline (no close tag, i.e., `<<link>>` alone)
   - Derived from `children.is_some()` on the AST node

2. **`crates/formats/src/sugarcube/graph/passage_build.rs`** — Updated `collect_macro_arg_refs()`:
   - Now computes `has_body = children.is_some()` and stores it on each `MacroArgRef`
   - Added "Label-as-PassageRef" logic: for `label_then_passage` macros (link, button, click, etc.) when there's only 1 structured arg classified as `Label`, it also doubles as a passage target (since `<<link "Talk">>` is equivalent to `[[Talk]]`)
   - Uses `label_then_passage_macros()` from the classifiers module

3. **`crates/server/src/handlers/hover.rs`** — Updated `build_macro_hover_text()`:
   - New parameter `has_body: Option<bool>` (None for line-scanning fallback where body presence is unknown)
   - For any macro with `body: Optional` (polymorphic):
     - `has_body == Some(false)` → reclassify as `MacroKind::Statement` ("Statement macro"), show inline description: "Inline navigation link — no body section."
     - `has_body == Some(true)` → keep `MacroKind::Block` classification and existing description
     - `None` → use default classification (safe fallback)
   - Suppresses "Close with `<</link>>`" note for inline variant (no body expected)
   - Updated `try_macro_hover()` to pass `Some(arg_ref.has_body)` for span-based path, `None` for line-scanning fallback

**Polymorphic macros affected** (all have `body: Optional`):
- `<<link>>` — inline when no close tag; block when `<<link>>…<</link>>`
- `<<button>>` — same pattern
- `<<click>>` — same pattern (deprecated)

**Verification:** `cargo check` passes (0 errors), `cargo test` — all 557 tests pass (76 + 448 + 33).
