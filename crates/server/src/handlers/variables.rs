//! Custom LSP request handlers for variable analysis (knot/variableFlow, knot/watchVariables)
//! and passage variable reference extraction from format plugins.

use crate::lsp_ext::*;
use crate::state::{DocumentCache, ServerState};
use knot_core::AnalysisEngine;
use knot_core::Workspace;
use knot_formats::plugin as fmt_plugin;
use std::collections::HashMap;
use url::Url;

/// Convert a single format-agnostic `VariablePropertyNode` to LSP wire type
/// `KnotVariableProperty`. This is a pure mechanical translation with no
/// format-specific logic.
fn convert_property_node(p: knot_formats::types::VariablePropertyNode) -> KnotVariableProperty {
    let kind_str = match p.kind {
        knot_formats::types::PropertyKind::Scalar => "scalar",
        knot_formats::types::PropertyKind::Object => "object",
        knot_formats::types::PropertyKind::Array => "array",
        knot_formats::types::PropertyKind::Unknown => "unknown",
    };
    let element_shape = p.element_shape.map(|shape| {
        Box::new(convert_property_node(*shape))
    });
    KnotVariableProperty {
        name: p.name,
        full_name: p.full_name,
        state_path: p.state_path,
        written_in: p.written_in.into_iter().map(|l| KnotVariableLocation {
            passage_name: l.passage_name,
            file_uri: l.file_uri,
            is_write: l.is_write,
            line: l.line,
            span: l.span.map(|s| (s.start as u32, s.end as u32)),
        }).collect(),
        read_in: p.read_in.into_iter().map(|l| KnotVariableLocation {
            passage_name: l.passage_name,
            file_uri: l.file_uri,
            is_write: l.is_write,
            line: l.line,
            span: l.span.map(|s| (s.start as u32, s.end as u32)),
        }).collect(),
        properties: convert_properties(p.properties),
        kind: kind_str.to_string(),
        element_shape,
        coverage: p.coverage.map(|c| c.to_string()),
    }
}

/// Recursively convert format-agnostic `VariablePropertyNode` instances
/// to LSP wire type `KnotVariableProperty`. This is a pure mechanical
/// translation with no format-specific logic.
fn convert_properties(
    props: Vec<knot_formats::types::VariablePropertyNode>,
) -> Vec<KnotVariableProperty> {
    props.into_iter().map(convert_property_node).collect()
}

// ===========================================================================
// Passage variable reference extraction (format plugin → passage diagnostics)
// ===========================================================================

/// Build variable references for a specific passage using the format plugin's
/// variable extraction.
///
/// This is the wiring between the format plugin system and passage diagnostics.
/// It:
/// 1. Delegates to the format plugin's `extract_passage_variable_refs()` method
/// 2. The format plugin parses the passage, extracts variable accesses,
///    and filters for the requested passage
/// 3. The returned `PassageVarRef` entries carry line numbers mapped
///    back to the original source file
///
/// The line numbers come from the format plugin's position mapping,
/// which maps extracted positions back to the original source file.
/// This is what enables showing exact read/write lines in the passage
/// diagnostics panel.
pub(crate) fn build_passage_variable_references(
    workspace: &Workspace,
    format_registry: &fmt_plugin::FormatRegistry,
    open_documents: &HashMap<Url, String>,
    passage_name: &str,
) -> Vec<KnotVariableReference> {
    let format = workspace.resolve_format();
    let Some(plugin) = format_registry.get(&format) else {
        return Vec::new();
    };

    // Use the format plugin's extraction (format-isolation-compliant)
    let source_text = DocumentCache(open_documents);
    let var_refs = plugin.extract_passage_variable_refs(workspace, &source_text, passage_name);

    // Pure mechanical translation: format-agnostic PassageVarRef → LSP wire type
    let mut references: Vec<KnotVariableReference> = var_refs
        .into_iter()
        .map(|r| KnotVariableReference {
            variable_name: r.variable_name,
            is_write: r.is_write,
            line: r.line,
            file_uri: r.file_uri,
            passage_name: r.passage_name,
            span_start: r.span.as_ref().map(|s| s.start as u32),
            span_end: r.span.as_ref().map(|s| s.end as u32),
        })
        .collect();

    // Sort by line number for display, then by variable name
    references.sort_by(|a, b| {
        a.line.cmp(&b.line)
            .then_with(|| a.variable_name.cmp(&b.variable_name))
    });

    references
}

