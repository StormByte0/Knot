//! Core virtual document construction — format-agnostic.
//!
//! This module provides the shared infrastructure for building sectioned virtual
//! JavaScript documents from Twine workspaces. The virtual document is the
//! foundation for cross-passage variable tracking, deep alias resolution, and
//! unified path-centric analysis.
//!
//! ## Why this is format-agnostic
//!
//! `[script]` passages are a **Twine core concept** defined by the Twee 3 spec,
//! not by any individual story format. Every format (SugarCube, Harlowe, Snowman,
//! Chapbook) has `[script]` passages that contain JavaScript executing at startup
//! in a shared scope. The core virtual document builder handles the parts that
//! are identical across all formats:
//!
//! - Collecting `[script]` passages from the workspace
//! - Concatenating them into a unified section with line mappings
//! - Scaffolding macro/template sections (one per passage)
//! - Line mapping from virtual document lines back to original source
//!
//! Format-specific behavior (alias regex patterns, macro→JS translation, variable
//! sigil resolution) is provided by the `FormatPlugin` trait hooks. The core
//! builder calls these hooks at the right points, so each format only needs to
//! implement a few focused methods instead of reimplementing the entire pipeline.
//!
//! ## Architecture
//!
//! The virtual document has two kinds of sections:
//!
//! 1. **Unified script section**: All `[script]` passage bodies concatenated
//!    in document order. Script passages execute at startup in a deterministic
//!    sequence, sharing a single JS scope. This section is where startup aliases
//!    are defined (e.g., `var g = gs()` in SugarCube, `var s = window.story.state`
//!    in Snowman, `var s = state` in Chapbook).
//!
//! 2. **Format-translated sections**: Each non-script passage that contains
//!    variable-affecting content is translated to JavaScript using the format
//!    plugin's `translate_passage_to_js()` hook. These are kept as individual
//!    sections — one per passage — because non-script passages execute
//!    non-deterministically based on player choices.
//!
//! ## Key Design Decisions
//!
//! - **Sectioned, not flat**: Format-translated sections are NOT concatenated
//!   with the script section. This avoids conflating "shares scope" (true for
//!   all JS across the session) with "shares execution flow" (only true for
//!   script passages at startup).
//!
//! - **Startup alias table**: Extracted from the unified script section via
//!   the format plugin's `extract_startup_aliases()` hook, and shared across
//!   all sections. This lets format-translated sections resolve aliases like
//!   `g.x` → `State.variables.x` (SugarCube) or `s.x` → `window.story.state.x`
//!   (Snowman) without re-deriving them.
//!
//! - **Line mapping**: Every virtual line maps back to the original passage
//!   and source line, enabling "go to definition" from analysis results.

use crate::plugin::SourceTextProvider;
use crate::types::{
    LineMapping, PassageInfo, StartupAlias, UserCallable, VirtualDocument, VirtualSection,
    VirtualSectionKind,
};
use knot_core::passage::{Block, Passage};
use regex::Regex;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Comment stripping (shared across all formats — JS comments are universal)
// ---------------------------------------------------------------------------

static RE_LINE_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"//[^\n]*").unwrap());
static RE_BLOCK_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/\*[\s\S]*?\*/").unwrap());

