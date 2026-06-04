# Knot Virtual Document Architecture — Work Log

## Overview

This worklog tracks the implementation of the Virtual Document Architecture overhaul
as described in `Vdoc architecture.md`. The migration follows a 9-step plan.

---

Task ID: 1
Agent: Main
Task: Create VirtualDocAdapter trait and VirtualDocManager in core

Work Log:
- Created `crates/core/src/virtual_doc.rs` with full module
- Defined `VirtualDocAdapter` trait with: `should_include_passage()`, `translate_passage()`, `resolve_source_location()`, `interpret_diagnostic()`, `clear_state()`, `invalidate_passage()`
- Defined supporting types: `TranslatedBlock`, `AdapterContext`, `StartupAlias`, `UserCallable`, `UserCallableKind`, `SourceLocation`, `JsDiagnostic`, `TwDiagnostic`, `DiagnosticSeverity`
- Defined `SourceTextProvider` trait in core (so adapter trait doesn't depend on knot_formats)
- Defined `PassageEntry` struct for byte-range index
- Implemented `VirtualDocManager` with: `new()`, `rebuild()`, `update_passage()`, `remove_passage()`, `remove_passage_at()`, `content()`, `find_passage_for_byte_range()`, `passage_names()`, `is_empty()`, `len()`, `get_entry()`
- Added `pub mod virtual_doc;` to `crates/core/src/lib.rs`
- Added re-exports in `lib.rs`
- Added unit tests for VirtualDocManager

Stage Summary:
- Step 1 COMPLETE
- Core infrastructure ready for format adapters to implement

---

Task ID: 2
Agent: Main
Task: Create SugarCubeAdapter + stub adapters for other formats

Work Log:
- Created `crates/formats/src/sugarcube/adapter.rs` with `SugarCubeAdapter`
- SugarCubeAdapter implements `VirtualDocAdapter` using existing `walk_translate` and `VirtualDocMap`
- Internal `PassageReverseMap` struct stores per-passage reverse mapping data
- `translate_passage()` delegates to `translate_script_passage()` or `translate_macro_passage()`
- `resolve_source_location()` walks js_block lines to find matching line map entry
- `interpret_diagnostic()` filters SugarCube false positives and converts JS errors to warnings
- `compute_body_offset()` extracts body offset from source text
- `extract_body_text()` extracts body text from passage (prefers source text, falls back to blocks)
- Added `pub mod adapter;` to sugarcube mod.rs
- Created stub adapters for Harlowe, Chapbook, Snowman (return None/empty)

Stage Summary:
- Step 2 COMPLETE
- SugarCubeAdapter wraps existing translation pipeline
- Stub adapters for other formats ready for future implementation

---

Task ID: 3
Agent: Main
Task: Wire VirtualDocManager into server state

Work Log:
- Added `VirtualDocManager` and `Option<Box<dyn VirtualDocAdapter>>` to `ServerStateInner`
- Created `CoreDocumentCache` newtype implementing core's `SourceTextProvider`
- Created `DocumentCache` newtype implementing formats' `SourceTextProvider`
- Both wrap `&HashMap<Url, String>` (the open_documents cache)
- Initialized both fields in `ServerState::new()`

Stage Summary:
- Step 3 COMPLETE
- Server state now owns virtual doc lifecycle infrastructure

---

Task ID: 4
Agent: Main
Task: Update knot_virtual_doc handler

Work Log:
- Updated `knot_virtual_doc` handler in `knot_ext.rs`
- New path: if VirtualDocManager is populated, use its content directly
- Computes line map from PassageEntry byte ranges
- Falls back to old FormatPlugin path if VirtualDocManager is empty

Stage Summary:
- Step 4 COMPLETE
- Handler prefers new VirtualDocManager with backward-compatible fallback

---

Task ID: 5
Agent: Main
Task: Update refresh pipeline

Work Log:
- Added `rebuild_virtual_doc_if_available()` to `indexing.rs`
  - Lazily creates SugarCubeAdapter when first needed
  - Calls VirtualDocManager::rebuild() with adapter
- Added `update_virtual_doc_passage()` to `indexing.rs`
  - Calls VirtualDocManager::update_passage() for surgical updates
- Updated `did_open` to call rebuild_virtual_doc_if_available()
- Updated `did_change` to call update_virtual_doc_passage() for modified/added passages
  - Calls remove_passage() for removed passages
  - Calls rebuild_virtual_doc_if_available() on format change
- Updated `index_workspace` to call rebuild_virtual_doc_if_available() after graph rebuild
- All three sync paths (did_open, did_change, index_workspace) now maintain the virtual doc

Stage Summary:
- Step 5 COMPLETE
- Full lifecycle management: rebuild on open/index, surgical update on change, remove on delete

---

Task ID: 6
Agent: Main
Task: Implement client-to-server diagnostic relay

Work Log:
- Updated Vdoc architecture.md Section 10 with corrected design
- Key insight: VSCode's built-in JS service runs client-side, not as a separate LSP
- Designed `knot/jsDiagnostics` custom notification (client → server)
- Client-side KnotVirtualDocDiagnostics becomes a thin relay
- Server runs two-stage diagnostic relay and publishes via textDocument/publishDiagnostics
- No more client-side DiagnosticCollection for .tw diagnostics
- Updated Section 6.3 (complete flow) to include client-to-server relay steps
- Updated Section 11.2 (enhanced pipeline) to remove "push to JS LSP" step
- Updated Section 12 migration plan with new Steps 6-7
- Updated Section 12.1 (what gets deleted) with client-side items
- Updated Section 12.3 (what gets created) with new wire types and handlers
- Updated Section 14 (open questions) with relay-specific questions

Stage Summary:
- ARCHITECTURE DESIGN COMPLETE for Steps 6-7
- Code implementation pending (next tasks)

---

Task ID: 2b
Agent: Main
Task: Fix missing `pub mod adapter;` in Harlowe/Chapbook/Snowman mod.rs

Work Log:
- Added `pub mod adapter;` to `crates/formats/src/harlowe/mod.rs`
- Added `pub mod adapter;` to `crates/formats/src/chapbook/mod.rs`
- Added `pub mod adapter;` to `crates/formats/src/snowman/mod.rs`
- Previously only SugarCube had the module declaration; other formats' adapter stubs existed but weren't wired into the module tree

Stage Summary:
- Step 2 gap FIXED
- All format adapters now properly declared in module tree

---

Task ID: 6b
Agent: Main
Task: Implement JS diagnostic relay — server-side handler and wire types

Work Log:
- Added `KnotJsDiagnosticsParams`, `KnotJsDiagnostic`, `KnotJsDiagnosticsResponse` types to `lsp_ext.rs`
- Wire format uses line/character positions (matching VSCode's diagnostic API), not byte offsets
- Added `js_diagnostics: HashMap<Url, Vec<Diagnostic>>` field to `ServerStateInner` in `state.rs`
- Initialized the field in `ServerState::new()`
- Registered `knot/jsDiagnostics` handler in `lib.rs` via `.custom_method()`
- Added `line_char_to_byte_range()` helper function to `knot_ext.rs`
  - Walks virtual doc content to convert line/char positions to byte offsets
  - Used by the diagnostic relay handler for Stage 1 passage lookup
- Added `byte_range_to_lsp_range()` helper function to `knot_ext.rs`
  - Converts byte ranges in .tw source text back to LSP Range positions
  - Used when publishing resolved .tw diagnostics
- Added `knot_js_diagnostics()` handler to `knot_ext.rs`
  - Stage 1: Converts line/char → byte offsets, binary searches VirtualDocManager to find passage
  - Stage 2: Delegates to adapter.resolve_source_location() and adapter.interpret_diagnostic()
  - Stores resolved diagnostics in inner.js_diagnostics
  - Publishes via textDocument/publishDiagnostics
  - Returns count of processed diagnostics
- Used request pattern (not notification) for tower-lsp compatibility

Stage Summary:
- Step 6 SERVER SIDE COMPLETE
- Two-stage diagnostic relay handler implemented
- Wire types defined for client-server communication

---

Task ID: 6c
Agent: Main
Task: Implement JS diagnostic relay — client-side thin relay

Work Log:
- Added `sendNotification(method, params)` to `KnotLanguageClient` interface in `types.ts`
- Added `KnotJsDiagnosticsParams`, `KnotJsDiagnostic`, `KnotJsDiagnosticsResponse` types to `types.ts`
- Refactored `KnotVirtualDocDiagnostics` class in `virtualDocProvider.ts`:
  - REMOVED: `diagnosticCollection` (knot-virtual-doc DiagnosticCollection)
  - REMOVED: `twDiagnostics` Map and all deduplication logic
  - REMOVED: `findPassageBodyLine()` method
  - REMOVED: `VDOC_DIAGNOSTIC_COLLECTION` constant
  - ADDED: Debounce timer (300ms) for batching diagnostics before relay
  - ADDED: `pendingDiagnostics` array for collecting raw JS diagnostics
  - ADDED: `flushDiagnostics()` method that sends batch to server via `knot/jsDiagnostics`
  - ADDED: `convertSeverity()` method for VSCode → wire format conversion
  - CHANGED: `onDiagnosticsChanged()` now collects raw JS diagnostics and relays to server
  - Client is now a pure relay — no local diagnostic publishing

Stage Summary:
- Step 6 CLIENT SIDE COMPLETE
- Client is a thin relay — collects JS diagnostics and forwards to server
- No more conflicting DiagnosticCollection on .tw files
- Debounced batch sending (300ms) to avoid bursts

---

Task ID: 7
Agent: Main
Task: Bug fixes — byte offsets, diagnostic merging, startup aliases, line mapping

Work Log:
- **Bug #2 FIX**: `find_passage_for_byte_range()` now returns passage-relative byte ranges
  instead of absolute vdoc byte ranges. The adapter's `resolve_source_location()` walks
  its js_block from byte 0, so it needs passage-relative offsets. The conversion is done
  by subtracting the passage's entry.byte_range.start from the absolute range.
  - Updated trait docstring in `core/src/virtual_doc.rs` to document passage-relative semantics
  - Updated `SugarCubeAdapter::resolve_source_location()` comments to reflect the fix

- **Bug #3 FIX**: JS diagnostics now merge with graph/format diagnostics instead of replacing them
  - Added `js_diagnostics` parameter to `publish_all_diagnostics()` in `diagnostics.rs`
  - The function now publishes all three sources together (graph + format + JS), preventing
    LSP's publishDiagnostics from wiping out one source when another publishes
  - Updated all 7 call sites (1 in indexing.rs, 6 in sync.rs) to pass `&inner.js_diagnostics`
  - Updated `knot_js_diagnostics()` handler to call `publish_all_diagnostics()` after updating
    stored JS diagnostics, ensuring a consistent merged snapshot

- **Bug #1 FIX**: startup_aliases and user_callables now populated in AdapterContext
  - Added `extract_startup_aliases()` and `extract_user_callables()` methods to VirtualDocAdapter trait
    with default empty implementations (so stub adapters don't need changes)
  - Implemented both in SugarCubeAdapter: delegates to existing `extract_startup_aliases()` and
    `extract_user_callables()` functions in `sugarcube/virtual_doc.rs`, converting between
    formats' types and core's types
  - Added `startup_aliases` and `user_callables` fields to `VirtualDocManager` struct
  - `rebuild()` now calls adapter.extract_startup_aliases() and extract_user_callables() before
    the translation loop, storing results in self
  - `update_passage()` now uses stored aliases/callables instead of empty vecs

- **Bug #4 FIX**: knot_virtual_doc line_map now computes original_line
  - Rewrote line map building to use `find_passage_for_byte_range()` per line
  - For lines within a passage, computes passage-relative line number by counting
    newlines from the passage start to the current byte offset
  - Lines in gaps/preamble get passage_name="" and original_line=0

- **Dead code cleanup**:
  - Removed `if inner_body.is_empty() { "" } else { "" }` dead code in SugarCubeAdapter
    (both translate_script_passage and translate_macro_passage)

Stage Summary:
- All 4 identified bugs FIXED
- Diagnostic pipeline now correctly merges all three sources
- Adapter context now receives proper startup aliases and user callables
- Byte-range calculations are now passage-relative throughout
- Line map provides passage-relative line numbers

---

Task ID: 8
Agent: Main
Task: Step 8 — Remove old virtual doc code (VirtualDocHooks, FormatPlugin methods, old types, fallback paths)

Work Log:
- Removed old FormatPlugin fallback path in knot_virtual_doc handler (knot_ext.rs)
- Removed FormatPlugin::virtual_doc_content(), virtual_doc_line_map() trait methods
- Removed FormatPlugin::extract_startup_aliases(), has_variable_affecting_content(), translate_passage_to_js(), build_virtual_document() trait methods
- Removed blanket impl VirtualDocHooks for T: FormatPlugin
- Removed VirtualDocHooks trait, build_core_virtual_document(), build_format_section_line_map() from formats/src/virtual_doc.rs
- Removed VirtualDocLineMapEntry, VirtualDocument, VirtualSection, VirtualSectionKind, LineMapping types from formats/src/types.rs
- Removed corresponding re-exports from formats/src/lib.rs
- Removed SugarCube FormatPlugin hook overrides (extract_startup_aliases, has_variable_affecting_content, translate_passage_to_js, extract_user_callables, virtual_doc_content, virtual_doc_line_map)
- Refactored extract_startup_aliases() in sugarcube/virtual_doc.rs: changed signature from `&[VirtualSection]` to `&str` to decouple from old types
- Updated SugarCubeAdapter::extract_startup_aliases() to call refactored function directly (no more synthetic VirtualSection construction)
- Fixed compilation errors: indexing module visibility, CoreDocumentCache trait import, removed analyze_workspace call
- Fixed all compilation warnings: unused imports, dead code annotations
- Zero errors, zero warnings on cargo check

Stage Summary:
- Step 8 COMPLETE — all old virtual doc code paths removed
- VirtualDocManager is now the sole source of virtual doc content
- Old VirtualDocHooks/FormatPlugin hook pipeline fully excised
- Old types (VirtualDocument, VirtualSection, etc.) removed from formats crate
- Clean compilation with no warnings

---

Task ID: 9
Agent: Main
Task: Step 9 — Byte-span propagation (Phase 2)

Work Log:
- Added `original_end_byte: usize` field to `ExactLineMapping` in `walk_translate.rs`
  - Provides the one-past-the-end byte offset for precise source byte spans
  - Together with `original_start_byte`, defines the full source byte span
- Updated `append_with_mapping()` to accept `source_end_byte: usize` parameter
  - All produced `ExactLineMapping` entries now carry the full span
- Updated all callers of `append_with_mapping()` in `walk_translate.rs`:
  - Macro nodes: `source_end_byte = span.end`
  - Close tag spans: `source_end_byte = close_span.end`
  - Expression/Error nodes: `source_end_byte = span.end`
  - Text nodes: `source_end_byte = span.end`
- Updated all `ExactLineMapping` constructors in `walk_translate.rs`:
  - Function header/temp var/close brace sentinels: `original_end_byte: body_offset`
  - `<<script>>` block lines: `original_end_byte: span.end`
- Updated all `ExactLineMapping` constructors in `custom_macros.rs`:
  - All entries use `original_end_byte: body_offset` (conservative for script passages)
- Updated all `ExactLineMapping` constructors in `virtual_doc_map.rs`:
  - Preamble/separator entries: `original_end_byte: 0`
  - Test entries: matching `original_start_byte` values
- Updated all `ExactLineMapping` constructors in `adapter.rs`:
  - Sentinel entries (annotation, function declaration, closing brace): `original_end_byte: body_offset`
  - Body lines cloned from walk_translate output inherit `original_end_byte` automatically
- Rewrote `resolve_source_location()` to use `original_end_byte` for precise byte ranges:
  - Previously: used `original_start_byte` + heuristic `min(80)` cap for range end
  - Now: uses `original_end_byte` from the reverse mapping for the exact source span
  - Multi-line diagnostics: merges overlapping line spans (min start, max end)
- Cleaned up `#[allow(dead_code)]` annotations:
  - Removed blanket annotation on `PassageReverseMap` struct
  - Added targeted annotations on legitimately unused fields (`file_uri`, `body_offset`, `original_line`)
  - `original_start_byte` and `original_end_byte` are now actively consumed by `resolve_source_location()`
- Zero errors, zero warnings on cargo check

Stage Summary:
- Step 9 COMPLETE — byte-span propagation implemented
- `ExactLineMapping` now carries full byte spans (start + end) for each source construct
- `resolve_source_location()` produces precise source byte ranges instead of heuristic approximations
- All 9 migration steps complete
