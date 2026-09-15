//! Unified zoning engine — SugarCube zone map builder.
//!
//! This module contains the **builder** for [`ZoneMap`]: the logic that walks
//! the SugarCube AST and produces a byte-covering array of leaf zones plus
//! a parallel array of macro-body context records.
//!
//! The **type definitions** ([`ZoneMap`], [`LeafZone`], [`LeafKind`],
//! [`MacroBody`], etc.) and **query methods** live in [`knot_core::zoning`],
//! because `ZoneMap` is stored on [`knot_core::passage::Passage`] and
//! `knot-core` must not depend on `knot-formats`. This module re-exports
//! those types for convenience.
//!
//! ## Coordinate system
//!
//! All spans in the resulting [`ZoneMap`] are **passage-relative** (0 =
//! passage head `::`), matching the canonical internal coordinate system.
//! The builder shifts body-relative AST spans → passage-relative at build
//! time via `body_offset_in_passage`. Translation to document-absolute LSP
//! positions happens at the LSP boundary only.
//!
//! ## Invariants
//!
//! - `leaves` is sorted by `span.start`.
//! - `leaves` covers the body region with no gaps and no overlaps (every byte
//!   is in exactly one leaf).
//! - `bodies` is sorted by `span.start` but MAY overlap (nested bodies).
//! - Every `LeafZone.body_idx` is either `None` (top-level) or a valid index
//!   into `bodies` whose `span` contains `leaf.span`.

// Re-export all zone types from `knot-core` so consumers using
// `knot_formats::zoning::*` don't need to change their imports.
pub use knot_core::zoning::{
    ErrorKind, LeafKind, LeafZone, MacroBody, MarkupKind, RawLanguage, TagPart, ZoneMap,
};

use std::ops::Range;

use crate::sugarcube::ast::{AstNode, ExprKind};
use crate::sugarcube::macros::find_macro;
use crate::sugarcube::registries::CustomMacroRegistry;
use crate::types::{BodyRequirement, MacroKind};

// ---------------------------------------------------------------------------
// MacroDefSnapshot — normalize builtin + custom macro lookups
// ---------------------------------------------------------------------------

/// A normalized snapshot of macro catalog data, used during zone building.
#[derive(Clone, Copy)]
struct MacroDefSnapshot {
    kind: MacroKind,
    body_requirement: BodyRequirement,
    body_is_raw: bool,
}

fn lookup_macro(name: &str, custom_macros: &CustomMacroRegistry) -> Option<MacroDefSnapshot> {
    // Try builtin catalog first.
    if let Some(def) = find_macro(name) {
        return Some(MacroDefSnapshot {
            kind: def.kind,
            body_requirement: def.body,
            body_is_raw: def.body_is_raw,
        });
    }
    // Check custom macro registry.
    if let Some(custom) = custom_macros.get(name) {
        let kind = match custom.body {
            BodyRequirement::Required | BodyRequirement::Optional => MacroKind::Container,
            BodyRequirement::Never => MacroKind::Inline,
        };
        return Some(MacroDefSnapshot {
            kind,
            body_requirement: custom.body,
            body_is_raw: false,
        });
    }
    None
}

// ---------------------------------------------------------------------------
// ZoneBuilder — the AST → ZoneMap builder
// ---------------------------------------------------------------------------

struct ZoneBuilder<'a> {
    leaves: Vec<LeafZone>,
    bodies: Vec<MacroBody>,
    body_offset: usize,
    custom_macros: &'a CustomMacroRegistry,
}

impl<'a> ZoneBuilder<'a> {
    /// Shift a body-relative span to passage-relative.
    fn shift(&self, span: &Range<usize>) -> Range<usize> {
        (span.start + self.body_offset)..(span.end + self.body_offset)
    }

    /// Look up a macro in both the builtin catalog and the custom registry.
    fn lookup(&self, name: &str) -> Option<MacroDefSnapshot> {
        lookup_macro(name, self.custom_macros)
    }

    /// Walk a slice of AST nodes, emitting leaves (and bodies for macros).
    fn walk_nodes(&mut self, nodes: &[AstNode], parent_body_idx: Option<usize>, depth: u32) {
        for node in nodes {
            self.walk_node(node, parent_body_idx, depth);
        }
    }

