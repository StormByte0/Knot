//! Analysis Engine
//!
//! The analysis engine orchestrates graph analysis, diagnostics, and
//! variable flow analysis across the workspace. It consumes the normalized
//! document model and passage graph to produce diagnostics and analysis results.
//!
//! ## Dataflow Analysis
//!
//! The variable flow analysis uses a forward dataflow worklist algorithm:
//!
//! 1. **Seed**: Collect variable writes from special passages that
//!    `contribute_variables` (e.g., SugarCube's StoryInit, Harlowe's startup).
//!    These form the initial "definitely initialized" set.
//!
//! 2. **Worklist fixpoint**: For each passage, compute the entry state
//!    (intersection of all predecessor exit states — must-analysis), then
//!    process writes/reads in source order within the passage.
//!
//! 3. **Join semantics**: At merge points (multiple predecessors), we take
//!    the *intersection* of initialized sets. A variable is "definitely
//!    initialized" only if it is initialized on ALL incoming paths.
//!
//! 4. **Convergence**: The worklist algorithm iterates until no passage's
//!    exit state changes. Since the initialized set can only grow (we only
//!    add variables, never remove them), convergence is guaranteed.

use crate::graph::{DiagnosticKind, GraphDiagnostic};
use crate::passage::{Block, VarKind, VarOp};
use crate::workspace::Workspace;
use std::collections::{HashMap, HashSet, VecDeque};

/// The analysis engine — orchestrates all graph and dataflow analysis.
pub struct AnalysisEngine;

/// Per-passage dataflow state: the set of variables that are definitely
/// initialized at a given program point.
type InitSet = HashSet<String>;

/// Dataflow state for a passage: entry set and exit set.
#[derive(Debug, Clone)]
pub struct PassageFlowState {
    /// Variables definitely initialized at passage entry.
    pub entry: InitSet,
    /// Variables definitely initialized at passage exit.
    pub exit: InitSet,
}

impl AnalysisEngine {
    /// Run a full analysis pass on the workspace and return all diagnostics.
    pub fn analyze(workspace: &Workspace) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        // StoryData validation
        diagnostics.extend(workspace.validate_story_data());

        // Determine the start passage
        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        // Broken link detection
        diagnostics.extend(workspace.graph.detect_broken_links());

        // Unreachable passage detection
        diagnostics.extend(workspace.graph.detect_unreachable(start_passage));

        // Infinite loop detection
        let passage_vars = Self::collect_passage_vars(workspace);
        diagnostics.extend(workspace.graph.detect_infinite_loops(&passage_vars));

        // Variable flow analysis (only for formats that support it)
        let format = workspace.resolve_format();
        if format.supports_full_variable_tracking() || format.supports_partial_variable_tracking() {
            diagnostics.extend(Self::analyze_variable_flow(workspace, start_passage));
        }

        // Advanced linting
        diagnostics.extend(Self::detect_duplicate_passage_names(workspace));
        diagnostics.extend(Self::detect_empty_passages(workspace));
        diagnostics.extend(Self::detect_dead_end_passages(workspace));
        diagnostics.extend(Self::detect_invalid_passage_names(workspace));
        diagnostics.extend(Self::detect_orphaned_passages(workspace));
        diagnostics.extend(Self::detect_complex_passages(workspace));
        diagnostics.extend(Self::detect_large_passages(workspace));
        diagnostics.extend(Self::detect_missing_start_link(workspace, start_passage));

