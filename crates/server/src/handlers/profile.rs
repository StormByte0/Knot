//! Custom LSP request handler for workspace profiling (knot/profile).

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;
use knot_core::Block;
use std::collections::HashMap;

impl ServerState {
    /// `knot/profile` — return workspace profiling statistics.
    pub async fn knot_profile(
        &self,
        params: KnotProfileParams,
    ) -> Result<KnotProfileResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/profile: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let workspace = &inner.workspace;

        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        // Basic counts
        let document_count = workspace.document_count() as u32;
        let passage_count = workspace.passage_count() as u32;

        let mut special_passage_count: u32 = 0;
        let mut metadata_passage_count: u32 = 0;
        let mut total_word_count: u32 = 0;
        let mut dead_end_count: u32 = 0;
        let mut unreachable_orphan_count: u32 = 0;

        let mut all_variables: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut var_writes_for_loops: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        // Per-passage word counts for complexity metrics
        let mut passage_word_counts: Vec<u32> = Vec::new();
        let mut passage_out_links: Vec<u32> = Vec::new();
        // Per-tag statistics
        let mut tag_data: HashMap<String, (u32, u32, u32)> = HashMap::new(); // tag → (count, total_words, total_out_links)

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    metadata_passage_count += 1;
                }
                if passage.is_special {
                    special_passage_count += 1;
                }

                // Word count (approximate from text blocks)
                let passage_words: u32 = passage
                    .body
                    .iter()
                    .map(|block| match block {
                        knot_core::passage::Block::Text { content, .. } => content.split_whitespace().count() as u32,
                        _ => 0,
                    })
                    .sum();
                total_word_count += passage_words;

                if !passage.is_metadata() {
                    passage_word_counts.push(passage_words);

                    let out_count = workspace.graph.outgoing_neighbors(&passage.name).len() as u32;
                    passage_out_links.push(out_count);

                    // Dead-end detection
                    if !passage.is_special {
                        let has_outgoing = !passage.links.is_empty() || out_count > 0;
                        if !has_outgoing {
                            dead_end_count += 1;
                        }
                    }

                    // Unreachable-orphan detection (0 incoming links, not special)
                    // These are a subset of unreachable passages — the most
                    // severe case where nothing links to the passage at all.
                    let in_count = workspace.graph.incoming_neighbors(&passage.name).len();
                    if in_count == 0 && !passage.is_special {
                        unreachable_orphan_count += 1;
                    }
                }

                // Per-tag statistics
                for tag in &passage.tags {
                    let entry = tag_data.entry(tag.clone()).or_insert((0, 0, 0));
                    entry.0 += 1;
                    entry.1 += passage_words;
                    entry.2 += workspace.graph.outgoing_neighbors(&passage.name).len() as u32;
                }

                // Variable collection
                for var in &passage.vars {
                    if !var.is_temporary {
                        all_variables.insert(var.name.clone());
                    }
                }

                // Collect var writes for game loop detection
                let writes: Vec<String> = passage
                    .persistent_variable_inits()
                    .map(|v| v.name.clone())
                    .collect();
                if !writes.is_empty() {
                    var_writes_for_loops.insert(passage.name.clone(), writes);
                }
            }
        }

        // Build tag stats
        let tag_stats: Vec<KnotTagStat> = tag_data
            .into_iter()
            .map(|(tag, (count, total_words, total_out))| KnotTagStat {
                tag,
                passage_count: count,
                avg_word_count: if count > 0 { total_words as f64 / count as f64 } else { 0.0 },
                total_word_count: total_words,
                avg_out_links: if count > 0 { total_out as f64 / count as f64 } else { 0.0 },
            })
            .collect();

        // Complexity metrics
        let avg_word_count = if !passage_word_counts.is_empty() {
            passage_word_counts.iter().sum::<u32>() as f64 / passage_word_counts.len() as f64
        } else {
            0.0
        };

        let mut sorted_words = passage_word_counts.clone();
        sorted_words.sort();
        let median_word_count = if !sorted_words.is_empty() {
            let mid = sorted_words.len() / 2;
            if sorted_words.len().is_multiple_of(2) {
                (sorted_words[mid - 1] + sorted_words[mid]) as f64 / 2.0
            } else {
                sorted_words[mid] as f64
            }
        } else {
            0.0
        };

        let max_word_count = passage_word_counts.iter().max().copied().unwrap_or(0);
        let min_word_count = passage_word_counts.iter().filter(|&&w| w > 0).min().copied().unwrap_or(0);

        let avg_out_links = if !passage_out_links.is_empty() {
            let sum: u32 = passage_out_links.iter().sum();
            sum as f64 / passage_out_links.len() as f64
        } else {
            0.0
        };

        let out_links_stddev = if passage_out_links.len() > 1 {
            let mean = avg_out_links;
            let variance: f64 = passage_out_links
                .iter()
                .map(|&x| (x as f64 - mean).powi(2))
                .sum::<f64>()
                / passage_out_links.len().saturating_sub(1) as f64;
            variance.sqrt()
        } else {
            0.0
        };

        let complex_passage_count = passage_out_links.iter().filter(|&&x| x > 6).count() as u32;

        // Graph analysis (use format-delegated variable diagnostics)
        let diagnostics = helpers::analyze_with_format_vars(workspace, &inner.format_registry);
        let unreachable_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::UnreachablePassage)).count() as u32;
        let broken_link_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::BrokenLink)).count() as u32;
        // Compute game loop count from the graph
        let game_loop_count = workspace.graph.game_loop_count(&var_writes_for_loops) as u32;
        let variable_issue_count: u32 = diagnostics.iter().filter(|d| matches!(
            d.kind,
            knot_core::graph::DiagnosticKind::UninitializedVariable
            | knot_core::graph::DiagnosticKind::UnusedVariable
            | knot_core::graph::DiagnosticKind::RedundantWrite
            | knot_core::graph::DiagnosticKind::VariableAvailabilityHint
            | knot_core::graph::DiagnosticKind::UnusedVariableHint
            | knot_core::graph::DiagnosticKind::RedundantWriteHint
            | knot_core::graph::DiagnosticKind::UnknownPropertyHint
        )).count() as u32;

        let total_links = workspace.graph.edge_count() as u32;
        let graph_passage_count = workspace.graph.passage_count() as u32;

        let avg_out_degree = if graph_passage_count > 0 {
            total_links as f64 / graph_passage_count as f64
        } else {
            0.0
        };

        let avg_in_degree = avg_out_degree;

        // Compute max depth from start passage using BFS
        let max_depth = helpers::compute_max_depth(workspace, start_passage);

        // Link distribution
        let mut zero_links: u32 = 0;
        let mut few_links: u32 = 0;
        let mut moderate_links: u32 = 0;
        let mut many_links: u32 = 0;

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }
                let out_count = workspace.graph.outgoing_neighbors(&passage.name).len() as u32;
                match out_count {
                    0 => zero_links += 1,
                    1..=2 => few_links += 1,
                    3..=5 => moderate_links += 1,
                    _ => many_links += 1,
                }
            }
        }

        let format = workspace.resolve_format();
        let format_version = workspace.metadata.as_ref().and_then(|m| m.format_version.clone());
        let has_story_data = workspace.metadata.is_some();
        let ifid = workspace.metadata.as_ref().and_then(|m| m.ifid.clone());

        // Extract story name from the StoryTitle passage body.
        // In Twee, the story name is the body text of the special "StoryTitle" passage.
        let story_name = workspace.documents().find_map(|doc| {
            doc.story_title().and_then(|p| {
                // The body is typically a single Text block containing the name.
                p.body.first().and_then(|block| match block {
                    Block::Text { content, .. } => {
                        let trimmed = content.trim();
                        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
                    }
                    _ => None,
                })
            })
        });

        // Structural balance analysis
        let non_meta_count = if passage_count > metadata_passage_count {
            passage_count - metadata_passage_count
        } else {
            1
        };

        let dead_end_ratio = dead_end_count as f64 / non_meta_count as f64;
        let unreachable_ratio = unreachable_orphan_count as f64 / non_meta_count as f64;

        // Compute connected components using graph
        let connected_components = helpers::compute_connected_components(workspace);
        let is_well_connected = connected_components <= 1;

        // Compute diameter (longest shortest path from start)
        let diameter = max_depth; // Simplified — uses max depth as approximation

        // Average clustering coefficient
        let avg_clustering = helpers::compute_avg_clustering(workspace);

        Ok(KnotProfileResponse {
            document_count,
            passage_count,
            special_passage_count,
            metadata_passage_count,
            unreachable_passage_count: unreachable_count,
            broken_link_count,
            game_loop_count,
            total_links,
            avg_out_degree,
            avg_in_degree,
            max_depth,
            dead_end_count,
            variable_count: all_variables.len() as u32,
            variable_issue_count,
            story_name,
            format: format.to_string(),
            format_version,
            ifid,
            has_story_data,
            total_word_count,
            link_distribution: KnotLinkDistribution {
                zero_links,
                few_links,
                moderate_links,
                many_links,
            },
            tag_stats,
            complexity_metrics: KnotComplexityMetrics {
                avg_word_count,
                median_word_count,
                max_word_count,
                min_word_count,
                avg_out_links,
                out_links_stddev,
                complex_passage_count,
            },
            structural_balance: KnotStructuralBalance {
                dead_end_ratio,
                unreachable_ratio,
                is_well_connected,
                connected_components,
                diameter,
                avg_clustering,
            },
        })
    }
}
