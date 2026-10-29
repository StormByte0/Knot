//! Moved verbatim from the former `sugarcube/mod.rs` monolith
//! (pure move, plan §4.6 — no signature or logic changes).

use super::*;

// ===========================================================================
// Completion context helpers (SugarCube-specific)
// ===========================================================================

/// Convert a (line, character) position to a document-absolute byte offset.
pub(super) fn line_char_to_byte_offset(text: &str, line: u32, character: u32) -> usize {
    let mut byte_offset = 0;
    let mut current_line = 0u32;
    for ch in text.chars() {
        if current_line == line {
            break;
        }
        byte_offset += ch.len_utf8();
        if ch == '\n' {
            current_line += 1;
        }
    }
    if current_line < line {
        return text.len();
    }
    // Now we're at the start of the target line.
    // Convert UTF-16 character offset to byte offset within the line.
    let line_start = byte_offset;
    let line_end = text[line_start..]
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(text.len());
    let line_text = &text[line_start..line_end];
    line_start + char_to_byte_offset(line_text, character as usize)
}

/// Convert a UTF-16 character offset to a byte offset within a line.
pub(super) fn char_to_byte_offset(line: &str, char_offset: usize) -> usize {
    let mut utf16_count = 0usize;
    let mut byte_offset = 0usize;
    for ch in line.chars() {
        if utf16_count >= char_offset {
            break;
        }
        let code_units = if (ch as u32) < 0x10000 {
            1usize
        } else {
            2usize
        };
        utf16_count += code_units;
        byte_offset += ch.len_utf8();
    }
    byte_offset
}

/// Find the variable name at a byte offset in the workspace.
pub(super) fn find_variable_name_at_offset(
    workspace: &knot_core::Workspace,
    uri: &url::Url,
    byte_offset: usize,
) -> Option<String> {
    let doc = workspace.get_document(uri)?;
    for passage in &doc.passages {
        for var in &passage.vars {
            if passage.span_contains_abs_offset(&var.span, byte_offset) {
                return Some(var.name.clone());
            }
        }
    }
    None
}

/// Extract the partial variable/property identifier after a sigil (`$` or `_`)
/// on the current line.
///
/// For `$pl` → returns `"pl"`. For just `$` → returns `""`.
/// Stops at characters that aren't valid in variable identifiers or
/// dot-notation paths (spaces, operators, delimiters, etc.).
pub(super) fn extract_partial_after_sigil(before_cursor: &str, sigil: char) -> &str {
    if let Some(pos) = before_cursor.rfind(sigil) {
        let after = &before_cursor[pos + 1..];
        let end = after
            .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .unwrap_or(after.len());
        &after[..end]
    } else {
        ""
    }
}

/// Compute a `FormatTextEdit` for variable/property completions.
///
/// Follows the same design as `compute_macro_text_edit`: the sigil (`$`/`_`)
/// stays in the document and is NOT covered by the textEdit range.
/// VS Code uses the text within the textEdit range as the filter prefix,
/// so covering the `$` would break client-side filtering (our filter_text
/// values are bare names like "player", not "$player").
///
/// When the user types `$pl` and hits Ctrl+Space:
/// - `partial` = `"pl"` (after the `$`)
/// - textEdit replaces `"pl"` with `"player"`
/// - The `$` remains in the document → result: `$player`
pub(super) fn compute_variable_text_edit(
    partial: &str,
    line: u32,
    character: u32,
    replacement: &str,
) -> Option<crate::types::FormatTextEdit> {
    use crate::types::FormatTextEdit;

    let partial_len = partial.chars().count() as u32;
    let start_char = character.saturating_sub(partial_len);
    Some(FormatTextEdit {
        start_line: line,
        start_character: start_char,
        end_line: line,
        end_character: character,
        new_text: replacement.to_string(),
    })
}

/// Find the `MacroArgRef` at a byte offset in the workspace.
pub(super) fn find_macro_arg_ref_at_offset(
    workspace: &knot_core::Workspace,
    uri: &url::Url,
    byte_offset: usize,
) -> Option<knot_core::passage::MacroArgRef> {
    let doc = workspace.get_document(uri)?;
    for passage in &doc.passages {
        for arg_ref in &passage.macro_arg_refs {
            if passage.span_contains_abs_offset(&arg_ref.span, byte_offset) {
                return Some(arg_ref.clone());
            }
        }
    }
    None
}

