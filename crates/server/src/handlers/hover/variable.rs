//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for a variable when the cursor is within a
/// variable's span.
///
/// Uses `passage.vars[].span` from the workspace index for precise byte-range
/// matching instead of manually scanning the line text for format-specific
/// sigils (e.g., `$var` or `_var`). This correctly handles multi-byte
/// characters, variables inside macros, and avoids false negatives from
/// manual char scanning.
pub(super) fn try_variable_hover(
    text: &str,
    byte_offset: usize,
    doc: Option<&knot_core::Document>,
    workspace: &knot_core::Workspace,
    plugin: &dyn fmt_plugin::FormatPlugin,
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
) -> Option<Hover> {
    let doc = doc?;

    // Guard: if there's a Function semantic token at the cursor position,
    // skip variable hover. The cursor is on a method/function name (e.g.,
    // `.last()` in `$arr.last()`), which should be handled by
    // try_function_hover (step 4c, runs after this). Without this guard,
    // variable hover fires on the method name because the var_op's span
    // (from the fallback var_refs scanner) covers the full dot-path
    // (`$arr.last`), and the cursor on "last" falls within that span.
    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != knot_formats::plugin::SemanticTokenType::Function {
                continue;
            }
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            if byte_offset >= abs_start && byte_offset < abs_end {
                return None; // Function token at cursor — let function hover handle it
            }
        }
    }

    // Iterate over all passages in the document and check if the cursor
    // byte offset falls within any variable's span. Variable spans are
    // absolute byte offsets in the document text, so we can match directly.
    for passage in &doc.passages {
        for var in &passage.vars {
            if passage.span_contains_abs_offset(&var.span, byte_offset) {
                let var_name = &var.name;
                let enclosing_passage_name = passage.name.clone();
                let is_temporary = var.is_temporary;

                // Find where this variable is written and read.
                //
                // For persistent (`$`) vars: aggregate across the whole
                // workspace — they are workspace-global by design.
                //
                // For temporary (`_`) vars: scope to the enclosing passage
                // only. SugarCube `_` variables are passage-scoped at
                // runtime; aggregating across passages gives wrong counts.
                //
                // TODO: replace with `plugin.variable_hover_info` in Phase 6
                // (provide_hover refactor) so the `is_temporary` check
                // lives in the format plugin, not the handler.
                let mut write_locations: Vec<String> = Vec::new();
                let mut read_count = 0;
                for wp_doc in workspace.documents() {
                    for wp_passage in &wp_doc.passages {
                        if is_temporary && wp_passage.name != enclosing_passage_name {
                            continue;
                        }
                        for v in &wp_passage.vars {
                            if v.name == *var_name {
                                match v.kind {
                                    knot_core::passage::VarKind::Init => {
                                        write_locations.push(wp_passage.name.clone());
                                    }
                                    knot_core::passage::VarKind::Read => {
                                        read_count += 1;
                                    }
                                }
                            }
                        }
                    }
                }

                let write_info = if write_locations.is_empty() {
                    "Never written".to_string()
                } else if write_locations.len() <= 5 {
                    format!("Written in: {}", write_locations.join(", "))
                } else {
                    format!("Written in {} passages", write_locations.len())
                };

                // Determine the sigil character from the variable name.
                // The name includes the sigil (e.g., "$gold" or "_temp"),
                // so the first character is the sigil. Fall back to the
                // format's primary sigil (or 'v' if none) rather than
                // hardcoding SugarCube's `$` — Snowman/Chapbook have no
                // sigil and would otherwise misroute to SugarCube's lookup.
                let sigil = var_name.chars().next().unwrap_or_else(|| {
                    plugin
                        .variable_sigils()
                        .first()
                        .map(|s| s.sigil)
                        .unwrap_or('v')
                });
                let sigil_desc = plugin.describe_variable_sigil(sigil).unwrap_or("variable");

                let var_type = plugin
                    .resolve_variable_sigil(sigil)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "variable".to_string());

                // Surface variable diagnostics (unused, redundant write,
                // unknown property, availability hint). These are computed
                // by the format plugin and filtered to the hovered variable.
                // For temps, only diagnostics from the enclosing passage
                // are shown (matching the scope rule above).
                let var_diagnostics = collect_variable_diagnostics(plugin, workspace);
                let relevant_diagnostics: Vec<_> = var_diagnostics
                    .iter()
                    .filter(|d| {
                        d.message.contains(var_name)
                            && (!is_temporary || d.passage_name == enclosing_passage_name)
                    })
                    .collect();

                // Meaningfulness gate: skip variable hover when there's
                // nothing to add beyond what's visible in the code. A
                // variable with no diagnostics, at most one write, and at
                // most one read is trivially obvious — the code speaks for
                // itself. Don't fill the hover with filler.
                //
                // EXCEPTION: persistent (`$`) variables in prose context
                // (naked variable markup like `You have $gold gold.`) should
                // always get hover — the user explicitly wrote the variable
                // to be rendered, and they want to know what it refers to
                // even if it's only read once. We can't directly tell if a
                // var is in prose vs code from `VarOp`, but persistent vars
                // with `Read` kind are overwhelmingly prose references (code
                // reads go through `<<run>>`/`<<set>>` which have JS analysis).
                // So we skip the gate for persistent read-only vars.
                let is_persistent_read =
                    !is_temporary && matches!(var.kind, knot_core::passage::VarKind::Read);
                if !is_persistent_read
                    && relevant_diagnostics.is_empty()
                    && write_locations.len() <= 1
                    && read_count <= 1
                {
                    return None;
                }

                let mut hover_text = format!(
                    "**{}** `{}`\n\n{}\nRead in {} location(s)",
                    var_name, var_type, write_info, read_count
                );

                // Only show sigil description for temps (the scoping rule
                // is non-obvious). For persistent vars, the sigil is
                // self-explanatory boilerplate — skip it.
                if is_temporary {
                    hover_text.push_str(&format!("\n\n---\n\n{}", sigil_desc));
                }

                if !relevant_diagnostics.is_empty() {
                    hover_text.push_str("\n\n---\n\n**Diagnostics:**\n");
                    for d in &relevant_diagnostics {
                        hover_text.push_str(&format!("- {}\n", d.message));
                    }
                }

                // Convert the variable's byte span to an LSP Range.
                let hover_range =
                    helpers::byte_range_to_lsp_range(text, &passage.abs_range(&var.span));

                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: hover_text,
                    }),
                    range: Some(hover_range),
                });
            }
        }
    }

    // ── Fallback: text-scan for prose `$var` / `_var` ──────────────────
    //
    // If `passage.vars` didn't have an entry covering the cursor (e.g., the
    // document hasn't been re-parsed yet, or the variable scanner missed an
    // edge case), fall back to scanning the text around the cursor for a
    // `$name` or `_name` pattern. This makes hover robust against stale
    // passage.vars and ensures prose `$vars` always get hover info.
    if let Some((sigil, name, name_start, name_end)) = scan_variable_at_cursor(text, byte_offset) {
        let var_name = format!("{}{}", sigil, name);
        let is_temporary = sigil == '_';

        // Determine the sigil description from the plugin (format-agnostic).
        let sigil_desc = plugin.describe_variable_sigil(sigil).unwrap_or("variable");
        let var_type = plugin
            .resolve_variable_sigil(sigil)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "variable".to_string());

        // For persistent (`$`) vars: aggregate across the whole workspace.
        // For temporary (`_`) vars: scope to the enclosing passage only.
        let enclosing_passage_name = doc
            .passages
            .iter()
            .find(|p| p.contains_abs_offset(byte_offset))
            .map(|p| p.name.clone());

        let mut write_locations: Vec<String> = Vec::new();
        let mut read_count = 0;
        for wp_doc in workspace.documents() {
            for wp_passage in &wp_doc.passages {
                if is_temporary && Some(&wp_passage.name) != enclosing_passage_name.as_ref() {
                    continue;
                }
                for v in &wp_passage.vars {
                    if v.name == var_name {
                        match v.kind {
                            knot_core::passage::VarKind::Init => {
                                write_locations.push(wp_passage.name.clone());
                            }
                            knot_core::passage::VarKind::Read => {
                                read_count += 1;
                            }
                        }
                    }
                }
            }
        }

        let write_info = if write_locations.is_empty() {
            "Never written".to_string()
        } else if write_locations.len() <= 5 {
            format!("Written in: {}", write_locations.join(", "))
        } else {
            format!("Written in {} passages", write_locations.len())
        };

        let mut hover_text = format!(
            "**{}** `{}`\n\n{}\nRead in {} location(s)",
            var_name, var_type, write_info, read_count
        );
        if is_temporary {
            hover_text.push_str(&format!("\n\n---\n\n{}", sigil_desc));
        }

        let hover_range = helpers::byte_range_to_lsp_range(text, &(name_start..name_end));
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_text,
            }),
            range: Some(hover_range),
        });
    }

    None
}

