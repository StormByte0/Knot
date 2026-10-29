//! Moved verbatim from the former `sugarcube/mod.rs` monolith
//! (pure move, plan §4.6 — no signature or logic changes).

use super::*;

// ---------------------------------------------------------------------------
// HTML-stratum completion guard (plan.md Phase 2.5)
// ---------------------------------------------------------------------------

/// The zone stratum at a cursor position, classified from the ZoneMap (the
/// per-byte language authority built by `zoning::build_from_ast`).
///
/// This is the completion-side answer to the F6 bug class: position-based
/// features must CONSULT the zone structure instead of re-deriving context
/// by re-scanning raw text (the exact behavior that made `<<` inside a tag
/// interior or a JS left-shift inside `<script>` look like macro markup).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionZone {
    /// Wikified markup content — every completion behavior stays active.
    Prose,
    /// Inside a tag interior (start-tag bytes or attribute values).
    /// `position` says WHERE in the tag: the value of an `@attr`/
    /// `sc-eval:attr` directive is a TwineScript expression where SugarCube
    /// variable completions stay meaningful (upstream evaluates it via
    /// `Scripting.evalTwineScript`); a plain value offers nothing; an
    /// attribute-NAME position offers the HTML attribute completions
    /// (incl. the directive families); the element-name region offers
    /// nothing.
    TagInterior { position: AttrPosition },
    /// Inside a `<script>` raw body — JS with SugarCube `$var` syntax; the
    /// variable sigils stay live, all markup completions suppressed.
    RawScript,
    /// Inside a `<style>` raw body — pure CSS; everything suppressed.
    RawStyle,
}

/// Classify the completion zone at `byte_offset` by reparsing the enclosing
/// normal passage and consulting its ZoneMap.
///
/// Conservative fallbacks, all returning [`CompletionZone::Prose`] (legacy
/// behavior): no enclosing header, cursor on a header line, code-like
/// passages ([script]/[stylesheet]/[style]/[init]/[widget] tags or
/// StoryInit/StoryData-style names — their bodies are not SugarCube markup
/// so the HTML stratum never applies), empty bodies, or uncovered gaps
/// between leaves.
impl SugarCubePlugin {
    /// Classify the completion zone at `byte_offset` through the plugin's
    /// zone analyzer ([`FormatPlugin::zone_analyze`], the Phase 4.1
    /// ZoneAnalyzer capability — the per-byte language authority).
    pub(super) fn completion_zone_at(&self, text: &str, byte_offset: usize) -> CompletionZone {
        // Line-start table (CRLF-safe: \n positions suffice for line starts).
        let mut line_starts = vec![0usize];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let last_start_idx = line_starts.len() - 1;
        let cursor_line = match line_starts.binary_search(&byte_offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1).min(last_start_idx),
        };

        // Walk backwards (inclusive) for the nearest `::` header.
        let mut header_idx = None;
        let mut header = None;
        for idx in (0..=cursor_line).rev() {
            let ls = line_starts[idx];
            let le = text[ls..].find('\n').map(|p| ls + p).unwrap_or(text.len());
            if let Some(h) = crate::header::parse_twee_header(&text[ls..le], ls) {
                header_idx = Some(idx);
                header = Some((h, le + 1)); // body starts after the header line
                break;
            }
        }
        let Some((h, body_start)) = header else {
            return CompletionZone::Prose;
        };
        if byte_offset < body_start {
            // Cursor on the header line itself — header completion contexts own
            // that region.
            return CompletionZone::Prose;
        }

        // Code-like passages: bodies are not SugarCube markup, so the HTML
        // stratum (and this guard) never applies. Tag matching is
        // case-insensitive (Twee 3); the name list is the code/JSON subset of
        // the special passages (markup passages like PassageHeader stay
        // guarded like any normal passage).
        let code_tags = ["script", "stylesheet", "style", "init", "widget"];
        if h.tags
            .iter()
            .any(|t| code_tags.contains(&t.to_ascii_lowercase().as_str()))
            || matches!(
                h.name.as_str(),
                "StoryInit"
                    | "Story JavaScript"
                    | "StoryCSS"
                    | "Story Stylesheet"
                    | "StoryData"
                    | "Story Metadata"
            )
        {
            return CompletionZone::Prose;
        }

