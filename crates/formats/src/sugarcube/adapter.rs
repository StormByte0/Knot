//! SugarCube adapter implementing the VirtualDocAdapter trait.
//!
//! This adapter wraps the existing SugarCube translation logic
//! (passage_tree::walk_translate, VirtualDocMap, etc.) to implement the
//! new `VirtualDocAdapter` trait defined in `knot_core::virtual_doc`.
//!
//! ## Stateful Design
//!
//! The adapter is stateful. It maintains internal reverse-mapping tables
//! built during `translate_passage()` and read during `resolve_source_location()`.
//! This is the SugarCube-internal `VirtualDocMap`, repurposed as the adapter's
//! state store.
//!
//! ## Migration Strategy
//!
//! This adapter is the bridge between the old architecture (SugarCube owns
//! virtual doc state via `SugarCubePlugin::virtual_docs`) and the new
//! architecture (Core's `VirtualDocManager` drives the pipeline, adapter
//! provides content). During migration, the adapter delegates to the same
//! `walk_translate()` and `VirtualDocMap` code that SugarCube already uses.

use knot_core::passage::Passage;
use knot_core::virtual_doc::{
    AdapterContext, DiagnosticSeverity, JsDiagnostic, SourceLocation, SourceTextProvider,
    StartupAlias, TranslatedBlock, TwDiagnostic, UserCallable, VirtualDocAdapter,
};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::RwLock;

use super::comments;
use super::custom_macros;
use super::passage_tree::{parse_passage_body, walk_translate, ExactLineMapping};
use super::virtual_doc::{extract_startup_aliases, extract_user_callables};
use super::virtual_doc_map::VirtualDocMap;

// ---------------------------------------------------------------------------
// Reverse-mapping state per passage
// ---------------------------------------------------------------------------

/// Reverse-mapping data for a single passage, stored in the adapter.
///
/// This is built during `translate_passage()` and consumed during
/// `resolve_source_location()`. It carries the exact byte-level mapping
/// from virtual doc positions back to .tw source positions.
#[derive(Debug, Clone)]
struct PassageReverseMap {
    /// The file URI where this passage originates.
    /// Stored for cross-file diagnostic verification and potential
    /// future use in multi-file workspaces.
    #[allow(dead_code)] // Stored for verification, not yet read in resolve path
    file_uri: String,

    /// Per-line mapping from JS output lines to source positions.
    /// Each entry maps one line of the `js_block` to the original
    /// source line within the passage body.
    line_map: Vec<ExactLineMapping>,

    /// The byte offset where the passage body starts in the source file.
    /// This is needed to convert body-relative line numbers back to
    /// document-absolute byte positions.
    #[allow(dead_code)] // Used in debug assertions, reserved for future enhancements
    body_offset: usize,

    /// The complete JS function block (annotation + wrapper + body).
    /// Used during `resolve_source_location()` to compute byte offsets
    /// within the virtual doc for a given passage.
    js_block: String,
}

// ---------------------------------------------------------------------------
// SugarCubeAdapter
// ---------------------------------------------------------------------------

/// SugarCube's implementation of the `VirtualDocAdapter` trait.
///
/// Wraps the existing translation pipeline:
/// - `parse_passage_body()` → `walk_translate()` for macro passages
/// - `custom_macros::build_script_passage_js()` for script passages
/// - Internal `VirtualDocMap` for per-passage state management
/// - `ExactLineMapping` reverse maps for diagnostic resolution
///
/// ## Lifecycle
///
/// The adapter is a long-lived object that persists alongside the
/// `VirtualDocManager`. When `clear_state()` is called (during rebuild),
/// all internal reverse-mapping data is discarded. When
/// `invalidate_passage()` is called (during surgical update), just that
/// passage's data is removed.
pub struct SugarCubeAdapter {
    /// Per-passage reverse-mapping data, keyed by passage name.
    /// Built during `translate_passage()`, read during `resolve_source_location()`.
    reverse_maps: RwLock<HashMap<String, PassageReverseMap>>,

    /// The existing VirtualDocMap, repurposed as the adapter's state store.
    /// This stores per-passage entries with JS functions and line maps,
    /// and is used by `assemble_virtual_doc()` for backward compatibility
    /// during the migration period.
    virtual_doc_map: RwLock<VirtualDocMap>,
}

impl SugarCubeAdapter {
    /// Create a new SugarCubeAdapter.
    pub fn new() -> Self {
        Self {
            reverse_maps: RwLock::new(HashMap::new()),
            virtual_doc_map: RwLock::new(VirtualDocMap::new()),
        }
    }
}

