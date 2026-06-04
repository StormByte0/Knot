# Knot Virtual Document Architecture Design Spec

**Project:** Knot — VSCode Extension for Twine  
**Component:** JS Virtual Document Pipeline  
**Date:** 2026-06-04  
**Status:** Implementation In Progress (Steps 1-5 complete, Steps 6-9 pending)  

---

## 1. Problem Statement

The current virtual document architecture has the abstraction boundary inverted: **formats own everything, core owns nothing**. This creates several concrete problems:

1. **`VirtualDocHooks` trait is dead code.** The `build_core_virtual_document()` pipeline in `formats/src/virtual_doc.rs` is never called by the server. The server bypasses it entirely.

2. **Format plugins store virtual doc state.** `FormatPlugin::virtual_doc_content()` and `FormatPlugin::virtual_doc_line_map()` serve the assembled virtual doc directly from format-internal state. Core has no involvement in the virtual doc lifecycle.

3. **SugarCube bypasses the hooks entirely.** It builds its own `VirtualDocMap` internally and serves the monolithic output through `FormatPlugin` methods. The `VirtualDocHooks` abstraction is irrelevant.

4. **No centralized JS LSP integration.** Each format would need to independently manage the JS LSP connection, workspace initialization, and file updates. This logic should be shared infrastructure.

5. **Byte-span propagation was bolted on.** The previous byte-span work (saved as reference zips) had to fight the architecture because `VirtualDocLineMapEntry` only carries `original_line: u32`, forcing precision loss at the `ExactLineMapping → VirtualDocLineMapEntry` conversion boundary.

---

## 2. Design Principle: Core-Infra, Format-Content

The central organizing principle is:

> **Core owns the infrastructure (lifecycle, indexing, JS LSP, refresh signaling).  
> Format owns the content (what goes in, how to interpret what comes out).**

Core does not understand what's inside the virtual doc. It only knows the spatial layout: which byte ranges belong to which passages. Format understands the content: how to translate macros to JS, and how to reverse that translation when diagnostics come back.

This is a **dependency inversion**: Core defines the interface (`VirtualDocAdapter` trait), formats implement it. Core drives the pipeline; formats are adapters.

---

## 3. The @knot-passage Annotation Convention

### 3.1 Problem with Name-Based Wrapping

Using passage names as JavaScript function names requires normalization (spaces → underscores, stripping special characters). This is fragile:

| Passage Name | Normalized Function | Problem |
|---|---|---|
| `"My Passage"` | `My_Passage` | Ambiguous — could collide with `"My_Passage"` |
| `"My_Passage"` | `My_Passage` | **Collision** with above |
| `"passage with 🎮"` | Invalid JS identifier | Can't represent at all |

Twine passage names are required to be unique within a story (spec requirement), but normalization breaks this guarantee.

### 3.2 Annotation-Based Keying

Instead, each passage's wrapper function uses a sequential, meaningless name, with the real passage name embedded as a string annotation:

```js
/** @knot-passage "My Passage With Spaces" */
function __knot_0() {
    State.variables.x = 5;
}

/** @knot-passage "Another Passage" */
function __knot_1() {
    State.variables.y = 10;
}
```

- `__knot_N` names are sequential and meaningless — they are never used as lookup keys.
- The `@knot-passage` string literal is the **only mapping key**. It is the exact Twine passage name.
- No normalization. No collisions. Passage name uniqueness is guaranteed by the Twine spec.
- Core's index is built directly from the passage name string. No reverse-normalization table needed.

### 3.3 Annotation Format

```
/** @knot-passage "<passage_name>" */
```

- Must be a JSDoc-style comment on the line immediately preceding the function declaration.
- The passage name is enclosed in double quotes to handle spaces and special characters.
- The format adapter produces this annotation as part of the wrapper. Core reads it only for indexing (or more precisely, Core indexes by passage name from the workspace, not by parsing the annotation — but the annotation makes the mapping human-readable in the virtual doc).
- The annotation is a **convention** that all format adapters should follow, but Core does not parse it for mapping. Core's mapping comes from its `PassageEntry` index, which is built from workspace data, not from the virtual doc content.

---

## 4. VirtualDocAdapter Trait Specification

### 4.1 Location

The `VirtualDocAdapter` trait lives in `knot_core` (the `crates/core/` crate), not in `knot_formats`. This is the dependency inversion: Core defines the interface, formats implement it.

### 4.2 Trait Definition

