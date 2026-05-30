//! Cross-format integration tests.
//!
//! These tests exercise the full parse → graph → analysis pipeline for each
//! format plugin, verifying that they all integrate correctly with the core
//! engine and produce consistent results for shared features like broken link
//! detection, unreachable passage detection, and variable analysis.

use knot_core::document::Document;
use knot_core::editing::graph_surgery;
use knot_core::graph::{DiagnosticKind, PassageEdge, PassageNode, PassageGraph};
use knot_core::passage::StoryFormat;
use knot_core::workspace::{StoryMetadata, Workspace};
use knot_core::AnalysisEngine;
use crate::plugin::FormatRegistry;
use url::Url;

/// Parse a document using the format plugin system and insert it into the workspace.
fn parse_and_insert(
    workspace: &mut Workspace,
    registry: &FormatRegistry,
    uri: &Url,
    text: &str,
    format: StoryFormat,
) {
    let plugin = registry.get(&format).or_else(|| {
        let default = StoryFormat::default_format();
        registry.get(&default)
    });

    if let Some(plugin) = plugin {
        let result = plugin.parse(uri, text);
        let mut doc = Document::new(uri.clone(), format);
        doc.passages = result.passages.clone();
        workspace.insert_document(doc);
    }
}

/// Rebuild the workspace graph from all documents.
#[allow(clippy::type_complexity)]
fn rebuild_graph(workspace: &mut Workspace) {
    let info: Vec<(String, String, bool, bool, Vec<(Option<String>, String)>)> = workspace
        .documents()
        .flat_map(|doc| {
            doc.passages.iter().map(|p| {
                let edges: Vec<(Option<String>, String)> = p
                    .links
                    .iter()
                    .map(|l| (l.display_text.clone(), l.target.clone()))
                    .collect();
                (
                    p.name.clone(),
                    doc.uri.to_string(),
                    p.is_special,
                    p.is_metadata(),
                    edges,
                )
            })
        })
        .collect();

    workspace.graph = PassageGraph::new();

    for (name, file_uri, is_special, is_metadata, _) in &info {
        workspace.graph.add_passage(PassageNode {
            name: name.clone(),
            file_uri: file_uri.clone(),
            is_special: *is_special,
            is_metadata: *is_metadata,
            is_placeholder: false,
            layer: None,
            category: if *is_metadata {
                knot_core::passage::PassageCategory::CoreMetadata
            } else if *is_special {
                knot_core::passage::PassageCategory::FormatNamed
            } else {
                knot_core::passage::PassageCategory::Regular
            },
            behavior: None,
        });
    }

    for (source, _, _, _, edges) in &info {
        for (display_text, target) in edges {
            let target_exists = workspace.graph.contains_passage(target);
            workspace.graph.add_edge(
                source,
                target,
                PassageEdge {
                    display_text: display_text.clone(),
                    edge_type: if !target_exists {
                        knot_core::graph::EdgeType::Broken
                    } else {
                        knot_core::graph::EdgeType::Navigation
                    },
                    pre_broken_type: None,
                },
            );
        }
    }
}

/// Helper to set up a workspace with metadata.
fn workspace_with_metadata(format: StoryFormat, start: &str) -> Workspace {
    let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
    ws.metadata = Some(StoryMetadata {
        format,
        format_version: None,
        start_passage: start.to_string(),
        ifid: None,
    });
    ws
}

// ===========================================================================
// Registry tests
// ===========================================================================

#[test]
fn registry_contains_all_formats() {
    let registry = FormatRegistry::with_defaults();
    let formats = registry.formats();
    assert_eq!(formats.len(), 5, "Should have 5 format plugins (Core + 4 story formats)");
    assert!(formats.contains(&StoryFormat::Core));
    assert!(formats.contains(&StoryFormat::SugarCube));
    assert!(formats.contains(&StoryFormat::Harlowe));
    assert!(formats.contains(&StoryFormat::Chapbook));
    assert!(formats.contains(&StoryFormat::Snowman));
}

#[test]
fn registry_default_includes_core() {
    let registry = FormatRegistry::default();
    assert!(registry.get(&StoryFormat::Core).is_some(), "Core plugin should be registered");
    assert!(registry.get(&StoryFormat::SugarCube).is_some());
}

// ===========================================================================
// Core (base Twine engine) end-to-end
// ===========================================================================