/// Strip JS comments from source text before alias extraction.
///
/// This is format-agnostic because all Twine formats use JavaScript in
/// `[script]` passages, and JS comments are the same regardless of the
/// story format layered on top.
pub fn strip_comments(src: &str) -> String {
    let no_block = RE_BLOCK_COMMENT.replace_all(src, "");
    let no_line = RE_LINE_COMMENT.replace_all(&no_block, "");
    no_line.to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute 0-based line number from a byte offset in a string.
pub fn line_from_offset(text: &str, offset: usize) -> u32 {
    text[..offset.min(text.len())]
        .chars()
        .filter(|&c| c == '\n')
        .count() as u32
}

/// Extract the raw body text from a passage, combining all blocks.
///
/// This is format-agnostic because it works with the core `Block` model.
/// For script passages, the body is stored as `Block::Text` (raw JS).
/// For other passages, we reconstruct the full text including macros
/// from the block model so that the format plugin's translator can
/// process it.
pub fn extract_body_text(
    passage: &Passage,
    source_text: &dyn SourceTextProvider,
    file_uri: &str,
) -> String {
    let mut body = String::new();
    for block in &passage.body {
        match block {
            Block::Text { content, .. } => {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(content);
            }
            Block::Macro { name, args, .. } => {
                // Reconstruct the macro text for the format translator.
                // We don't know the delimiter syntax here (<<>>, (), []),
                // so we emit a neutral format that the format plugin can
                // parse in its translate_passage_to_js() hook. We use
                // the <<>> syntax as a default because the format plugin
                // will re-parse from the original passage body anyway.
                if !body.is_empty() {
                    body.push('\n');
                }
                if !args.is_empty() {
                    body.push_str(&format!("<<{} {}>>", name, args));
                } else {
                    body.push_str(&format!("<<{}>>", name));
                }
            }
            Block::Incomplete { content, .. } => {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(content);
            }
            _ => {}
        }
    }

    // If we couldn't get body from blocks, try source text
    if body.is_empty() {
        if let Some(text) = source_text.get_source_text(file_uri) {
            if passage.span.start < text.len() && passage.span.end <= text.len() {
                body = text[passage.span.start..passage.span.end].to_string();
                // Strip the header line
                if let Some(newline_pos) = body.find('\n') {
                    body = body[newline_pos + 1..].to_string();
                }
            }
        }
    }

    body
}

// ---------------------------------------------------------------------------
// Core virtual document builder
// ---------------------------------------------------------------------------

/// Build the virtual document from all passages in the workspace.
///
/// This is the format-agnostic core entry point. It:
/// 1. Collects all `[script]` passages and concatenates them into a unified
///    script section (in document order — the Twine spec execution order)
/// 2. Calls the format plugin's `extract_startup_aliases()` hook on the
///    unified script section to build the alias table
/// 3. Calls the format plugin's `extract_user_callables()` hook to detect
///    custom macros (Macro.add) and widgets (<<widget>>)
/// 4. Iterates non-script passages, calling the format plugin's
///    `has_variable_affecting_content()` and `translate_passage_to_js()`
///    hooks to build format-translated sections
///
/// Format plugins should call this from their `build_virtual_document()`
/// trait method implementation, passing themselves as the hook provider.
pub fn build_core_virtual_document<H: VirtualDocHooks>(
    workspace: &knot_core::Workspace,
    source_text: &dyn SourceTextProvider,
    hooks: &H,
) -> VirtualDocument {
    let mut sections: Vec<VirtualSection> = Vec::new();
    let mut script_passages: Vec<(String, String, String)> = Vec::new(); // (passage_name, file_uri, body_text)

    // ── Phase 1: Collect script passages ──────────────────────────────
    // `[script]` is a Twine core tag — all formats handle it the same way.
    // Also collect passage info for all passages (for user callable extraction).
    let mut all_passage_info: Vec<PassageInfo> = Vec::new();

    for doc in workspace.documents() {
        let file_uri = doc.uri.to_string();
        for passage in &doc.passages {
            if passage.is_metadata() {
                continue;
            }

            let body_text = extract_body_text(passage, source_text, &file_uri);

            // Collect passage info for user callable extraction
            all_passage_info.push(PassageInfo {
                name: passage.name.clone(),
                file_uri: file_uri.clone(),
                tags: passage.tags.clone(),
                body_text: body_text.clone(),
            });

            if passage.is_script_passage() {
                script_passages.push((passage.name.clone(), file_uri.clone(), body_text));
            }
        }
    }

    // ── Phase 2: Build unified script section ──────────────────────────
    // Script passages execute in document order (Twine spec).
    // We concatenate them with separator comments for clarity.
    if !script_passages.is_empty() {
        let mut unified_js = String::new();
        let mut line_map: Vec<LineMapping> = Vec::new();

        for (passage_name, file_uri, body_text) in &script_passages {
            // Add a section separator comment
            let separator = format!("// ── [script] {} ──\n", passage_name);
            unified_js.push_str(&separator);
            line_map.push(LineMapping {
                passage_name: passage_name.clone(),
                file_uri: file_uri.clone(),
                original_line: 0, // separator line maps to passage header
            });

            // Add the body lines with line mappings
            for (line_idx, line) in body_text.lines().enumerate() {
                unified_js.push_str(line);
                unified_js.push('\n');
                line_map.push(LineMapping {
                    passage_name: passage_name.clone(),
                    file_uri: file_uri.clone(),
                    original_line: line_idx as u32,
                });
            }
        }

        sections.push(VirtualSection {
            kind: VirtualSectionKind::UnifiedScript,
            source_text: unified_js,
            line_map,
        });
    }

    // ── Phase 3: Extract startup aliases from unified script section ───
    // The format plugin provides its own alias patterns. SugarCube looks
    // for State.variables, gs(), etc. Snowman looks for s, window.story.state.
    // Chapbook looks for state. Harlowe doesn't have JS aliases.
    let startup_aliases = hooks.extract_startup_aliases(&sections);

    // ── Phase 3.5: Extract user-defined callables ──────────────────────
    // Custom macros (Macro.add in script passages) and widgets
    // (<<widget>> in widget-tagged passages). These are used by the
    // translator to recognize <<macroName args>> as function calls.
    let user_callables = hooks.extract_user_callables(&all_passage_info);

    // ── Phase 4: Build format-translated sections ──────────────────────
    // Each format decides what constitutes "variable-affecting content"
    // and how to translate it to JS.
    for doc in workspace.documents() {
        let file_uri = doc.uri.to_string();
        for passage in &doc.passages {
            if passage.is_metadata() || passage.is_script_passage() {
                continue;
            }

            let body_text = extract_body_text(passage, source_text, &file_uri);

            // Ask the format plugin if this passage has variable content
            if !hooks.has_variable_affecting_content(&body_text) {
                continue;
            }

            // Ask the format plugin to translate to JS (with callable info + exact mapping)
            let (translated_js, line_map) = match hooks.translate_passage_to_js(&body_text, &user_callables, &passage.name, &file_uri) {
                Some(result) => result,
                None => continue, // Format can't translate this passage
            };

            sections.push(VirtualSection {
                kind: VirtualSectionKind::MacroTranslated {
                    passage_name: passage.name.clone(),
                },
                source_text: translated_js,
                line_map,
            });
        }
    }

    VirtualDocument {
        sections,
        startup_aliases,
        user_callables,
    }
}

/// Build a line map for a format-translated section.
///
/// Since format-translated sections are per-passage, all lines map to
/// the same passage. The original line numbers are derived by correlating
/// the translated JS output with the original passage body text.
///
/// ## Line Mapping Strategy
///
/// The macro translator processes each original source line and may produce
/// 0-N translated JS lines. We use a heuristic approach:
///
/// 1. Count the number of original source lines (from `original_body_text`).
/// 2. Count the number of translated JS lines.
/// 3. Distribute the translated lines proportionally across the original
///    lines. For example, if there are 10 original lines and 30 translated
///    lines, each original line "owns" ~3 translated lines.
///
/// This produces a best-effort mapping that is significantly more accurate
/// than the naive `original_line == virtual_line` approach, which always
/// gives wrong line numbers when the translator expands macros into
/// multi-line JS constructs.
///
/// **Why not exact mapping?** The recursive descent translator in
/// `translate_macros_to_js()` does not currently emit line-number
/// annotations. Adding those would require changing the translator's
/// return type and is a larger refactor. The proportional mapping is
/// a practical improvement that fixes the most common case (variable
/// references in short passages where the mapping is close to 1:1).
/// Build a line map for a format-translated section using proportional mapping.
///
/// This is the fallback mapping strategy. SugarCube now uses exact mapping
/// via `walk_translate()` in `passage_tree.rs`. Other formats that don't
/// implement exact mapping can use this as a fallback.
#[allow(dead_code)] // Retained as fallback for other formats
pub(crate) fn build_format_section_line_map(
    translated_js: &str,
    original_body_text: &str,
    passage_name: &str,
    file_uri: &str,
) -> Vec<LineMapping> {
    let original_line_count = original_body_text.lines().count().max(1);
    let translated_line_count = translated_js.lines().count().max(1);

    translated_js
        .lines()
        .enumerate()
        .map(|(line_idx, _)| {
            // Proportional mapping: each translated line maps to the
            // original source line that "owns" it based on proportional
            // distribution.
            let original_line = if translated_line_count <= original_line_count {
                // Fewer or equal translated lines than original — 1:1 or
                // some original lines produced no output (e.g., blank lines).
                // Map directly.
                line_idx.min(original_line_count - 1) as u32
            } else {
                // More translated lines than original — some original lines
                // expanded into multiple JS lines. Distribute proportionally.
                ((line_idx as f64 * original_line_count as f64 / translated_line_count as f64)
                    .floor() as usize)
                    .min(original_line_count - 1) as u32
            };

            LineMapping {
                passage_name: passage_name.to_string(),
                file_uri: file_uri.to_string(),
                original_line,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// VirtualDocHooks trait — format-specific extension points
// ---------------------------------------------------------------------------

/// Format-specific hooks for virtual document construction.
///
/// Each format plugin implements this trait to provide the format-specific
/// parts of virtual document building. The core builder calls these hooks
/// at the appropriate points, so formats only need to implement a few
/// focused methods instead of reimplementing the entire pipeline.
///
/// ## Implementors
///
/// - **SugarCube**: Alias patterns for `State.variables`, `gs()`,
///   `SugarCube.State.Variables`. Macro→JS translation for `<<set>>`,
///   `<<run>>`, `<<capture>>`, `<<unset>>`, `<<if>>`, etc. Custom macro
///   detection via `Macro.add()`. Widget detection via `<<widget>>`.
/// - **Snowman**: Alias patterns for `s`, `window.story.state`. No macro
///   translation (Snowman uses ERB-style `<%= %>` which is already JS).
/// - **Chapbook**: Alias patterns for `state`. Translation for `[javascript]`
///   blocks and `{{expression}}` inserts.
/// - **Harlowe**: Minimal (Harlowe doesn't expose a JS state API in
///   `[script]` passages — variables are macro-only). Translation for
///   `(set:)`, `(put:)`, `(move:)`, etc.
pub trait VirtualDocHooks {
    /// Extract startup aliases from the unified script section.
    ///
    /// This is called after the unified script section is built (Phase 3).
    /// The format plugin should strip JS comments and apply its own regex
    /// patterns to find alias definitions.
    ///
    /// Return an empty Vec if the format doesn't support JS aliases
    /// (e.g., Harlowe).
    fn extract_startup_aliases(&self, sections: &[VirtualSection]) -> Vec<StartupAlias>;

    /// Check if a passage body contains variable-affecting content.
    ///
    /// The format plugin decides what counts. SugarCube checks for `<<set>>`,
    /// `<<run>>`, `$`, etc. Harlowe checks for `(set:)`, `(put:)`, `$`, etc.
    /// Snowman checks for `s.` and `window.story.state.`. Chapbook checks
    /// for `state.` and `{{...}}`.
    fn has_variable_affecting_content(&self, passage_body: &str) -> bool;

    /// Translate a passage body to JavaScript, with line mappings.
    ///
    /// The format plugin should convert its format-specific variable syntax
    /// to JavaScript that uses the format's state accessor path. Return
    /// `None` if the passage cannot be translated.
    ///
    /// The return value is a tuple of `(js_string, line_mappings)`. The line
    /// mappings are a `Vec<LineMapping>` with one entry per line of the JS
    /// output, mapping each virtual line back to the original source location.
    /// Formats that don't have exact mapping can use `build_format_section_line_map()`
    /// as a fallback for proportional mapping.
    ///
    /// The `callables` parameter provides the list of user-defined callables
    /// (custom macros and widgets) so the translator can recognize invocations
    /// like `<<useItem matchbox>>` as function calls.
    ///
    /// - SugarCube: `<<set $x to 5>>` → `State.variables.x = 5;` (exact mapping)
    /// - Harlowe: `(set: $x to 5)` → `State.variables.x = 5;` (proportional mapping)
    /// - Snowman: `<%= s.x %>` → `window.story.state.x` (already JS)
    /// - Chapbook: `[javascript] state.x = 5; [/javascript]` → `state.x = 5;`
    fn translate_passage_to_js(
        &self,
        passage_body: &str,
        callables: &[UserCallable],
        passage_name: &str,
        file_uri: &str,
    ) -> Option<(String, Vec<LineMapping>)>;

    /// Extract user-defined callables from all passages.
    ///
    /// This is called after the unified script section is built (Phase 3.5).
    /// The format plugin should scan for custom macro definitions
    /// (e.g., SugarCube's `Macro.add('name', { handler: ... })`) and
    /// widget definitions (e.g., `<<widget name>>...<</widget>>`).
    ///
    /// Return an empty Vec if the format doesn't support user-defined
    /// callables.
    fn extract_user_callables(&self, passages: &[PassageInfo]) -> Vec<UserCallable>;
}
