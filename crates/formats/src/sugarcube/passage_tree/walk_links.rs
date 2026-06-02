//! Link extraction walks for the passage tree.
//!
//! Contains `walk_links()` and helper functions that collect `[[...]]` links,
//! implicit passage references, and macro passage references from the tree.

use std::collections::HashSet;

use knot_core::passage::Link;

use super::{PassageNode, compute_args_offset};

// ---------------------------------------------------------------------------
// walk_links() — Replace extract_links() + implicit + macro passage refs
// ---------------------------------------------------------------------------

/// Walk the tree and extract all passage links.
///
/// Replaces `extract_links()` + `extract_implicit_passage_refs()` +
/// `extract_macro_passage_refs()` with a single tree walk. Collects:
///
/// - `[[...]]` links from text nodes (already extracted by the tree builder)
/// - Implicit passage refs from text and macro args (`Engine.play()`,
///   `data-passage`, `Story.get()`, `Story.has()`, `UI.goto()`,
///   `UI.include()`, etc.)
/// - Macro passage refs from `<<goto>>`, `<<link>>`, `<<include>>`,
///   `<<button>>`, etc.
///
/// All links are deduplicated by `(display_text, target)` to avoid
/// double-counting the same passage reference.
pub(crate) fn walk_links(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<Link> {
    let mut links: Vec<Link> = Vec::new();

    // Collect [[links]] from tree nodes (already extracted)
    collect_tree_links(nodes, &mut links);

    // Collect implicit passage refs from text and macro args
    collect_implicit_refs(nodes, body, body_offset, &mut links);

    // Collect macro passage refs (<<goto>>, <<link>>, <<include>>, etc.)
    collect_macro_passage_refs(nodes, body, body_offset, &mut links);

    // Deduplicate by (display_text, target)
    let mut seen: HashSet<(Option<String>, String)> = HashSet::new();
    links.retain(|link| {
        let key = (link.display_text.clone(), link.target.clone());
        seen.insert(key)
    });

    links
}

/// Recursively collect [[links]] from text nodes in the tree.
fn collect_tree_links(nodes: &[PassageNode], links: &mut Vec<Link>) {
    for node in nodes {
        match node {
            PassageNode::Text { links: node_links, .. } => {
                links.extend(node_links.iter().cloned());
            }
            PassageNode::Macro { children, .. } => {
                if let Some(children) = children {
                    collect_tree_links(children, links);
                }
            }
            PassageNode::Expression { .. }
            | PassageNode::Heading { .. }
            | PassageNode::Error { .. } => {}
        }
    }
}

/// Collect implicit passage references from text and macro args.
///
/// Detects patterns like `data-passage="..."`, `Engine.play("...")`,
/// `Story.get("...")`, `Story.has("...")`, `UI.goto("...")`,
/// `UI.include("...")` that reference passages but aren't standard
/// `[[links]]` or `<<macro>>` passage-args.
fn collect_implicit_refs(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    links: &mut Vec<Link>,
) {
    use super::super::links::{
        RE_DATA_PASSAGE, RE_ENGINE_PLAY, RE_ENGINE_GOTO,
        RE_STORY_GET, RE_STORY_PASSAGE, RE_STORY_HAS,
        RE_UI_GOTO, RE_UI_INCLUDE,
    };

    let patterns: &[(&std::sync::LazyLock<regex::Regex>, Option<knot_core::graph::EdgeType>)] = &[
        (&RE_DATA_PASSAGE, None),
        (&RE_ENGINE_PLAY, None),
        (&RE_ENGINE_GOTO, Some(knot_core::graph::EdgeType::Jump)),
        (&RE_STORY_GET, None),
        (&RE_STORY_PASSAGE, None),
        (&RE_STORY_HAS, None),
        (&RE_UI_GOTO, Some(knot_core::graph::EdgeType::Jump)),
        (&RE_UI_INCLUDE, Some(knot_core::graph::EdgeType::Include)),
    ];

    for node in nodes {
        // Scan text nodes
        if let PassageNode::Text { content, span, .. } = node {
            let text_offset = span.start;
            for (re, edge_hint) in patterns {
                for caps in re.captures_iter(content) {
                    if let Some(target_match) = caps.get(1) {
                        let full_match = caps.get(0).unwrap();
                        let target = target_match.as_str().trim().to_string();
                        if !target.is_empty() {
                            links.push(Link {
                                display_text: None,
                                target,
                                span: text_offset + full_match.start()
                                    ..text_offset + full_match.end(),
                                edge_type_hint: *edge_hint,
                            });
                        }
                    }
                }
            }
        }

        // Scan macro args
        if let PassageNode::Macro { parsed, .. } = node {
            if !parsed.args.is_empty() {
                let args_offset = compute_args_offset(parsed, body, body_offset);
                for (re, edge_hint) in patterns {
                    for caps in re.captures_iter(&parsed.args) {
                        if let Some(target_match) = caps.get(1) {
                            let full_match = caps.get(0).unwrap();
                            let target = target_match.as_str().trim().to_string();
                            if !target.is_empty() {
                                links.push(Link {
                                    display_text: None,
                                    target,
                                    span: args_offset + full_match.start()
                                        ..args_offset + full_match.end(),
                                    edge_type_hint: *edge_hint,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            collect_implicit_refs(children, body, body_offset, links);
        }
    }
}

/// Collect passage references from macro invocations.
///
/// Uses the macro catalog's `passage_arg_macro_names()` to determine which
/// macros have passage-ref arguments, and `get_passage_arg_index()` to find
/// the correct argument position.
fn collect_macro_passage_refs(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    links: &mut Vec<Link>,
) {
    let passage_arg_macros = super::super::macros::passage_arg_macro_names();

    for node in nodes {
        if let PassageNode::Macro { parsed, .. } = node {
            // Skip close tags
            if parsed.name.starts_with('/') {
                continue;
            }

            let macro_name = parsed.name.as_str();

            // Only process macros that have passage-ref arguments
            if !passage_arg_macros.contains(macro_name) {
                // Recurse into children even if this macro isn't a passage ref
                if let PassageNode::Macro {
                    children: Some(children),
                    ..
                } = node
                {
                    collect_macro_passage_refs(children, body, body_offset, links);
                }
                continue;
            }

            let args_str = parsed.args.as_str();
            if args_str.is_empty() {
                continue;
            }

            // Parse quoted string arguments from the args string.
            let string_args = super::super::blocks::parse_quoted_args(args_str);

            if string_args.is_empty() {
                continue;
            }

            // Determine which argument is the passage reference.
            let arg_count = string_args.len();
            let passage_idx = super::super::macros::get_passage_arg_index(macro_name, arg_count);

            if passage_idx < 0 {
                continue;
            }

            let idx = passage_idx as usize;
            if idx < string_args.len() {
                let (content, rel_start, rel_end) = &string_args[idx];
                if !content.is_empty() {
                    let name_end_in_body = parsed.name_start + parsed.name_len;
                    let body_after_name = &body[name_end_in_body..parsed.end.saturating_sub(2)];
                    let trimmed_start = body_after_name.len() - body_after_name.trim_start().len();
                    let args_offset_in_body = name_end_in_body + trimmed_start;

                    // Classify edge type
                    let edge_type_hint = match macro_name {
                        "goto" => Some(knot_core::graph::EdgeType::Jump),
                        "include" => Some(knot_core::graph::EdgeType::Include),
                        "return" | "back" => Some(knot_core::graph::EdgeType::Navigation),
                        _ => None,
                    };

                    links.push(Link {
                        display_text: None,
                        target: content.clone(),
                        span: body_offset + args_offset_in_body + *rel_start
                            ..body_offset + args_offset_in_body + *rel_end,
                        edge_type_hint,
                    });
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            collect_macro_passage_refs(children, body, body_offset, links);
        }
    }
}