#[test]
fn core_parse_passages_and_links() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::Core).expect("Core plugin should be registered");

    let src = ":: StoryData\n{\"ifid\":\"TEST-IFID\"}\n:: Start\nYou are at the start. [[Forest]]\n:: Forest\nYou are in the forest.\n";
    let result = plugin.parse(&Url::parse("file:///project/story.tw").unwrap(), src);

    // Core should parse passages
    assert!(result.passages.len() >= 2, "Core should parse at least 2 passages");

    // Core should extract links
    let start_passage = result.passages.iter().find(|p| p.name == "Start");
    assert!(start_passage.is_some(), "Should find Start passage");
    let start = start_passage.unwrap();
    assert!(start.links.iter().any(|l| l.target == "Forest"), "Start should link to Forest");

    // Core should NOT provide macros
    assert!(plugin.builtin_macros().is_empty(), "Core should have no macros");

    // Core should NOT provide variable sigils
    assert!(plugin.variable_sigils().is_empty(), "Core should have no variable sigils");
}

// ===========================================================================
// SugarCube end-to-end
// ===========================================================================

#[test]
fn sugarcube_parse_and_analyze() {
    let registry = FormatRegistry::with_defaults();
    let mut ws = workspace_with_metadata(StoryFormat::SugarCube, "Start");

    let src = ":: StoryData\n{\"format\":\"SugarCube\",\"ifid\":\"TEST-IFID\"}\n:: Start\n<<set $gold to 10>>You have $gold coins. [[Forest]]\n:: Forest\nYou are in the forest.\n";
    let uri = Url::parse("file:///project/story.tw").unwrap();
    parse_and_insert(&mut ws, &registry, &uri, src, StoryFormat::SugarCube);
    rebuild_graph(&mut ws);

    let diagnostics = AnalysisEngine::analyze(&ws);

    // Should have no broken links (Forest exists)
    let broken: Vec<_> = diagnostics.iter().filter(|d| d.kind == DiagnosticKind::BrokenLink).collect();
    assert!(broken.is_empty(), "SugarCube: no broken links expected");

    // Should have $gold as a written variable
    let doc = ws.get_document(&uri).unwrap();
    let start_passage = doc.find_passage("Start").unwrap();
    assert!(start_passage.vars.iter().any(|v| v.name == "$gold" && v.kind == knot_core::VarKind::Init));
}

// ===========================================================================
// Harlowe end-to-end
// ===========================================================================

#[test]
fn harlowe_parse_and_analyze() {
    let registry = FormatRegistry::with_defaults();
    let mut ws = workspace_with_metadata(StoryFormat::Harlowe, "Start");

    let src = ":: Start\n(set: $health to 100)You are healthy. [[Forest]]\n:: Forest\nThe trees surround you.\n";
    let uri = Url::parse("file:///project/story.tw").unwrap();
    parse_and_insert(&mut ws, &registry, &uri, src, StoryFormat::Harlowe);
    rebuild_graph(&mut ws);

    let diagnostics = AnalysisEngine::analyze(&ws);

    // Should have no broken links
    let broken: Vec<_> = diagnostics.iter().filter(|d| d.kind == DiagnosticKind::BrokenLink).collect();
    assert!(broken.is_empty(), "Harlowe: no broken links expected");

    // Should detect variable $health
    let doc = ws.get_document(&uri).unwrap();
    let start_passage = doc.find_passage("Start").unwrap();
    assert!(start_passage.vars.iter().any(|v| v.name == "$health"));
}

#[test]
fn harlowe_broken_link_detection() {
    let registry = FormatRegistry::with_defaults();
    let mut ws = workspace_with_metadata(StoryFormat::Harlowe, "Start");

    let src = ":: Start\nGo to [[Cave]].\n";
    let uri = Url::parse("file:///project/story.tw").unwrap();
    parse_and_insert(&mut ws, &registry, &uri, src, StoryFormat::Harlowe);
    rebuild_graph(&mut ws);

    let diagnostics = AnalysisEngine::analyze(&ws);

    // Cave passage doesn't exist — should detect broken link
    assert!(
        diagnostics.iter().any(|d| d.kind == DiagnosticKind::BrokenLink && d.message.contains("Cave")),
        "Harlowe: should detect broken link to Cave"
    );
}

// ===========================================================================
// Chapbook end-to-end
// ===========================================================================

#[test]
fn chapbook_parse_and_analyze() {
    let registry = FormatRegistry::with_defaults();
    let mut ws = workspace_with_metadata(StoryFormat::Chapbook, "Start");

    let src = ":: Start\nWelcome [[Cave]].\n[javascript]\nstate.visited = true;\n[/javascript]\nYou have {{state.gold}} coins.\n:: Cave\nA dark cave.\n";
    let uri = Url::parse("file:///project/story.tw").unwrap();
    parse_and_insert(&mut ws, &registry, &uri, src, StoryFormat::Chapbook);
    rebuild_graph(&mut ws);

    let diagnostics = AnalysisEngine::analyze(&ws);

    // Should have no broken links (Cave exists)
    let broken: Vec<_> = diagnostics.iter().filter(|d| d.kind == DiagnosticKind::BrokenLink).collect();
    assert!(broken.is_empty(), "Chapbook: no broken links expected");

    // Should detect state.visited write and state.gold read
    let doc = ws.get_document(&uri).unwrap();
    let start_passage = doc.find_passage("Start").unwrap();
    assert!(
        start_passage.vars.iter().any(|v| v.name == "state.visited" && v.kind == knot_core::VarKind::Init),
        "Chapbook: should detect state.visited write"
    );
    assert!(
        start_passage.vars.iter().any(|v| v.name == "state.gold" && v.kind == knot_core::VarKind::Read),
        "Chapbook: should detect state.gold read"
    );
}