/// Find the variable path before a dot on the current line.
///
/// Tries multiple strategies:
/// 1. Arena tree via `variable_path_at_offset()` (most accurate)
/// 2. Scan `before_cursor` for `$name` or `_name` pattern (fallback)
pub(super) fn find_variable_path_before_dot(
    workspace: &knot_core::Workspace,
    uri: &url::Url,
    text: &str,
    byte_offset: usize,
    before_cursor: &str,
    registry: &SugarCubeRegistry,
) -> Option<String> {
    // ── Strategy 2 (line-based scan) runs FIRST ──────────────────
    //
    // When the text before the cursor contains a variable sigil (`$` or `_`),
    // the line-based scan is more reliable than the arena offset-based lookup.
    // The arena lookup's `build_path_for_segment` can return incomplete paths
    // when the root variable node is found but the deep path isn't resolved.
    // The line scan directly extracts the full dot-path from the source text.
    //
    // We run Strategy 2 first and only fall back to Strategy 1 when no sigil
    // is present in the text (e.g., namespace completions like `State.`).
    let before_dot = before_cursor.trim_end_matches('.');
    let sigils = ['$', '_'];
    for sigil in &sigils {
        if let Some(sigil_pos) = before_dot.rfind(*sigil) {
            let after_sigil = &before_dot[sigil_pos + 1..];
            // The part after the sigil should be a valid identifier path
            // (alphanumeric, underscores, dots for nested access, AND hyphens
            // — SugarCube property names can contain hyphens, e.g.
            // `$ITEMS.plain-bra-black`. The parser's `is_ident_char` includes
            // `-`, so we must match that here.)
            if after_sigil
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
            {
                let path = &before_dot[sigil_pos..];
                if !path.is_empty() {
                    // Validate: the path should exist in the arena tree.
                    // If it doesn't, it might be a partially-typed path
                    // that hasn't been parsed yet — still return it so
                    // that we can try completions at the right depth.
                    return Some(path.to_string());
                }
            }
        }
    }

    // ── Strategy 1: Arena tree offset-based lookup ───────────────
    //
    // Used as a fallback when no `$`/`_` sigil appears in the text
    // before the cursor. This covers namespace completions and cases
    // where the sigil is outside the current line fragment.
    if let Some(doc) = workspace.get_document(uri) {
        for passage in &doc.passages {
            if passage.contains_abs_offset(byte_offset) {
                let abs_start = passage.abs_offset(passage.span.start).min(text.len());
                let header_end = text[abs_start..]
                    .find('\n')
                    .map(|n| abs_start + n + 1)
                    .unwrap_or(abs_start);
                let body_offset = byte_offset.saturating_sub(header_end);
                let file_uri_str = uri.to_string();
                if let Some(path) =
                    registry
                        .variables()
                        .path_at_offset(&file_uri_str, &passage.name, body_offset)
                {
                    return Some(path);
                }
            }
        }
    }

    None
}

/// Find a global namespace name before a dot on the current line.
pub(super) fn find_namespace_before_dot(
    before_cursor: &str,
    plugin: &dyn FormatPlugin,
) -> Option<String> {
    let before_dot = before_cursor.trim_end_matches('.');
    let ident = before_dot
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
        .next()?;
    if plugin.global_object_names().contains(ident) {
        Some(ident.to_string())
    } else {
        None
    }
}

/// Extract partial text typed inside a quoted string on the current line.
///
/// Finds the last unmatched opening `"` and returns the text between it
/// and the cursor. This is used to extract partial passage names when
/// the user types `<<goto "Gar` — returning "Gar" for filtering.
pub(super) fn extract_partial_in_quote(before_cursor: &str) -> String {
    // Find the last opening quote that isn't closed
    let mut quote_positions: Vec<usize> = Vec::new();
    let bytes = before_cursor.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            // Check if escaped
            let mut backslash_count = 0;
            let mut j = i;
            while j > 0 && bytes[j - 1] == b'\\' {
                backslash_count += 1;
                j -= 1;
            }
            if backslash_count % 2 == 0 {
                quote_positions.push(i);
            }
        }
        i += 1;
    }
    // If there's an odd number of quotes, the last one is unmatched (open)
    if quote_positions.len() % 2 == 1 {
        let last_open = quote_positions[quote_positions.len() - 1];
        let partial = &before_cursor[last_open + 1..];
        return partial.to_string();
    }
    String::new()
}

