//! Virtual Document Architecture — Core-Infra, Format-Content.
//!
//! This module implements the new virtual document pipeline where **core owns
//! the infrastructure** (lifecycle, indexing, JS LSP integration, refresh
//! signaling) and **format owns the content** (what goes in, how to interpret
//! what comes out).
//!
//! ## Dependency Inversion
//!
//! Core defines the `VirtualDocAdapter` trait. Formats implement it. Core
//! drives the pipeline; formats are adapters. This inverts the current
//! architecture where formats own everything and core owns nothing.
//!
//! ## The @knot-passage Annotation Convention
//!
//! Each passage's wrapper function uses a sequential, meaningless name
//! (`__knot_N`) with the real passage name embedded as a string annotation:
//!
//! ```js
//! /** @knot-passage "My Passage With Spaces" */
//! function __knot_0() {
//!     State.variables.x = 5;
//! }
//! ```
//!
//! No normalization. No collisions. Passage name uniqueness is guaranteed
//! by the Twine spec.

use crate::passage::Passage;
use crate::workspace::Workspace;
use std::collections::HashMap;
use std::ops::Range;

// ---------------------------------------------------------------------------
// SourceTextProvider — moved to core so the adapter trait doesn't depend
// on knot_formats
// ---------------------------------------------------------------------------

/// A trait that provides source text for documents by URI.
///
/// The server implements this using its `open_documents` cache, making
/// document text available for byte-offset → line-number resolution.
/// Defined here in core so that `VirtualDocAdapter::translate_passage()`
/// can accept it without creating a core → formats dependency.
pub trait SourceTextProvider {
    /// Look up the source text of a document by its URI string.
    /// Returns `None` if the document is not available.
    fn get_source_text(&self, file_uri: &str) -> Option<&str>;
}

/// A no-op `SourceTextProvider` that always returns `None`.
pub struct NoSourceText;

impl SourceTextProvider for NoSourceText {
    fn get_source_text(&self, _file_uri: &str) -> Option<&str> {
        None
    }
}

// ---------------------------------------------------------------------------
// Diagnostic types
// ---------------------------------------------------------------------------

/// Severity levels for diagnostics, used by both JS and .tw diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

// ---------------------------------------------------------------------------
// Adapter supporting types
// ---------------------------------------------------------------------------

/// A startup alias extracted from [script] passages.
///
/// In SugarCube, `var g = gs()` creates an alias `g` that resolves to
/// `State.variables`. In Snowman, `var s = window.story.state` creates
/// an alias `s`. The adapter uses these to inject alias declarations
/// into the virtual doc's preamble or into individual passage wrappers.
#[derive(Debug, Clone)]
pub struct StartupAlias {
    /// The alias identifier (e.g., `g` for `var g = gs()`).
    pub alias_name: String,
    /// What this alias resolves to, as a JavaScript expression string.
    /// E.g., `"State.variables"` or `"window.story.state"`.
    pub resolves_to: String,
    /// The passage name where this alias is defined.
    pub defined_in: String,
}

/// The kind of a user-defined callable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCallableKind {
    /// Custom macro defined via `Macro.add('name', ...)`.
    CustomMacro,
    /// Widget defined via `<<widget name>>...<</widget>>`.
    Widget,
}

/// A user-defined callable (custom macro or widget) that can be invoked
/// like a function from macro passages.
#[derive(Debug, Clone)]
pub struct UserCallable {
    /// The callable name (e.g., "useItem" for `Macro.add('useItem', ...)`).
    pub name: String,
    /// The kind of callable (custom macro or widget).
    pub kind: UserCallableKind,
    /// Number of arguments this callable accepts, if known.
    pub arg_count: Option<usize>,
    /// The passage name where this callable is defined.
    pub defined_in: String,
    /// The file URI where this callable is defined.
    pub file_uri: String,
}

/// A single passage's contribution to the virtual doc.
#[derive(Debug, Clone)]
pub struct TranslatedBlock {
    /// Complete JS function block (annotation + wrapper + body).
    /// Core appends this verbatim to the monolithic virtual doc.
    pub js_block: String,
}