```rust
/// Adapter trait that a format implements to provide virtual doc content
/// and interpret virtual doc diagnostics.
///
/// Core owns the virtual doc lifecycle (assembly, indexing, JS LSP integration,
/// refresh signaling). Format is an adapter that:
/// - Provides what goes INTO the virtual doc (passage JS content)
/// - Interprets what comes OUT of the virtual doc (JS diagnostics → .tw diagnostics)
///
/// ## Stateful Design
///
/// The adapter is stateful. It maintains internal reverse-mapping tables
/// built during `translate_passage()` and read during `resolve_source_location()`.
/// This makes reversal straightforward — the format that produced the translation
/// has all the context needed to reverse it.
///
/// ## Lifecycle
///
/// The adapter is a long-lived object. It persists alongside the VirtualDocManager.
/// When VirtualDocManager.rebuild() is called, the adapter clears its internal state.
/// When a single passage is surgically updated, the adapter updates just that
/// passage's reverse-mapping entry.
pub trait VirtualDocAdapter: Send + Sync {
    // ── Content Production (what goes IN) ─────────────────────────────

    /// Should this passage contribute to the virtual doc?
    ///
    /// Pure text passages with no variable-affecting content → false.
    /// [script] passages → true (they contain raw JS).
    /// Macro passages with variable content → true.
    /// Metadata passages (StoryData, StoryTitle) → false.
    ///
    /// The format decides what counts as "variable-affecting content".
    /// SugarCube checks for `<<set>>`, `<<run>>`, `$`, etc.
    /// Harlowe checks for `(set:)`, `(put:)`, `$`, etc.
    fn should_include_passage(&self, passage: &Passage) -> bool;

    /// Produce the wrapped JS block for a passage.
    ///
    /// For [script] passages: annotation + wrapper + raw JS body (no translation).
    /// For macro passages: annotation + wrapper + translated JS body.
    ///
    /// The format handles ALL content decisions:
    /// - Translation logic (macros → JS)
    /// - Wrapper function structure
    /// - State object initialization / preamble
    /// - Alias injection (e.g., `var g = gs()` for SugarCube)
    /// - @knot-passage annotation format
    /// - Ordering of statements within the function body
    ///
    /// Core concatenates these blocks into the monolithic virtual doc
    /// and tracks byte ranges per passage name. Core does NOT interpret
    /// the content — it just records spatial layout.
    fn translate_passage(
        &self,
        passage: &Passage,
        source_text: &dyn SourceTextProvider,
        context: &AdapterContext,
    ) -> Option<TranslatedBlock>;

    // ── Diagnostic Interpretation (what comes OUT) ────────────────────

    /// Reverse-map a JS diagnostic to its exact .tw source location.
    ///
    /// Core has already identified which passage the diagnostic falls in
    /// (via binary search on byte ranges). The format now does the precise
    /// reversal: given the virtual-doc byte range and the passage identity,
    /// find the exact byte range in the .tw source file.
    ///
    /// The format can account for:
    /// - Its own translation expansions (e.g., `<<set $x to 5>>` →
    ///   `State.variables.x = 5;` — different byte lengths)
    /// - Whitespace/newline differences between .tw source and translated JS
    /// - Body offset (passage header → body start in the .tw file)
    /// - Wrapper function overhead (annotation + function declaration bytes
    ///   that aren't part of the passage body)
    ///
    /// ## Wrapper Overhead Subtraction
    ///
    /// The format knows exactly how many bytes the annotation line,
    /// `function __knot_N() {`, and closing `}` take. It subtracts these
    /// to get back to the passage-internal offset, then maps that to
    /// the .tw source using its internal reverse-mapping tables.
    fn resolve_source_location(
        &self,
        passage_name: &str,
        file_uri: &str,
        vdoc_byte_range: Range<usize>,
        source_text: &str,
    ) -> SourceLocation;

    /// Interpret a JS diagnostic for format-specific meaning.
    ///
    /// Optional — the format can transform, filter, or enrich the diagnostic.
    /// For example, a "variable x not found" JS error could be rephrased as
    /// "$x is not initialized" for SugarCube.
    ///
    /// Return `None` to suppress the diagnostic entirely (e.g., false positives
    /// from the JS LSP about format-specific global objects).
    fn interpret_diagnostic(
        &self,
        js_diagnostic: &JsDiagnostic,
        passage_name: &str,
        file_uri: &str,
    ) -> Option<TwDiagnostic>;
}
```

### 4.3 Supporting Types

```rust
/// A single passage's contribution to the virtual doc.
pub struct TranslatedBlock {
    /// Complete JS function block (annotation + wrapper + body).
    /// Core appends this verbatim to the monolithic virtual doc.
    pub js_block: String,
}

/// Context provided to the adapter during translation.
pub struct AdapterContext {
    /// Startup aliases extracted from [script] passages.
    /// SugarCube: `var g = gs()`, `var s = State.variables`.
    /// Snowman: `var s = window.story.state`.
    /// Chapbook: `var s = state`.
    /// Harlowe: empty (no JS aliases).
    pub startup_aliases: Vec<StartupAlias>,

    /// User-defined callables (custom macros and widgets).
    /// SugarCube: `Macro.add('name', ...)` and `<<widget name>>`.
    pub user_callables: Vec<UserCallable>,

    /// Sequential index for the wrapper function name (__knot_N).
    /// Core provides this so the format doesn't have to track numbering.
    pub function_index: usize,
}

/// A resolved source location in a .tw file.
pub struct SourceLocation {
    /// The file URI where the diagnostic should be reported.
    pub file_uri: String,
    /// The byte range within the .tw file.
    pub byte_range: Range<usize>,
}

/// A raw JS diagnostic from the JS LSP.
pub struct JsDiagnostic {
    /// Byte range within the virtual doc where the issue was found.
    pub byte_range: Range<usize>,
    /// The diagnostic message from the JS LSP.
    pub message: String,
    /// The severity (error, warning, info, hint).
    pub severity: DiagnosticSeverity,
    /// The diagnostic code, if any.
    pub code: Option<String>,
}

/// A format-interpreted diagnostic for a .tw file.
pub struct TwDiagnostic {
    /// The file URI where the diagnostic should be reported.
    pub file_uri: String,
    /// The byte range within the .tw file.
    pub byte_range: Range<usize>,
    /// The (possibly format-specific) diagnostic message.
    pub message: String,
    /// The severity.
    pub severity: DiagnosticSeverity,
    /// The diagnostic code, if any.
    pub code: Option<String>,
}
```

---

## 5. Core's VirtualDocManager Specification

### 5.1 Location