impl Default for SugarCubeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualDocAdapter for SugarCubeAdapter {
    fn should_include_passage(&self, passage: &Passage) -> bool {
        // Include script passages (they contain raw JS)
        if passage.is_script_passage() {
            return true;
        }

        // Exclude metadata passages (StoryData, StoryTitle)
        if passage.is_metadata() {
            return false;
        }

        // Exclude stylesheet passages
        if passage.is_stylesheet_passage() {
            return false;
        }

        // Include passages that have variable-affecting content.
        // SugarCube checks for `<<set>>`, `<<run>>`, `$`, `_`, etc.
        // We use the existing passage.vars as the indicator — if a passage
        // has any variable operations, it should be included.
        // Also include passages with macro blocks (Body::Macro entries).
        let has_vars = !passage.vars.is_empty();
        let has_macros = passage.body.iter().any(|b| {
            matches!(b, knot_core::passage::Block::Macro { .. })
        });
        let has_expressions = passage.body.iter().any(|b| {
            matches!(b, knot_core::passage::Block::Expression { .. })
        });

        has_vars || has_macros || has_expressions
    }

    fn translate_passage(
        &self,
        passage: &Passage,
        source_text: &dyn SourceTextProvider,
        context: &AdapterContext,
    ) -> Option<TranslatedBlock> {
        // The file URI is now provided by the AdapterContext, which
        // gets it from the VirtualDocManager's workspace iteration.
        let file_uri = &context.file_uri;

        // Compute body_offset from the passage span.
        // The span starts at the passage header (:: Name [tags] {position}).
        // The body starts after the first newline following the header line.
        // We try to compute this from the source text; if unavailable,
        // fall back to passage.span.start as a rough estimate.
        let body_offset = self.compute_body_offset(passage, source_text, file_uri);

        // Extract body text from the passage
        let body_text = self.extract_body_text(passage, source_text, file_uri, body_offset);

        if passage.is_script_passage() {
            self.translate_script_passage(
                &passage.name,
                file_uri,
                &body_text,
                body_offset,
                context,
            )
        } else {
            self.translate_macro_passage(
                &passage.name,
                file_uri,
                &body_text,
                body_offset,
                context,
            )
        }
    }

    fn resolve_source_location(
        &self,
        passage_name: &str,
        file_uri: &str,
        vdoc_byte_range: Range<usize>,
        _source_text: &str,
    ) -> SourceLocation {
        let maps = self.reverse_maps.read().unwrap();

        if let Some(reverse_map) = maps.get(passage_name) {
            // The vdoc_byte_range is a **passage-relative** byte range.
            // VirtualDocManager::find_passage_for_byte_range() already
            // subtracted the passage's absolute start offset, so byte 0
            // here corresponds to the first byte of the passage's js_block
            // (i.e., the first byte of the @knot-passage annotation line).
            //
            // Strategy: walk the js_block line by line, accumulating byte
            // offsets from 0. Find the line whose byte range overlaps with
            // the diagnostic's passage-relative byte range, then look up
            // the reverse mapping for that line to get the .tw source byte
            // span (using both original_start_byte and original_end_byte
            // for byte-precise resolution).

            let js_block = &reverse_map.js_block;
            let mut current_byte = 0usize;

            // Track the best match across all overlapping lines — we want
            // the byte range that covers the entire diagnostic span.
            let mut best_start: Option<usize> = None;
            let mut best_end: Option<usize> = None;

            for (line_idx, line) in js_block.lines().enumerate() {
                let line_start = current_byte;
                let line_end = current_byte + line.len() + 1; // +1 for \n

                // Check if the diagnostic byte range overlaps this line
                if vdoc_byte_range.start < line_end && vdoc_byte_range.end > line_start {
                    // This line contains (or overlaps) the diagnostic.
                    // Look up the reverse mapping for this line.
                    if line_idx < reverse_map.line_map.len() {
                        let mapping = &reverse_map.line_map[line_idx];

                        // Expand the best match to cover this line's source span.
                        // We take the minimum start and maximum end across all
                        // overlapping lines to produce the full source range.
                        let src_start = mapping.original_start_byte;
                        let src_end = mapping.original_end_byte.max(src_start);

                        best_start = Some(match best_start {
                            Some(s) => s.min(src_start),
                            None => src_start,
                        });
                        best_end = Some(match best_end {
                            Some(e) => e.max(src_end),
                            None => src_end,
                        });
                    }
                }

                current_byte = line_end;
            }

            // If we found at least one matching line, return the merged range
            if let (Some(start), Some(end)) = (best_start, best_end) {
                return SourceLocation {
                    file_uri: file_uri.to_string(),
                    byte_range: start..end,
                };
            }
        }

        // Fallback: return the start of the passage in the file
        SourceLocation {
            file_uri: file_uri.to_string(),
            byte_range: 0..1,
        }
    }

