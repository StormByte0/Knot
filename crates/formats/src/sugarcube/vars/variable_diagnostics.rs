//! Graph-BFS variable availability analysis and diagnostics for SugarCube.

use std::collections::{HashMap, HashSet};

use knot_core::passage::VarKind;

use crate::types::{StateVariable, VarAccessKind, VariableDiagnostic, VariableDiagnosticKind};

/// Compute variable-related diagnostics using graph-BFS availability analysis.
///
/// This is the SugarCube-specific replacement for the core's
/// `detect_uninitialized_reads()`, `detect_unused_variables()`, and
/// `detect_redundant_writes()`. The key insight is that SugarCube variables
/// are persistent `State.variables` entries — they are NOT traditional
/// scoped variables that need "definite assignment analysis".
///
/// ## Algorithm
///
/// 1. **Availability computation**: For each variable, find all passages that
///    write it. BFS forward from each write passage through the graph. Any
///    passage reachable from a write passage "has access" to that variable.
///    Variables seeded by special passages (StoryInit) and script-tagged
///    passages are considered available everywhere.
///
/// 2. **Diagnostics**: If a variable is read in a passage that is NOT reachable
///    from any write passage (and not seeded by special), emit a
///    `VariableAvailabilityHint`. This is a HINT, not an error, because the
///    variable might exist from a saved game or an unmodeled JS script.
///
/// 3. **Unused variables**: If a variable is written but never read in any
///    reachable passage, emit an `UnusedVariableHint`.
///
/// 4. **Redundant writes**: If a variable is written twice in the same passage
///    without an intervening read, emit a `RedundantWriteHint`.
///
/// 5. **Unknown properties**: If a property is read but never written anywhere,
///    emit an `UnknownPropertyHint`.
pub(crate) fn compute_variable_diagnostics(
    workspace: &knot_core::Workspace,
    start_passage: &str,
    registry: &HashMap<String, StateVariable>,
) -> Vec<VariableDiagnostic> {
    let mut diagnostics = Vec::new();

    // Collect the set of passages reachable from the start passage
    // (this is used to filter out diagnostics for unreachable passages,
    // which are already flagged by the core's unreachable passage detection)
    let reachable_from_start = bfs_reachable(workspace, start_passage);

    for (dollar_name, var) in registry {
        // Skip variables that are seeded by special passages (StoryInit, etc.)
        // They are always available from the start of the game.
        if var.seeded_by_special {
            continue;
        }

        // ── Variable availability hints ──────────────────────────────────
        // For each read location, check if the reading passage is reachable
        // from any write location via the narrative graph.
        if !var.write_locations.is_empty() {
            // Compute the set of passages that can "see" this variable
            // by BFS-ing forward from each write passage
            let mut available_passages: HashSet<String> = HashSet::new();
            for write_loc in &var.write_locations {
                available_passages.insert(write_loc.passage_name.clone());
                // BFS forward from this write passage
                let forward = bfs_forward(workspace, &write_loc.passage_name);
                for p in forward {
                    available_passages.insert(p);
                }
            }

            // Also make available from start passage if any write is in
            // a passage that precedes start (e.g., StoryInit)
            for write_loc in &var.write_locations {
                if is_pre_start_passage(workspace, &write_loc.passage_name) {
                    available_passages.insert(start_passage.to_string());
                    let forward = bfs_forward(workspace, start_passage);
                    for p in forward {
                        available_passages.insert(p);
                    }
                    break;
                }
            }

            // Check each read location for availability
            for read_loc in &var.read_locations {
                if !available_passages.contains(&read_loc.passage_name) {
                    // Only flag if the reading passage is itself reachable from start
                    // (unreachable passages are flagged separately by the core)
                    if reachable_from_start.contains(&read_loc.passage_name) {
                        diagnostics.push(VariableDiagnostic {
                            passage_name: read_loc.passage_name.clone(),
                            file_uri: read_loc.file_uri.clone(),
                            kind: VariableDiagnosticKind::VariableAvailabilityHint,
                            message: format!(
                                "Variable '{}' may not be available in passage '{}' \
                                 (no write in a preceding passage is reachable via narrative flow). \
                                 This is a hint — the variable may exist from a saved game.",
                                dollar_name, read_loc.passage_name
                            ),
                        });
                    }
                }
            }
        } else {
            // Variable has reads but NO writes anywhere — flag all reads
            for read_loc in &var.read_locations {
                if reachable_from_start.contains(&read_loc.passage_name) {
                    diagnostics.push(VariableDiagnostic {
                        passage_name: read_loc.passage_name.clone(),
                        file_uri: read_loc.file_uri.clone(),
                        kind: VariableDiagnosticKind::VariableAvailabilityHint,
                        message: format!(
                            "Variable '{}' is read but never written in any passage. \
                             It may come from a saved game or external script.",
                            dollar_name
                        ),
                    });
                }
            }
        }

        // ── Unused variable hints ─────────────────────────────────────────
        if !var.write_locations.is_empty() && var.read_locations.is_empty() {
            // Variable is written but never read
            if let Some(first_write) = var.write_locations.first() {
                if reachable_from_start.contains(&first_write.passage_name) {
                    diagnostics.push(VariableDiagnostic {
                        passage_name: first_write.passage_name.clone(),
                        file_uri: first_write.file_uri.clone(),
                        kind: VariableDiagnosticKind::UnusedVariableHint,
                        message: format!(
                            "Variable '{}' is written but never read in any reachable passage",
                            dollar_name
                        ),
                    });
                }
            }
        }

        // ── Unknown property hints ────────────────────────────────────────
        // Check if any property reads don't have corresponding property writes
        {
            let mut written_properties: HashSet<String> = HashSet::new();
            let mut read_properties: HashSet<(String, String)> = HashSet::new(); // (property_path, passage_name)

            for loc in &var.write_locations {
                if let VarAccessKind::PropertyWrite { path } = &loc.kind {
                    written_properties.insert(path.clone());
                }
            }
            // Base-level assigns also make all properties potentially available
            // (e.g., <<set $player to {name: "Alice"}>> makes $player.name available)
            let has_base_assign = var.write_locations.iter().any(|loc| {
                        matches!(&loc.kind, VarAccessKind::Assign)
                    });

            for loc in &var.read_locations {
                if let VarAccessKind::PropertyRead { path } = &loc.kind {
                    if !written_properties.contains(path) && !has_base_assign {
                        read_properties.insert((path.clone(), loc.passage_name.clone()));
                    }
                }
            }

            for (path, passage_name) in &read_properties {
                if reachable_from_start.contains(passage_name) {
                    diagnostics.push(VariableDiagnostic {
                        passage_name: passage_name.clone(),
                        file_uri: var.write_locations.first()
                            .or_else(|| var.read_locations.first())
                            .map(|l| l.file_uri.clone())
                            .unwrap_or_default(),
                        kind: VariableDiagnosticKind::UnknownPropertyHint,
                        message: format!(
                            "Property '{}.{}' is read but never written. \
                             The property may be set via base-level assignment \
                             (e.g., <<set {} to {{...}}>>)",
                            dollar_name, path, dollar_name
                        ),
                    });
                }
            }
        }
    }

    // ── Redundant write hints (intra-passage) ─────────────────────────────
    diagnostics.extend(compute_redundant_write_hints(workspace));

    diagnostics
}