`VirtualDocManager` lives in `knot_core` (the `crates/core/` crate), in a new module `crates/core/src/virtual_doc.rs`.

### 5.2 Data Structures

```rust
/// Tracks where a passage's content lives in the virtual doc.
struct PassageEntry {
    /// The exact Twine passage name (the mapping key).
    /// Matches the @knot-passage annotation string.
    passage_name: String,

    /// The file URI where this passage lives.
    file_uri: String,

    /// Byte range within the assembled virtual doc.
    /// entry.byte_range.start is the first byte of the @knot-passage annotation.
    /// entry.byte_range.end is one past the closing `}` of the wrapper function.
    byte_range: Range<usize>,
}

/// Core's virtual document manager. Owns the JS virtual doc lifecycle.
pub struct VirtualDocManager {
    /// The assembled monolithic virtual document content.
    content: String,

    /// Ordered list of passage entries, in the same order they appear
    /// in the virtual doc. Used for binary search on byte ranges.
    entries: Vec<PassageEntry>,

    /// Quick lookup: passage_name → index in entries.
    name_index: HashMap<String, usize>,

    /// Whether the JS LSP has been initialized for this virtual doc.
    lsp_initialized: bool,
}
```

### 5.3 Key Methods

```rust
impl VirtualDocManager {
    /// Rebuild the entire virtual doc from the workspace.
    ///
    /// Called on initial load or major structural change (format change,
    /// workspace re-index). Iterates all passages in the workspace,
    /// calls adapter.should_include_passage() and adapter.translate_passage(),
    /// concatenates the blocks into the monolithic content, and builds
    /// the entry index.
    ///
    /// This also triggers the adapter to clear and rebuild its internal
    /// reverse-mapping state.
    pub fn rebuild(
        &mut self,
        workspace: &Workspace,
        source_text: &dyn SourceTextProvider,
        adapter: &dyn VirtualDocAdapter,
    ) { ... }

    /// Surgically update a single passage in the virtual doc.
    ///
    /// Called on did_change — avoids full rebuild. Steps:
    /// 1. Call adapter.translate_passage() for the changed passage.
    /// 2. Compute the byte-length delta (new block vs old block).
    /// 3. Replace the passage's byte range in the content string.
    /// 4. Adjust byte ranges of all subsequent passages by the delta.
    /// 5. Update the entry in entries and name_index.
    ///
    /// If the passage was not previously in the virtual doc (e.g., user
    /// added a <<set>> to a previously pure-text passage), insert the
    /// new block at the appropriate position and shift subsequent entries.
    ///
    /// If the passage should no longer be in the virtual doc (e.g., user
    /// removed all macros), remove its block and shift subsequent entries.
    pub fn update_passage(
        &mut self,
        passage_name: &str,
        workspace: &Workspace,
        source_text: &dyn SourceTextProvider,
        adapter: &dyn VirtualDocAdapter,
    ) { ... }

    /// Remove a passage from the virtual doc.
    ///
    /// Called when a passage is deleted from the workspace.
    /// Removes the block, shifts subsequent entries, updates the index.
    pub fn remove_passage(&mut self, passage_name: &str) { ... }

    /// Get the current virtual doc content.
    ///
    /// Used for:
    /// - Sending to the JS LSP as a workspace file
    /// - Serving the knot/virtualDoc LSP request to VSCode
    pub fn content(&self) -> &str { &self.content }

    /// Resolve a virtual-doc byte range to a passage identity.
    ///
    /// This is Core's half of the diagnostic resolution. Given a byte range
    /// from a JS LSP diagnostic, binary search the entries to find which
    /// passage it belongs to.
    ///
    /// Returns (passage_name, file_uri, vdoc_byte_range) — the format adapter
    /// then handles the precise reversal via resolve_source_location().
    pub fn find_passage_for_byte_range(
        &self,
        byte_range: Range<usize>,
    ) -> Option<(&str, &str, Range<usize>)> { ... }

    /// Get the list of passage names in the virtual doc (for the
    /// knot/virtualDoc LSP response's passage_names field).
    pub fn passage_names(&self) -> Vec<&str> { ... }

    /// Check if the virtual doc is empty (no passages included).
    pub fn is_empty(&self) -> bool { ... }
}
```

---

## 6. Diagnostic Relay Pipeline

### 6.1 Two-Stage Mapping

The diagnostic relay has two distinct mapping stages with a clean boundary:

```
JS LSP diagnostic (byte span in virtual doc)
    │
    ▼  Stage 1: Core — "Which passage? Which file?"
    │   Binary search entries → (passage_name, file_uri)
    │   Core's job is done here.
    │
    ▼  Stage 2: Format — "Where exactly? What does it mean?"
    │   adapter.resolve_source_location() → exact .tw byte range
    │   adapter.interpret_diagnostic() → format-specific message
    │
    ▼  .tw diagnostic published to VSCode
```

### 6.2 Why This Boundary

**Core never needs to understand format-specific passage structure.** Core doesn't know about `body_offset`, SugarCube's `ExactLineMapping`, Harlowe's different syntax, or how whitespace maps between source and translation. It only knows spatial layout: byte ranges and passage names.

**Format never needs to understand virtual doc byte layout.** The format doesn't need to know how many passages are in the virtual doc, where other passages' blocks start, or how the monolithic file is structured. It receives the passage name and the virtual-doc byte range, and uses its internal reverse-mapping tables to find the exact .tw source location.

**Each stage is independently testable.** Core's binary search can be unit-tested with synthetic entries. Format's reversal can be unit-tested with synthetic translations.