/// Context provided to the adapter during translation.
#[derive(Debug, Clone)]
pub struct AdapterContext {
    /// The file URI where the passage being translated lives.
    /// This is provided by the VirtualDocManager from the workspace index,
    /// so the adapter doesn't need to look it up.
    pub file_uri: String,

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
#[derive(Debug, Clone)]
pub struct SourceLocation {
    /// The file URI where the diagnostic should be reported.
    pub file_uri: String,
    /// The byte range within the .tw file.
    pub byte_range: Range<usize>,
}

/// A raw JS diagnostic from the JS LSP.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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

// ---------------------------------------------------------------------------
// VirtualDocAdapter trait
// ---------------------------------------------------------------------------

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
    /// (via binary search on byte ranges) and converted the absolute byte
    /// range to a **passage-relative** byte range (offset 0 = first byte
    /// of this passage's js_block). The format now does the precise
    /// reversal: given the passage-relative byte range and the passage
    /// identity, find the exact byte range in the .tw source file.
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

    /// Clear the adapter's internal reverse-mapping state.
    ///
    /// Called by `VirtualDocManager::rebuild()` before re-translating all
    /// passages. The adapter should discard all cached reverse-mapping data.
    fn clear_state(&self);

    /// Update the adapter's internal state for a single passage.
    ///
    /// Called by `VirtualDocManager::update_passage()` after a surgical
    /// update. The adapter should update just that passage's reverse-mapping
    /// entry. The default implementation is a no-op; stateful adapters
    /// (like SugarCube) should override this.
    fn invalidate_passage(&self, _passage_name: &str) {
        // Default: no-op. Stateful adapters override to clear per-passage data.
    }

    // ── Pre-translation Context Extraction ───────────────────────────

    /// Extract startup aliases from the workspace.
    ///
    /// Called by `VirtualDocManager::rebuild()` before the translation loop.
    /// The adapter should scan script passages for alias definitions
    /// (e.g., `var g = gs()` in SugarCube, `var s = window.story.state`
    /// in Snowman). These aliases are passed to each `translate_passage()`
    /// call via `AdapterContext::startup_aliases` so that macro passages
    /// can resolve alias references.
    ///
    /// Default: empty Vec (formats without JS aliases, like Harlowe).
    fn extract_startup_aliases(
        &self,
        _workspace: &Workspace,
        _source_text: &dyn SourceTextProvider,
    ) -> Vec<StartupAlias> {
        Vec::new()
    }

    /// Extract user-defined callables from the workspace.
    ///
    /// Called by `VirtualDocManager::rebuild()` before the translation loop.
    /// The adapter should scan for custom macro definitions (e.g.,
    /// SugarCube's `Macro.add('name', ...)`) and widget definitions
    /// (e.g., `<<widget name>>...<</widget>>`). These callables are
    /// passed to each `translate_passage()` call via `AdapterContext::user_callables`
    /// so that the translator can recognize invocations like
    /// `<<useItem matchbox>>` as function calls rather than unknown macros.
    ///
    /// Default: empty Vec (formats without user-defined callables).
    fn extract_user_callables(
        &self,
        _workspace: &Workspace,
        _source_text: &dyn SourceTextProvider,
    ) -> Vec<UserCallable> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// PassageEntry — byte-range index entry
// ---------------------------------------------------------------------------

/// Tracks where a passage's content lives in the virtual doc.
#[derive(Debug, Clone)]
pub struct PassageEntry {
    /// The exact Twine passage name (the mapping key).
    /// Matches the @knot-passage annotation string.
    pub passage_name: String,

    /// The file URI where this passage lives.
    pub file_uri: String,

    /// Byte range within the assembled virtual doc.
    /// entry.byte_range.start is the first byte of the @knot-passage annotation.
    /// entry.byte_range.end is one past the closing `}` of the wrapper function.
    pub byte_range: Range<usize>,
}

// ---------------------------------------------------------------------------
// VirtualDocManager
// ---------------------------------------------------------------------------

/// Core's virtual document manager. Owns the JS virtual doc lifecycle.
///
/// The manager maintains a monolithic virtual document string and a sorted
/// index of `PassageEntry` instances that map byte ranges to passages.
/// It supports both full rebuilds and surgical single-passage updates.
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