/// Build temporary-variable summaries for a specific passage using the
/// format plugin's temp-variable extraction.
///
/// Mirrors [`build_passage_variable_references`] but walks the format
/// plugin's per-passage temp root instead of the persistent root.
/// Returns one [`KnotTemporaryVariable`] per distinct `_var` declared
/// in the passage, with aggregated read/write counts and line-level
/// references for navigation.
///
/// Formats without passage-scoped temporary variables (Harlowe,
/// Snowman, Chapbook) inherit the default empty implementation from
/// `FormatPlugin::extract_passage_temp_variables` and this returns
/// an empty Vec — the diagnostics panel will simply hide the section.
pub(crate) fn build_passage_temporary_variables(
    workspace: &Workspace,
    format_registry: &fmt_plugin::FormatRegistry,
    open_documents: &HashMap<Url, String>,
    passage_name: &str,
) -> Vec<KnotTemporaryVariable> {
    let format = workspace.resolve_format();
    let Some(plugin) = format_registry.get(&format) else {
        return Vec::new();
    };

    let source_text = DocumentCache(open_documents);
    let summaries = plugin.extract_passage_temp_variables(workspace, &source_text, passage_name);

    // Pure mechanical translation: format-agnostic PassageTempVarSummary
    // → LSP wire type. No format-specific logic lives here.
    summaries
        .into_iter()
        .map(|s| KnotTemporaryVariable {
            name: s.name,
            write_count: s.write_count,
            read_count: s.read_count,
            references: s.refs.into_iter().map(|r| KnotVariableReference {
                variable_name: r.variable_name,
                is_write: r.is_write,
                line: r.line,
                file_uri: r.file_uri,
                passage_name: r.passage_name,
                span_start: r.span.as_ref().map(|s| s.start as u32),
                span_end: r.span.as_ref().map(|s| s.end as u32),
            }).collect(),
        })
        .collect()
}

impl ServerState {
    /// `knot/variableFlow` — export variable dataflow information.
    ///
    /// Delegates to the format plugin's `build_variable_tree()` method, which
    /// produces format-agnostic `VariableTreeNode` instances. The server then
    /// performs a **pure mechanical translation** to LSP wire types — no
    /// format-specific logic (no `VarAccessKind` matching, no hardcoded
    /// `"State.variables"` strings) lives here.
    pub async fn knot_variable_flow(
        &self,
        params: KnotVariableFlowParams,
    ) -> Result<KnotVariableFlowResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/variableFlow: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let workspace = &inner.workspace;
        let format = workspace.resolve_format();

        // Only provide variable flow for formats that support it
        let plugin = inner.format_registry.get(&format);
        let supports_tracking = plugin.as_ref().map_or(false, |p| {
            p.supports_full_variable_tracking() || p.supports_partial_variable_tracking()
        });
        if !supports_tracking {
            return Ok(KnotVariableFlowResponse {
                variables: Vec::new(),
            });
        }

        // Delegate tree construction to the format plugin.
        // The plugin returns format-agnostic VariableTreeNode instances.
        // The server only does a mechanical translation to LSP wire types.
        let tree_nodes = if let Some(p) = plugin {
            let source_text = crate::state::DocumentCache(&inner.open_documents);
            p.build_variable_tree(workspace, &source_text)
        } else {
            Vec::new()
        };

        // Pure mechanical translation: format-agnostic tree → LSP wire types.
        // No VarAccessKind matching, no "State.variables" hardcoding.
        let variables: Vec<KnotVariableInfo> = tree_nodes
            .into_iter()
            .map(|node| {
                let kind_str = match node.kind {
                    knot_formats::types::PropertyKind::Scalar => "scalar",
                    knot_formats::types::PropertyKind::Object => "object",
                    knot_formats::types::PropertyKind::Array => "array",
                    knot_formats::types::PropertyKind::Unknown => "unknown",
                };
                KnotVariableInfo {
                    name: node.name,
                    state_path: node.state_path,
                    is_temporary: node.is_temporary,
                    written_in: node.written_in.into_iter().map(|l| KnotVariableLocation {
                        passage_name: l.passage_name,
                        file_uri: l.file_uri,
                        is_write: l.is_write,
                        line: l.line,
                        span: l.span.map(|s| (s.start as u32, s.end as u32)),
                    }).collect(),
                    read_in: node.read_in.into_iter().map(|l| KnotVariableLocation {
                        passage_name: l.passage_name,
                        file_uri: l.file_uri,
                        is_write: l.is_write,
                        line: l.line,
                        span: l.span.map(|s| (s.start as u32, s.end as u32)),
                    }).collect(),
                    initialized_at_start: node.initialized_at_start,
                    is_unused: node.is_unused,
                    properties: convert_properties(node.properties),
                    kind: kind_str.to_string(),
                    element_shape: node.element_shape.map(|shape| {
                        Box::new(convert_property_node(*shape))
                    }),
                }
            })
            .collect();

