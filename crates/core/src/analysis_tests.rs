//! Integration tests for the full analysis pipeline.
//!
//! These tests exercise the complete workflow:
//! parse → graph build → analysis → diagnostics

#[cfg(test)]
mod tests {
    use crate::graph::{DiagnosticKind, PassageEdge, PassageGraph, PassageNode};
    use crate::passage::{
        Link, Passage, SpecialPassageBehavior, SpecialPassageDef, StoryFormat, VarKind, VarOp,
    };
    use crate::document::Document;
    use crate::workspace::{StoryMetadata, Workspace};
    use crate::AnalysisEngine;
    use url::Url;

    /// Helper: create a simple passage with links.
    fn make_passage(name: &str, link_targets: &[&str]) -> Passage {
        let mut p = Passage::new(name.to_string(), 0..100);
        p.links = link_targets
            .iter()
            .map(|t| Link {
                display_text: None,
                target: t.to_string(),
                span: 0..t.len(),
            })
            .collect();
        p
    }

    /// Helper: create a passage with variable operations.
    fn make_passage_with_vars(name: &str, writes: &[&str], reads: &[&str]) -> Passage {
        let mut p = Passage::new(name.to_string(), 0..100);
        p.vars = writes
            .iter()
            .map(|v| VarOp {
                name: v.to_string(),
                kind: VarKind::Write,
                span: 0..v.len(),
                is_temporary: false,
            })
            .chain(reads.iter().map(|v| VarOp {
                name: v.to_string(),
                kind: VarKind::Read,
                span: 0..v.len(),
                is_temporary: false,
            }))
            .collect();
        p
    }

    /// Helper: create a StoryData metadata passage.
    fn make_story_data_passage() -> Passage {
        let mut p = Passage::new("StoryData".to_string(), 0..50);
        p.is_special = true;
        p.special_def = Some(SpecialPassageDef {
            name: "StoryData".to_string(),
            behavior: SpecialPassageBehavior::Metadata,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
        });
        p
    }

    /// Helper: rebuild workspace graph from all documents.
    #[allow(clippy::type_complexity)]
    fn rebuild_workspace_graph(workspace: &mut Workspace) {
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

    // -----------------------------------------------------------------------
    // Broken link detection
    // -----------------------------------------------------------------------

    #[test]
    fn broken_link_detection() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        doc.passages.push(make_passage("Start", &["Forest", "Cave"]));
        doc.passages.push(make_passage("Forest", &[]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let broken_links: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::BrokenLink)
            .collect();

        assert_eq!(broken_links.len(), 1, "Should detect exactly 1 broken link");
        assert!(broken_links[0].message.contains("Cave"));
    }

    // -----------------------------------------------------------------------
    // Unreachable passage detection
    // -----------------------------------------------------------------------

    #[test]
    fn unreachable_passage_detection() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        doc.passages.push(make_passage("Start", &["Forest"]));
        doc.passages.push(make_passage("Forest", &["Start"]));
        doc.passages.push(make_passage("Cave", &[])); // isolated
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let unreachable: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UnreachablePassage)
            .collect();