#[test]
fn chapbook_modify_block() {
    let registry = FormatRegistry::with_defaults();
    let mut ws = workspace_with_metadata(StoryFormat::Chapbook, "Start");

    let src = ":: Start\n[modify]\ngold: 10\nname: Alice\n[/modify]\n";
    let uri = Url::parse("file:///project/story.tw").unwrap();
    parse_and_insert(&mut ws, &registry, &uri, src, StoryFormat::Chapbook);

    let doc = ws.get_document(&uri).unwrap();
    let start_passage = doc.find_passage("Start").unwrap();
    assert!(
        start_passage.vars.iter().any(|v| v.name == "modify.gold" && v.kind == knot_core::VarKind::Init),
        "Chapbook: should detect modify.gold write"
    );
    assert!(
        start_passage.vars.iter().any(|v| v.name == "modify.name" && v.kind == knot_core::VarKind::Init),
        "Chapbook: should detect modify.name write"
    );
}

// ===========================================================================
// Snowman end-to-end
// ===========================================================================

#[test]
fn snowman_parse_and_analyze() {
    let registry = FormatRegistry::with_defaults();
    let mut ws = workspace_with_metadata(StoryFormat::Snowman, "Start");

    let src = ":: Start\n<% s.gold = 10; %>You have <%= s.gold %> coins. [[Cave]]\n:: Cave\nA dark cave.\n";
    let uri = Url::parse("file:///project/story.tw").unwrap();
    parse_and_insert(&mut ws, &registry, &uri, src, StoryFormat::Snowman);
    rebuild_graph(&mut ws);

    let diagnostics = AnalysisEngine::analyze(&ws);

    // Should have no broken links (Cave exists)
    let broken: Vec<_> = diagnostics.iter().filter(|d| d.kind == DiagnosticKind::BrokenLink).collect();
    assert!(broken.is_empty(), "Snowman: no broken links expected");

    // Should detect s.gold write and s.gold read
    let doc = ws.get_document(&uri).unwrap();
    let start_passage = doc.find_passage("Start").unwrap();
    assert!(
        start_passage.vars.iter().any(|v| v.name == "gold" && v.kind == knot_core::VarKind::Init),
        "Snowman: should detect gold write"
    );
    assert!(
        start_passage.vars.iter().any(|v| v.name == "gold" && v.kind == knot_core::VarKind::Read),
        "Snowman: should detect gold read"
    );
}

#[test]
fn snowman_broken_link_and_unreachable() {
    let registry = FormatRegistry::with_defaults();
    let mut ws = workspace_with_metadata(StoryFormat::Snowman, "Start");

    let src = ":: Start\nGo to [[MissingPassage]].\n:: Orphan\nNobody links here.\n";
    let uri = Url::parse("file:///project/story.tw").unwrap();
    parse_and_insert(&mut ws, &registry, &uri, src, StoryFormat::Snowman);
    rebuild_graph(&mut ws);

    let diagnostics = AnalysisEngine::analyze(&ws);

    // Should detect broken link to MissingPassage
    assert!(
        diagnostics.iter().any(|d| d.kind == DiagnosticKind::BrokenLink),
        "Snowman: should detect broken link"
    );

    // Should detect Orphan as unreachable
    assert!(
        diagnostics.iter().any(|d| d.kind == DiagnosticKind::UnreachablePassage && d.passage_name == "Orphan"),
        "Snowman: should detect Orphan as unreachable"
    );
}

// ===========================================================================
// Cross-format consistency tests
// ===========================================================================

#[test]
fn all_formats_parse_passage_headers() {
    let registry = FormatRegistry::with_defaults();
    let src = ":: Start\nHello world.\n:: Forest\nA forest.\n";

    for format in &[StoryFormat::SugarCube, StoryFormat::Harlowe, StoryFormat::Chapbook, StoryFormat::Snowman] {
        let plugin = registry.get(format).unwrap();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);
        assert_eq!(
            result.passages.len(), 2,
            "{:?}: should parse 2 passages from simple source", format
        );
        assert_eq!(result.passages[0].name, "Start", "{:?}: first passage should be 'Start'", format);
        assert_eq!(result.passages[1].name, "Forest", "{:?}: second passage should be 'Forest'", format);
    }
}

