//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Compute variable diagnostics for the workspace via the format plugin.
///
/// This is invoked on-demand from `try_variable_hover` to surface diagnostics
/// (unused variable, redundant write, unknown property, availability hint)
/// in the hover popup. Computing on every hover is acceptable because the
/// state variable registry + diagnostic computation is fast for typical
/// workspace sizes (hundreds of passages). If this becomes a hot path,
/// cache the result on `ServerStateInner` keyed by workspace version.
pub(super) fn collect_variable_diagnostics(
    plugin: &dyn fmt_plugin::FormatPlugin,
    workspace: &knot_core::Workspace,
) -> Vec<knot_formats::types::VariableDiagnostic> {
    let start_passage = workspace
        .metadata
        .as_ref()
        .map(|m| m.start_passage.as_str())
        .unwrap_or("Start");
    let state_registry = plugin.build_state_variable_registry(workspace);
    plugin.compute_variable_diagnostics(workspace, start_passage, &state_registry)
}

/// Build the standard hover text for a passage target (used by link hover and
/// macro arg ref hover). Renders the passage name, file URI, link counts,
/// tags, and any persistent variable reads/writes declared in the passage.
///
/// Extracted from `try_macro_arg_ref_hover` and `try_link_hover` to DRY up
/// the duplicated template.
pub(super) fn build_passage_target_hover_text(
    target: &str,
    target_doc: &knot_core::Document,
    target_passage: &Passage,
    workspace: &knot_core::Workspace,
) -> String {
    build_passage_target_hover_text_impl(target, target_doc, target_passage, workspace, false)
}

/// Compact form of passage target hover text, used for passage REFERENCES
/// (e.g., the `"Shop"` arg in `<<link "Talk" "Shop">>`).
///
/// A reference is NOT the same as the passage definition — the user is
/// asking "what does this reference point to?", not "tell me everything
/// about this passage". The compact form shows just:
/// - Passage name
/// - Tags (if any)
/// - Incoming link count (context: how much this passage is referenced)
///
/// It does NOT show:
/// - File path (the user can Ctrl+Click to navigate; path is noise here)
/// - Links out (irrelevant when you're looking at a reference)
/// - Variables written/read (too much detail for a reference hover)
pub(super) fn build_passage_target_hover_text_compact(
    target: &str,
    target_doc: &knot_core::Document,
    target_passage: &Passage,
    workspace: &knot_core::Workspace,
) -> String {
    build_passage_target_hover_text_impl(target, target_doc, target_passage, workspace, true)
}

fn build_passage_target_hover_text_impl(
    target: &str,
    target_doc: &knot_core::Document,
    target_passage: &Passage,
    workspace: &knot_core::Workspace,
    compact: bool,
) -> String {
    let incoming = helpers::count_incoming_links(workspace, target);

    if compact {
        // Compact form for references: name + tags + incoming count only.
        let tags_str = if target_passage.tags.is_empty() {
            "none".to_string()
        } else {
            target_passage.tags.join(", ")
        };
        return format!(
            "**{}** `Passage`\n\nTags: {} | Referenced by {} passage(s)",
            target, tags_str, incoming
        );
    }

    // Full form for [[link]] hovers and passage header hovers.
    // Show workspace-relative path instead of full file:// URI.
    // Authors don't want to see "file:///D:/codeWS/twine/..." — just
    // "src/passages/newtest.twee" or similar.
    let display_path = workspace
        .root_uri
        .to_file_path()
        .ok()
        .and_then(|root| {
            // target_doc.uri is a file:// URL — convert to path
            target_doc.uri.to_file_path().ok().and_then(|doc_path| {
                doc_path
                    .strip_prefix(&root)
                    .ok()
                    .map(|p| p.display().to_string())
            })
        })
        .unwrap_or_else(|| {
            // Fallback: show just the filename
            target_doc
                .uri
                .path_segments()
                .and_then(|mut s| s.next_back())
                .unwrap_or("unknown")
                .to_string()
        });

    let mut hover_text = format!(
        "**{}**\n\n{}\nLinks out: {} | Incoming: {} | Tags: {}",
        target,
        display_path,
        target_passage.links.len(),
        incoming,
        if target_passage.tags.is_empty() {
            "none".to_string()
        } else {
            target_passage.tags.join(", ")
        }
    );

    if !target_passage.vars.is_empty() {
        let writes: Vec<&str> = target_passage
            .persistent_variable_inits()
            .map(|v| v.name.as_str())
            .collect();
        let reads: Vec<&str> = target_passage
            .persistent_variable_reads()
            .map(|v| v.name.as_str())
            .collect();
        if !writes.is_empty() {
            hover_text.push_str(&format!("\nVariables written: {}", writes.join(", ")));
        }
        if !reads.is_empty() {
            hover_text.push_str(&format!("\nVariables read: {}", reads.join(", ")));
        }
    }

    hover_text
}