    fn interpret_diagnostic(
        &self,
        js_diagnostic: &JsDiagnostic,
        _passage_name: &str,
        file_uri: &str,
    ) -> Option<TwDiagnostic> {
        // SugarCube-specific diagnostic interpretation.
        //
        // Common JS LSP false positives in SugarCube virtual docs:
        // - "State is not defined" → suppress (we declare it in the preamble)
        // - "Macro is not defined" → suppress (SugarCube runtime provides it)
        // - "gs is not defined" → suppress (SugarCube helper function)
        // - "Engine is not defined" → suppress (SugarCube runtime object)
        // - "Dialog is not defined" → suppress (SugarCube runtime object)
        // - "settings is not defined" → suppress (SugarCube global)
        // - "setup is not defined" → suppress (SugarCube global)
        // - "SugarCube is not defined" → suppress (SugarCube namespace)

        let msg = &js_diagnostic.message;

        // Known false positive patterns — these are global objects that
        // exist in the SugarCube runtime but are not declared in the
        // virtual doc (except for State, which IS in the preamble).
        let false_positive_patterns = [
            "State is not defined",
            "Macro is not defined",
            "gs is not defined",
            "Engine is not defined",
            "Dialog is not defined",
            "settings is not defined",
            "setup is not defined",
            "SugarCube is not defined",
        ];

        for pattern in &false_positive_patterns {
            if msg.contains(pattern) {
                return None; // Suppress this diagnostic
            }
        }

        // Convert variable-related diagnostics to SugarCube-specific messages.
        // JS LSP says "State.variables.x is not defined" — but in SugarCube
        // terms, this means "$x may not be initialized" which is more helpful
        // to the user.
        let tw_message = if msg.contains("is not defined") && msg.contains("variables") {
            msg.replace("State.variables.", "$")
                .replace(" is not defined", " may not be initialized")
        } else {
            msg.clone()
        };

        // Downgrade JS errors to warnings. The virtual doc is a translation,
        // not the actual runtime, so JS type errors and strict-mode violations
        // are less severe than they would be in real JS code.
        let tw_severity = match js_diagnostic.severity {
            DiagnosticSeverity::Error => DiagnosticSeverity::Warning,
            other => other,
        };

        Some(TwDiagnostic {
            file_uri: file_uri.to_string(),
            byte_range: js_diagnostic.byte_range.clone(),
            message: tw_message,
            severity: tw_severity,
            code: js_diagnostic.code.clone(),
        })
    }

    fn clear_state(&self) {
        self.reverse_maps.write().unwrap().clear();
        // Don't clear virtual_doc_map here — that's managed separately
        // during the rebuild process.
    }

    fn invalidate_passage(&self, passage_name: &str) {
        self.reverse_maps.write().unwrap().remove(passage_name);
        self.virtual_doc_map.write().unwrap().remove_passage(passage_name);
    }

    fn extract_startup_aliases(
        &self,
        workspace: &knot_core::Workspace,
        source_text: &dyn SourceTextProvider,
    ) -> Vec<StartupAlias> {
        // Collect all script passage bodies into a unified string
        let mut unified_script = String::new();
        for doc in workspace.documents() {
            let file_uri = doc.uri.to_string();
            for passage in &doc.passages {
                if !passage.is_script_passage() || passage.is_metadata() {
                    continue;
                }
                let body_offset = self.compute_body_offset(passage, source_text, &file_uri);
                let body_text = self.extract_body_text(passage, source_text, &file_uri, body_offset);
                if !body_text.is_empty() {
                    unified_script.push_str(&body_text);
                    unified_script.push('\n');
                }
            }
        }

        if unified_script.is_empty() {
            return Vec::new();
        }

        // The refactored extract_startup_aliases takes &str directly
        let aliases = extract_startup_aliases(&unified_script);

        // Convert from formats' StartupAlias to core's StartupAlias
        aliases.into_iter().map(|a| {
            let resolves_to = match &a.resolution {
                crate::types::AliasResolution::StateVariables => {
                    "State.variables".to_string()
                }
                crate::types::AliasResolution::StateVariableProperty { base_name, property_path } => {
                    match property_path {
                        Some(path) => format!("State.variables.{}.{}", base_name, path),
                        None => format!("State.variables.{}", base_name),
                    }
                }
                crate::types::AliasResolution::GetterFunction => {
                    "State.variables".to_string()
                }
            };
            StartupAlias {
                alias_name: a.alias_name,
                resolves_to,
                defined_in: String::new(), // Not tracked at this level
            }
        }).collect()
    }