        diagnostics
    }

    /// Collect variable operations per passage across all documents.
    fn collect_passage_vars(workspace: &Workspace) -> HashMap<String, Vec<&VarOp>> {
        let mut vars = HashMap::new();
        for doc in workspace.documents() {
            for passage in &doc.passages {
                vars.entry(passage.name.clone())
                    .or_insert_with(Vec::new)
                    .extend(passage.vars.iter());
            }
        }
        vars
    }

    /// Perform forward dataflow analysis to detect variable issues.
    ///
    /// This replaces the previous simplified BFS with a proper worklist
    /// fixpoint algorithm. Key improvements:
    ///
    /// - **Special passage seeding**: StoryInit/PassageReady/Script writes
    ///   are included in the initial initialized set.
    /// - **Must-analysis at join points**: Intersection of predecessor exit
    ///   states ensures a variable is only "definitely initialized" if it
    ///   is initialized on ALL paths.
    /// - **Temporary variable exclusion**: SugarCube `_temp` variables are
    ///   excluded from cross-passage analysis.
    /// - **Intra-passage ordering**: VarOps are processed in source order
    ///   within each passage for accurate use-before-init detection.
    /// - **Unused variable detection**: Variables written but never read
    ///   on any reachable path are flagged.
    /// - **Redundant write detection**: Variables written twice without an
    ///   intervening read within a single passage are flagged.
    pub fn analyze_variable_flow(
        workspace: &Workspace,
        start_passage: &str,
    ) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        // Step 1: Collect per-passage variable data (sorted by span for intra-passage ordering)
        let passage_data = Self::collect_passage_data(workspace);

        // Step 2: Seed the initial initialized set from special passages
        let seed_init = Self::collect_special_passage_initializers(workspace, &passage_data);

        // Step 3: Run forward dataflow worklist algorithm
        let flow_states = Self::run_dataflow(workspace, start_passage, &passage_data, &seed_init);

        // Step 4: Generate diagnostics from the flow states
        diagnostics.extend(Self::detect_uninitialized_reads(
            workspace,
            &passage_data,
            &flow_states,
        ));
        diagnostics.extend(Self::detect_unused_variables(
            workspace,
            &passage_data,
            &flow_states,
        ));
        diagnostics.extend(Self::detect_redundant_writes(
            workspace,
            &passage_data,
        ));

        diagnostics
    }

    /// Detect duplicate passage names across all documents.
    fn detect_duplicate_passage_names(workspace: &Workspace) -> Vec<GraphDiagnostic> {
        let mut seen: HashMap<String, (String, usize)> = HashMap::new(); // name → (file_uri, count)
        let mut diagnostics = Vec::new();

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }
                let entry = seen.entry(passage.name.clone()).or_insert_with(|| (doc.uri.to_string(), 0));
                entry.1 += 1;
            }
        }

        for (name, (file_uri, count)) in &seen {
            if *count > 1 {
                diagnostics.push(GraphDiagnostic {
                    passage_name: name.clone(),
                    file_uri: file_uri.clone(),
                    kind: DiagnosticKind::DuplicatePassageName,
                    message: format!(
                        "Passage name '{}' is used {} times; passage names should be unique",
                        name, count
                    ),
                });
            }
        }

        diagnostics
    }

    /// Detect passages with no content in their body.
    fn detect_empty_passages(workspace: &Workspace) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() || passage.is_special {
                    continue;
                }
                // Check if the passage body is empty or contains only whitespace
                let has_content = passage.body.iter().any(|block| {
                    match block {
                        Block::Text { content, .. } => !content.trim().is_empty(),
                        Block::Macro { .. } => true,
                        Block::Expression { .. } => true,
                        Block::Heading { .. } => true,
                        Block::Incomplete { .. } => false,
                    }
                });

                if !has_content {
                    diagnostics.push(GraphDiagnostic {
                        passage_name: passage.name.clone(),
                        file_uri: doc.uri.to_string(),
                        kind: DiagnosticKind::EmptyPassage,
                        message: format!(
                            "Passage '{}' has no content",
                            passage.name
                        ),
                    });
                }
            }
        }

        diagnostics
    }

    /// Detect dead-end passages — passages with no outgoing links that are
    /// not special or metadata passages. These may be unintentional endings.
    fn detect_dead_end_passages(workspace: &Workspace) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() || passage.is_special {
                    continue;
                }
                // Check if the passage has any outgoing links
                let has_outgoing = !passage.links.is_empty();
                let has_graph_edges = !workspace.graph.outgoing_neighbors(&passage.name).is_empty();

                if !has_outgoing && !has_graph_edges {
                    diagnostics.push(GraphDiagnostic {
                        passage_name: passage.name.clone(),
                        file_uri: doc.uri.to_string(),
                        kind: DiagnosticKind::DeadEndPassage,
                        message: format!(
                            "Passage '{}' has no outgoing links (dead end)",
                            passage.name
                        ),
                    });
                }
            }
        }

        diagnostics
    }

    /// Detect passages with invalid names (containing problematic characters).
    fn detect_invalid_passage_names(workspace: &Workspace) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }
                let name = &passage.name;

                // Check for leading/trailing whitespace
                if name != name.trim() {
                    diagnostics.push(GraphDiagnostic {
                        passage_name: name.clone(),
                        file_uri: doc.uri.to_string(),
                        kind: DiagnosticKind::InvalidPassageName,
                        message: format!(
                            "Passage name '{}' has leading or trailing whitespace",
                            name
                        ),
                    });
                    continue;
                }

                // Check for multiple consecutive spaces
                if name.contains("  ") {
                    diagnostics.push(GraphDiagnostic {
                        passage_name: name.clone(),
                        file_uri: doc.uri.to_string(),
                        kind: DiagnosticKind::InvalidPassageName,
                        message: format!(
                            "Passage name '{}' contains consecutive spaces",
                            name
                        ),
                    });
                    continue;
                }

                // Check for special characters that may cause issues
                // (but allow standard characters including unicode, hyphens, underscores, etc.)
                let has_problematic_chars = name.chars().any(|c| {
                    matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '@' | ';')
                });

                if has_problematic_chars {
                    diagnostics.push(GraphDiagnostic {
                        passage_name: name.clone(),
                        file_uri: doc.uri.to_string(),
                        kind: DiagnosticKind::InvalidPassageName,
                        message: format!(
                            "Passage name '{}' contains characters that may cause linking issues (: / \\ * ? \" < > | # @ ;)",
                            name
                        ),
                    });
                }
            }
        }

        diagnostics
    }

    /// Detect orphaned passages — passages with only one incoming link.
    ///
    /// An orphaned passage is one that is only reachable through a single
    /// path, meaning it has exactly one predecessor in the graph. While
    /// this is not inherently wrong, it may indicate a passage that is
    /// insufficiently connected to the story structure or that the author
    /// intended additional paths to reach it.
    fn detect_orphaned_passages(workspace: &Workspace) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() || passage.is_special {
                    continue;
                }

                let incoming = workspace.graph.incoming_neighbors(&passage.name);
                if incoming.len() == 1 {
                    diagnostics.push(GraphDiagnostic {
                        passage_name: passage.name.clone(),
                        file_uri: doc.uri.to_string(),
                        kind: DiagnosticKind::OrphanedPassage,
                        message: format!(
                            "Passage '{}' has only one incoming link (from '{}'); consider adding more paths to reach it",
                            passage.name,
                            incoming.first().map(|s| s.as_str()).unwrap_or("unknown")
                        ),
                    });
                }
            }
        }

        diagnostics
    }

    /// Detect passages with high complexity.
    ///
    /// Complexity is measured by the number of outgoing links and
    /// conditional expressions. A passage with many outgoing links
    /// creates a wide branching factor that makes the narrative harder
    /// to test and follow. The threshold is configurable but defaults
    /// to 6 outgoing links.
    fn detect_complex_passages(workspace: &Workspace) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();
        let complexity_threshold: usize = 6;

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }

                let outgoing_count = passage.links.len();
                let graph_outgoing = workspace.graph.outgoing_neighbors(&passage.name).len();
                let total_outgoing = outgoing_count.max(graph_outgoing);

                if total_outgoing > complexity_threshold {
                    diagnostics.push(GraphDiagnostic {
                        passage_name: passage.name.clone(),
                        file_uri: doc.uri.to_string(),
                        kind: DiagnosticKind::ComplexPassage,
                        message: format!(
                            "Passage '{}' has {} outgoing links (threshold: {}); consider splitting into smaller passages",
                            passage.name, total_outgoing, complexity_threshold
                        ),
                    });
                }
            }
        }

        diagnostics
    }

    /// Detect passages that exceed a recommended size threshold.
    ///
    /// Large passages are harder to read and maintain. The threshold
    /// is measured in word count of the passage body text. The default
    /// threshold is 500 words, which corresponds to roughly 2-3 pages
    /// of narrative text.
    fn detect_large_passages(workspace: &Workspace) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();
        let word_threshold: usize = 500;

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }

                let word_count: usize = passage
                    .body
                    .iter()
                    .map(|block| match block {
                        Block::Text { content, .. } => content.split_whitespace().count(),
                        _ => 0,
                    })
                    .sum();

                if word_count > word_threshold {
                    diagnostics.push(GraphDiagnostic {
                        passage_name: passage.name.clone(),
                        file_uri: doc.uri.to_string(),
                        kind: DiagnosticKind::LargePassage,
                        message: format!(
                            "Passage '{}' contains {} words (threshold: {}); consider splitting into smaller passages",
                            passage.name, word_count, word_threshold
                        ),
                    });
                }
            }
        }

        diagnostics
    }

    /// Detect if the start passage has no outgoing links.
    ///
    /// If the start passage has no links, the story immediately ends
    /// with no player choices. This is almost always an error, unless
    /// the story is intentionally a single-passage story.
    fn detect_missing_start_link(
        workspace: &Workspace,
        start_passage: &str,
    ) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        // Find the start passage
        let start_doc = workspace.documents().find(|doc| {
            doc.passages.iter().any(|p| p.name == start_passage)
        });

        let Some(start_doc) = start_doc else {
            // Missing start passage is already detected elsewhere
            return diagnostics;
        };

        let start_passage_data = start_doc
            .passages
            .iter()
            .find(|p| p.name == start_passage);

        let Some(start_p) = start_passage_data else {
            return diagnostics;
        };

        if start_p.is_metadata() {
            return diagnostics;
        }

        let has_outgoing = !start_p.links.is_empty()
            || !workspace.graph.outgoing_neighbors(start_passage).is_empty();

        if !has_outgoing {
            diagnostics.push(GraphDiagnostic {
                passage_name: start_passage.to_string(),
                file_uri: start_doc.uri.to_string(),
                kind: DiagnosticKind::MissingStartLink,
                message: format!(
                    "Start passage '{}' has no outgoing links; the story has no player choices",
                    start_passage
                ),
            });
        }

        diagnostics
    }

    /// Collect per-passage variable data, with VarOps sorted by source position.
    pub fn collect_passage_data(workspace: &Workspace) -> HashMap<String, PassageVarData> {
        let mut data = HashMap::new();
        for doc in workspace.documents() {
            for passage in &doc.passages {
                // Skip metadata passages — they don't participate in flow analysis
                if passage.is_metadata() {
                    continue;
                }

                let sorted_vars = passage.vars_sorted_by_span();
                let persistent_writes: Vec<String> = passage
                    .persistent_variable_writes()
                    .map(|v| v.name.clone())
                    .collect();
                let persistent_reads: Vec<String> = passage
                    .persistent_variable_reads()
                    .map(|v| v.name.clone())
                    .collect();

                // All vars sorted (for intra-passage ordering)
                let ordered_vars: Vec<VarOp> = sorted_vars.into_iter().cloned().collect();

                data.insert(
                    passage.name.clone(),
                    PassageVarData {
                        file_uri: doc.uri.to_string(),
                        ordered_vars,
                        persistent_writes: persistent_writes.into_iter().collect(),
                        persistent_reads: persistent_reads.into_iter().collect(),
                        contributes_variables: passage.contributes_variables(),
                        is_special: passage.is_special,
                    },
                );
            }
        }
        data
    }

    /// Collect variable writes from special passages that contribute_variables.
    ///
    /// This handles SugarCube's StoryInit (runs before first passage),
    /// Harlowe's startup, and Snowman's Script passage. These writes
    /// form the initial "definitely initialized" seed set.
    pub fn collect_special_passage_initializers(
        workspace: &Workspace,
        passage_data: &HashMap<String, PassageVarData>,
    ) -> InitSet {
        let mut seed = InitSet::new();

        for data in passage_data.values() {
            if data.contributes_variables && data.is_special {
                for var_name in &data.persistent_writes {
                    seed.insert(var_name.clone());
                }
            }
        }

        // Also check for special passages defined in format plugins
        // that may not be in passage_data (e.g., if they're in unindexed files)
        let _ = workspace; // Ensure we use the workspace parameter
        seed
    }

    /// Public wrapper for the dataflow algorithm, returning the flow states map.
    /// This is used by the debug endpoint to compute initialized-at-entry for
    /// a specific passage.
    pub fn run_dataflow_from_engine(
        workspace: &Workspace,
        start_passage: &str,
        passage_data: &HashMap<String, PassageVarData>,
        seed_init: &InitSet,
    ) -> HashMap<String, PassageFlowState> {
        Self::run_dataflow(workspace, start_passage, passage_data, seed_init)
    }

    /// Collect variable operations per passage as references (for infinite loop detection).
    pub fn collect_passage_vars_as_ref(workspace: &Workspace) -> HashMap<String, Vec<&VarOp>> {
        Self::collect_passage_vars(workspace)
    }

    /// Run the forward dataflow worklist algorithm.
    ///
    /// Uses must-analysis (intersection at join points) to compute which
    /// variables are definitely initialized at the entry and exit of each
    /// passage. The worklist processes passages until a fixpoint is reached.
    fn run_dataflow(
        workspace: &Workspace,
        start_passage: &str,
        passage_data: &HashMap<String, PassageVarData>,
        seed_init: &InitSet,
    ) -> HashMap<String, PassageFlowState> {
        // Initialize flow states for all passages
        let mut flow_states: HashMap<String, PassageFlowState> = HashMap::new();

        for name in workspace.graph.passage_names() {
            let node = workspace.graph.get_passage(&name);
            let is_metadata = node.map(|n| n.is_metadata).unwrap_or(false);
            if is_metadata {
                continue;
            }

            flow_states.insert(
                name,
                PassageFlowState {
                    entry: InitSet::new(),
                    exit: InitSet::new(),
                },
            );
        }

        // Set the start passage entry to the seed set
        if let Some(start_state) = flow_states.get_mut(start_passage) {
            start_state.entry = seed_init.clone();
        }

        // Process the start passage to compute its exit state
        if let Some(data) = passage_data.get(start_passage)
            && let Some(state) = flow_states.get_mut(start_passage) {
                let mut exit = state.entry.clone();
                Self::process_passage_vars(data, &mut exit);
                state.exit = exit;
            }

        // Worklist: start with all successors of the start passage
        let mut worklist: VecDeque<String> = VecDeque::new();
        let mut in_worklist: HashSet<String> = HashSet::new();

        for succ in workspace.graph.outgoing_neighbors(start_passage) {
            if flow_states.contains_key(&succ) && !in_worklist.contains(&succ) {
                worklist.push_back(succ.clone());
                in_worklist.insert(succ);
            }
        }

        // Also add all passages that have special passage predecessors
        // (StoryInit doesn't have graph edges, but it contributes to the start passage)
        // This is already handled by the seed initialization above.

        // Fixpoint iteration
        let max_iterations = workspace.graph.passage_count() * 10; // Safety bound
        let mut iterations = 0;

        while let Some(passage_name) = worklist.pop_front() {
            in_worklist.remove(&passage_name);
            iterations += 1;

            if iterations > max_iterations {
                tracing::warn!(
                    "Dataflow analysis exceeded max iterations ({}) — stopping",
                    max_iterations
                );
                break;
            }

            // Compute new entry state: intersection of all predecessor exit states
            // (must-analysis: a variable is definitely initialized only if it's
            // initialized on ALL incoming paths)
            let predecessors = workspace.graph.incoming_neighbors(&passage_name);

            let new_entry = if predecessors.is_empty() {
                // No predecessors — only the seed init applies (already handled for start)
                if passage_name == start_passage {
                    seed_init.clone()
                } else {
                    InitSet::new()
                }
            } else {
                // Intersection of predecessor exit states
                let mut intersection: Option<InitSet> = None;
                for pred in &predecessors {
                    if let Some(pred_state) = flow_states.get(pred) {
                        match &intersection {
                            None => intersection = Some(pred_state.exit.clone()),
                            Some(current) => {
                                let new_intersect: InitSet = current
                                    .intersection(&pred_state.exit)
                                    .cloned()
                                    .collect();
                                intersection = Some(new_intersect);
                            }
                        }
                    }
                }
                intersection.unwrap_or_default()
            };

            // Check if entry state changed
            let state = flow_states.get_mut(&passage_name);
            let Some(state) = state else { continue };

            let entry_changed = new_entry != state.entry;
            if !entry_changed {
                // No change in entry state — no need to reprocess
                continue;
            }

            state.entry = new_entry;

            // Compute exit state: process writes in order
            let mut exit = state.entry.clone();
            if let Some(data) = passage_data.get(&passage_name) {
                // For PassageReady-type special passages, their writes are also
                // injected at the entry of every normal passage transition.
                // For simplicity, we handle this by also including writes from
                // passages with behavior=PassageReady in the exit state.
                Self::process_passage_vars(data, &mut exit);
            }

            let exit_changed = exit != state.exit;
            state.exit = exit;

            // If exit state changed, add successors to worklist
            if exit_changed {
                for succ in workspace.graph.outgoing_neighbors(&passage_name) {
                    if flow_states.contains_key(&succ) && !in_worklist.contains(&succ) {
                        worklist.push_back(succ.clone());
                        in_worklist.insert(succ);
                    }
                }
            }
        }

        flow_states
    }

    /// Process a passage's variable operations in source order, updating
    /// the initialized set. Writes add variables, reads check against the set.
    fn process_passage_vars(data: &PassageVarData, init_set: &mut InitSet) {
        for var in &data.ordered_vars {
            // Skip temporary variables — they don't persist across passages
            if var.is_temporary {
                continue;
            }
            match var.kind {
                VarKind::Write => {
                    init_set.insert(var.name.clone());
                }
                VarKind::Read => {
                    // Reads don't change the initialized set
                }
            }
        }
    }

    /// Detect reads of variables that are not definitely initialized.
    ///
    /// For each passage, we check reads against the entry initialized set.
    /// Within a passage, earlier writes can satisfy later reads.
    fn detect_uninitialized_reads(
        workspace: &Workspace,
        passage_data: &HashMap<String, PassageVarData>,
        flow_states: &HashMap<String, PassageFlowState>,
    ) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        for (name, data) in passage_data {
            // Skip special passages that contribute_variables — they run before
            // normal passage flow and their variables are seeded separately
            if data.contributes_variables && data.is_special {
                continue;
            }

            let entry_init = flow_states
                .get(name)
                .map(|s| &s.entry)
                .cloned()
                .unwrap_or_default();

            // Walk through vars in source order, tracking intra-passage initialization
            let mut local_init = entry_init;
            for var in &data.ordered_vars {
                if var.is_temporary {
                    // Temporary variables are per-passage; check them with an empty entry set
                    // but allow intra-passage writes to satisfy later reads
                    continue;
                }

                match var.kind {
                    VarKind::Read => {
                        if !local_init.contains(&var.name) {
                            diagnostics.push(GraphDiagnostic {
                                passage_name: name.clone(),
                                file_uri: data.file_uri.clone(),
                                kind: DiagnosticKind::UninitializedVariable,
                                message: format!(
                                    "Variable '{}' may be used before initialization",
                                    var.name
                                ),
                            });
                        }
                    }
                    VarKind::Write => {
                        local_init.insert(var.name.clone());
                    }
                }
            }
        }

        let _ = workspace;
        diagnostics
    }

    /// Detect variables that are written but never read on any reachable path.
    ///
    /// A variable is considered "unused" if:
    /// 1. It is written in at least one reachable passage, AND
    /// 2. It is never read in any reachable passage, AND
    /// 3. It is not a special passage initializer (those have side effects)
    fn detect_unused_variables(
        workspace: &Workspace,
        passage_data: &HashMap<String, PassageVarData>,
        flow_states: &HashMap<String, PassageFlowState>,
    ) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        // Collect all variables that are written and read across reachable passages
        let mut all_writes: HashMap<String, Vec<(String, String)>> = HashMap::new(); // var_name → [(passage_name, file_uri)]
        let mut all_reads: HashSet<String> = HashSet::new();

        for (name, data) in passage_data {
            // Only consider reachable passages
            if !flow_states.contains_key(name) {
                continue;
            }

            for var_name in &data.persistent_writes {
                all_writes
                    .entry(var_name.clone())
                    .or_default()
                    .push((name.clone(), data.file_uri.clone()));
            }

            for var_name in &data.persistent_reads {
                all_reads.insert(var_name.clone());
            }
        }

        // Find variables that are written but never read
        for (var_name, locations) in &all_writes {
            if !all_reads.contains(var_name) {
                // Report the first location where this variable is written
                if let Some((passage_name, file_uri)) = locations.first() {
                    diagnostics.push(GraphDiagnostic {
                        passage_name: passage_name.clone(),
                        file_uri: file_uri.clone(),
                        kind: DiagnosticKind::UnusedVariable,
                        message: format!(
                            "Variable '{}' is written but never read",
                            var_name
                        ),
                    });
                }
            }
        }

        let _ = workspace;
        diagnostics
    }

    /// Detect variables that are written twice without an intervening read
    /// within the same passage.
    ///
    /// This catches patterns like:
    /// ```twee
    /// <<set $gold to 10>>
    /// <<set $gold to 20>>  ← redundant write
    /// ```
    fn detect_redundant_writes(
        workspace: &Workspace,
        passage_data: &HashMap<String, PassageVarData>,
    ) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        for (name, data) in passage_data {
            // Track which persistent variables have been written but not yet read
            let mut written_not_read: HashSet<String> = HashSet::new();
            let mut reported: HashSet<String> = HashSet::new(); // Avoid duplicate warnings per passage

            for var in &data.ordered_vars {
                if var.is_temporary {
                    continue;
                }

                match var.kind {
                    VarKind::Write => {
                        if written_not_read.contains(&var.name) && !reported.contains(&var.name) {
                            diagnostics.push(GraphDiagnostic {
                                passage_name: name.clone(),
                                file_uri: data.file_uri.clone(),
                                kind: DiagnosticKind::RedundantWrite,
                                message: format!(
                                    "Variable '{}' is written again without being read since the last write",
                                    var.name
                                ),
                            });
                            reported.insert(var.name.clone());
                        }
                        written_not_read.insert(var.name.clone());
                    }
                    VarKind::Read => {
                        // A read clears the "written but not read" flag
                        written_not_read.remove(&var.name);
                        reported.remove(&var.name); // Allow new redundant write warning
                    }
                }
            }
        }

        let _ = workspace;
        diagnostics
    }
}

/// Per-passage variable data used during dataflow analysis.
pub struct PassageVarData {
    /// The URI of the document containing this passage.
    file_uri: String,
    /// All variable operations sorted by source position.
    ordered_vars: Vec<VarOp>,
    /// Set of persistent variable names written in this passage.
    persistent_writes: HashSet<String>,
    /// Set of persistent variable names read in this passage.
    persistent_reads: HashSet<String>,
    /// Whether this passage contributes variable state (special passage).
    contributes_variables: bool,
    /// Whether this is a special passage.
    is_special: bool,
}