/// Detect passage-in-quote context from before_cursor text.
///
/// Used when the `" ` trigger fires but the span-based data doesn't
/// find a MacroArgRef (e.g., when the user just typed the opening
/// quote and the AST hasn't been updated yet).
pub(super) fn detect_passage_in_quote(
    before_cursor: &str,
    plugin: &dyn FormatPlugin,
) -> Option<crate::types::CompletionContext> {
    let passage_arg_names = plugin.passage_arg_macro_names();
    if passage_arg_names.is_empty() {
        return None;
    }

    // Find the most recent macro open context
    let mut best_match: Option<(&str, usize)> = None;
    for &macro_name in &passage_arg_names {
        let open_pattern = plugin.format_macro_label(macro_name);
        let open_prefix = open_pattern
            .trim_end_matches('>')
            .trim_end_matches(')')
            .trim_end_matches(']')
            .trim_end_matches('}');

        if let Some(pos) = before_cursor.rfind(open_prefix) {
            match best_match {
                None => best_match = Some((macro_name, pos)),
                Some((_, prev_pos)) if pos > prev_pos => best_match = Some((macro_name, pos)),
                _ => {}
            }
        }
    }

    let (macro_name, open_pos) = best_match?;
    let after_open = &before_cursor[open_pos..];

    // Must not contain the closing delimiter
    let macro_label = plugin.format_macro_label(macro_name);
    let close_delim = if macro_label.starts_with("<<") {
        ">>"
    } else if macro_label.starts_with('(') {
        ")"
    } else if macro_label.starts_with('[') {
        "]"
    } else if macro_label.starts_with("{{") {
        "}}"
    } else {
        ""
    };
    if !close_delim.is_empty() && after_open.contains(close_delim) {
        return None;
    }

    // Check if we're inside a quoted string
    let is_in_quote = after_open.matches('"').count() % 2 == 1;
    if !is_in_quote {
        return None;
    }

    Some(crate::types::CompletionContext::MacroPassageRef {
        target: String::new(),
        macro_name: macro_name.to_string(),
        has_body: false,
    })
}

/// Resolve the macro name and has_body from the line text when the
/// span-based `MacroArgRef` data isn't available yet.
///
/// Scans `before_cursor` for the most recent `<<name` pattern and checks
/// whether the macro is a body macro (container) via the plugin catalog.
/// This is the fallback for the PassageRef semantic token case where we
/// have the token text but no MacroArgRef at the offset.
pub(super) fn resolve_macro_name_from_offset(
    before_cursor: &str,
    _byte_offset: usize,
    plugin: &dyn FormatPlugin,
) -> (String, bool) {
    // Find the most recent << before the cursor
    if let Some(delim_pos) = before_cursor.rfind("<<") {
        let after = &before_cursor[delim_pos + 2..];
        // Extract the macro name (first word after <<)
        let name = after
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .next()
            .unwrap_or("");
        if !name.is_empty() {
            let has_body = plugin
                .find_macro(name)
                .map(|m| m.body != crate::types::BodyRequirement::Never)
                .unwrap_or(false);
            return (name.to_string(), has_body);
        }
    }
    (String::new(), false)
}

/// Find the namespace token that immediately precedes a given byte offset.
pub(super) fn find_preceding_namespace_token(
    token_groups: &[crate::plugin::PassageTokenGroup],
    text: &str,
    before_offset: usize,
) -> String {
    use crate::plugin::SemanticTokenType;

    let mut best_name = String::new();
    let mut best_end = 0usize;

    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != SemanticTokenType::Namespace {
                continue;
            }
            let abs_end = token.start + group_offset + token.length;
            if abs_end <= before_offset && before_offset - abs_end <= 10 && abs_end > best_end {
                let abs_start = token.start + group_offset;
                if abs_start < text.len() && abs_end <= text.len() {
                    best_name = text[abs_start..abs_end].to_string();
                    best_end = abs_end;
                }
            }
        }
    }

    best_name
}

/// Find the passage header name at a given line number.
pub(super) fn find_passage_header_at_position(text: &str, line: u32) -> Option<String> {
    let line_text = text.lines().nth(line as usize)?;
    if let Some(stripped) = line_text.strip_prefix("::") {
        let name = crate::header::extract_passage_name(stripped);
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

/// Find the name of the passage that encloses `line` in `text`.
///
/// Scans backwards from `line` for the nearest `:: Name` header. If the
/// cursor is itself on a passage header line, returns that passage's
/// name (matching [`find_passage_header_at_position`]). Returns `None`
/// if there is no preceding passage header — i.e., the line lives in
/// the pre-story preamble (scripting, stylesheets, story metadata).
///
/// This is the passage-resolution helper used by completion contexts
/// that need to scope SugarCube `_` temporary variables to their
/// declaring passage. Temp vars are passage-scoped at runtime; without
/// this resolution, the LSP would leak `_foo` from `:: Start` into
/// completions for `:: Inventory`.
///
/// [`find_passage_header_at_position`]: find_passage_header_at_position
pub(super) fn find_enclosing_passage_name(text: &str, line: u32) -> Option<String> {
    // Walk backwards from `line` (inclusive) — the cursor might itself
    // be on a header line. We stop at the first `::` header that
    // yields a non-empty passage name.
    let lines: Vec<&str> = text.lines().collect();
    let mut idx = (line as usize).min(lines.len().saturating_sub(1));
    loop {
        let line_text = lines.get(idx)?;
        if let Some(rest) = line_text.strip_prefix("::") {
            let name = crate::header::extract_passage_name(rest);
            if !name.is_empty() {
                return Some(name);
            }
        }
        if idx == 0 {
            break;
        }
        idx -= 1;
    }
    None
}