#[test]
fn all_formats_extract_simple_links() {
    let registry = FormatRegistry::with_defaults();
    let src = ":: Start\nGo to [[Forest]].\n:: Forest\nTrees.\n";

    for format in &[StoryFormat::SugarCube, StoryFormat::Harlowe, StoryFormat::Chapbook, StoryFormat::Snowman] {
        let plugin = registry.get(format).unwrap();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);
        assert_eq!(
            result.passages[0].links.len(), 1,
            "{:?}: should extract 1 link from Start", format
        );
        assert_eq!(
            result.passages[0].links[0].target, "Forest",
            "{:?}: link target should be 'Forest'", format
        );
    }
}

#[test]
fn all_formats_handle_empty_input() {
    let registry = FormatRegistry::with_defaults();

    for format in &[StoryFormat::SugarCube, StoryFormat::Harlowe, StoryFormat::Chapbook, StoryFormat::Snowman] {
        let plugin = registry.get(format).unwrap();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), "");
        assert!(
            result.passages.is_empty(),
            "{:?}: empty input should produce no passages", format
        );
    }
}

#[test]
fn all_formats_produce_semantic_tokens() {
    let registry = FormatRegistry::with_defaults();
    let src = ":: Start\nHello [[World]].\n:: World\nDone.\n";

    for format in &[StoryFormat::SugarCube, StoryFormat::Harlowe, StoryFormat::Chapbook, StoryFormat::Snowman] {
        let plugin = registry.get(format).unwrap();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);
        assert!(
            !result.tokens.is_empty(),
            "{:?}: should produce semantic tokens", format
        );
    }
}

#[test]
fn all_formats_define_special_passages() {
    let registry = FormatRegistry::with_defaults();

    for format in &[StoryFormat::SugarCube, StoryFormat::Harlowe, StoryFormat::Chapbook, StoryFormat::Snowman] {
        let plugin = registry.get(format).unwrap();
        let specials = plugin.special_passages();
        assert!(
            !specials.is_empty(),
            "{:?}: should define at least one special passage", format
        );
        // All formats should recognize StoryData and StoryTitle as special
        assert!(plugin.is_special_passage("StoryData"), "{:?}: StoryData should be special", format);
        assert!(plugin.is_special_passage("StoryTitle"), "{:?}: StoryTitle should be special", format);
    }
}

// ===========================================================================
// Graph surgery with format-parsed passages
// ===========================================================================

#[test]
fn graph_surgery_with_sugarcube_parse() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::SugarCube).unwrap();

    let src = ":: Start\nHello [[Forest]].\n:: Forest\nTrees.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let mut graph = PassageGraph::new();
    graph_surgery(&mut graph, &[], &result.passages, "file:///project/story.tw", &[]);

    assert!(graph.contains_passage("Start"));
    assert!(graph.contains_passage("Forest"));
    assert_eq!(graph.edge_count(), 1, "Should have 1 edge: Start → Forest");
}

#[test]
fn graph_surgery_with_harlowe_parse() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::Harlowe).unwrap();

    let src = ":: Start\n(set: $x to 5)Go [[Cave]].\n:: Cave\nDark.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let mut graph = PassageGraph::new();
    graph_surgery(&mut graph, &[], &result.passages, "file:///project/story.tw", &[]);

    assert!(graph.contains_passage("Start"));
    assert!(graph.contains_passage("Cave"));
    assert_eq!(graph.edge_count(), 1, "Should have 1 edge: Start → Cave");
}

#[test]
fn graph_surgery_with_chapbook_parse() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::Chapbook).unwrap();

    let src = ":: Start\nWelcome [[Cave]].\n:: Cave\nDark.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let mut graph = PassageGraph::new();
    graph_surgery(&mut graph, &[], &result.passages, "file:///project/story.tw", &[]);

    assert!(graph.contains_passage("Start"));
    assert!(graph.contains_passage("Cave"));
}

#[test]
fn graph_surgery_with_snowman_parse() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::Snowman).unwrap();

    let src = ":: Start\n<% s.x = 1; %>Go [[Cave]].\n:: Cave\nDark.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let mut graph = PassageGraph::new();
    graph_surgery(&mut graph, &[], &result.passages, "file:///project/story.tw", &[]);

    assert!(graph.contains_passage("Start"));
    assert!(graph.contains_passage("Cave"));
}

// ===========================================================================
// Format-specific variable model compliance
// ===========================================================================

#[test]
fn sugarcube_full_variable_tracking() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::SugarCube).unwrap();
    assert!(plugin.supports_full_variable_tracking());
    assert!(!plugin.supports_partial_variable_tracking());
}