### 6.3 Complete Flow

The diagnostic relay involves both client and server components. VSCode's built-in JS service runs client-side and cannot be directly driven by the server. Instead, the client relays JS diagnostics to the server for processing.

```
1. .tw file edit
   │
   ▼  sync.rs did_change → Core's refresh pipeline
   │
   ▼  Core detects which passages changed
   │
   ▼  For each changed passage:
   │   ├─ adapter.should_include_passage()? → skip or include
   │   └─ adapter.translate_passage() → TranslatedBlock
   │       (format builds/updates internal reverse map as side effect)
   │
   ▼  Core: VirtualDocManager.update_passage()
   │   - Replace byte range in monolithic content
   │   - Adjust subsequent passages' byte ranges
   │   - Update entry index
   │
   ▼  Core: send knot/refreshVirtualDoc notification to client
   │
   ▼  Client: refreshVirtualDoc() → fetch content via knot/virtualDoc
   │   → content provider fires onDidChange → VSCode JS service re-validates
   │
   ▼  Client: VSCode JS service produces diagnostics on knot-vdoc:// URI
   │
   ▼  Client: KnotVirtualDocDiagnostics.onDiagnosticsChanged()
   │   → relay diagnostics to server via knot/jsDiagnostics notification
   │
   ▼  Server: handle_js_diagnostics()
   │   ├─ Convert line/char positions to byte offsets
   │   ├─ Stage 1: VirtualDocManager.find_passage_for_byte_range()
   │   │   → (passage_name, file_uri, vdoc_byte_range)
   │   ├─ Stage 2: adapter.resolve_source_location() → exact .tw byte range
   │   └─ adapter.interpret_diagnostic() → format-specific message/filter
   │
   ▼  Server: publish .tw diagnostics via textDocument/publishDiagnostics
```

This design ensures:
- **Single diagnostic source:** All `.tw` diagnostics come from the server via `textDocument/publishDiagnostics`. No conflicting client-side DiagnosticCollection.
- **Full adapter precision:** The server has access to the adapter's reverse-mapping tables for byte-level accuracy and format-aware filtering.
- **Thin client:** The client is a pure relay — it detects JS diagnostics and forwards them without interpretation.

---

## 7. Stateful Adapter Design

### 7.1 Why Stateful

The adapter is stateful because it needs to reverse its own translations. The reverse-mapping data (byte offsets, exact line mappings, body offsets) is naturally produced during `translate_passage()` and consumed during `resolve_source_location()`. Storing this in the adapter is the simplest and most correct approach.

### 7.2 Memory Cost

For a SugarCube passage with 50 translated JS lines:

| Field | Type | Size |
|-------|------|------|
| `body_offset` | `usize` | 8 bytes |
| `ExactLineMapping × 50` | `(original_start_byte, original_end_byte, original_line)` each | ~20 bytes × 50 = 1,000 bytes |
| **Total per passage** | | **~1 KB** |

For a project with 500 passages containing macros: **~500 KB**. This is negligible for a desktop VSCode extension. The Workspace struct already holds every passage's full text in memory — that's orders of magnitude more.

### 7.3 Lazy Optimization (Future)

If memory becomes a concern, the adapter can use lazy reverse-map construction:

```rust
struct SugarCubeAdapter {
    /// Cheap, always stored — just the byte offset where the passage body starts.
    body_offsets: HashMap<String, usize>,

    /// Expensive, computed lazily — only for passages that get diagnostics.
    reverse_maps: HashMap<String, Vec<ExactLineMapping>>,
}
```

- On `translate_passage()`: store `body_offset`, skip building the full reverse map.
- On `resolve_source_location()`: if the passage isn't in `reverse_maps`, re-translate just that passage to build the mapping on the fly, then cache it.

This doesn't change the interface — it's purely an internal optimization.

---

## 8. Script Passages

### 8.1 Same Treatment as Macro Passages

[script] passages are wrapped in the same `@knot-passage` + `function __knot_N()` structure as macro passages. The only difference: the body is raw JS, not translated.

```
/** @knot-passage "StoryInit" */
function __knot_0() {
    var g = gs();
    State.variables.player = { name: "Ada" };
}

/** @knot-passage "Forest Clearing" */
function __knot_1() {
    State.variables.x = 5;
    if (State.variables.hasTorch === true) {
        // translated from <<if $hasTorch eq true>>
    }
}
```

### 8.2 Why Wrap Script Passages

1. **Uniform indexing.** Core's `PassageEntry` index treats all passages identically. No special cases for script vs macro.
2. **Uniform diagnostic resolution.** The binary search works the same way regardless of passage type.
3. **Scope isolation.** Wrapping in a function means script passage variables don't leak into the global scope of the virtual doc. Each passage's contribution is self-contained.
4. **The format decides what goes inside.** If a format needs to inject state initialization or alias setup at the top of a script passage's wrapper, it can. Core doesn't care — it just concatenates.

### 8.3 Preamble

The state object initialization (e.g., `State.variables = {}`) is not passage-specific. It's a preamble that the format provides once at the top of the virtual doc. This can be handled in one of two ways:

**Option A: Format provides a special `translate_preamble()` method.** Core calls this once during rebuild to get the preamble content, which it prepends to the monolithic doc before any passage blocks.

**Option B: Format includes the preamble in the first passage's block.** The format knows when it's producing the first block and can prepend the preamble.

Option A is cleaner because it keeps preamble separate from passage content. Core can track the preamble's byte range separately (it maps to no passage — diagnostics in the preamble are ignored or reported as internal).