        assert_eq!(unreachable.len(), 1, "Should detect exactly 1 unreachable passage");
        assert!(unreachable[0].message.contains("Cave"));
    }

    // -----------------------------------------------------------------------
    // Infinite loop detection
    // -----------------------------------------------------------------------

    #[test]
    fn infinite_loop_cycle_no_mutation() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        doc.passages.push(make_passage("Start", &["Forest"]));
        doc.passages.push(make_passage("Forest", &["Start"]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let loops_: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::InfiniteLoop)
            .collect();

        assert_eq!(loops_.len(), 1, "Should detect 1 infinite loop");
    }

    #[test]
    fn no_infinite_loop_with_mutation() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        let start = make_passage("Start", &["Forest"]);
        let mut forest = make_passage("Forest", &["Start"]);
        forest.vars.push(VarOp {
            name: "$visited".to_string(),
            kind: VarKind::Write,
            span: 0..8,
            is_temporary: false,
        });
        doc.passages.push(start);
        doc.passages.push(forest);
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let loops_: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::InfiniteLoop)
            .collect();

        assert!(loops_.is_empty(), "Cycle with mutation should not be flagged");
    }

    // -----------------------------------------------------------------------
    // Uninitialized variable detection
    // -----------------------------------------------------------------------

    #[test]
    fn uninitialized_variable_detection() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        doc.passages.push(make_passage_with_vars("Start", &[], &["$gold"]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let uninit: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UninitializedVariable)
            .collect();

        assert_eq!(uninit.len(), 1, "Should detect 1 uninitialized variable");
        assert!(uninit[0].message.contains("$gold"));
    }

    #[test]
    fn initialized_variable_not_flagged() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        doc.passages.push(make_passage_with_vars("Start", &["$gold"], &["$gold"]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let uninit: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UninitializedVariable)
            .collect();

        assert!(uninit.is_empty(), "Written variable should not be flagged");
    }

    // -----------------------------------------------------------------------
    // Missing StoryData
    // -----------------------------------------------------------------------

    #[test]
    fn missing_story_data_diagnostic() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        doc.passages.push(make_passage("Start", &[]));

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        assert!(diagnostics.iter().any(|d| d.kind == DiagnosticKind::MissingStoryData));
    }

    // -----------------------------------------------------------------------
    // Missing start passage
    // -----------------------------------------------------------------------

    #[test]
    fn missing_start_passage_diagnostic() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Prologue".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        doc.passages.push(make_passage("Start", &[]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        assert!(diagnostics.iter().any(|d| d.kind == DiagnosticKind::MissingStartPassage));
    }

    // -----------------------------------------------------------------------
    // Multi-document workspace
    // -----------------------------------------------------------------------

    #[test]
    fn multi_document_workspace_analysis() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        // File 1: Start and Forest
        let mut doc1 = Document::new(
            Url::parse("file:///project/part1.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        doc1.passages.push(make_passage("Start", &["Forest"]));
        doc1.passages.push(make_passage("Forest", &[]));

        // File 2: Cave (unreachable) and StoryData
        let mut doc2 = Document::new(
            Url::parse("file:///project/part2.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        doc2.passages.push(make_passage("Cave", &[]));
        doc2.passages.push(make_story_data_passage());

        workspace.insert_document(doc1);
        workspace.insert_document(doc2);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        // Cave should be unreachable
        assert!(diagnostics.iter().any(|d| {
            d.kind == DiagnosticKind::UnreachablePassage && d.passage_name == "Cave"
        }));

        // Forest should NOT be unreachable (linked from Start)
        assert!(!diagnostics.iter().any(|d| {
            d.kind == DiagnosticKind::UnreachablePassage && d.passage_name == "Forest"
        }));
    }

    // -----------------------------------------------------------------------
    // Graph surgery with file_uri
    // -----------------------------------------------------------------------

    #[test]
    fn graph_surgery_preserves_file_uri() {
        use crate::editing::graph_surgery;

        let mut graph = PassageGraph::new();

        // Add initial passage
        let old_passages = vec![make_passage("Start", &["Forest"])];
        graph_surgery(&mut graph, &[], &old_passages, "file:///project/story.tw");

        // Verify the passage has the correct file_uri
        let node = graph.get_passage("Start");
        assert!(node.is_some());
        assert_eq!(node.unwrap().file_uri, "file:///project/story.tw");
    }

    #[test]
    fn graph_surgery_adds_new_passage() {
        use crate::editing::graph_surgery;

        let mut graph = PassageGraph::new();

        // Initial state: just Start
        let initial = vec![make_passage("Start", &["Forest"])];
        graph_surgery(&mut graph, &[], &initial, "file:///project/story.tw");

        // Add Forest passage
        let updated = vec![
            make_passage("Start", &["Forest"]),
            make_passage("Forest", &[]),
        ];
        let result = graph_surgery(
            &mut graph,
            &initial,
            &updated,
            "file:///project/story.tw",
        );

        assert!(result.added.contains(&"Forest".to_string()));
        assert!(result.needs_analysis);
        assert!(graph.contains_passage("Forest"));
    }

    #[test]
    fn graph_surgery_removes_passage() {
        use crate::editing::graph_surgery;

        let mut graph = PassageGraph::new();

        let initial = vec![
            make_passage("Start", &["Forest"]),
            make_passage("Forest", &[]),
        ];
        graph_surgery(&mut graph, &[], &initial, "file:///project/story.tw");

        // Remove Forest passage AND update Start to not link to it
        let updated = vec![make_passage("Start", &[])];
        let result = graph_surgery(
            &mut graph,
            &initial,
            &updated,
            "file:///project/story.tw",
        );

        assert!(result.removed.contains(&"Forest".to_string()));
        assert!(!graph.contains_passage("Forest"));
    }

    #[test]
    fn graph_surgery_modifies_passage_links() {
        use crate::editing::graph_surgery;

        let mut graph = PassageGraph::new();

        let initial = vec![
            make_passage("Start", &["Forest"]),
            make_passage("Forest", &[]),
        ];
        graph_surgery(&mut graph, &[], &initial, "file:///project/story.tw");

        // Modify Start to link to Cave instead of Forest
        let updated = vec![
            make_passage("Start", &["Cave"]),
            make_passage("Forest", &[]),
        ];
        let result = graph_surgery(
            &mut graph,
            &initial,
            &updated,
            "file:///project/story.tw",
        );

        assert!(result.modified.contains(&"Start".to_string()));
        assert!(result.needs_analysis);
    }

    // -----------------------------------------------------------------------
    // recheck_broken_links
    // -----------------------------------------------------------------------

    #[test]
    fn recheck_broken_links_after_addition() {
        let mut graph = PassageGraph::new();

        // Start links to Forest, but Forest doesn't exist yet — broken link
        let start = PassageNode {
            name: "Start".to_string(),
            file_uri: "file:///project/story.tw".to_string(),
            is_special: false,
            is_metadata: false,
            is_placeholder: false,
        };
        graph.add_passage(start);
        graph.add_edge(
            "Start",
            "Forest",
            PassageEdge {
                display_text: None,
                is_broken: true,
            },
        );

        // Now add Forest — broken link should be resolved
        let forest = PassageNode {
            name: "Forest".to_string(),
            file_uri: "file:///project/story.tw".to_string(),
            is_special: false,
            is_metadata: false,
            is_placeholder: false,
        };
        graph.add_passage(forest);
        graph.recheck_broken_links();

        // The edge should no longer be broken
        let broken_count = graph.detect_broken_links().len();
        assert_eq!(broken_count, 0, "Broken link should be resolved after adding Forest");
    }

    #[test]
    fn recheck_broken_links_after_removal() {
        let mut graph = PassageGraph::new();

        // Start and Forest both exist — no broken links
        let start = PassageNode {
            name: "Start".to_string(),
            file_uri: "file:///project/story.tw".to_string(),
            is_special: false,
            is_metadata: false,
            is_placeholder: false,
        };
        let forest = PassageNode {
            name: "Forest".to_string(),
            file_uri: "file:///project/story.tw".to_string(),
            is_special: false,
            is_metadata: false,
            is_placeholder: false,
        };
        graph.add_passage(start);
        graph.add_passage(forest);
        graph.add_edge(
            "Start",
            "Forest",
            PassageEdge {
                display_text: None,
                is_broken: false,
            },
        );

        // Remove Forest — petgraph removes all connected edges too.
        // In a real scenario, the source document would be re-parsed and
        // the link to Forest would be re-added as a broken link.
        graph.remove_passage("Forest");

        // The edge from Start to Forest is gone (petgraph removes it with the node)
        // Simulate re-adding the edge after re-parsing the source document
        graph.add_edge(
            "Start",
            "Forest",
            PassageEdge {
                display_text: None,
                is_broken: true, // Forest no longer exists
            },
        );
        graph.recheck_broken_links();

        let broken_count = graph.detect_broken_links().len();
        assert_eq!(broken_count, 1, "Link should be broken after removing Forest");
    }

    // -----------------------------------------------------------------------
    // Workspace config loading
    // -----------------------------------------------------------------------

    #[test]
    fn workspace_load_valid_config() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        let config = r#"{
            "compiler_path": "/usr/local/bin/tweego",
            "build": {
                "output_dir": "dist",
                "flags": ["--module", "sugarcube-2"]
            },
            "diagnostics": {
                "broken-link": "Error",
                "unreachable-passage": "Warning"
            },
            "ignore": ["node_modules", ".git"],
            "format": "Harlowe"
        }"#;
        ws.load_config(config).expect("config should load");

        assert_eq!(ws.config.compiler_path, Some(std::path::PathBuf::from("/usr/local/bin/tweego")));
        assert_eq!(ws.config.build.output_dir, "dist");
        assert_eq!(ws.config.build.flags, vec!["--module", "sugarcube-2"]);
        assert_eq!(ws.config.format, Some("Harlowe".to_string()));
    }

    #[test]
    fn workspace_load_invalid_config_fails() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        let config = "not valid json {{{";
        let result = ws.load_config(config);
        assert!(result.is_err());
        // Default config should still be in place
        assert!(ws.config.compiler_path.is_none());
    }

    #[test]
    fn workspace_config_format_overrides_default() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        let config = r#"{ "format": "Harlowe" }"#;
        ws.load_config(config).expect("config should load");

        // With no metadata set, config format should take priority (Priority 2)
        assert_eq!(ws.resolve_format(), StoryFormat::Harlowe);
    }

    // -----------------------------------------------------------------------
    // Document removal with graph update
    // -----------------------------------------------------------------------

    #[test]
    fn remove_document_and_update_graph() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());

        let uri1 = Url::parse("file:///project/part1.tw").unwrap();
        let mut doc1 = Document::new(uri1.clone(), StoryFormat::SugarCube);
        doc1.passages.push(make_passage("Start", &["Forest"]));

        let uri2 = Url::parse("file:///project/part2.tw").unwrap();
        let mut doc2 = Document::new(uri2.clone(), StoryFormat::SugarCube);
        doc2.passages.push(make_passage("Forest", &[]));

        ws.insert_document(doc1);
        ws.insert_document(doc2);
        rebuild_workspace_graph(&mut ws);

        assert!(ws.contains_document(&uri1));
        assert!(ws.contains_document(&uri2));
        assert!(ws.graph.contains_passage("Start"));
        assert!(ws.graph.contains_passage("Forest"));

        // Remove the document containing Forest
        let removed = ws.remove_document_and_update_graph(&uri2);
        assert!(removed.is_some());
        assert!(!ws.contains_document(&uri2));
        assert!(!ws.graph.contains_passage("Forest"));
        assert!(ws.graph.contains_passage("Start"));
    }

    #[test]
    fn find_passage_file_uri() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());

        let uri1 = Url::parse("file:///project/story.tw").unwrap();
        let mut doc = Document::new(uri1.clone(), StoryFormat::SugarCube);
        doc.passages.push(make_passage("Start", &[]));

        ws.insert_document(doc);

        let found_uri = ws.find_passage_file_uri("Start");
        assert!(found_uri.is_some());
        assert_eq!(found_uri.unwrap(), uri1);

        let not_found = ws.find_passage_file_uri("NonExistent");
        assert!(not_found.is_none());
    }

    // -----------------------------------------------------------------------
    // Graph export with metadata
    // -----------------------------------------------------------------------

    #[test]
    fn export_graph_with_metadata_includes_tags_and_degrees() {
        let mut graph = PassageGraph::new();

        let start = PassageNode {
            name: "Start".to_string(),
            file_uri: "file:///project/story.tw".to_string(),
            is_special: false,
            is_metadata: false,
            is_placeholder: false,
        };
        let forest = PassageNode {
            name: "Forest".to_string(),
            file_uri: "file:///project/story.tw".to_string(),
            is_special: false,
            is_metadata: false,
            is_placeholder: false,
        };
        let story_init = PassageNode {
            name: "StoryInit".to_string(),
            file_uri: "file:///project/story.tw".to_string(),
            is_special: true,
            is_metadata: false,
            is_placeholder: false,
        };
        let story_data = PassageNode {
            name: "StoryData".to_string(),
            file_uri: "file:///project/story.tw".to_string(),
            is_special: true,
            is_metadata: true,
            is_placeholder: false,
        };

        graph.add_passage(start);
        graph.add_passage(forest);
        graph.add_passage(story_init);
        graph.add_passage(story_data);

        graph.add_edge("Start", "Forest", PassageEdge {
            display_text: Some("Go to forest".to_string()),
            is_broken: false,
        });
        graph.add_edge("Start", "MissingPassage", PassageEdge {
            display_text: None,
            is_broken: true,
        });

        let mut tags = std::collections::HashMap::new();
        tags.insert("Start".to_string(), vec!["important".to_string(), "begin".to_string()]);
        tags.insert("Forest".to_string(), vec!["outdoor".to_string()]);

        let unreachable = std::collections::HashSet::new();

        let export = graph.export_graph_with_metadata(&tags, &unreachable);

        // Verify nodes — 4 real nodes (Start, Forest, StoryInit, StoryData).
        // MissingPassage is a placeholder node (created by get_or_create_node for
        // broken-link targets) and is filtered out of the export.
        assert_eq!(export.nodes.len(), 4);

        let start_node = export.nodes.iter().find(|n| n.id == "Start").unwrap();
        assert_eq!(start_node.tags, vec!["important", "begin"]);
        assert_eq!(start_node.out_degree, 2); // Start → Forest, Start → MissingPassage
        assert_eq!(start_node.in_degree, 0);
        assert!(!start_node.is_special);
        assert!(!start_node.is_metadata);

        let forest_node = export.nodes.iter().find(|n| n.id == "Forest").unwrap();
        assert_eq!(forest_node.tags, vec!["outdoor"]);
        assert_eq!(forest_node.out_degree, 0);
        assert_eq!(forest_node.in_degree, 1);
        assert!(!forest_node.is_special);
        assert!(!forest_node.is_metadata);

        let init_node = export.nodes.iter().find(|n| n.id == "StoryInit").unwrap();
        assert!(init_node.is_special);
        assert!(!init_node.is_metadata);

        let data_node = export.nodes.iter().find(|n| n.id == "StoryData").unwrap();
        assert!(data_node.is_metadata);

        // Verify edges
        assert_eq!(export.edges.len(), 2);

        let broken_edge = export.edges.iter().find(|e| e.is_broken).unwrap();
        assert_eq!(broken_edge.source, "Start");
        assert_eq!(broken_edge.target, "MissingPassage");

        let normal_edge = export.edges.iter().find(|e| !e.is_broken).unwrap();
        assert_eq!(normal_edge.source, "Start");
        assert_eq!(normal_edge.target, "Forest");
        assert_eq!(normal_edge.display_text, Some("Go to forest".to_string()));
    }

    // -----------------------------------------------------------------------
    // Phase 3: Dataflow engine tests
    // -----------------------------------------------------------------------

    /// Helper: create a passage with variable operations including temporary vars.
    fn make_passage_with_temp_vars(
        name: &str,
        writes: &[&str],
        reads: &[&str],
        temp_writes: &[&str],
        temp_reads: &[&str],
    ) -> Passage {
        let mut p = Passage::new(name.to_string(), 0..100);
        p.vars = writes
            .iter()
            .map(|v| VarOp {
                name: v.to_string(),
                kind: VarKind::Write,
                span: 0..v.len(),
                is_temporary: false,
            })
            .chain(reads.iter().map(|v| VarOp {
                name: v.to_string(),
                kind: VarKind::Read,
                span: 0..v.len(),
                is_temporary: false,
            }))
            .chain(temp_writes.iter().map(|v| VarOp {
                name: v.to_string(),
                kind: VarKind::Write,
                span: 0..v.len(),
                is_temporary: true,
            }))
            .chain(temp_reads.iter().map(|v| VarOp {
                name: v.to_string(),
                kind: VarKind::Read,
                span: 0..v.len(),
                is_temporary: true,
            }))
            .collect();
        p
    }

    /// Helper: create a special passage with variable contributions (like StoryInit).
    fn make_story_init_passage() -> Passage {
        let mut p = Passage::new("StoryInit".to_string(), 0..100);
        p.is_special = true;
        p.special_def = Some(SpecialPassageDef {
            name: "StoryInit".to_string(),
            behavior: SpecialPassageBehavior::Startup,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(0),
        });
        p.vars = vec![
            VarOp {
                name: "$initialized".to_string(),
                kind: VarKind::Write,
                span: 0..13,
                is_temporary: false,
            },
        ];
        p
    }

    #[test]
    fn story_init_seeds_initialized_variables() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        // StoryInit sets $initialized
        doc.passages.push(make_story_init_passage());
        // Start reads $initialized — should NOT be flagged because StoryInit seeds it
        doc.passages.push(make_passage_with_vars("Start", &[], &["$initialized"]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let uninit: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UninitializedVariable)
            .collect();

        assert!(
            uninit.is_empty(),
            "Variable initialized by StoryInit should not be flagged as uninitialized"
        );
    }

    #[test]
    fn branching_path_must_analysis() {
        // Start writes $gold, then branches to Forest and Cave.
        // Forest reads $gold (initialized), Cave does NOT read $gold.
        // This tests that must-analysis correctly handles branching.
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        let mut start = make_passage_with_vars("Start", &["$gold"], &[]);
        start.links.push(Link { display_text: None, target: "Forest".to_string(), span: 0..6 });
        start.links.push(Link { display_text: None, target: "Cave".to_string(), span: 0..4 });
        doc.passages.push(start);
        doc.passages.push(make_passage_with_vars("Forest", &[], &["$gold"]));
        doc.passages.push(make_passage_with_vars("Cave", &[], &[]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let uninit: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UninitializedVariable && d.passage_name == "Forest")
            .collect();

        assert!(
            uninit.is_empty(),
            "$gold is initialized in Start before Forest reads it"
        );
    }

    #[test]
    fn join_point_intersection() {
        // Start branches to PathA and PathB.
        // PathA writes $sword, PathB writes $shield.
        // Both converge on Boss which reads both $sword and $shield.
        // At the join point, neither is definitely initialized (must-analysis = intersection).
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        let mut start = make_passage_with_vars("Start", &[], &[]);
        start.links.push(Link { display_text: None, target: "PathA".to_string(), span: 0..5 });
        start.links.push(Link { display_text: None, target: "PathB".to_string(), span: 0..5 });
        doc.passages.push(start);
        let mut path_a = make_passage_with_vars("PathA", &["$sword"], &[]);
        path_a.links.push(Link { display_text: None, target: "Boss".to_string(), span: 0..4 });
        doc.passages.push(path_a);
        let mut path_b = make_passage_with_vars("PathB", &["$shield"], &[]);
        path_b.links.push(Link { display_text: None, target: "Boss".to_string(), span: 0..4 });
        doc.passages.push(path_b);
        doc.passages.push(make_passage_with_vars(
            "Boss",
            &[],
            &["$sword", "$shield"],
        ));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let uninit: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UninitializedVariable && d.passage_name == "Boss")
            .collect();

        assert_eq!(
            uninit.len(),
            2,
            "At join point, neither $sword nor $shield is definitely initialized (must-analysis)"
        );
    }

    #[test]
    fn unused_variable_detection() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        // $unused is written but never read anywhere
        doc.passages.push(make_passage_with_vars("Start", &["$unused"], &["$used"]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let unused: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UnusedVariable)
            .collect();

        assert_eq!(unused.len(), 1, "Should detect exactly 1 unused variable");
        assert!(unused[0].message.contains("$unused"));
    }

    #[test]
    fn redundant_write_detection() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );

        // Create a passage with two writes to $gold without an intervening read
        let mut p = Passage::new("Start".to_string(), 0..100);
        p.vars = vec![
            VarOp {
                name: "$gold".to_string(),
                kind: VarKind::Write,
                span: 0..5,
                is_temporary: false,
            },
            VarOp {
                name: "$gold".to_string(),
                kind: VarKind::Write,
                span: 10..15,
                is_temporary: false,
            },
        ];
        doc.passages.push(p);
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let redundant: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::RedundantWrite)
            .collect();

        assert_eq!(redundant.len(), 1, "Should detect exactly 1 redundant write");
        assert!(redundant[0].message.contains("$gold"));
    }

    #[test]
    fn write_then_read_not_redundant() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );

        // Write, then read, then write again — the second write is NOT redundant
        // because there was an intervening read.
        let mut p = Passage::new("Start".to_string(), 0..100);
        p.vars = vec![
            VarOp {
                name: "$gold".to_string(),
                kind: VarKind::Write,
                span: 0..5,
                is_temporary: false,
            },
            VarOp {
                name: "$gold".to_string(),
                kind: VarKind::Read,
                span: 5..10,
                is_temporary: false,
            },
            VarOp {
                name: "$gold".to_string(),
                kind: VarKind::Write,
                span: 10..15,
                is_temporary: false,
            },
        ];
        doc.passages.push(p);
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let redundant: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::RedundantWrite)
            .collect();

        assert!(
            redundant.is_empty(),
            "Write → Read → Write should NOT be flagged as redundant"
        );
    }

    #[test]
    fn temporary_vars_excluded_from_cross_passage_flow() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        // Start writes _temp (temporary) and reads _temp in Forest
        // Temporary vars should NOT cause cross-passage uninitialized warnings
        let mut start = make_passage_with_temp_vars(
            "Start",
            &["$gold"],       // persistent write
            &[],
            &["_temp"],        // temp write
            &[],
        );
        start.links.push(Link { display_text: None, target: "Forest".to_string(), span: 0..6 });
        doc.passages.push(start);
        doc.passages.push(make_passage_with_temp_vars(
            "Forest",
            &[],
            &["$gold"],        // persistent read — should be fine
            &[],
            &["_temp"],        // temp read — should NOT be cross-passage analyzed
        ));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        // $gold should NOT be flagged as uninitialized (written in Start, read in Forest)
        let uninit_gold: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UninitializedVariable && d.message.contains("$gold"))
            .collect();
        assert!(uninit_gold.is_empty(), "$gold is initialized in Start");

        // _temp should NOT appear in cross-passage diagnostics at all
        // (temporary vars are excluded from cross-passage flow analysis)
        let uninit_temp: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UninitializedVariable && d.message.contains("_temp"))
            .collect();
        assert!(
            uninit_temp.is_empty(),
            "Temporary variables should not appear in cross-passage flow diagnostics"
        );
    }

    #[test]
    fn harlowe_partial_variable_tracking() {
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::Harlowe,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::Harlowe,
        );
        doc.passages.push(make_passage_with_vars("Start", &["$gold"], &["$gold"]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        // Harlowe has partial variable tracking — should still produce analysis
        let uninit: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UninitializedVariable)
            .collect();
        assert!(uninit.is_empty(), "Written variable should not be flagged");
    }

    #[test]
    fn cross_passage_variable_flow() {
        // Start writes $gold, Forest reads it — should not be uninitialized
        let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
        workspace.metadata = Some(StoryMetadata {
            format: StoryFormat::SugarCube,
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        });

        let mut doc = Document::new(
            Url::parse("file:///project/story.tw").unwrap(),
            StoryFormat::SugarCube,
        );
        let mut start = make_passage_with_vars("Start", &["$gold"], &[]);
        start.links.push(Link { display_text: None, target: "Forest".to_string(), span: 0..6 });
        doc.passages.push(start);
        doc.passages.push(make_passage_with_vars("Forest", &[], &["$gold"]));
        doc.passages.push(make_story_data_passage());

        workspace.insert_document(doc);
        rebuild_workspace_graph(&mut workspace);

        let diagnostics = AnalysisEngine::analyze(&workspace);

        let uninit: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UninitializedVariable && d.passage_name == "Forest")
            .collect();

        assert!(
            uninit.is_empty(),
            "$gold is initialized in Start and should be available in Forest"
        );
    }
}