#[test]
fn snowman_full_variable_tracking() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::Snowman).unwrap();
    assert!(plugin.supports_full_variable_tracking());
    assert!(!plugin.supports_partial_variable_tracking());
}

#[test]
fn harlowe_partial_variable_tracking() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::Harlowe).unwrap();
    assert!(!plugin.supports_full_variable_tracking());
    assert!(plugin.supports_partial_variable_tracking());
}

#[test]
fn chapbook_no_variable_tracking() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::Chapbook).unwrap();
    assert!(!plugin.supports_full_variable_tracking());
    assert!(!plugin.supports_partial_variable_tracking());
}

// ===========================================================================
// Semantic token position verification tests
// ===========================================================================

/// Helper: Convert a byte offset in text to (line, character) using the same
/// logic as the LSP server's `byte_offset_to_position()`.
fn byte_offset_to_position(text: &str, offset: usize) -> (u32, u32) {
    let safe_offset = offset.min(text.len());
    let text_before = &text[..safe_offset];

    let line = if text_before.is_empty() {
        0u32
    } else {
        let line_count = text_before.lines().count() as u32;
        if text_before.ends_with('\n') {
            line_count
        } else {
            line_count - 1
        }
    };

    let last_newline = text_before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_text_before_offset = &text[last_newline..safe_offset];

    let character: u32 = line_text_before_offset
        .chars()
        .map(|c| if (c as u32) < 0x10000 { 1u32 } else { 2u32 })
        .sum();

    (line, character)
}

#[test]
fn sugarcube_header_token_positions_simple() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::SugarCube).unwrap();
    let src = ":: Start\nHello world.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    // :: prefix should be at byte 0 → LSP (0, 0)
    // Note: "Start" is a core special passage, so it uses SpecialPassageHeader
    let header_tok = result.tokens.iter().find(|t| 
        t.token_type == crate::plugin::SemanticTokenType::PassageHeader 
        || t.token_type == crate::plugin::SemanticTokenType::SpecialPassageHeader
    );
    assert!(header_tok.is_some(), "Should have PassageHeader or SpecialPassageHeader token");
    let ht = header_tok.unwrap();
    assert_eq!(ht.start, 0, ":: prefix should start at byte 0");
    assert_eq!(ht.length, 2, ":: prefix should be 2 bytes");
    let (line, char) = byte_offset_to_position(src, ht.start);
    assert_eq!(line, 0, ":: prefix should be on line 0");
    assert_eq!(char, 0, ":: prefix should start at char 0");

    // Passage name "Start" should be at byte 3 → LSP (0, 3)
    // Note: "Start" is a core special passage, so it uses SpecialPassage
    let name_tok = result.tokens.iter().find(|t| 
        t.token_type == crate::plugin::SemanticTokenType::PassageName 
        || t.token_type == crate::plugin::SemanticTokenType::SpecialPassage
    );
    assert!(name_tok.is_some(), "Should have PassageName or SpecialPassage token");
    let nt = name_tok.unwrap();
    assert_eq!(nt.start, 3, "Name 'Start' should start at byte 3");
    assert_eq!(nt.length, 5, "Name 'Start' should be 5 bytes");
    let (line, char) = byte_offset_to_position(src, nt.start);
    assert_eq!(line, 0, "Name should be on line 0");
    assert_eq!(char, 3, "Name should start at char 3");
}

#[test]
fn sugarcube_header_token_positions_with_tags() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::SugarCube).unwrap();
    let src = ":: Forest [dark scary]\nSome content.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    // Print all tokens for debugging
    eprintln!("Tokens for: {:?}", src);
    for tok in &result.tokens {
        let (line, char) = byte_offset_to_position(src, tok.start);
        let tok_text = &src[tok.start.min(src.len())..(tok.start + tok.length).min(src.len())];
        eprintln!("  type={:?} start={} len={} text={:?} -> LSP({}, {})", tok.token_type, tok.start, tok.length, tok_text, line, char);
    }

    // :: Forest [dark scary]
    // 0123456789012345678901
    //           111111111122
    // [ is at byte 10, "dark" starts at byte 11, "scary" starts at byte 16

    // Name "Forest" at byte 3 → LSP (0, 3)
    let name_tok = result.tokens.iter().find(|t| t.token_type == crate::plugin::SemanticTokenType::PassageName);
    assert!(name_tok.is_some(), "Should have PassageName token");
    let nt = name_tok.unwrap();
    assert_eq!(nt.start, 3, "Name 'Forest' should start at byte 3");
    let (_line, char) = byte_offset_to_position(src, nt.start);
    assert_eq!(char, 3, "Name 'Forest' should start at char 3");

    // Tags should be present
    let tag_toks: Vec<_> = result.tokens.iter().filter(|t| t.token_type == crate::plugin::SemanticTokenType::Tag).collect();
    assert!(!tag_toks.is_empty(), "Should have Tag tokens");

    if tag_toks.len() >= 2 {
        // "dark" should be at byte 11 → LSP (0, 11)
        let (line, char) = byte_offset_to_position(src, tag_toks[0].start);
        assert_eq!(line, 0, "Tag 'dark' should be on line 0");
        assert_eq!(char, 11, "Tag 'dark' should start at char 11, got {}", char);

        // "scary" should be at byte 16 → LSP (0, 16)
        let (line, char) = byte_offset_to_position(src, tag_toks[1].start);
        assert_eq!(line, 0, "Tag 'scary' should be on line 0");
        assert_eq!(char, 16, "Tag 'scary' should start at char 16, got {}", char);
    }
}