---

## 9. Surgical Passage Updates

### 9.1 The Problem

When the user edits a passage in a .tw file, the virtual doc needs to be updated. A full rebuild (re-translating every passage) is expensive and unnecessary for a single passage change.

### 9.2 Surgical Update Algorithm

```rust
fn update_passage(
    &mut self,
    passage_name: &str,
    workspace: &Workspace,
    source_text: &dyn SourceTextProvider,
    adapter: &dyn VirtualDocAdapter,
) {
    let passage = workspace.find_passage(passage_name);
    let should_include = passage.map_or(false, |p| adapter.should_include_passage(p));

    if let Some(idx) = self.name_index.get(passage_name) {
        // Passage already exists in the virtual doc
        if should_include {
            // UPDATE: replace the existing block
            let old_range = self.entries[*idx].byte_range.clone();
            let new_block = adapter.translate_passage(passage.unwrap(), source_text, &context);
            let new_len = new_block.js_block.len();
            let delta = new_len as i64 - (old_range.end - old_range.start) as i64;

            // Replace the byte range in the content string
            self.content.replace_range(old_range.clone(), &new_block.js_block);

            // Adjust subsequent entries' byte ranges
            let offset_delta = delta;
            for entry in &mut self.entries[idx + 1..] {
                entry.byte_range.start = (entry.byte_range.start as i64 + offset_delta) as usize;
                entry.byte_range.end = (entry.byte_range.end as i64 + offset_delta) as usize;
            }

            // Update this entry's end
            self.entries[*idx].byte_range.end = 
                (old_range.start as i64 + new_len as i64) as usize;
        } else {
            // REMOVE: passage no longer has variable content
            self.remove_passage(passage_name);
        }
    } else if should_include {
        // INSERT: passage newly has variable content
        // Find the correct position (after the last existing passage in the
        // same file, or at the end of the virtual doc)
        let new_block = adapter.translate_passage(passage.unwrap(), source_text, &context);
        // ... insert at position, shift subsequent entries
    }
}
```

### 9.3 Triggering Surgical Updates

The existing refresh pipeline in `sync.rs` already detects which passages changed when a .tw file is edited. The change is:

1. **Before:** After detecting changed passages, rebuild the graph, publish diagnostics, and call `send_virtual_doc_refresh()`.
2. **After:** After detecting changed passages, also call `VirtualDocManager::update_passage()` for each changed passage, then push the updated content to the JS LSP workspace file, then call `send_virtual_doc_refresh()`.

---

## 10. JS Validation Integration

### 10.1 Key Insight: VSCode's JS Service Is Client-Side

VSCode's built-in JavaScript/TypeScript language service runs inside the **extension host process** — it is not a separate LSP server that the Knot language server can directly interact with. The JS service validates files that VSCode has open, including virtual documents served by the `knot-vdoc://` URI scheme.

This means:

1. **The server cannot directly drive the JS service.** There is no `JsLspClient` to send `textDocument/didOpen` or `textDocument/didChange` to. The JS service is controlled by VSCode, not by Knot.
2. **The server cannot directly consume JS diagnostics.** JS diagnostics are produced by VSCode's built-in service and appear as `vscode.Diagnostic` objects on the client side. The server never sees them natively.
3. **A client-to-server relay is needed.** The client must detect JS diagnostics on the virtual doc and **relay** them to the server, which then runs the diagnostic relay pipeline (binary search → adapter resolution → format interpretation) and publishes `.tw` diagnostics via `textDocument/publishDiagnostics`.

### 10.2 Why Not Client-Side Diagnostic Routing?

The current implementation routes JS diagnostics entirely on the client side (in `KnotVirtualDocDiagnostics`). This approach has several problems:

1. **Conflicting diagnostic sources.** The server publishes `.tw` diagnostics via `textDocument/publishDiagnostics` (graph analysis, format diagnostics). The client publishes `.tw` diagnostics via a separate `knot-virtual-doc` DiagnosticCollection. Both appear on the same `.tw` files but come from different pipelines with different lifecycle semantics. This creates confusion: stale client-side diagnostics may persist after the server has already cleared its diagnostics, or vice versa.

2. **Lossy reverse-mapping.** The client-side routing uses `KnotVirtualDocLineEntry` (which carries `original_line: u32`) — line-level granularity only. The adapter's `resolve_source_location()` can provide byte-level precision, but that precision is lost if the client does the mapping.

3. **Duplicate logic.** The client must replicate reverse-mapping logic that the adapter already has (and does better). The `findPassageBodyLine()` method in `KnotVirtualDocDiagnostics` is a rough approximation; the adapter's `ExactLineMapping` tables are exact.

4. **No format-aware filtering.** The client cannot filter false positives (e.g., "State is not defined" in SugarCube) because it doesn't know the format's runtime globals. The adapter's `interpret_diagnostic()` handles this.

5. **Thin client principle.** Complex diagnostic processing belongs on the server side, where it can be shared across all clients (VSCode, Neovim, Emacs) without reimplementation.

### 10.3 Client-to-Server Diagnostic Relay

The relay mechanism works as follows:

**Client side:**
1. The `knot-vdoc://` content provider serves the virtual doc to VSCode.
2. VSCode's built-in JS service validates the virtual doc and produces diagnostics.
3. A **thin listener** (`KnotVirtualDocDiagnostics`) detects JS diagnostics on the virtual doc URI.
4. Instead of routing them locally, the listener **relays** each diagnostic to the server via a custom notification: `knot/jsDiagnostics`.
5. The client does NOT maintain its own `knot-virtual-doc` DiagnosticCollection. It is a pure relay.