    /// Startup aliases extracted during the last rebuild.
    /// These are passed to `translate_passage()` via `AdapterContext`
    /// so that macro passages can resolve alias references.
    startup_aliases: Vec<StartupAlias>,

    /// User-defined callables extracted during the last rebuild.
    /// These are passed to `translate_passage()` via `AdapterContext`
    /// so that the translator can recognize custom macro/widget invocations.
    user_callables: Vec<UserCallable>,
}

impl Default for VirtualDocManager {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualDocManager {
    /// Create a new, empty VirtualDocManager.
    pub fn new() -> Self {
        Self {
            content: String::new(),
            entries: Vec::new(),
            name_index: HashMap::new(),
            lsp_initialized: false,
            startup_aliases: Vec::new(),
            user_callables: Vec::new(),
        }
    }

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
    ) {
        // Clear previous state
        self.content.clear();
        self.entries.clear();
        self.name_index.clear();
        self.lsp_initialized = false;

        // Tell the adapter to clear its internal reverse-mapping state
        adapter.clear_state();

        // Collect passages with their file URIs, compute what to include
        let mut included_passages: Vec<(String, String)> = Vec::new(); // (passage_name, file_uri)

        for doc in workspace.documents() {
            let file_uri = doc.uri.to_string();
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }
                if adapter.should_include_passage(passage) {
                    included_passages.push((passage.name.clone(), file_uri.clone()));
                }
            }
        }

        // Extract startup aliases and user callables from the workspace
        // via the adapter. This is the multi-phase approach: extract context
        // first, then use it during translation. The adapter knows how to
        // scan for format-specific patterns (SugarCube's `var g = gs()`,
        // `Macro.add(...)`, `<<widget>>`, etc.).
        self.startup_aliases = adapter.extract_startup_aliases(workspace, source_text);
        self.user_callables = adapter.extract_user_callables(workspace, source_text);

        // Translate each included passage and concatenate
        let mut function_index: usize = 0;
        for (passage_name, file_uri) in &included_passages {
            // Find the passage in the workspace
            let passage = match workspace.find_passage(passage_name) {
                Some((_, p)) => p.clone(),
                None => continue,
            };

            let context = AdapterContext {
                file_uri: file_uri.clone(),
                startup_aliases: self.startup_aliases.clone(),
                user_callables: self.user_callables.clone(),
                function_index,
            };

            let block = match adapter.translate_passage(&passage, source_text, &context) {
                Some(b) => b,
                None => continue,
            };

            // Record the byte range for this passage
            let start = self.content.len();
            self.content.push_str(&block.js_block);
            let end = self.content.len();

            let idx = self.entries.len();
            self.entries.push(PassageEntry {
                passage_name: passage_name.clone(),
                file_uri: file_uri.clone(),
                byte_range: start..end,
            });
            self.name_index.insert(passage_name.clone(), idx);

            function_index += 1;
        }
    }

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
    /// new block at the end and return `true`.
    ///
    /// If the passage should no longer be in the virtual doc (e.g., user
    /// removed all macros), remove its block and return `true`.
    ///
    /// Returns `true` if the virtual doc content changed.
    pub fn update_passage(
        &mut self,
        passage_name: &str,
        workspace: &Workspace,
        source_text: &dyn SourceTextProvider,
        adapter: &dyn VirtualDocAdapter,
    ) -> bool {
        let passage = match workspace.find_passage(passage_name) {
            Some((doc, p)) => (doc.uri.to_string(), p.clone()),
            None => return false,
        };
        let (file_uri, passage) = passage;

        let should_include = adapter.should_include_passage(&passage);

        if let Some(&idx) = self.name_index.get(passage_name) {
            // Passage already exists in the virtual doc
            if should_include {
                // UPDATE: replace the existing block
                let old_range = self.entries[idx].byte_range.clone();

                // Tell the adapter to invalidate its cached data for this passage
                adapter.invalidate_passage(passage_name);

                let context = AdapterContext {
                    file_uri: file_uri.clone(),
                    startup_aliases: self.startup_aliases.clone(),
                    user_callables: self.user_callables.clone(),
                    function_index: idx,
                };

                let new_block = match adapter.translate_passage(&passage, source_text, &context) {
                    Some(b) => b,
                    None => {
                        // Translation failed — remove the passage
                        self.remove_passage_at(idx);
                        return true;
                    }
                };

                let new_len = new_block.js_block.len();
                let old_len = old_range.end - old_range.start;
                let delta = new_len as i64 - old_len as i64;

                // Replace the byte range in the content string
                self.content.replace_range(old_range.clone(), &new_block.js_block);

                // Adjust subsequent entries' byte ranges
                if delta != 0 {
                    for entry in &mut self.entries[idx + 1..] {
                        entry.byte_range.start =
                            (entry.byte_range.start as i64 + delta) as usize;
                        entry.byte_range.end =
                            (entry.byte_range.end as i64 + delta) as usize;
                    }
                }

                // Update this entry's end
                self.entries[idx].byte_range.end = old_range.start + new_len;

                true
            } else {
                // REMOVE: passage no longer has variable content
                self.remove_passage_at(idx);
                true
            }
        } else if should_include {
            // INSERT: passage newly has variable content
            adapter.invalidate_passage(passage_name);

            let context = AdapterContext {
                file_uri: file_uri.clone(),
                startup_aliases: self.startup_aliases.clone(),
                user_callables: self.user_callables.clone(),
                function_index: self.entries.len(),
            };

            let new_block = match adapter.translate_passage(&passage, source_text, &context) {
                Some(b) => b,
                None => return false,
            };

            // Append at the end of the virtual doc
            let start = self.content.len();
            self.content.push_str(&new_block.js_block);
            let end = self.content.len();

            let idx = self.entries.len();
            self.entries.push(PassageEntry {
                passage_name: passage_name.to_string(),
                file_uri,
                byte_range: start..end,
            });
            self.name_index.insert(passage_name.to_string(), idx);

            true
        } else {
            // Passage shouldn't be included and isn't already — no change
            false
        }
    }

    /// Remove a passage from the virtual doc by name.
    ///
    /// Called when a passage is deleted from the workspace.
    /// Removes the block, shifts subsequent entries, updates the index.
    pub fn remove_passage(&mut self, passage_name: &str) -> bool {
        if let Some(&idx) = self.name_index.get(passage_name) {
            self.remove_passage_at(idx);
            true
        } else {
            false
        }
    }

    /// Internal: remove a passage at a given index and shift subsequent entries.
    fn remove_passage_at(&mut self, idx: usize) {
        let old_range = self.entries[idx].byte_range.clone();
        let removed_len = old_range.end - old_range.start;

        // Remove the byte range from the content string
        self.content.replace_range(old_range.clone(), "");

        // Rebuild the name_index and adjust subsequent entries
        let removed_name = self.entries[idx].passage_name.clone();
        self.name_index.remove(&removed_name);

        // Adjust subsequent entries' byte ranges
        for entry in &mut self.entries[idx + 1..] {
            entry.byte_range.start -= removed_len;
            entry.byte_range.end -= removed_len;
        }

        // Remove the entry
        self.entries.remove(idx);

        // Rebuild name_index for entries after the removed one
        for (new_idx, entry) in self.entries[idx..].iter().enumerate() {
            self.name_index.insert(entry.passage_name.clone(), idx + new_idx);
        }
    }

    /// Get the current virtual doc content.
    ///
    /// Used for:
    /// - Sending to the JS LSP as a workspace file
    /// - Serving the knot/virtualDoc LSP request to VSCode
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Resolve a virtual-doc byte range to a passage identity.
    ///
    /// This is Core's half of the diagnostic resolution. Given a byte range
    /// from a JS LSP diagnostic, binary search the entries to find which
    /// passage it belongs to.
    ///
    /// Returns (passage_name, file_uri, passage_relative_byte_range) — the
    /// format adapter then handles the precise reversal via resolve_source_location().
    ///
    /// The returned byte range is **passage-relative**: offset 0 is the first
    /// byte of the passage's js_block within the virtual doc. This matches
    /// the adapter's internal reverse-mapping tables, which are built relative
    /// to the passage's own js_block.
    pub fn find_passage_for_byte_range(
        &self,
        byte_range: Range<usize>,
    ) -> Option<(&str, &str, Range<usize>)> {
        // Binary search: find the entry whose byte_range contains
        // the start of the given byte_range.
        let target = byte_range.start;

        // Entries are sorted by byte_range.start (they appear in document order).
        // Find the last entry whose start <= target.
        let mut lo = 0usize;
        let mut hi = self.entries.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.entries[mid].byte_range.start <= target {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        // lo is now one past the last entry with start <= target.
        if lo == 0 {
            return None; // Target is before the first entry
        }

        let idx = lo - 1;
        let entry = &self.entries[idx];

        // Check that the target is within this entry's range
        if target < entry.byte_range.end {
            // Convert the absolute vdoc byte range to passage-relative.
            // The adapter's reverse-mapping tables are built relative to the
            // passage's own js_block (starting from byte 0 of the block).
            let passage_start = entry.byte_range.start;
            let relative_range =
                (byte_range.start - passage_start)..(byte_range.end - passage_start);
            Some((&entry.passage_name, &entry.file_uri, relative_range))
        } else {
            None // Target is in a gap between entries
        }
    }

    /// Get the list of passage names in the virtual doc.
    pub fn passage_names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.passage_name.as_str()).collect()
    }

    /// Check if the virtual doc is empty (no passages included).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the number of passages in the virtual doc.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Look up a passage entry by name.
    pub fn get_entry(&self, passage_name: &str) -> Option<&PassageEntry> {
        self.name_index
            .get(passage_name)
            .map(|&idx| &self.entries[idx])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal adapter for testing: includes all non-metadata passages,
    /// produces trivial JS blocks.
    struct TestAdapter;

    impl VirtualDocAdapter for TestAdapter {
        fn should_include_passage(&self, passage: &Passage) -> bool {
            !passage.is_metadata()
        }

        fn translate_passage(
            &self,
            passage: &Passage,
            _source_text: &dyn SourceTextProvider,
            context: &AdapterContext,
        ) -> Option<TranslatedBlock> {
            let js_block = format!(
                "/** @knot-passage \"{}\" */\nfunction __knot_{}() {{\n  // body\n}}\n",
                passage.name, context.function_index
            );
            Some(TranslatedBlock { js_block })
        }

        fn resolve_source_location(
            &self,
            _passage_name: &str,
            file_uri: &str,
            _vdoc_byte_range: Range<usize>,
            _source_text: &str,
        ) -> SourceLocation {
            SourceLocation {
                file_uri: file_uri.to_string(),
                byte_range: 0..1,
            }
        }

        fn interpret_diagnostic(
            &self,
            _js_diagnostic: &JsDiagnostic,
            _passage_name: &str,
            _file_uri: &str,
        ) -> Option<TwDiagnostic> {
            None
        }

        fn clear_state(&self) {}

        fn invalidate_passage(&self, _passage_name: &str) {}
    }

    #[test]
    fn test_empty_manager() {
        let manager = VirtualDocManager::new();
        assert!(manager.is_empty());
        assert_eq!(manager.len(), 0);
        assert!(manager.content().is_empty());
        assert!(manager.passage_names().is_empty());
    }

    #[test]
    fn test_find_passage_for_byte_range_empty() {
        let manager = VirtualDocManager::new();
        assert!(manager.find_passage_for_byte_range(0..5).is_none());
    }

    #[test]
    fn test_diagnostic_severity_debug() {
        assert_eq!(format!("{:?}", DiagnosticSeverity::Error), "Error");
        assert_eq!(format!("{:?}", DiagnosticSeverity::Warning), "Warning");
    }

    #[test]
    fn test_no_source_text() {
        let nosrc = NoSourceText;
        assert!(nosrc.get_source_text("file:///test.tw").is_none());
    }
}