#[test]
fn sugarcube_header_token_positions_with_tags_and_metadata() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::SugarCube).unwrap();
    let src = ":: Forest [dark scary] {\"position\":\"100,200\"}\nSome content.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    eprintln!("Tokens for: {:?}", src);
    for tok in &result.tokens {
        let (line, char) = byte_offset_to_position(src, tok.start);
        let tok_text = &src[tok.start.min(src.len())..(tok.start + tok.length).min(src.len())];
        eprintln!("  type={:?} start={} len={} text={:?} -> LSP({}, {})", tok.token_type, tok.start, tok.length, tok_text, line, char);
    }

    // :: Forest [dark scary] {"position":"100,200"}
    // 0123456789012345678901234567890123456789
    //           1111111111222222222233333333
    // [ is at byte 10, "dark" at byte 11, "scary" at byte 16

    let tag_toks: Vec<_> = result.tokens.iter().filter(|t| t.token_type == crate::plugin::SemanticTokenType::Tag).collect();
    assert!(!tag_toks.is_empty(), "Should have Tag tokens even with metadata");

    if tag_toks.len() >= 2 {
        let (_line, char) = byte_offset_to_position(src, tag_toks[0].start);
        assert_eq!(char, 11, "Tag 'dark' should start at char 11, got {}", char);

        let (_line, char) = byte_offset_to_position(src, tag_toks[1].start);
        assert_eq!(char, 16, "Tag 'scary' should start at char 16, got {}", char);
    }
}

#[test]
fn sugarcube_body_token_positions() {
    let registry = FormatRegistry::with_defaults();
    let plugin = registry.get(&StoryFormat::SugarCube).unwrap();
    let src = ":: Start\n<<set $gold to 10>>You have $gold coins.\n:: Forest\nTrees.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    eprintln!("Tokens for: {:?}", src);
    for tok in &result.tokens {
        let (line, char) = byte_offset_to_position(src, tok.start);
        let tok_text = &src[tok.start.min(src.len())..(tok.start + tok.length).min(src.len())];
        eprintln!("  type={:?} start={} len={} text={:?} -> LSP({}, {})", tok.token_type, tok.start, tok.length, tok_text, line, char);
    }

    // Body starts at byte 9 (after ":: Start\n")
    // <<set $gold to 10>> starts at byte 9
    // "set" name starts at byte 11 (after <<)
    // Expected: macro "set" at LSP (1, 2)

    let macro_toks: Vec<_> = result.tokens.iter().filter(|t| t.token_type == crate::plugin::SemanticTokenType::Macro).collect();
    assert!(!macro_toks.is_empty(), "Should have Macro tokens");

    let set_macro = macro_toks.iter().find(|t| {
        let tok_text = &src[t.start.min(src.len())..(t.start + t.length).min(src.len())];
        tok_text == "set"
    });
    assert!(set_macro.is_some(), "Should have 'set' macro token");
    let sm = set_macro.unwrap();

    let (line, char) = byte_offset_to_position(src, sm.start);
    assert_eq!(line, 1, "Macro 'set' should be on line 1, got {}", line);
    assert_eq!(char, 2, "Macro 'set' should start at char 2, got {}", char);
}