**Server side:**
1. The server receives `knot/jsDiagnostics` notifications.
2. For each JS diagnostic, the server runs the two-stage diagnostic relay:
   - Stage 1: `VirtualDocManager::find_passage_for_byte_range()` → passage identity
   - Stage 2: `adapter.resolve_source_location()` → exact `.tw` byte range, `adapter.interpret_diagnostic()` → format-specific message
3. The server publishes `.tw` diagnostics via `textDocument/publishDiagnostics`, alongside its existing graph/format diagnostics.
4. This ensures a **single diagnostic source** per `.tw` file: the server.

### 10.4 Custom Notification: `knot/jsDiagnostics`

**Direction:** Client → Server (notification, no response)

**Parameters:**

```typescript
interface KnotJsDiagnosticsParams {
    /** The URI of the virtual doc (always knot-vdoc://workspace/virtual-doc.js). */
    uri: string;
    /** JS diagnostics from VSCode's built-in JS service. */
    diagnostics: KnotJsDiagnostic[];
}

interface KnotJsDiagnostic {
    /** 0-based line number in the virtual doc where the diagnostic starts. */
    start_line: number;
    /** 0-based character offset on the start line. */
    start_character: number;
    /** 0-based line number in the virtual doc where the diagnostic ends. */
    end_line: number;
    /** 0-based character offset on the end line. */
    end_character: number;
    /** The diagnostic message from the JS service. */
    message: string;
    /** Severity: 1=Error, 2=Warning, 3=Info, 4=Hint. */
    severity: number;
    /** The diagnostic code, if any (e.g., TS error code like "2304"). */
    code?: string | number;
}
```

**Why line+character instead of byte offsets?** VSCode's diagnostic API uses `Position` (line/character), not byte offsets. The server can convert these to byte offsets using its virtual doc content, which it already has. This avoids the client needing to compute byte offsets.

### 10.5 Server-Side Diagnostic Relay Handler

When the server receives a `knot/jsDiagnostics` notification:

```rust
async fn handle_js_diagnostics(
    state: &ServerState,
    params: KnotJsDiagnosticsParams,
) {
    let inner = state.inner.read().await;
    
    // 1. Convert line/character positions to byte offsets using the virtual doc content
    let vdoc_content = inner.virtual_doc_manager.content();
    
    // 2. For each JS diagnostic, run the two-stage relay
    let mut tw_diagnostics: HashMap<Url, Vec<Diagnostic>> = HashMap::new();
    
    for js_diag in &params.diagnostics {
        // Convert line/char → byte offset
        let byte_range = line_char_to_byte_range(vdoc_content, js_diag.start_line, js_diag.start_character, js_diag.end_line, js_diag.end_character);
        
        // Stage 1: Find which passage this falls in
        let (passage_name, file_uri, vdoc_range) = match inner.virtual_doc_manager.find_passage_for_byte_range(byte_range) {
            Some(result) => result,
            None => continue, // Preamble or gap — skip
        };
        
        // Build JsDiagnostic for the adapter
        let js_diagnostic = JsDiagnostic {
            byte_range: vdoc_range,
            message: js_diag.message.clone(),
            severity: convert_severity(js_diag.severity),
            code: js_diag.code.map(|c| c.to_string()),
        };
        
        // Stage 2: Format-specific resolution and interpretation
        if let Some(ref adapter) = inner.virtual_doc_adapter {
            let source_text = CoreDocumentCache(&inner.open_documents);
            let source_str = source_text.get_source_text(file_uri).unwrap_or("");
            
            let source_location = adapter.resolve_source_location(
                passage_name, file_uri, js_diagnostic.byte_range.clone(), source_str,
            );
            
            if let Some(tw_diag) = adapter.interpret_diagnostic(&js_diagnostic, passage_name, file_uri) {
                // Convert byte range to LSP Position using the .tw source text
                let source_text = inner.open_documents.get(&Url::parse(file_uri).ok()).map(|s| s.as_str()).unwrap_or("");
                let range = byte_range_to_lsp_range(source_text, tw_diag.byte_range);
                
                let lsp_diag = Diagnostic {
                    range,
                    severity: Some(convert_tw_severity(tw_diag.severity)),
                    message: tw_diag.message,
                    source: Some("knot (virtual doc)".to_string()),
                    ..Default::default()
                };
                
                if let Ok(uri) = Url::parse(&tw_diag.file_uri) {
                    tw_diagnostics.entry(uri).or_default().push(lsp_diag);
                }
            }
        }
    }
    
    // 3. Publish the .tw diagnostics via the standard LSP mechanism
    for (uri, diags) in tw_diagnostics {
        state.client.publish_diagnostics(uri, diags, None).await;
    }
}
```

### 10.6 What Gets Removed from the Client

After the relay is implemented, the client-side `KnotVirtualDocDiagnostics` class is simplified to a **thin relay**:

- **Remove:** The `knot-virtual-doc` DiagnosticCollection (no more client-side diagnostic publishing)
- **Remove:** The `findPassageBodyLine()` method (no more client-side reverse mapping)
- **Remove:** The `twDiagnostics` Map and all deduplication logic
- **Keep:** The `onDiagnosticsChanged()` listener, but refactored to relay diagnostics to the server via `knot/jsDiagnostics` instead of publishing locally
- **Keep:** The `knot-vdoc://` content provider and `refreshVirtualDoc()` (these still serve the virtual doc content to VSCode's JS service)

### 10.7 Throttling and Debouncing

JS diagnostics can arrive in bursts (e.g., after a full virtual doc refresh, VSCode's JS service re-validates the entire file). The client should debounce the relay:

1. Collect diagnostics for 300ms after the first diagnostic appears.
2. Send a single `knot/jsDiagnostics` notification with all collected diagnostics.
3. If the virtual doc is refreshed (new content), clear the pending batch and start fresh.

The server should also be prepared to receive multiple `knot/jsDiagnostics` notifications in quick succession and deduplicate `.tw` diagnostics appropriately.

### 10.8 Virtual Doc Content Delivery

The virtual doc is still served to VSCode via the `knot-vdoc://` URI scheme content provider. The server does NOT need to directly push content to a JS LSP — it just needs to:

1. Maintain the virtual doc content in `VirtualDocManager`.
2. Serve it to the client via `knot/virtualDoc` on request.
3. Notify the client to refresh via `knot/refreshVirtualDoc` when content changes.

VSCode's JS service automatically re-validates the virtual doc whenever the content provider fires its `onDidChange` event.

---

## 11. Refresh Pipeline Integration

### 11.1 Current Refresh Mechanism

The current refresh pipeline works as follows:

1. `.tw` file edit → `sync.rs::did_change()`
2. Re-parse the changed document
3. Rebuild the graph
4. Publish diagnostics
5. Call `send_virtual_doc_refresh()` → client fetches via `knot/virtualDoc`

### 11.2 Enhanced Pipeline

The enhanced pipeline adds virtual doc management and the client-to-server diagnostic relay:

1. `.tw` file edit → `sync.rs::did_change()`
2. Re-parse the changed document
3. **Detect which passages changed** (via diff of old vs new parse result)
4. **For each changed passage:** `VirtualDocManager::update_passage()`
5. Rebuild the graph
6. Publish diagnostics (graph-based and format-based)
7. **Send `knot/refreshVirtualDoc`** → client fetches content → VSCode JS service re-validates
8. **Client relays JS diagnostics** → `knot/jsDiagnostics` notification → server
9. **Server runs diagnostic relay** → `find_passage_for_byte_range()` → `adapter.resolve_source_location()` → `adapter.interpret_diagnostic()`
10. **Server publishes JS-LSP-based `.tw` diagnostics** via `textDocument/publishDiagnostics`

---

## 12. Migration Plan

### 12.1 What Gets Deleted

| Current Code | Location | Reason |
|---|---|---|
| `VirtualDocHooks` trait | `formats/src/virtual_doc.rs` | Replaced by `VirtualDocAdapter` in core |
| `build_core_virtual_document()` | `formats/src/virtual_doc.rs` | Replaced by `VirtualDocManager::rebuild()` |
| `build_format_section_line_map()` | `formats/src/virtual_doc.rs` | Replaced by adapter's internal reverse mapping |
| `FormatPlugin::virtual_doc_content()` | `formats/src/plugin.rs` | Replaced by `VirtualDocManager::content()` |
| `FormatPlugin::virtual_doc_line_map()` | `formats/src/plugin.rs` | Replaced by diagnostic relay pipeline |
| `VirtualDocLineMapEntry` type | `formats/src/types.rs` | Replaced by `PassageEntry` in core |
| `VirtualDocument` / `VirtualSection` / `LineMapping` | `formats/src/types.rs` | Replaced by `TranslatedBlock` / `PassageEntry` |
| Blanket impl `FormatPlugin → VirtualDocHooks` | `formats/src/plugin.rs` | No longer needed |
| `KnotVirtualDocLineEntry` (LSP wire type) | `server/src/lsp_ext.rs` | Replaced by passage-name-based response |
| `knot-virtual-doc` DiagnosticCollection | `vscode/src/virtualDocProvider.ts` | Replaced by server-side diagnostic relay |
| `findPassageBodyLine()` method | `vscode/src/virtualDocProvider.ts` | Reverse mapping now done server-side |
| Client-side diagnostic publishing logic | `vscode/src/virtualDocProvider.ts` | Replaced by `knot/jsDiagnostics` relay |

### 12.2 What Gets Moved

| Current Code | From | To | Change |
|---|---|---|---|
| SugarCube's translation logic | `formats/src/sugarcube/walk_translate.rs` | Stays in formats | Implement `VirtualDocAdapter::translate_passage()` |
| SugarCube's `VirtualDocMap` | `formats/src/sugarcube/virtual_doc_map.rs` | Stays in formats | Becomes adapter's internal state |
| SugarCube's `body_offset` computation | `formats/src/sugarcube/mod.rs` | Stays in formats | Used in `resolve_source_location()` |

### 12.3 What Gets Created

| New Code | Location | Purpose |
|---|---|---|
| `VirtualDocAdapter` trait | `core/src/virtual_doc.rs` | The format adapter interface |
| `VirtualDocManager` | `core/src/virtual_doc.rs` | Core's virtual doc lifecycle manager |
| `PassageEntry` | `core/src/virtual_doc.rs` | Byte-range index entry |
| `TranslatedBlock` | `core/src/virtual_doc.rs` | Format's output per passage |
| `AdapterContext` | `core/src/virtual_doc.rs` | Context passed to adapter |
| `JsDiagnostic`, `TwDiagnostic`, `SourceLocation` | `core/src/virtual_doc.rs` | Diagnostic types |
| `SugarCubeAdapter` | `formats/src/sugarcube/adapter.rs` | SugarCube's `VirtualDocAdapter` impl |
| Harlowe/Chapbook/Snowman stub adapters | `formats/src/{format}/adapter.rs` | Stub implementations (return `None`) |
| `knot/jsDiagnostics` notification | `server/src/lsp_ext.rs` | Client → server JS diagnostic relay |
| `KnotJsDiagnostic` wire type | `server/src/lsp_ext.rs` | JS diagnostic from client |
| `handle_js_diagnostics()` | `server/src/handlers/` | Server-side diagnostic relay handler |
| `line_char_to_byte_range()` helper | `server/src/handlers/helpers/` | Position → byte offset conversion |
| `byte_range_to_lsp_range()` helper | `server/src/handlers/helpers/` | Byte offset → LSP Range conversion |
| Refactored `KnotVirtualDocDiagnostics` | `vscode/src/virtualDocProvider.ts` | Thin relay (sends `knot/jsDiagnostics` instead of publishing locally) |

### 12.4 Migration Order

1. **Create `VirtualDocAdapter` trait and `VirtualDocManager` in core.** New code, no existing code affected.
2. **Create `SugarCubeAdapter`** that wraps existing SugarCube translation logic. It implements `VirtualDocAdapter` by delegating to the existing `walk_translate` and `VirtualDocMap` code.
3. **Wire `VirtualDocManager` into server state.** Add `virtual_doc_manager: VirtualDocManager` to `ServerStateInner`.
4. **Update `knot_virtual_doc` handler** to query `VirtualDocManager` instead of `FormatPlugin`.
5. **Update refresh pipeline** to call `VirtualDocManager::update_passage()` on changes.
6. **Implement client-to-server diagnostic relay.** Add `knot/jsDiagnostics` custom notification (client → server). Add server-side handler that runs the two-stage diagnostic relay (binary search + adapter resolution). Refactor client-side `KnotVirtualDocDiagnostics` to relay diagnostics instead of publishing locally. Remove the `knot-virtual-doc` DiagnosticCollection from the client.
7. **Implement server-side diagnostic relay pipeline.** Add `line_char_to_byte_range()` helper, `byte_range_to_lsp_range()` helper, severity conversion, and the full `handle_js_diagnostics()` handler. Integrate with `publish_all_diagnostics` so JS-LSP-based diagnostics are published alongside graph/format diagnostics.
8. **Remove old code** (VirtualDocHooks, FormatPlugin virtual doc methods, old types, client-side `findPassageBodyLine`, client-side DiagnosticCollection).
9. **Implement byte-span propagation** on the clean foundation (phase 2, referencing the zip files).

---

## 13. Byte-Span Propagation (Phase 2)

### 13.1 Current State

Byte-span data exists in SugarCube's `ExactLineMapping` (`original_start_byte`, `original_end_byte`) but is currently dropped at the `VirtualDocLineMapEntry` conversion boundary, which only carries `original_line: u32`.

### 13.2 How the New Architecture Enables Byte-Span

With the `VirtualDocAdapter` design, byte-span propagation is straightforward:

1. **Adapter's `translate_passage()`** builds `ExactLineMapping` entries as internal state (as SugarCube already does).
2. **Adapter's `resolve_source_location()`** receives the virtual-doc byte range, subtracts wrapper overhead, uses the reverse-mapping table to find the exact .tw byte range.
3. **No precision is lost** because the reverse mapping carries full byte offsets, not just line numbers.

The previous byte-span work (saved in the reference zips) can be re-implemented on this clean foundation. The key difference: instead of threading byte offsets through `VirtualDocLineMapEntry` → LSP wire type → VSCode client, the bytes stay server-side in the adapter's internal state and are resolved directly to .tw byte ranges.

### 13.3 What the Reference Zips Contained

- `knot-repo-byte-span-changes.zip` (97K): 10 modified files adding byte-span fields to `VirtualDocLineMapEntry`, `KnotVirtualDocLineEntry`, and the VSCode TypeScript types.
- `knot-ver2-byte-span-changes.zip` (64K): 8 modified files from an earlier iteration.

These are reference material for phase 2. The new architecture should produce a cleaner implementation.

---

## 14. Open Questions

1. **Preamble handling.** Should the format provide a separate `translate_preamble()` method, or should it embed the preamble in the first passage's block? A separate method is cleaner but adds another trait method.

2. **Passage ordering in the virtual doc.** What order should passages appear in? Current SugarCube puts script passages first, then macro passages. Should this be the format's decision (via `translate_passage` call order) or should Core enforce a convention?

3. **Diagnostic deduplication.** If the same .tw location gets diagnostics from both the graph analysis (uninitialized variable, broken link) and the JS service (type error, undefined variable), how should they be merged? Should the format adapter's `interpret_diagnostic()` have access to existing graph diagnostics to avoid duplicates?

4. **Multi-format workspaces.** The current architecture detects one format per workspace. If a workspace could contain multiple formats (unlikely but possible), the adapter pattern would need to support multiple simultaneous adapters.

5. **Diagnostic relay timing.** After `knot/refreshVirtualDoc`, VSCode's JS service may take time to re-validate. Should the server clear previous JS-derived `.tw` diagnostics immediately when the virtual doc changes, or wait for the new `knot/jsDiagnostics` batch to arrive? Clearing immediately avoids stale diagnostics but may cause a brief "flicker" (diags appear, disappear, re-appear).

6. **Client-side `line_map` in `knot/virtualDoc` response.** After the relay is implemented, the client no longer needs the `line_map` for diagnostic routing. Should we remove it from the response entirely, or keep it for other client-side features (e.g., "go to virtual doc line" navigation)?
