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
                    is_broken: !target_exists,
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
fn registry_contains_all_four_formats() {
    let registry = FormatRegistry::with_defaults();
    let formats = registry.formats();
    assert_eq!(formats.len(), 4, "Should have exactly 4 format plugins");
    assert!(formats.contains(&StoryFormat::SugarCube));
    assert!(formats.contains(&StoryFormat::Harlowe));
    assert!(formats.contains(&StoryFormat::Chapbook));
    assert!(formats.contains(&StoryFormat::Snowman));
}

#[test]
fn registry_default_includes_sugarcube() {
    let registry = FormatRegistry::default();
    assert!(registry.get(&StoryFormat::SugarCube).is_some());
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
    assert!(start_passage.vars.iter().any(|v| v.name == "$gold" && v.kind == knot_core::VarKind::Write));
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
        start_passage.vars.iter().any(|v| v.name == "state.visited" && v.kind == knot_core::VarKind::Write),
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
        start_passage.vars.iter().any(|v| v.name == "modify.gold" && v.kind == knot_core::VarKind::Write),
        "Chapbook: should detect modify.gold write"
    );
    assert!(
        start_passage.vars.iter().any(|v| v.name == "modify.name" && v.kind == knot_core::VarKind::Write),
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
        start_passage.vars.iter().any(|v| v.name == "gold" && v.kind == knot_core::VarKind::Write),
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
    graph_surgery(&mut graph, &[], &result.passages, "file:///project/story.tw");

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
    graph_surgery(&mut graph, &[], &result.passages, "file:///project/story.tw");

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
    graph_surgery(&mut graph, &[], &result.passages, "file:///project/story.tw");

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
    graph_surgery(&mut graph, &[], &result.passages, "file:///project/story.tw");

    assert!(graph.contains_passage("Start"));
    assert!(graph.contains_passage("Cave"));
}

// ===========================================================================
// Format-specific variable model compliance
// ===========================================================================

#[test]
fn sugarcube_full_variable_tracking() {
    assert!(StoryFormat::SugarCube.supports_full_variable_tracking());
    assert!(!StoryFormat::SugarCube.supports_partial_variable_tracking());
}

#[test]
fn snowman_full_variable_tracking() {
    assert!(StoryFormat::Snowman.supports_full_variable_tracking());
    assert!(!StoryFormat::Snowman.supports_partial_variable_tracking());
}

#[test]
fn harlowe_partial_variable_tracking() {
    assert!(!StoryFormat::Harlowe.supports_full_variable_tracking());
    assert!(StoryFormat::Harlowe.supports_partial_variable_tracking());
}

#[test]
fn chapbook_no_variable_tracking() {
    assert!(!StoryFormat::Chapbook.supports_full_variable_tracking());
    assert!(!StoryFormat::Chapbook.supports_partial_variable_tracking());
}