    /// Walk a single AST node.
    fn walk_node(&mut self, node: &AstNode, parent_body_idx: Option<usize>, depth: u32) {
        match node {
            AstNode::Text { span, is_prose, .. } => {
                // If the parent body is raw (JS/CSS), emit Raw instead of Prose.
                let kind = if let Some(body_idx) = parent_body_idx {
                    let body = &self.bodies[body_idx];
                    if let Some(lang) = body.raw_language {
                        LeafKind::Raw { language: lang }
                    } else {
                        LeafKind::Prose {
                            is_prose: *is_prose,
                        }
                    }
                } else {
                    LeafKind::Prose {
                        is_prose: *is_prose,
                    }
                };
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind,
                    body_idx: parent_body_idx,
                });
            }

            AstNode::Macro {
                name,
                name_span,
                open_span,
                close_span,
                close_name_span,
                full_span,
                children,
                ..
            } => {
                let snap = self.lookup(name);

                // 1. Emit MacroTag leaf for the open tag.
                self.leaves.push(LeafZone {
                    span: self.shift(open_span),
                    kind: LeafKind::MacroTag {
                        macro_name: name.clone(),
                        part: TagPart::Open,
                        macro_kind: snap.map(|s| s.kind),
                        body_requirement: snap.map(|s| s.body_requirement),
                        orphan: false,
                    },
                    body_idx: parent_body_idx,
                });

                // 2. If this macro has a body (children.is_some()), emit a MacroBody.
                let body_idx = if children.is_some() {
                    let body_start = open_span.end;
                    let body_end = close_span
                        .as_ref()
                        .map(|c| c.start)
                        .unwrap_or(full_span.end);
                    let idx = self.bodies.len();
                    self.bodies.push(MacroBody {
                        span: self.shift(&(body_start..body_end)),
                        name_span: self.shift(name_span),
                        close_span: close_span.as_ref().map(|c| self.shift(c)),
                        close_name_span: close_name_span.as_ref().map(|c| self.shift(c)),
                        macro_name: name.clone(),
                        macro_kind: snap.map(|s| s.kind),
                        body_requirement: snap.map(|s| s.body_requirement),
                        unclosed: close_span.is_none(),
                        parent_body: parent_body_idx,
                        depth,
                        raw_language: snap.and_then(|s| {
                            if s.body_is_raw {
                                Some(RawLanguage::Js)
                            } else {
                                None
                            }
                        }),
                    });
                    Some(idx)
                } else {
                    None
                };

                // 3. Recurse into children with the new body_idx.
                if let Some(ch) = children {
                    self.walk_nodes(ch, body_idx, depth + 1);
                }

                // 4. Emit MacroTag leaf for the close tag, if present.
                if let Some(cs) = close_span.as_ref() {
                    self.leaves.push(LeafZone {
                        span: self.shift(cs),
                        kind: LeafKind::MacroTag {
                            macro_name: name.clone(),
                            part: TagPart::Close,
                            macro_kind: snap.map(|s| s.kind),
                            body_requirement: snap.map(|s| s.body_requirement),
                            orphan: false,
                        },
                        body_idx: parent_body_idx,
                    });
                }
            }