/// Try to show hover info for a passage header when the cursor is on the
/// header line.
///
/// Uses `passage.header_name_span` for the hover range when available
/// (SugarCube), falling back to [`compute_passage_header_range`] for formats
/// that don't populate it.
pub(super) fn try_passage_header_hover(
    text: &str,
    byte_offset: usize,
    passage: &Passage,
    workspace: &knot_core::Workspace,
) -> Option<Hover> {
    // Check if the cursor is on the header line of this passage.
    // The header line starts at passage.span.start and ends at the first newline.
    let span_start = passage.abs_offset(passage.span.start).min(text.len());
    let header_end = text[span_start..]
        .find('\n')
        .map(|n| span_start + n)
        .unwrap_or(passage.abs_offset(passage.span.end).min(text.len()));

    if byte_offset < span_start || byte_offset > header_end {
        return None;
    }

    let passage_name = &passage.name;
    let links_count = passage.links.len();
    let vars_count = passage.vars.len();
    let tags = if passage.tags.is_empty() {
        "none".to_string()
    } else {
        passage.tags.join(", ")
    };

    let incoming = helpers::count_incoming_links(workspace, passage_name);
    let incoming_sources = helpers::incoming_link_sources(workspace, passage_name);

    // Check for special passage info
    let special_info = if passage.is_special {
        if let Some(ref def) = passage.special_def {
            let behavior = match &def.behavior {
                knot_core::passage::SpecialPassageBehavior::Startup => "Startup",
                knot_core::passage::SpecialPassageBehavior::PassageReady => "PassageReady",
                knot_core::passage::SpecialPassageBehavior::Chrome => "Chrome",
                knot_core::passage::SpecialPassageBehavior::ChromeInterceptor => {
                    "Chrome Interceptor"
                }
                knot_core::passage::SpecialPassageBehavior::StructureTemplate => {
                    "Structure Template"
                }
                knot_core::passage::SpecialPassageBehavior::Metadata => "Metadata",
                knot_core::passage::SpecialPassageBehavior::ScriptInjection => "Script Injection",
                knot_core::passage::SpecialPassageBehavior::StyleInjection => "Style Injection",
                knot_core::passage::SpecialPassageBehavior::Custom(s) => s,
            };
            let layer = match &def.layer {
                knot_core::passage::SpecialPassageLayer::TwineCore => " (Twine Core)",
                knot_core::passage::SpecialPassageLayer::LegacyCore => " (Legacy Core)",
                knot_core::passage::SpecialPassageLayer::StoryFormat => "",
                knot_core::passage::SpecialPassageLayer::UserDefined => " (User Defined)",
            };
            format!("\n**Special passage** — {}{}", behavior, layer)
        } else {
            "\n**Special passage**".to_string()
        }
    } else {
        String::new()
    };

    let incoming_detail = if incoming <= 5 && !incoming_sources.is_empty() {
        format!("{} ({})", incoming, incoming_sources.join(", "))
    } else {
        incoming.to_string()
    };

    let hover_text = format!(
        "**{}**{}\n\nLinks: {} | Variables: {} | Tags: {} | Incoming: {}",
        passage_name, special_info, links_count, vars_count, tags, incoming_detail
    );

    // Compute an explicit hover range covering the full passage name.
    // Prefer the span-based header_name_span when available (avoids re-parsing
    // the header line). Fall back to compute_passage_header_range for formats
    // that don't populate header_name_span.
    let hover_range = if let Some(ref name_span) = passage.header_name_span {
        helpers::byte_range_to_lsp_range(text, &passage.abs_range(name_span))
    } else {
        let position = helpers::byte_offset_to_position(text, byte_offset);
        compute_passage_header_range(text, position)?
    };

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover_text,
        }),
        range: Some(hover_range),
    })
}

/// Compute the hover range for a passage header at the given position.
///
/// The range covers the full passage name (including spaces) from `::` to
/// the end of the name, so that hovering over `:: My Passage Name` shows
/// the hover popup for the entire name, not just the first word.
///
/// Returns `None` if the position is not on a `::` header line.
///
/// This is used as a fallback when `passage.header_name_span` is not
/// available (e.g., for formats other than SugarCube).
fn compute_passage_header_range(text: &str, position: Position) -> Option<Range> {
    let line_text = text.lines().nth(position.line as usize)?;

    if !line_text.starts_with("::") {
        return None;
    }

    // Parse the passage name from the header, accounting for whitespace
    // between `::` and the name.
    let after_colons = &line_text[2..];
    let whitespace_len = after_colons.len() - after_colons.trim_start().len();
    // Trim trailing \r for CRLF robustness — mirrors the format plugins'
    // parse_header_line() CRLF fix.
    let rest = after_colons.trim_start().trim_end_matches('\r');

    // The name extends to the `[` bracket (for tags) or `{` (for JSON metadata)
    // or the end of the line. Strip JSON metadata first (must end with '}'),
    // then tags — matching the format plugins' parse_header_line() order.
    let rest_before_json = if let Some(brace_start) = rest.rfind('{') {
        if rest.ends_with('}') {
            &rest[..brace_start]
        } else {
            rest
        }
    } else {
        rest
    };
    // Use rfind('[') + ends_with(']') to match the lexer's tag detection.
    // This avoids false matches on '[' characters inside passage names.
    let name_end = if let Some(bracket_start) = rest_before_json.rfind('[') {
        if rest_before_json.ends_with(']') {
            bracket_start
        } else {
            rest_before_json.len()
        }
    } else {
        rest_before_json.len()
    };
    let name_text = rest_before_json[..name_end].trim_end();

    // Compute the byte offset where the name starts and ends.
    let name_byte_start = 2 + whitespace_len;
    let name_byte_end = name_byte_start + name_text.len();

    // Convert to UTF-16 for LSP.
    let utf16_start = helpers::utf16_len_up_to(line_text, name_byte_start);
    let utf16_end = helpers::utf16_len_up_to(line_text, name_byte_end);

    Some(Range {
        start: Position {
            line: position.line,
            character: utf16_start,
        },
        end: Position {
            line: position.line,
            character: utf16_end,
        },
    })
}