/// Compute redundant write hints: a variable written twice in the same
/// passage without an intervening read.
fn compute_redundant_write_hints(
    workspace: &knot_core::Workspace,
) -> Vec<VariableDiagnostic> {
    let mut diagnostics = Vec::new();

    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.is_metadata() {
                continue;
            }

            let mut written_not_read: HashSet<String> = HashSet::new();
            let mut reported: HashSet<String> = HashSet::new();

            let sorted_vars = passage.vars_sorted_by_span();
            for var in sorted_vars {
                if var.is_temporary {
                    continue;
                }

                match var.kind {
                    VarKind::Init => {
                        if written_not_read.contains(&var.name) && !reported.contains(&var.name) {
                            diagnostics.push(VariableDiagnostic {
                                passage_name: passage.name.clone(),
                                file_uri: doc.uri.to_string(),
                                kind: VariableDiagnosticKind::RedundantWriteHint,
                                message: format!(
                                    "Variable '{}' is assigned again without being read \
                                     since the last assignment in passage '{}'",
                                    var.name, passage.name
                                ),
                            });
                            reported.insert(var.name.clone());
                        }
                        written_not_read.insert(var.name.clone());
                    }
                    VarKind::Read => {
                        written_not_read.remove(&var.name);
                        reported.remove(&var.name);
                    }
                }
            }
        }
    }

    diagnostics
}

/// BFS forward from a passage through the narrative graph.
/// Returns the set of passage names reachable via outgoing edges.
fn bfs_forward(workspace: &knot_core::Workspace, start: &str) -> HashSet<String> {
    let mut visited = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    queue.push_back(start.to_string());

    while let Some(current) = queue.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());

        for neighbor in workspace.graph.outgoing_neighbors(&current) {
            if !visited.contains(&neighbor) {
                queue.push_back(neighbor);
            }
        }
    }

    visited
}

/// BFS from the start passage to determine all reachable passages.
fn bfs_reachable(workspace: &knot_core::Workspace, start: &str) -> HashSet<String> {
    let mut visited = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    queue.push_back(start.to_string());

    while let Some(current) = queue.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());

        for neighbor in workspace.graph.outgoing_neighbors(&current) {
            if !visited.contains(&neighbor) {
                queue.push_back(neighbor);
            }
        }
    }

    visited
}

/// Check if a passage runs before the start passage in the SugarCube lifecycle.
/// These passages (StoryInit, script-tagged passages) contribute variables that are
/// available from the very beginning of the game.
fn is_pre_start_passage(workspace: &knot_core::Workspace, passage_name: &str) -> bool {
    // Find the passage in the workspace
    for doc in workspace.documents() {
        if let Some(passage) = doc.passages.iter().find(|p| p.name == passage_name) {
            if passage.is_special {
                if let Some(ref def) = passage.special_def {
                    // Startup passages (StoryInit) and script-injection passages
                    // both run before the start passage. Script-injection passages
                    // (tagged [script] or legacy-named "script") are compiled into
                    // <script> elements that execute at story load time.
                    return matches!(
                        def.behavior,
                        knot_core::passage::SpecialPassageBehavior::Startup
                            | knot_core::passage::SpecialPassageBehavior::ScriptInjection
                    );
                }
            }
            // Fallback: tag-based detection for unclassified script passages.
            // This path is reached when special_def is not set (shouldn't happen
            // in normal parse flow, but defensive).
            if passage.is_script_passage() {
                return true;
            }
            return false;
        }
    }
    false
}