    fn extract_user_callables(
        &self,
        workspace: &knot_core::Workspace,
        source_text: &dyn SourceTextProvider,
    ) -> Vec<UserCallable> {
        // Collect passage info for all passages — the existing
        // extract_user_callables() scans for Macro.add and <<widget>>.
        use crate::types::PassageInfo;
        let mut passage_infos: Vec<PassageInfo> = Vec::new();
        for doc in workspace.documents() {
            let file_uri = doc.uri.to_string();
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }
                let body_offset = self.compute_body_offset(passage, source_text, &file_uri);
                let body_text = self.extract_body_text(passage, source_text, &file_uri, body_offset);
                passage_infos.push(PassageInfo {
                    name: passage.name.clone(),
                    file_uri: file_uri.clone(),
                    tags: passage.tags.clone(),
                    body_text,
                });
            }
        }

        let callables = extract_user_callables(&passage_infos);

        // Convert from formats' UserCallable to core's UserCallable
        callables.into_iter().map(|c| UserCallable {
            name: c.name,
            kind: match c.kind {
                crate::types::UserCallableKind::CustomMacro => {
                    knot_core::virtual_doc::UserCallableKind::CustomMacro
                }
                crate::types::UserCallableKind::Widget => {
                    knot_core::virtual_doc::UserCallableKind::Widget
                }
            },
            arg_count: c.arg_count,
            defined_in: c.defined_in,
            file_uri: c.file_uri,
        }).collect()
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

impl SugarCubeAdapter {
    /// Compute the byte offset where the passage body starts in the source file.
    ///
    /// The passage span starts at the `::` header. The body begins after
    /// the first newline following the header line. We look at the source
    /// text to find the exact position.
    fn compute_body_offset(
        &self,
        passage: &Passage,
        source_text: &dyn SourceTextProvider,
        file_uri: &str,
    ) -> usize {
        if let Some(text) = source_text.get_source_text(file_uri) {
            if passage.span.start < text.len() {
                // Find the first newline after the span start — that's
                // the end of the header line. The body starts after it.
                let search_from = passage.span.start;
                if let Some(newline_pos) = text[search_from..].find('\n') {
                    let header_end = search_from + newline_pos;
                    // Skip past the newline (1 byte for \n, 2 for \r\n)
                    if text.get(header_end..header_end + 2) == Some("\r\n") {
                        return header_end + 2;
                    } else {
                        return header_end + 1;
                    }
                }
            }
        }

        // Fallback: estimate body_offset as span.start. This is
        // imprecise but works for cases where source text is unavailable
        // (e.g., testing with NoSourceText).
        passage.span.start
    }

    /// Extract body text from a passage.
    ///
    /// First tries to extract from the source text (most accurate), then
    /// falls back to reconstructing from the passage's parsed blocks.
    fn extract_body_text(
        &self,
        passage: &Passage,
        source_text: &dyn SourceTextProvider,
        file_uri: &str,
        body_offset: usize,
    ) -> String {
        // Prefer source text — it preserves the exact formatting, whitespace,
        // and raw macro syntax that walk_translate() expects.
        if let Some(text) = source_text.get_source_text(file_uri) {
            if body_offset < text.len() && passage.span.end <= text.len() {
                return text[body_offset..passage.span.end].to_string();
            }
        }

        // Fallback: reconstruct from parsed blocks. This is lossy — we
        // lose exact whitespace and raw syntax — but works when source
        // text is unavailable.
        let mut body = String::new();
        for block in &passage.body {
            match block {
                knot_core::passage::Block::Text { content, .. } => {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(content);
                }
                knot_core::passage::Block::Macro { name, args, .. } => {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    if args.is_empty() {
                        body.push_str(&format!("<<{}>>", name));
                    } else {
                        body.push_str(&format!("<<{} {}>>", name, args));
                    }
                }
                knot_core::passage::Block::Incomplete { content, .. } => {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(content);
                }
                _ => {}
            }
        }

        body
    }