#[test]
fn test_semantic_token_positions_header() {
    use crate::plugin::{FormatPlugin, SemanticTokenType};
    use crate::sugarcube::SugarCubePlugin;
    
    let plugin = SugarCubePlugin::new();
    let text = ":: Start [tag] {\"position\":\"100,200\"}\n<<set $x to 5>>\n:: End\n";
    
    let result = plugin.parse(&url::Url::parse("file:///test.tw").unwrap(), text);
    
    // Debug: print all tokens with their positions
    for (i, tok) in result.tokens.iter().enumerate() {
        let safe_start = tok.start.min(text.len());
        let safe_end = (tok.start + tok.length).min(text.len());
        let tok_text = &text[safe_start..safe_end];
        eprintln!("  token {:2}: type={:?} start={} len={} text='{}' modifier={:?}", 
                 i, tok.token_type, tok.start, tok.length, tok_text, tok.modifier);
    }
    
    // Verify header tokens for ":: Start [tag]"
    // Byte positions in the text:
    // 0: ':', 1: ':', 2: ' ', 3: 'S', 4: 't', 5: 'a', 6: 'r', 7: 't',
    // 8: ' ', 9: '[', 10: 't', 11: 'a', 12: 'g', 13: ']'
    
    // The :: prefix should be at byte 0, length 2
    let prefix_tokens: Vec<_> = result.tokens.iter()
        .filter(|t| matches!(t.token_type, SemanticTokenType::PassageHeader | SemanticTokenType::SpecialPassageHeader))
        .collect();
    assert!(!prefix_tokens.is_empty(), "Should have at least one passage header prefix token");
    let first_prefix = prefix_tokens[0];
    assert_eq!(first_prefix.start, 0, ":: prefix should start at byte 0, got {}", first_prefix.start);
    assert_eq!(first_prefix.length, 2, ":: prefix should have length 2");
    
    // The passage name "Start" should be at byte 3, length 5
    let name_tokens: Vec<_> = result.tokens.iter()
        .filter(|t| matches!(t.token_type, SemanticTokenType::PassageName | SemanticTokenType::SpecialPassage))
        .collect();
    assert!(!name_tokens.is_empty(), "Should have at least one passage name token");
    let first_name = name_tokens[0];
    assert_eq!(first_name.start, 3, "Passage name should start at byte 3, got {}", first_name.start);
    assert_eq!(first_name.length, 5, "Passage name should have length 5, got {}", first_name.length);
    
    // The tag "tag" should be at byte 10, length 3
    let tag_tokens: Vec<_> = result.tokens.iter()
        .filter(|t| matches!(t.token_type, SemanticTokenType::Tag))
        .collect();
    assert!(!tag_tokens.is_empty(), "Should have at least one tag token");
    let first_tag = tag_tokens[0];
    assert_eq!(first_tag.start, 10, "Tag should start at byte 10, got {}", first_tag.start);
    assert_eq!(first_tag.length, 3, "Tag should have length 3, got {}", first_tag.length);
}

#[test]
fn sugarcube_no_space_after_colons_token_positions() {
    // Regression test: Headers without space after :: (e.g., ::Start instead
    // of :: Start) should have correct token positions. The name should start
    // at byte 2 (right after ::), not byte 3.
    use crate::plugin::{FormatPlugin, SemanticTokenType};
    use crate::sugarcube::SugarCubePlugin;

    let plugin = SugarCubePlugin::new();
    let text = "::Start {\"position\":\"420,60\"}\n<<setSceneLoc \"records-desk\">>\n";
    let result = plugin.parse(&url::Url::parse("file:///test.tw").unwrap(), text);

    eprintln!("=== No-space-after-colons test ===");
    for (i, tok) in result.tokens.iter().enumerate() {
        let safe_start = tok.start.min(text.len());
        let safe_end = (tok.start + tok.length).min(text.len());
        let tok_text = &text[safe_start..safe_end];
        let (line, char) = byte_offset_to_position(text, tok.start);
        eprintln!("  token {:2}: type={:?} start={} len={} text='{}' -> LSP({}, {})",
                 i, tok.token_type, tok.start, tok.length, tok_text, line, char);
    }

    // ::Start {"position":"420,60"}
    // 0123456789012345678901234567890
    // :: at bytes 0-1, Start at bytes 2-6

    let prefix_tok = result.tokens.iter().find(|t|
        matches!(t.token_type, SemanticTokenType::PassageHeader | SemanticTokenType::SpecialPassageHeader)
    );
    assert!(prefix_tok.is_some(), "Should have header prefix token");
    let pt = prefix_tok.unwrap();
    assert_eq!(pt.start, 0, ":: prefix should start at byte 0, got {}", pt.start);
    assert_eq!(pt.length, 2, ":: prefix should be 2 bytes");

    let name_tok = result.tokens.iter().find(|t|
        matches!(t.token_type, SemanticTokenType::PassageName | SemanticTokenType::SpecialPassage)
    );
    assert!(name_tok.is_some(), "Should have passage name token");
    let nt = name_tok.unwrap();
    assert_eq!(nt.start, 2, "Name 'Start' should start at byte 2 (no space after ::), got {}", nt.start);
    assert_eq!(nt.length, 5, "Name 'Start' should be 5 bytes");

    let (line, char) = byte_offset_to_position(text, nt.start);
    assert_eq!(line, 0, "Name should be on line 0");
    assert_eq!(char, 2, "Name should start at char 2 (no space after ::), got {}", char);

    // Body tokens: macro "setSceneLoc" should be at the right position
    let macro_toks: Vec<_> = result.tokens.iter()
        .filter(|t| t.token_type == SemanticTokenType::Macro)
        .collect();
    assert!(!macro_toks.is_empty(), "Should have Macro tokens in body");

    let set_scene = macro_toks.iter().find(|t| {
        let tok_text = &text[t.start.min(text.len())..(t.start + t.length).min(text.len())];
        tok_text == "setSceneLoc"
    });
    assert!(set_scene.is_some(), "Should find 'setSceneLoc' macro token");
    let sm = set_scene.unwrap();
    let (line, char) = byte_offset_to_position(text, sm.start);
    eprintln!("  setSceneLoc macro at byte {} -> LSP({}, {})", sm.start, line, char);
    assert_eq!(line, 1, "Macro should be on line 1");
    assert_eq!(char, 2, "Macro 'setSceneLoc' should start at char 2 on line 1, got {}", char);
}