/// Scan the text around `byte_offset` for a `$name` or `_name` variable
/// reference in prose.
///
/// Returns `(sigil, name, span_start, span_end)` where `sigil` is `$` or `_`,
/// `name` is the variable name WITHOUT the sigil, and `span_start..span_end`
/// is the byte range covering the SIGIL PLUS the name (the full variable
/// token, including any property path).
///
/// Returns `None` if the cursor isn't on a `$name` / `_name` pattern, or if
/// the pattern is inside a context where `$` / `_` isn't a variable sigil
/// (e.g., `_` in the middle of a word like `snake_case`, or `$` at end of
/// text without a following identifier).
pub(super) fn scan_variable_at_cursor(
    text: &str,
    byte_offset: usize,
) -> Option<(char, String, usize, usize)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if byte_offset > len {
        return None;
    }

    // Helper: is byte an identifier char (SugarCube variables allow _ and $ in
    // continuation, but we only match `$` or `_` as sigils at the start)?
    let is_ident_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let is_ident_start = |b: u8| b.is_ascii_alphabetic() || b == b'_';
    let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

    // Case 1: cursor is ON the sigil (`$` or `_`).
    let on_sigil = match bytes.get(byte_offset) {
        Some(&b'$') => Some('$'),
        Some(&b'_') => {
            // `_` is only a temp-var sigil at a word boundary (not inside
            // `snake_case` etc.).
            let prev_is_word = byte_offset > 0
                && bytes
                    .get(byte_offset - 1)
                    .copied()
                    .is_some_and(is_word_char);
            if prev_is_word { None } else { Some('_') }
        }
        _ => None,
    };
    if let Some(sigil) = on_sigil {
        // Scan forward for the name + property path.
        let name_start = byte_offset + 1;
        if name_start >= len || !is_ident_start(bytes[name_start]) {
            return None;
        }
        let mut name_end = name_start + 1;
        // Allow property path: .prop.prop2 (prop names allow hyphens too).
        while name_end < len {
            let b = bytes[name_end];
            if is_ident_char(b) {
                name_end += 1;
            } else if b == b'.'
                && name_end + 1 < len
                && (is_ident_start(bytes[name_end + 1]) || bytes[name_end + 1] == b'_')
            {
                name_end += 1; // consume `.`
                while name_end < len && (is_ident_char(bytes[name_end]) || bytes[name_end] == b'-')
                {
                    name_end += 1;
                }
            } else {
                break;
            }
        }
        let name = text[name_start..name_end].to_string();
        return Some((sigil, name, byte_offset, name_end));
    }

    // Case 2: cursor is ON the name part. Scan backward for the sigil.
    if byte_offset < len && is_ident_char(bytes[byte_offset]) {
        let mut probe = byte_offset;
        // Walk backward through ident chars and `-`. Special case: when we
        // encounter `_`, check if it's a temp-var sigil (at a word boundary)
        // or part of a snake_case identifier. If it's a sigil, stop here
        // (don't include it in the name).
        while probe > 0 {
            let prev = bytes[probe - 1];
            if prev == b'_' {
                // `_` is a sigil only if NOT preceded by a word char (i.e.,
                // it's at a word boundary). Otherwise it's part of an
                // identifier like `snake_case`.
                let prev_prev_is_word = probe >= 2 && is_word_char(bytes[probe - 2]);
                if !prev_prev_is_word {
                    // `_` is the sigil — stop here.
                    break;
                }
                // `_` is part of `snake_case` — continue walking backward.
                probe -= 1;
            } else if is_ident_char(prev) || prev == b'-' {
                probe -= 1;
            } else {
                break;
            }
        }
        // Also walk back through `.prop` segments.
        // (probe is at the start of the current segment.)
        // Now check if there's a `.` before probe, and another segment, and so on.
        // Walk back through segments separated by `.`.
        let mut name_start = probe;
        while name_start > 0
            && bytes[name_start - 1] == b'.'
            && name_start >= 2
            && (is_ident_char(bytes[name_start - 2]) || bytes[name_start - 2] == b'-')
        {
            // Walk back through the previous segment.
            name_start -= 1; // skip the `.`
            while name_start > 0
                && (is_ident_char(bytes[name_start - 1]) || bytes[name_start - 1] == b'-')
            {
                name_start -= 1;
            }
        }
        // Now `name_start` is the start of the first segment. The sigil
        // should be at `name_start - 1`.
        if name_start == 0 {
            return None;
        }
        let sigil_byte = bytes[name_start - 1];
        let sigil = match sigil_byte {
            b'$' => '$',
            b'_' => {
                // `_` is only a sigil at a word boundary.
                let prev_is_word =
                    name_start >= 2 && bytes.get(name_start - 2).copied().is_some_and(is_word_char);
                if prev_is_word {
                    return None;
                }
                // Also, the first segment must start with an ident-start char
                // (i.e., the char at name_start must be a letter, not `_`).
                // SugarCube `_var` requires `_` followed by a letter.
                if !is_ident_start(bytes[name_start]) {
                    return None;
                }
                '_'
            }
            _ => return None,
        };
        // Scan forward for the end of the name (including property path).
        let mut name_end = name_start + 1;
        while name_end < len {
            let b = bytes[name_end];
            if is_ident_char(b) {
                name_end += 1;
            } else if b == b'.'
                && name_end + 1 < len
                && (is_ident_start(bytes[name_end + 1]) || bytes[name_end + 1] == b'_')
            {
                name_end += 1;
                while name_end < len && (is_ident_char(bytes[name_end]) || bytes[name_end] == b'-')
                {
                    name_end += 1;
                }
            } else {
                break;
            }
        }
        let name = text[name_start..name_end].to_string();
        return Some((sigil, name, name_start - 1, name_end));
    }

    None
}