    /// Translate a script passage to JS.
    ///
    /// Script passages contain raw JavaScript. We delegate to
    /// `custom_macros::build_script_passage_js()` for the actual
    /// translation (which handles $var translation and Macro.add
    /// function extraction), then wrap the result with the
    /// @knot-passage annotation and __knot_N function name.
    fn translate_script_passage(
        &self,
        passage_name: &str,
        file_uri: &str,
        body_text: &str,
        body_offset: usize,
        context: &AdapterContext,
    ) -> Option<TranslatedBlock> {
        // Convert core's UserCallable to formats' UserCallable for
        // build_script_passage_js.
        let custom_macros_in_passage: Vec<crate::types::UserCallable> = context
            .user_callables
            .iter()
            .filter(|c| {
                c.kind == knot_core::virtual_doc::UserCallableKind::CustomMacro
                    && c.defined_in == passage_name
            })
            .map(|c| crate::types::UserCallable {
                name: c.name.clone(),
                kind: crate::types::UserCallableKind::CustomMacro,
                arg_count: c.arg_count,
                defined_in: c.defined_in.clone(),
                file_uri: c.file_uri.clone(),
                defined_at_line: 0,
                body: None,
            })
            .collect();

        // Use the existing build_script_passage_js() to get the
        // translated JS function and line map.
        let (old_js_function, old_line_map) = custom_macros::build_script_passage_js(
            passage_name,
            body_text,
            body_offset,
            &custom_macros_in_passage,
        );

        // Now re-wrap the old-style function with @knot-passage
        // annotation and __knot_N naming convention.
        //
        // Old format: "function script_PassageName() {\n  ...body...\n}\n"
        // New format: "/** @knot-passage \"PassageName\" */\nfunction __knot_N() {\n  ...body...\n}\n"
        let annotation = format!("/** @knot-passage \"{}\" */", passage_name);
        let func_name = format!("__knot_{}", context.function_index);

        let js_block = if let Some(brace_pos) = old_js_function.find("{\n") {
            let body_start = brace_pos + 2;
            let body_end = old_js_function
                .rfind("}\n")
                .unwrap_or(old_js_function.len().saturating_sub(2));
            let inner_body = &old_js_function[body_start..body_end];

            format!(
                "{}\nfunction {}() {{\n{}}}\n",
                annotation,
                func_name,
                inner_body,
            )
        } else {
            // Fallback: wrap the entire output
            format!(
                "{}\nfunction {}() {{\n  {};\n}}\n",
                annotation,
                func_name,
                old_js_function.replace('\n', " ")
            )
        };

        // Build line mappings for the new format.
        // The annotation adds 1 line and the function declaration adds 1 line,
        // so we prepend 2 sentinel entries. The old line_map's first entry
        // (function header) maps to the same place as our annotation, so we
        // skip it. The old line_map's last entry (closing brace) is replaced
        // by our own closing brace sentinel.
        let mut new_line_map = Vec::new();

        // Annotation line → passage header sentinel
        new_line_map.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: body_offset,
            original_end_byte: body_offset,
        });

        // Function declaration line → passage header sentinel
        new_line_map.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: body_offset,
            original_end_byte: body_offset,
        });

        // Body lines from the original translation (skip first entry which
        // is the old function header, and skip last entry which is the old
        // closing brace)
        if old_line_map.len() > 1 {
            new_line_map.extend(old_line_map.iter().skip(1).take(old_line_map.len() - 2).cloned());
        }

        // Closing brace → passage end sentinel
        new_line_map.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: body_offset,
            original_end_byte: body_offset,
        });

        // Verify the invariant: js_block line count == line_map length
        let line_count = js_block.lines().count();
        debug_assert_eq!(
            line_count,
            new_line_map.len(),
            "translate_script_passage: js_block has {} lines but line_map has {} entries",
            line_count,
            new_line_map.len(),
        );

        // Store the reverse-mapping data
        self.reverse_maps.write().unwrap().insert(
            passage_name.to_string(),
            PassageReverseMap {
                file_uri: file_uri.to_string(),
                line_map: new_line_map,
                body_offset,
                js_block: js_block.clone(),
            },
        );

        Some(TranslatedBlock { js_block })
    }

    /// Translate a macro passage to JS.
    ///
    /// Macro passages have their SugarCube macros translated to JavaScript
    /// using the existing `walk_translate()` pipeline.
    fn translate_macro_passage(
        &self,
        passage_name: &str,
        file_uri: &str,
        body_text: &str,
        body_offset: usize,
        context: &AdapterContext,
    ) -> Option<TranslatedBlock> {
        // Parse the passage body into a tree
        let tree = parse_passage_body(body_text, body_offset);

        // Find comment spans for the body
        let comment_spans = comments::find_all_comment_spans(body_text, false);

        // Determine if this is a widget passage
        let is_widget = context
            .user_callables
            .iter()
            .any(|c| c.kind == knot_core::virtual_doc::UserCallableKind::Widget && c.defined_in == passage_name);

        // Convert core's UserCallable to formats' UserCallable for walk_translate
        let format_callables: Vec<crate::types::UserCallable> = context
            .user_callables
            .iter()
            .map(|c| crate::types::UserCallable {
                name: c.name.clone(),
                kind: match c.kind {
                    knot_core::virtual_doc::UserCallableKind::CustomMacro => {
                        crate::types::UserCallableKind::CustomMacro
                    }
                    knot_core::virtual_doc::UserCallableKind::Widget => {
                        crate::types::UserCallableKind::Widget
                    }
                },
                arg_count: c.arg_count,
                defined_in: c.defined_in.clone(),
                file_uri: c.file_uri.clone(),
                defined_at_line: 0,
                body: None,
            })
            .collect();

        // Use the existing walk_translate() pipeline
        let translate_result = walk_translate(
            &tree,
            body_text,
            body_offset,
            &format_callables,
            passage_name,
            is_widget,
            &comment_spans,
        );

        // Build the @knot-passage annotation and wrapper
        let annotation = format!("/** @knot-passage \"{}\" */", passage_name);
        let func_name = format!("__knot_{}", context.function_index);

        // The walk_translate already produces a function wrapper.
        // We need to replace the old-style function name with the new
        // __knot_N convention and add the @knot-passage annotation.
        let old_js = &translate_result.js_function;

        // Extract the body from the old function wrapper.
        // Old format: "function passage_Name() {\n  ...body...\n}\n"
        //   or widget: "function myWidget() {\n  ...body...\n}\n"
        // New format: "/** @knot-passage \"Name\" */\nfunction __knot_N() {\n  ...body...\n}\n"
        let js_block = if let Some(brace_pos) = old_js.find("{\n") {
            let body_start = brace_pos + 2;
            let body_end = old_js
                .rfind("}\n")
                .unwrap_or(old_js.len().saturating_sub(2));
            let inner_body = &old_js[body_start..body_end];

            format!(
                "{}\nfunction {}() {{\n{}}}\n",
                annotation,
                func_name,
                inner_body,
            )
        } else {
            // Fallback: wrap the entire output
            format!(
                "{}\nfunction {}() {{\n  {};\n}}\n",
                annotation,
                func_name,
                old_js.replace('\n', " ")
            )
        };

        // Build line mappings for the new format.
        // The annotation adds 1 line, the function declaration adds 1 line,
        // so we prepend 2 sentinel entries. The old line_map's first entry
        // (function header) is replaced by our annotation+function lines.
        // The old line_map's last entry (closing brace) is replaced by our
        // own closing brace sentinel.
        let mut new_line_map = Vec::new();

        // Annotation line → passage header sentinel
        new_line_map.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: body_offset,
            original_end_byte: body_offset,
        });

        // Function declaration line → passage header sentinel
        new_line_map.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: body_offset,
            original_end_byte: body_offset,
        });

        // Body lines from the original translation (skip first entry which
        // is the old function header, and skip last entry which is the old
        // closing brace)
        let old_line_map = &translate_result.line_map;
        if old_line_map.len() > 1 {
            new_line_map.extend(old_line_map.iter().skip(1).take(old_line_map.len() - 2).cloned());
        }

        // Closing brace → passage end sentinel
        new_line_map.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: body_offset,
            original_end_byte: body_offset,
        });

        // Verify the invariant: js_block line count == line_map length
        let line_count = js_block.lines().count();
        debug_assert_eq!(
            line_count,
            new_line_map.len(),
            "translate_macro_passage: js_block has {} lines but line_map has {} entries",
            line_count,
            new_line_map.len(),
        );

        // Store the reverse-mapping data
        self.reverse_maps.write().unwrap().insert(
            passage_name.to_string(),
            PassageReverseMap {
                file_uri: file_uri.to_string(),
                line_map: new_line_map,
                body_offset,
                js_block: js_block.clone(),
            },
        );

        Some(TranslatedBlock { js_block })
    }
}