            AstNode::Expression { span, kind, .. } => {
                // <<=>>expr>> or <<->>expr>> — single MacroTag leaf, no body.
                let macro_name = match kind {
                    ExprKind::Print => "<<=>>",
                    ExprKind::Silent => "<<->>",
                };
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::MacroTag {
                        macro_name: macro_name.to_string(),
                        part: TagPart::Expression,
                        macro_kind: None,
                        body_requirement: Some(BodyRequirement::Never),
                        orphan: false,
                    },
                    body_idx: parent_body_idx,
                });
            }

            AstNode::Link { span, .. } => {
                // For Phase 1, emit one Markup(Link) leaf for the full span.
                // Sub-spans (display, target, setter) are not broken out —
                // that can be refined later if needed.
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::Markup(MarkupKind::Link),
                    body_idx: parent_body_idx,
                });
            }

            AstNode::Comment { span, .. } => {
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::Markup(MarkupKind::Comment),
                    body_idx: parent_body_idx,
                });
            }

            AstNode::InlineStyle { span, children, .. } => {
                // Gap approach: emit Markup(InlineStyle) for wrapper bytes
                // (the `@@`, class, `;`, closing `@@`), recurse into children.
                self.emit_markup_with_gaps(
                    span,
                    MarkupKind::InlineStyle,
                    children,
                    parent_body_idx,
                    depth,
                );
            }

            AstNode::TextFormat { span, children, .. } => {
                // Phase 1.3 (plan.md): format content is wikified — the
                // parser attaches recursive children, so the delimiters get
                // Markup(TextFormat) gap leaves and the content inside is
                // zoned by its own children (a `[[link]]` inside `''…''`
                // gets a Link leaf, `$var` gets Prose/variable zones).
                //
                // This resolves the KNOWN DIVERGENCE noted here previously
                // (verified against upstream 2026-09-15): SugarCube 2's
                // `formatByChar` subWikifies formatting content, so links,
                // macros and variables inside ''…''-style constructs are
                // live upstream; the old opaque raw-content zone hid all of
                // them from link, variable, and diagnostic passes.
                self.emit_markup_with_gaps(
                    span,
                    MarkupKind::TextFormat,
                    children,
                    parent_body_idx,
                    depth,
                );
            }

            AstNode::Heading { span, children, .. } => {
                self.emit_markup_with_gaps(
                    span,
                    MarkupKind::Heading,
                    children,
                    parent_body_idx,
                    depth,
                );
            }

            AstNode::HorizontalRule { span } => {
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::Markup(MarkupKind::HorizontalRule),
                    body_idx: parent_body_idx,
                });
            }

            AstNode::ListItem { span, children, .. } => {
                self.emit_markup_with_gaps(
                    span,
                    MarkupKind::ListItem,
                    children,
                    parent_body_idx,
                    depth,
                );
            }

            AstNode::Blockquote { span, children, .. } => {
                self.emit_markup_with_gaps(
                    span,
                    MarkupKind::Blockquote,
                    children,
                    parent_body_idx,
                    depth,
                );
            }

            AstNode::BlockquoteBlock {
                span,
                open_span,
                close_span,
                children,
            } => {
                // Emit Markup(Blockquote) for the open `<<<` line.
                self.leaves.push(LeafZone {
                    span: self.shift(open_span),
                    kind: LeafKind::Markup(MarkupKind::Blockquote),
                    body_idx: parent_body_idx,
                });
                // Recurse into children for the body.
                self.walk_nodes(children, parent_body_idx, depth);
                // Emit Markup(Blockquote) for the close `<<<` line, if present.
                if let Some(cs) = close_span {
                    self.leaves.push(LeafZone {
                        span: self.shift(cs),
                        kind: LeafKind::Markup(MarkupKind::Blockquote),
                        body_idx: parent_body_idx,
                    });
                } else {
                    // Unclosed: emit Markup for the trailing gap (if any).
                    let children_end = children
                        .last()
                        .map(|c| node_span(c).end)
                        .unwrap_or(open_span.end);
                    if children_end < span.end {
                        self.leaves.push(LeafZone {
                            span: self.shift(&(children_end..span.end)),
                            kind: LeafKind::Markup(MarkupKind::Blockquote),
                            body_idx: parent_body_idx,
                        });
                    }
                }
            }

            AstNode::Table { span, .. } => {
                // Phase 1 simplification: emit one Markup(Table) leaf for
                // the entire table span. Cell-internal links/macros are not
                // broken out into separate leaves — this can be refined later.
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::Markup(MarkupKind::Table),
                    body_idx: parent_body_idx,
                });
            }

            AstNode::CodeBlock { span, .. } => {
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::Markup(MarkupKind::CodeBlock),
                    body_idx: parent_body_idx,
                });
            }

            AstNode::InlineCode { span, .. } => {
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::Markup(MarkupKind::InlineCode),
                    body_idx: parent_body_idx,
                });
            }

            AstNode::Verbatim { span, .. } => {
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::Markup(MarkupKind::Verbatim),
                    body_idx: parent_body_idx,
                });
            }

            AstNode::MacroClose { span, .. } => {
                // Should not appear in the final AST (tree_builder removes
                // them). If one slips through, emit it as an Error.
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::Error {
                        message: "Stray MacroClose in final AST".to_string(),
                        kind: ErrorKind::Parse,
                    },
                    body_idx: parent_body_idx,
                });
            }

            AstNode::Error { message, span } => {
                let kind = if message.starts_with("Unexpected close tag") {
                    ErrorKind::OrphanClose
                } else {
                    ErrorKind::Parse
                };
                self.leaves.push(LeafZone {
                    span: self.shift(span),
                    kind: LeafKind::Error {
                        message: message.clone(),
                        kind,
                    },
                    body_idx: parent_body_idx,
                });
            }
        }
    }

    /// Emit a Markup leaf for the wrapper bytes (gaps between children),
    /// and recurse into children for their content.
    ///
    /// This resolves the "overlapping markup" problem (§6.4 of plan.md):
    /// `InlineStyle`, `Heading`, `ListItem`, `Blockquote` all have a `span`
    /// covering the whole construct AND `children` whose spans are inside.
    /// We emit Markup leaves for the gaps (marker, delimiters) and recurse
    /// into children for the body content, preserving byte-coverage with
    /// no overlaps.
    fn emit_markup_with_gaps(
        &mut self,
        parent_span: &Range<usize>,
        kind: MarkupKind,
        children: &[AstNode],
        parent_body_idx: Option<usize>,
        depth: u32,
    ) {
        let mut cursor = parent_span.start;

        // Pass 1: emit Markup leaves for gaps between children.
        for child in children {
            let cs = node_span(child);
            if cs.start > cursor {
                self.leaves.push(LeafZone {
                    span: self.shift(&(cursor..cs.start)),
                    kind: LeafKind::Markup(kind),
                    body_idx: parent_body_idx,
                });
            }
            cursor = cs.end;
        }
        // Trailing gap after last child.
        if cursor < parent_span.end {
            self.leaves.push(LeafZone {
                span: self.shift(&(cursor..parent_span.end)),
                kind: LeafKind::Markup(kind),
                body_idx: parent_body_idx,
            });
        }

        // Pass 2: recurse into children.
        self.walk_nodes(children, parent_body_idx, depth);
    }

    /// Finalize: sort leaves by span.start, validate, return the ZoneMap.
    fn finish(mut self) -> ZoneMap {
        self.leaves.sort_by_key(|z| z.span.start);

        // Debug-mode validation: check for overlaps.
        // (Gaps are acceptable in edge cases — the parser may not cover
        // every byte in malformed input. Overlaps are bugs.)
        #[cfg(debug_assertions)]
        {
            for w in self.leaves.windows(2) {
                if w[0].span.end > w[1].span.start {
                    tracing::warn!(
                        "Zone overlap detected: [{}, {}) overlaps [{}, {}) — {:?} vs {:?}",
                        w[0].span.start,
                        w[0].span.end,
                        w[1].span.start,
                        w[1].span.end,
                        w[0].kind,
                        w[1].kind
                    );
                }
            }
        }

        ZoneMap::from_parts(self.leaves, self.bodies)
    }
}