        Ok(KnotVariableFlowResponse {
            variables,
        })
    }

    /// `knot/watchVariables` — get variable state at a specific passage.
    pub async fn knot_watch_variables(
        &self,
        params: KnotWatchVariablesParams,
    ) -> Result<KnotWatchVariablesResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/watchVariables: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let workspace = &inner.workspace;
        let format = workspace.resolve_format();

        // Only provide variable watch for formats that support it
        let plugin = inner.format_registry.get(&format);
        let supports_tracking = plugin.as_ref().map_or(false, |p| {
            p.supports_full_variable_tracking() || p.supports_partial_variable_tracking()
        });
        if !supports_tracking {
            return Ok(KnotWatchVariablesResponse {
                at_passage: params.at_passage,
                initialized_at_entry: Vec::new(),
                written_in_passage: Vec::new(),
                read_in_passage: Vec::new(),
                potentially_uninitialized: Vec::new(),
            });
        }

        // Run dataflow analysis
        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        let passage_data = AnalysisEngine::collect_passage_data(workspace);
        let seed_init = AnalysisEngine::collect_special_passage_initializers(workspace, &passage_data);
        let flow_states = AnalysisEngine::run_dataflow_from_engine(workspace, start_passage, &passage_data, &seed_init);

        // Get passage info
        let (doc_uri, passage) = match workspace.find_passage(&params.at_passage) {
            Some((doc, p)) => (doc.uri.to_string(), p),
            None => {
                return Ok(KnotWatchVariablesResponse {
                    at_passage: params.at_passage,
                    initialized_at_entry: Vec::new(),
                    written_in_passage: Vec::new(),
                    read_in_passage: Vec::new(),
                    potentially_uninitialized: Vec::new(),
                });
            }
        };

        let entry_init = flow_states
            .get(&params.at_passage)
            .map(|s| &s.entry)
            .cloned()
            .unwrap_or_default();

        // Apply filter if specified
        let filter_set: Option<std::collections::HashSet<String>> = params
            .filter
            .map(|f| f.into_iter().collect());

        // Build initialized-at-entry list
        let initialized_at_entry: Vec<KnotWatchVariable> = entry_init
            .iter()
            .filter(|v| {
                filter_set.as_ref().is_none_or(|f| f.contains(*v))
            })
            .map(|v| KnotWatchVariable {
                name: v.clone(),
                is_temporary: false,
                file_uri: doc_uri.clone(),
                last_written_in: None, // Could be enhanced with backward tracing
            })
            .collect();

        // Build written-in-passage list
        let written_in_passage: Vec<KnotWatchVariable> = passage
            .persistent_variable_inits()
            .filter(|v| {
                filter_set.as_ref().is_none_or(|f| f.contains(&v.name))
            })
            .map(|v| KnotWatchVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
                file_uri: doc_uri.clone(),
                last_written_in: Some(params.at_passage.clone()),
            })
            .collect();

        // Build read-in-passage list
        let read_in_passage: Vec<KnotWatchVariable> = passage
            .persistent_variable_reads()
            .filter(|v| {
                filter_set.as_ref().is_none_or(|f| f.contains(&v.name))
            })
            .map(|v| KnotWatchVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
                file_uri: doc_uri.clone(),
                last_written_in: None,
            })
            .collect();

        // Build potentially-uninitialized list
        let mut local_init = entry_init;
        let mut potentially_uninitialized = Vec::new();

        for var in passage.vars_sorted_by_span() {
            if var.is_temporary { continue; }
            if filter_set.as_ref().is_none_or(|f| f.contains(&var.name)) {
                match var.kind {
                    knot_core::passage::VarKind::Read => {
                        if !local_init.contains(&var.name) {
                            potentially_uninitialized.push(KnotWatchVariable {
                                name: var.name.clone(),
                                is_temporary: false,
                                file_uri: doc_uri.clone(),
                                last_written_in: None,
                            });
                        }
                    }
                    knot_core::passage::VarKind::Init => {
                        local_init.insert(var.name.clone());
                    }
                }
            }
        }

        Ok(KnotWatchVariablesResponse {
            at_passage: params.at_passage,
            initialized_at_entry,
            written_in_passage,
            read_in_passage,
            potentially_uninitialized,
        })
    }
}