        // Body extent: from after the header line to the next header (or EOF).
        let mut body_end = text.len();
        for &ls in line_starts.iter().skip(header_idx.unwrap() + 1) {
            let le = text[ls..].find('\n').map(|p| ls + p).unwrap_or(text.len());
            if crate::header::parse_twee_header(&text[ls..le], ls).is_some() {
                body_end = ls;
                break;
            }
        }
        if byte_offset >= body_end || body_start >= body_end {
            return CompletionZone::Prose;
        }
        let body = &text[body_start..body_end];
        let offset_in_body = byte_offset - body_start;

        // Phase 4.1: the zone map comes from the plugin's ZoneAnalyzer
        // capability (per-byte language authority) instead of a local
        // build_from_ast call — LSP features consume the analyzer, they do not
        // re-derive it.
        let zones = self.zone_analyze(body);
        let Some(leaf) = zones.leaf_at(offset_in_body) else {
            return CompletionZone::Prose;
        };
        // Trailing unterminated tag (the mid-typing shape): `<div cla|`
        // never closed, so the flat parser kept it as prose and no AST
        // HtmlTag exists — but the author is inside a tag, and attribute
        // completions must fire. Same-line only (multi-line unclosed tags
        // are a miss, prose after a stray unclosed tag must stay prose).
        if matches!(leaf.kind.language(), knot_core::zoning::Language::Markup)
            && let Some(zone) = unterminated_tag_zone(body, offset_in_body)
        {
            return zone;
        }
        match leaf.kind.language() {
            knot_core::zoning::Language::Html => CompletionZone::TagInterior {
                position: attr_position_at(
                    &parser::parse_passage_body(body, 0, ParseMode::Normal).nodes,
                    offset_in_body,
                ),
            },
            knot_core::zoning::Language::Js => CompletionZone::RawScript,
            knot_core::zoning::Language::Css => CompletionZone::RawStyle,
            knot_core::zoning::Language::Markup => CompletionZone::Prose,
        }
    }
}

/// Where the cursor sits inside an `AstNode::HtmlTag`'s OPEN tag (open spans
/// never nest, so the first containing tag wins).
///
/// Values' END is inclusive: a completion cursor sits one byte PAST the
/// last typed character, so after typing `@class="$` the cursor is at
/// `value.end` and variable completions must still fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttrPosition {
    /// Inside the VALUE of an `@attr`/`sc-eval:attr` directive.
    DirectiveValue,
    /// Inside the VALUE of a plain (non-directive) attribute.
    PlainValue,
    /// In the attribute-NAME region: after the tag name, between
    /// attributes, on a name or its directive sigil — where the HTML
    /// attribute completions fire.
    AttrName,
    /// Inside the element NAME region itself (`<di|v`) — completions
    /// suppressed (tag-name completion is not part of this feature).
    TagName,
}

fn attr_position_at(nodes: &[ast::AstNode], offset: usize) -> AttrPosition {
    attr_position_in(nodes, offset).unwrap_or(AttrPosition::TagName)
}