/// Build a [`ZoneMap`] from a parsed AST.
///
/// - `nodes`: the nested AST nodes (output of `tree_builder::build_tree`).
/// - `body_offset_in_passage`: the byte offset from the passage head (`::`)
///   to the body start. Used to shift body-relative AST spans to
///   passage-relative zone spans.
/// - `custom_macros`: the custom macro registry (populated from `[widget]`
///   and `[script]` passages processed earlier in the pipeline).
pub fn build_from_ast(
    nodes: &[AstNode],
    body_offset_in_passage: usize,
    custom_macros: &CustomMacroRegistry,
) -> ZoneMap {
    let mut builder = ZoneBuilder {
        leaves: Vec::new(),
        bodies: Vec::new(),
        body_offset: body_offset_in_passage,
        custom_macros,
    };
    builder.walk_nodes(nodes, None, 0);
    builder.finish()
}

/// Extract the primary span from an AstNode (body-relative).
fn node_span(node: &AstNode) -> &Range<usize> {
    match node {
        AstNode::Text { span, .. } => span,
        AstNode::Macro { full_span, .. } => full_span,
        AstNode::Expression { span, .. } => span,
        AstNode::Link { span, .. } => span,
        AstNode::Comment { span, .. } => span,
        AstNode::InlineStyle { span, .. } => span,
        AstNode::TextFormat { span, .. } => span,
        AstNode::MacroClose { span, .. } => span,
        AstNode::Error { span, .. } => span,
        AstNode::Heading { span, .. } => span,
        AstNode::HorizontalRule { span } => span,
        AstNode::ListItem { span, .. } => span,
        AstNode::Blockquote { span, .. } => span,
        AstNode::BlockquoteBlock { span, .. } => span,
        AstNode::Table { span, .. } => span,
        AstNode::CodeBlock { span, .. } => span,
        AstNode::InlineCode { span, .. } => span,
        AstNode::Verbatim { span, .. } => span,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::parser::parse_passage_body;
    use crate::sugarcube::registries::CustomMacroRegistry;
    use crate::types::BodyRequirement;

    /// Helper: parse a body string and build a ZoneMap with body_offset=0
    /// (so passage-relative == body-relative for tests) and an empty custom
    /// macro registry.
    fn zones_for(body: &str) -> (String, ZoneMap) {
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let zones = build_from_ast(&ast.nodes, 0, &CustomMacroRegistry::new());
        (body.to_string(), zones)
    }

    /// Helper: parse with a custom macro registry (for custom widget tests).
    fn zones_for_with_custom(body: &str, custom: &CustomMacroRegistry) -> (String, ZoneMap) {
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let zones = build_from_ast(&ast.nodes, 0, custom);
        (body.to_string(), zones)
    }

    /// Helper: parse with a body offset (for coordinate shift tests).
    fn zones_for_with_offset(body: &str, offset: usize) -> (String, ZoneMap) {
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let zones = build_from_ast(&ast.nodes, offset, &CustomMacroRegistry::new());
        // Build a "padded" body so spans line up with the offset.
        let padded = format!("{}{}", " ".repeat(offset), body);
        (padded, zones)
    }

    #[test]
    fn test_pure_prose() {
        let (body, zones) = zones_for("Hello world");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_pure_markup() {
        let (body, zones) = zones_for("! Heading\n* item\n''bold''");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_single_inline_macro() {
        let (body, zones) = zones_for("<<set $x to 1>> text");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_single_block_macro() {
        let (body, zones) = zones_for("<<if $x>>text<</if>>");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_nested_macros() {
        let input = "\
<<macro1>>
text
<<macro2>>inner<</macro2>>
<<macro3>>
body text
<</macro3>>
<</macro1>>";
        let (body, zones) = zones_for(input);
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_unclosed_block_macro() {
        let (body, zones) = zones_for("<<if $x>>text");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_orphan_close_tag() {
        let (body, zones) = zones_for("text<</if>>");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_script_raw_body() {
        let (body, zones) = zones_for("<<script>>console.log($x);<</script>>");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_switch_case_submacro() {
        let input = "<<switch $x>><<case 1>>a<</case>><<default>>b<</default>><</switch>>";
        let (body, zones) = zones_for(input);
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_if_else_submacro() {
        let (body, zones) = zones_for("<<if $x>>a<<else>>b<</if>>");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_inline_style_with_link() {
        let (body, zones) = zones_for("@@class;[[link]]@@");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_expression_macro() {
        let (body, zones) = zones_for("<<=>>$x.length>>");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_custom_widget() {
        let mut custom = CustomMacroRegistry::new();
        custom.register_widget(
            "myWidget",
            "Widgets",
            "file:///test.tw",
            0,
            None,
            BodyRequirement::Required,
        );
        let (body, zones) = zones_for_with_custom("<<myWidget>>body<</myWidget>>", &custom);
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_unknown_macro() {
        // No custom registry — `nonExistent` is unknown.
        let (body, zones) = zones_for("<<nonExistent>>text");
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_post_close_sibling() {
        // The check_container_violation bug case from §6.5:
        // cursor after `<</if>>` should NOT be inside <<if>>'s body.
        let input = "<<if $x>>\n  text\n<</if>>\n<<set $y to 1>>";
        let (body, zones) = zones_for(input);
        insta::assert_snapshot!(zones.debug_dump(&body));

        // Verify: a leaf at the `<<set>>` position has body_idx == None.
        let set_offset = input.find("<<set").unwrap();
        let leaf = zones.leaf_at(set_offset).expect("leaf at <<set>>");
        assert!(
            leaf.body_idx.is_none(),
            "leaf at <<set>> should be top-level"
        );
    }

    #[test]
    fn test_silently_body() {
        let input = "<<silently>><<set $x to 1>><</silently>> text";
        let (body, zones) = zones_for(input);
        insta::assert_snapshot!(zones.debug_dump(&body));
    }

    #[test]
    fn test_body_offset_shift() {
        // Verify that body_offset_in_passage shifts all spans correctly.
        let (body, zones) = zones_for_with_offset("text <<if $x>>body<</if>>", 10);
        insta::assert_snapshot!(zones.debug_dump(&body));

        // The first leaf should start at offset 10, not 0.
        assert!(zones.leaf_count() > 0, "should have at least one leaf");
        // Access first leaf via iter_leaves (leaf_at needs an offset).
        let first = zones.iter_leaves().next().unwrap();
        assert!(
            first.span.start >= 10,
            "first leaf should start at >= 10 (got {})",
            first.span.start
        );
    }

    #[test]
    fn test_enclosing_body_at() {
        // Use three levels of block macros (link → capture → if) so each
        // contributes a body. <<set>> is inline (no body), so it wouldn't
        // appear in the body stack.
        let input = "<<link>>\n  <<capture $x>>\n    <<if $y>>\n      body\n    <</if>>\n  <</capture>>\n<</link>>";
        let (_body, zones) = zones_for(input);

        // Cursor inside <<if>>'s body (the word "body").
        let if_body_offset = input.find("body").unwrap();
        let stack = zones.body_stack_at(if_body_offset);
        let names: Vec<&str> = stack.iter().map(|b| b.macro_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["link", "capture", "if"],
            "body stack should be [link, capture, if] inside <<if>> body"
        );
    }

    #[test]
    fn test_renders_to_player() {
        let (body, zones) = zones_for("Hello <<set $x to 1>> world");
        for leaf in zones.iter_leaves() {
            let snippet = &body[leaf.span.start.min(body.len())..leaf.span.end.min(body.len())];
            match &leaf.kind {
                LeafKind::Prose { is_prose: true } => {
                    assert!(
                        ZoneMap::renders_to_player(leaf),
                        "prose should render: {:?}",
                        snippet
                    );
                }
                LeafKind::MacroTag { .. } => {
                    assert!(
                        !ZoneMap::renders_to_player(leaf),
                        "macro tag should not render: {:?}",
                        snippet
                    );
                }
                _ => {}
            }
        }
    }

    /// Phase 2 verification: ensure `Passage.zones` is populated after the
    /// full parse → build_passage pipeline (not just direct `build_from_ast`).
    #[test]
    fn test_zones_populated_on_passage() {
        use crate::header::TweeHeader;
        use crate::sugarcube::classifier::{ClassifiedPassage, PassageCategory};
        use crate::sugarcube::graph::passage_build::build_passage;

        let body = "<<if $x>>\n  Hello <<set $y to 1>>\n<</if>>";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        // Manually build zones (simulating what parse_pipeline does).
        let zones = build_from_ast(&ast.nodes, 0, &CustomMacroRegistry::new());

        // Build a minimal ClassifiedPassage for build_passage.
        let cp = ClassifiedPassage {
            header: TweeHeader {
                name: "Test".to_string(),
                tags: Vec::new(),
                header_start: 0,
                name_start: 3,
                metadata_json: None,
                name_text_raw: "Test".to_string(),
                tags_raw: "Test".to_string(),
            },
            body_text: body.to_string(),
            file_uri: "file:///test.tw".to_string(),
            special_def: None,
            category: PassageCategory::Regular,
            processing_priority: 40,
        };

        // Build a PassageAst with zones populated (simulating parse_pipeline).
        let mut ast_with_zones = ast;
        ast_with_zones.zones = zones;

        let passage = build_passage(&cp, &ast_with_zones, 0, 0);

        // Verify zones landed on the Passage.
        assert_eq!(
            passage.zones.leaf_count(),
            ast_with_zones.zones.leaf_count(),
            "Passage.zones should have the same leaf count as the AST zones"
        );
        assert!(
            passage.zones.leaf_count() > 0,
            "Passage.zones should be populated (non-empty) for a normal passage"
        );
        assert!(
            passage.zones.body_count() > 0,
            "Passage.zones should have at least one body for <<if>>...<</if>>"
        );

        // Verify a leaf query works on the Passage's zones.
        // The `<<set>>` macro is at offset 23 in the body.
        let set_offset = body.find("<<set").unwrap();
        let leaf = passage.zones.leaf_at(set_offset);
        assert!(leaf.is_some(), "leaf_at should find the <<set>> macro tag");
        match &leaf.unwrap().kind {
            LeafKind::MacroTag { macro_name, .. } => {
                assert_eq!(
                    macro_name, "set",
                    "leaf at <<set>> should be a MacroTag for 'set'"
                );
            }
            other => panic!("expected MacroTag, got {:?}", other),
        }
    }
}
