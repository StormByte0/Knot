//! Code Action helpers — extract diagnostic info and create workspace edits.

use knot_core::Workspace;
use knot_core::passage::SpecialPassageBehavior;
use lsp_types::*;
use std::collections::HashMap;
use url::Url;

use super::position::{parse_passage_name_from_header, set_reachable_in_header, utf16_len};

/// Extract a quoted name from a diagnostic message, e.g. "Broken link to 'Foo'"
pub(crate) fn extract_quoted_name(message: &str) -> Option<String> {
    if let Some(start) = message.find('\'') {
        let rest = &message[start + 1..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    // Also try double quotes
    if let Some(start) = message.find('"') {
        let rest = &message[start + 1..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// Extract a passage name from a diagnostic message.
pub(crate) fn extract_passage_from_diag(message: &str) -> Option<String> {
    extract_quoted_name(message).or_else(|| {
        // Try to extract after "passage" keyword
        let lower = message.to_lowercase();
        if let Some(idx) = lower.find("passage ") {
            let rest = &message[idx + 8..];
            let name = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(':')
                .to_string();
            if !name.is_empty() { Some(name) } else { None }
        } else {
            None
        }
    })
}

/// Extract a variable name from a diagnostic message.
///
/// Tries to find a word starting with a format-specific variable sigil.
/// The `sigils` parameter should come from the format plugin's
/// `variable_sigils()` method. This is a best-effort heuristic — the
/// format plugin's parsed variable data is the authoritative source.
pub(crate) fn extract_variable_name(message: &str, sigils: &[char]) -> Option<String> {
    for word in message.split_whitespace() {
        if let Some(first_char) = word.chars().next()
            && sigils.contains(&first_char)
        {
            let name = word.trim_end_matches(':').trim_end_matches(',').to_string();
            // Filter out standalone sigils (e.g., just "$" or "_")
            if name.len() > 1 {
                return Some(name);
            }
        }
    }
    None
}

/// Create a WorkspaceEdit that creates a new passage at the end of the file
/// where the broken link occurs.
pub(crate) fn create_passage_edit(
    inner: &crate::state::ServerStateInner,
    name: &str,
) -> WorkspaceEdit {
    // Find any open document to add the passage to (prefer the one with StoryData)
    let target_uri = inner
        .workspace
        .find_passage_file_uri("StoryData")
        .or_else(|| inner.open_documents.keys().next().cloned());

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some(uri) = target_uri
        && let Some(text) = inner.open_documents.get(&uri)
    {
        let line_count = text.lines().count() as u32;
        changes.insert(
            uri,
            vec![TextEdit {
                range: Range {
                    start: Position {
                        line: line_count,
                        character: 0,
                    },
                    end: Position {
                        line: line_count,
                        character: 0,
                    },
                },
                new_text: format!("\n:: {}\n", name),
            }],
        );
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Find the nearest reachable passage to a given unreachable one.
pub(crate) fn find_nearest_reachable_passage(workspace: &Workspace, name: &str) -> Option<String> {
    let start_passage = workspace
        .metadata
        .as_ref()
        .map(|m| m.start_passage.as_str())
        .unwrap_or("Start");

    // Reachability now considers ALL roots (start + manual `reachable`
    // metadata entries), matching the diagnostics the user actually sees.
    let unreachable = workspace.detect_unreachable_passages();
    let unreachable_set: std::collections::HashSet<String> =
        unreachable.iter().map(|d| d.passage_name.clone()).collect();

    // Look for passages that link to passages that are near the unreachable one
    // Try finding a passage in the same file first
    if let Some((doc, _)) = workspace.find_passage(name) {
        for passage in &doc.passages {
            if !unreachable_set.contains(&passage.name) && passage.name != name {
                return Some(passage.name.clone());
            }
        }
    }

    // Fall back to the start passage
    if workspace.find_passage(start_passage).is_some() {
        Some(start_passage.to_string())
    } else {
        None
    }
}

/// Resolve the unreachable-passage quickfix for a cursor position, when the
/// client-provided diagnostics don't carry it.
///
/// VS Code only includes diagnostics intersecting the cursor range in
/// `CodeActionContext.diagnostics` (LSP: "an array of diagnostics known in
/// the content range"), and the UnreachablePassage squiggle covers just
/// the passage NAME in the header. With the cursor in the passage body —
/// where authors actually edit — the context arrives empty and no
/// quickfix ever surfaces. This derives the passage containing the
/// cursor (header or body, via span containment) and returns its name
/// when it is currently flagged unreachable.
pub(crate) fn unreachable_passage_under_cursor(
    workspace: &Workspace,
    text: &str,
    uri: &Url,
    position: lsp_types::Position,
) -> Option<String> {
    // Respect the same severity suppression the published diagnostics use
    // ("Off" hides the squiggle) — don't offer quickfixes for a diagnostic
    // the user has turned off.
    if let Some(knot_core::workspace::DiagnosticSeverity::Off) =
        workspace.config.diagnostics.get("UnreachablePassage")
    {
        return None;
    }
    let name = super::position::find_passage_containing_position(text, workspace, uri, position)?;
    workspace
        .detect_unreachable_passages()
        .iter()
        .any(|d| d.passage_name == name)
        .then_some(name)
}

/// Create a WorkspaceEdit that adds a link from one passage to another.
pub(crate) fn add_link_edit(
    inner: &crate::state::ServerStateInner,
    from_passage: &str,
    to_passage: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some((doc, _)) = inner.workspace.find_passage(from_passage)
        && let Some(text) = inner.open_documents.get(&doc.uri)
    {
        // Find the passage header line and add a link at the end of its body
        let mut header_line: Option<u32> = None;
        let mut end_line: u32 = 0;
        for (i, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let pname = parse_passage_name_from_header(&line[2..]);
                if pname == from_passage {
                    header_line = Some(i as u32);
                } else if header_line.is_some() {
                    end_line = i as u32;
                    break;
                }
            }
            if header_line.is_some() {
                end_line = i as u32 + 1;
            }
        }

        if let Some(hl) = header_line {
            let insert_line = if end_line > hl { end_line } else { hl + 1 };
            changes.insert(
                doc.uri.clone(),
                vec![TextEdit {
                    range: Range {
                        start: Position {
                            line: insert_line,
                            character: 0,
                        },
                        end: Position {
                            line: insert_line,
                            character: 0,
                        },
                    },
                    new_text: format!("[[{}]]\n", to_passage),
                }],
            );
        }
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Create a WorkspaceEdit that marks a passage as a manual reachability
/// entry by writing `"reachable":true` into its header metadata.
///
/// This backs the "it's not really unreachable" quickfix on the
/// UnreachablePassage diagnostic: static analysis can't see through
/// variable-aliased navigation (`<<goto $next>>`, `<<link $label $target>>`),
/// so authors override the analyzer by hand. The edit replaces the
/// passage's whole header line with one whose JSON metadata block
/// carries the flag — merging with any existing position/group/color
/// values — and returns an empty edit when the passage or its file
/// can't be found (the code action is then a no-op for the client).
pub(crate) fn mark_reachable_edit(
    inner: &crate::state::ServerStateInner,
    name: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some((doc, _)) = inner.workspace.find_passage(name)
        && let Some(text) = inner.open_documents.get(&doc.uri)
    {
        for (i, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let pname = parse_passage_name_from_header(&line[2..]);
                if pname == name {
                    changes.insert(
                        doc.uri.clone(),
                        vec![TextEdit {
                            range: Range {
                                start: Position {
                                    line: i as u32,
                                    character: 0,
                                },
                                end: Position {
                                    line: i as u32,
                                    character: utf16_len(line),
                                },
                            },
                            new_text: set_reachable_in_header(line),
                        }],
                    );
                    break;
                }
            }
        }
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Create a WorkspaceEdit that adds content template to an empty passage.
pub(crate) fn add_content_template_edit(
    inner: &crate::state::ServerStateInner,
    name: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some((doc, _)) = inner.workspace.find_passage(name)
        && let Some(text) = inner.open_documents.get(&doc.uri)
    {
        for (i, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let pname = parse_passage_name_from_header(&line[2..]);
                if pname == name {
                    // Insert template content after the header line
                    let template = format!("{}\n", name);
                    changes.insert(
                        doc.uri.clone(),
                        vec![TextEdit {
                            range: Range {
                                start: Position {
                                    line: (i as u32) + 1,
                                    character: 0,
                                },
                                end: Position {
                                    line: (i as u32) + 1,
                                    character: 0,
                                },
                            },
                            new_text: template,
                        }],
                    );
                    break;
                }
            }
        }
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Create a WorkspaceEdit that initializes a variable in the startup passage.
///
/// Queries the format plugin to find the startup passage name and the
/// appropriate variable assignment syntax, instead of hardcoding
/// SugarCube-specific names and macros.
pub(crate) fn initialize_var_in_story_init_edit(
    inner: &crate::state::ServerStateInner,
    var_name: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    // Query the format plugin for the startup passage name and assignment syntax
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    // Find the startup passage name from the merged special passage registry
    let startup_passage_name: Option<String> = plugin.and_then(|p| {
        p.all_special_passages()
            .into_iter()
            .find(|def| {
                def.contributes_variables && matches!(def.behavior, SpecialPassageBehavior::Startup)
            })
            .map(|def| def.name)
    });

    // Get the format-specific variable assignment snippet.
    // This delegates to the format plugin so that SugarCube uses `<<set>>`,
    // Harlowe uses `(set:)`, Snowman uses `<% s.var = %>`, etc.
    let snippet = plugin.and_then(|p| p.variable_assignment_snippet(var_name, "0"));

    if let Some(ref startup_name) = startup_passage_name {
        if let Some(snippet_text) = snippet {
            // Find the startup passage in the workspace
            if let Some((doc, _)) = inner.workspace.find_passage(startup_name)
                && let Some(text) = inner.open_documents.get(&doc.uri)
            {
                // Find the last line of the startup passage
                let mut init_start: Option<u32> = None;
                let mut init_end: u32 = 0;
                for (i, line) in text.lines().enumerate() {
                    if line.starts_with("::") {
                        let pname = parse_passage_name_from_header(&line[2..]);
                        if pname == *startup_name {
                            init_start = Some(i as u32);
                        } else if init_start.is_some() {
                            init_end = i as u32;
                            break;
                        }
                    }
                    if init_start.is_some() {
                        init_end = i as u32 + 1;
                    }
                }

                let insert_line = if init_end > init_start.unwrap_or(0) {
                    init_end
                } else {
                    init_start.unwrap_or(0) + 1
                };
                changes.insert(
                    doc.uri.clone(),
                    vec![TextEdit {
                        range: Range {
                            start: Position {
                                line: insert_line,
                                character: 0,
                            },
                            end: Position {
                                line: insert_line,
                                character: 0,
                            },
                        },
                        new_text: format!("{}\n", snippet_text),
                    }],
                );
            }
        }
        // If no snippet is available for this format, the code action
        // is silently skipped — we cannot produce format-correct syntax.
    } else {
        // No format plugin or no startup passage — fall back to creating a
        // default startup passage using the format's snippet if available
        if let Some(snippet_text) = snippet
            && let Some(uri) = inner.open_documents.keys().next()
            && let Some(text) = inner.open_documents.get(uri)
        {
            let line_count = text.lines().count() as u32;
            // Use the startup passage name from the plugin, or a generic
            // default. We avoid SugarCube-specific names like "StoryInit"
            // when the format is unknown.
            let passage_name = startup_passage_name.as_deref().unwrap_or("Startup");
            changes.insert(
                uri.clone(),
                vec![TextEdit {
                    range: Range {
                        start: Position {
                            line: line_count,
                            character: 0,
                        },
                        end: Position {
                            line: line_count,
                            character: 0,
                        },
                    },
                    new_text: format!("\n:: {}\n{}\n", passage_name, snippet_text),
                }],
            );
        }
        // If no snippet is available, the code action is silently skipped.
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use knot_core::document::Document;
    use knot_core::graph::{PassageEdge, PassageGraph, PassageNode};
    use knot_core::passage::StoryFormat;
    use knot_core::workspace::{StoryMetadata, Workspace};
    use knot_formats::FormatPluginMut;
    use knot_formats::sugarcube::SugarCubePlugin;
    use lsp_types::Position;
    use url::Url;

    /// Build a workspace from twee source through the real SugarCube
    /// parser, with a rebuilt passage graph (nodes + navigation edges) and
    /// StoryData-style start metadata — the minimum `detect_unreachable_
    /// passages` needs to answer reachability queries.
    fn workspace_with(src: &str, uri: &Url) -> Workspace {
        let mut plugin = SugarCubePlugin::new();
        let result = plugin.parse_mut(uri, src);
        let mut doc = Document::new(uri.clone(), StoryFormat::SugarCube);
        doc.passages = result.passages.clone();
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        ws.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });
        ws.insert_document(doc);

        // Rebuild the graph from the parsed passages (nodes + link edges),
        // mirroring what the indexing pipeline does after insert. Node and
        // edge data are collected first so the immutable document borrow
        // ends before the graph is mutated.
        let nodes: Vec<PassageNode> = {
            let doc = ws.get_document(uri).unwrap();
            doc.passages
                .iter()
                .map(|p| PassageNode {
                    name: p.name.clone(),
                    file_uri: uri.to_string(),
                    is_special: p.is_special,
                    is_metadata: p.is_metadata(),
                    is_placeholder: false,
                    layer: None,
                    category: knot_core::PassageCategory::Regular,
                    behavior: None,
                })
                .collect()
        };
        let edges: Vec<(String, String, PassageEdge)> = {
            let doc = ws.get_document(uri).unwrap();
            doc.passages
                .iter()
                .flat_map(|p| {
                    p.links
                        .iter()
                        .filter(|l| !l.target.is_empty())
                        .map(|l| {
                            (
                                p.name.clone(),
                                l.target.clone(),
                                PassageEdge {
                                    display_text: l.display_text.clone(),
                                    edge_type: knot_core::graph::EdgeType::Navigation,
                                    pre_broken_type: None,
                                },
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        ws.graph = PassageGraph::new();
        for node in nodes {
            ws.graph.add_passage(node);
        }
        for (from, to, edge) in edges {
            ws.graph.add_edge(&from, &to, edge);
        }
        ws
    }

    #[test]
    fn unreachable_quickfix_resolves_from_cursor_in_body() {
        // Start → Forest (reachable); Cave is orphaned. The quickfix must
        // resolve with the cursor anywhere INSIDE Cave — including deep in
        // the body, where the client-side diagnostics context is empty.
        let src = ":: Start\n[[Forest]]\n:: Forest\nTrees.\n:: Cave\nDeep in the body text.\n";
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let ws = workspace_with(src, &uri);

        let body_line = 5u32; // "Deep in the body text."
        let got = unreachable_passage_under_cursor(
            &ws,
            src,
            &uri,
            Position {
                line: body_line,
                character: 7,
            },
        );
        assert_eq!(got.as_deref(), Some("Cave"));
    }

    #[test]
    fn unreachable_quickfix_resolves_from_cursor_on_header() {
        let src = ":: Start\n[[Forest]]\n:: Forest\nTrees.\n:: Cave\nDeep in the body text.\n";
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let ws = workspace_with(src, &uri);

        let got = unreachable_passage_under_cursor(
            &ws,
            src,
            &uri,
            Position {
                line: 4,
                character: 4, // on "Cave" in the header
            },
        );
        assert_eq!(got.as_deref(), Some("Cave"));
    }

    #[test]
    fn reachable_passage_under_cursor_yields_none() {
        // Cursor inside a REACHABLE passage's body — no quickfix fallback.
        let src = ":: Start\n[[Forest]]\n:: Forest\nTrees.\n:: Cave\nDeep in the body text.\n";
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let ws = workspace_with(src, &uri);

        let got = unreachable_passage_under_cursor(
            &ws,
            src,
            &uri,
            Position {
                line: 3,
                character: 2, // inside "Trees."
            },
        );
        assert_eq!(got, None);
    }

    #[test]
    fn cursor_between_passages_yields_none() {
        // Position outside any passage span (past the end of the text).
        let src = ":: Start\n[[Forest]]\n:: Forest\nTrees.\n:: Cave\nDeep.\n";
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let ws = workspace_with(src, &uri);

        let got = unreachable_passage_under_cursor(
            &ws,
            src,
            &uri,
            Position {
                line: 99,
                character: 0,
            },
        );
        assert_eq!(got, None);
    }

    #[test]
    fn manual_entry_passage_is_not_offered_the_fallback() {
        // A passage marked reachable in its header metadata is not
        // unreachable — no fallback quickfix for it.
        let src = concat!(
            ":: Start\n[[Forest]]\n:: Forest\nTrees.\n",
            ":: IslandA {\"reachable\":true}\nEntered via variable navigation.\n",
        );
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let ws = workspace_with(src, &uri);

        let got = unreachable_passage_under_cursor(
            &ws,
            src,
            &uri,
            Position {
                line: 5,
                character: 5,
            },
        );
        assert_eq!(got, None);
    }
}