/// The mid-typing complement of [`attr_position_at`]: the cursor sits in
/// prose that is really an UNTERMINATED trailing tag (`<div cla|`, tag
/// never closed — the flat parser degrades it to text, so no AST node
/// exists to ask).
///
/// Returns the [`CompletionZone`] for the position inside the would-be
/// tag, or `None` when the cursor is not in that shape:
///
/// - The last `<` + ASCII letter before the cursor must be on the SAME
///   LINE — prose on later lines after a stray unclosed tag keeps its
///   prose completions (multi-line unclosed tags are a documented miss).
/// - That `<` must not be a macro opener (`<<if …`) — the byte before it
///   must not be another `<`.
/// - The tokenizer's detailed scan from that `<` must report `eof-in-tag`
///   with NO formed tag — a formed tag means the AST path owns it (or the
///   shape is a closed macro, which is markup).
///
/// The position walk is quote-aware: an unclosed quote at the cursor is a
/// VALUE position (directive vs. plain by the attribute name before its
/// `=`); otherwise the cursor is the element-name region until the name
/// run ends, and the attribute-name region after it.
fn unterminated_tag_zone(body: &str, cursor: usize) -> Option<CompletionZone> {
    let bytes = body.as_bytes();
    let cursor = cursor.min(bytes.len());
    // Last `<` + ASCII letter on the cursor's line, not a `<<` macro opener.
    let mut lt = None;
    let mut i = cursor;
    while i > 0 {
        i -= 1;
        if bytes[i] == b'\n' {
            break; // same-line gate
        }
        if bytes[i] == b'<'
            && bytes.get(i + 1).is_some_and(|b| b.is_ascii_alphabetic())
            && !bytes[..i].ends_with(b"<")
        {
            lt = Some(i);
            break;
        }
    }
    let lt = lt?;
    let scan = knot_core::html::scan_leading_tag_detailed(&body[lt..]);
    if scan.tag.is_some() || !scan.errors.iter().any(|e| e.code == "eof-in-tag") {
        return None;
    }

    // Element-name run, then the attribute region with quote tracking.
    let mut pos = lt + 1;
    while pos < cursor
        && (bytes[pos].is_ascii_alphanumeric() || matches!(bytes[pos], b'-' | b'_' | b':'))
    {
        pos += 1;
    }
    if cursor <= pos {
        return Some(CompletionZone::TagInterior {
            position: AttrPosition::TagName,
        });
    }

    // Attribute region: track the quoted value containing the cursor (if
    // any) and the attribute name that precedes its `=`.
    let mut quote: Option<u8> = None;
    let mut value_attr: Option<&str> = None;
    let mut j = pos;
    while j < cursor {
        let b = bytes[j];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None => {
                if b == b'"' || b == b'\'' {
                    quote = Some(b);
                    // The attribute name is the word before the last
                    // unquoted `=` before this quote.
                    let name = body[pos..j]
                        .rsplit_once('=')
                        .map(|(before_eq, _)| {
                            let trimmed = before_eq.trim_end();
                            let start = trimmed
                                .rfind(|c: char| c.is_ascii_whitespace())
                                .map(|p| trimmed[..=p].len())
                                .unwrap_or(0);
                            &trimmed[start..]
                        })
                        .unwrap_or("");
                    value_attr = Some(name);
                }
            }
        }
        j += 1;
    }
    if quote.is_some() {
        let position = match value_attr {
            Some(name) if name.starts_with('@') || name.starts_with("sc-eval:") => {
                AttrPosition::DirectiveValue
            }
            _ => AttrPosition::PlainValue,
        };
        return Some(CompletionZone::TagInterior { position });
    }
    Some(CompletionZone::TagInterior {
        position: AttrPosition::AttrName,
    })
}

fn attr_position_in(nodes: &[ast::AstNode], offset: usize) -> Option<AttrPosition> {
    for node in nodes {
        if let ast::AstNode::HtmlTag {
            name_span,
            attrs,
            children,
            open_span,
            full_span,
            ..
        } = node
        {
            // This tag's OPEN span contains the offset (open spans never
            // nest — the first match owns the verdict).
            if open_span.start <= offset && offset < open_span.end {
                // Element-name region — suppressed position.
                if name_span.start <= offset && offset <= name_span.end {
                    return Some(AttrPosition::TagName);
                }
                for a in attrs {
                    if let Some(vs) = &a.value_span
                        && vs.start <= offset
                        && offset <= vs.end
                    {
                        return Some(if a.directive.is_some() {
                            AttrPosition::DirectiveValue
                        } else {
                            AttrPosition::PlainValue
                        });
                    }
                }
                return Some(AttrPosition::AttrName);
            }
            // Between this tag's open and close: only a CHILD tag's open
            // span can claim the offset (content between elements is
            // prose — the zone map never routes it here).
            if full_span.start <= offset
                && offset < full_span.end
                && let Some(pos) = attr_position_in(children, offset)
            {
                return Some(pos);
            }
        }
    }
    None
}