#[test]
fn sugarcube_script_tag_token_positions() {
    // Regression test: Passages tagged [script] should have Tag tokens
    // with the TwineCore modifier.
    use crate::plugin::{FormatPlugin, SemanticTokenType, SemanticTokenModifier};
    use crate::sugarcube::SugarCubePlugin;

    let plugin = SugarCubePlugin::new();
    let text = "::MyScript [script] {\"position\":\"100,200\"}\nconsole.log('hello');\n";
    let result = plugin.parse(&url::Url::parse("file:///test.tw").unwrap(), text);

    eprintln!("=== Script tag test ===");
    for (i, tok) in result.tokens.iter().enumerate() {
        let safe_start = tok.start.min(text.len());
        let safe_end = (tok.start + tok.length).min(text.len());
        let tok_text = &text[safe_start..safe_end];
        eprintln!("  token {:2}: type={:?} start={} len={} text='{}' modifier={:?}",
                 i, tok.token_type, tok.start, tok.length, tok_text, tok.modifier);
    }

    // Should have a Tag token for "script"
    let tag_toks: Vec<_> = result.tokens.iter()
        .filter(|t| t.token_type == SemanticTokenType::Tag)
        .collect();
    assert!(!tag_toks.is_empty(), "Should have Tag tokens for [script]");

    let script_tag = tag_toks.iter().find(|t| {
        let tok_text = &text[t.start.min(text.len())..(t.start + t.length).min(text.len())];
        tok_text == "script"
    });
    assert!(script_tag.is_some(), "Should find 'script' tag token");
    let st = script_tag.unwrap();
    // ::MyScript [script] — ::(0-1) MyScript(2-9) ' '(10) [(11) script(12-17) ](18)
    assert_eq!(st.start, 12, "'script' tag should start at byte 12, got {}", st.start);
    assert_eq!(st.length, 6, "'script' tag should be 6 bytes");
    assert_eq!(st.modifier, Some(SemanticTokenModifier::TwineCore),
               "'script' tag should have TwineCore modifier");
}

#[test]
fn sugarcube_stylesheet_tag_token_positions() {
    // Regression test: Passages tagged [stylesheet] should have Tag tokens.
    use crate::plugin::{FormatPlugin, SemanticTokenType, SemanticTokenModifier};
    use crate::sugarcube::SugarCubePlugin;

    let plugin = SugarCubePlugin::new();
    let text = "::MyCSS [stylesheet] {\"position\":\"100,200\"}\nbody { color: red; }\n";
    let result = plugin.parse(&url::Url::parse("file:///test.tw").unwrap(), text);

    eprintln!("=== Stylesheet tag test ===");
    for (i, tok) in result.tokens.iter().enumerate() {
        let safe_start = tok.start.min(text.len());
        let safe_end = (tok.start + tok.length).min(text.len());
        let tok_text = &text[safe_start..safe_end];
        eprintln!("  token {:2}: type={:?} start={} len={} text='{}' modifier={:?}",
                 i, tok.token_type, tok.start, tok.length, tok_text, tok.modifier);
    }

    let tag_toks: Vec<_> = result.tokens.iter()
        .filter(|t| t.token_type == SemanticTokenType::Tag)
        .collect();
    assert!(!tag_toks.is_empty(), "Should have Tag tokens for [stylesheet]");

    let stylesheet_tag = tag_toks.iter().find(|t| {
        let tok_text = &text[t.start.min(text.len())..(t.start + t.length).min(text.len())];
        tok_text == "stylesheet"
    });
    assert!(stylesheet_tag.is_some(), "Should find 'stylesheet' tag token");
    let st = stylesheet_tag.unwrap();
    assert_eq!(st.modifier, Some(SemanticTokenModifier::TwineCore),
               "'stylesheet' tag should have TwineCore modifier");
}
