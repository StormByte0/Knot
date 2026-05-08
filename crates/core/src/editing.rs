//! Incremental Editing Pipeline
//!
//! Knot is designed around incremental, non-blocking updates. This module
//! implements the editing workflow:
//!
//! 1. File Change (didChange event)
//! 2. Debounce (avoid excessive recomputation)
//! 3. Incremental Parse (only modified passages)
//! 4. Graph Surgery (update graph in-place)
//! 5. Analysis Invalidation (only affected regions)
//! 6. Diagnostics Return

use crate::graph::{PassageEdge, PassageNode, PassageGraph};
use crate::passage::Passage;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// The debounce interval for edit events (in milliseconds).
const DEBOUNCE_MS: u64 = 50;

/// Result of an incremental update.
#[derive(Debug, Clone)]
pub struct UpdateResult {
    /// Passage names that were added.
    pub added: Vec<String>,
    /// Passage names that were removed.
    pub removed: Vec<String>,
    /// Passage names that were modified.
    pub modified: Vec<String>,
    /// Whether graph analysis needs to be rerun.
    pub needs_analysis: bool,
}

impl UpdateResult {
    /// Whether any passages changed.
    pub fn has_changes(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty() || !self.modified.is_empty()
    }
}

/// Performs graph surgery — incrementally updates the passage graph
/// based on passage changes in a document.
///
/// The `file_uri` is the URI of the document being modified, which is used
/// to populate the `PassageNode::file_uri` field for newly added or modified
/// passages.
pub fn graph_surgery(
    graph: &mut PassageGraph,
    old_passages: &[Passage],
    new_passages: &[Passage],
    file_uri: &str,
) -> UpdateResult {
    let old_names: HashSet<String> = old_passages.iter().map(|p| p.name.clone()).collect();
    let new_names: HashSet<String> = new_passages.iter().map(|p| p.name.clone()).collect();

    // Passages that were removed
    let removed: Vec<String> = old_names.difference(&new_names).cloned().collect();
    // Passages that were added
    let added: Vec<String> = new_names.difference(&old_names).cloned().collect();
    // Passages that exist in both (may be modified)
    let modified: Vec<String> = new_names.intersection(&old_names).cloned().collect();

    // Remove deleted passages from the graph
    for name in &removed {
        graph.remove_passage(name);
    }

    // Remove edges from modified passages (they'll be re-added)
    for name in &modified {
        graph.remove_edges_from(name);
    }

    // Add new passages and re-add edges for modified passages
    for passage in new_passages {
        if added.contains(&passage.name) || modified.contains(&passage.name) {
            // Add/update the node with the correct file URI
            let node = PassageNode {
                name: passage.name.clone(),
                file_uri: file_uri.to_string(),
                is_special: passage.is_special,
                is_metadata: passage.is_metadata(),
            };
            graph.add_passage(node);

            // Re-add edges for this passage
            for link in &passage.links {
                let target_exists = graph.contains_passage(&link.target);
                let edge = PassageEdge {
                    display_text: link.display_text.clone(),
                    is_broken: !target_exists,
                };
                graph.add_edge(&passage.name, &link.target, edge);
            }
        }
    }

    UpdateResult {
        needs_analysis: !removed.is_empty() || !added.is_empty() || !modified.is_empty(),
        added,
        removed,
        modified,
    }
}

/// A debounce timer for edit events that ensures analysis always fires
/// after the final edit in a burst.
///
/// Unlike a simple "is_pending / is_ready" gate, this timer tracks whether
/// an edit was skipped during a debounce window so that a follow-up
/// analysis can be triggered once the window expires — even if no new
/// `did_change` event arrives.
#[derive(Debug)]
pub struct DebounceTimer {
    /// The last time an edit was received.
    last_edit: Option<Instant>,
    /// The debounce duration.
    duration: Duration,
    /// Whether we skipped analysis for at least one edit during the current
    /// debounce window. When the window expires, the caller must re-run
    /// analysis and clear this flag.
    skipped: bool,
}

impl DebounceTimer {
    /// Create a new debounce timer with the default duration.
    pub fn new() -> Self {
        Self {
            last_edit: None,
            duration: Duration::from_millis(DEBOUNCE_MS),
            skipped: false,
        }
    }

    /// Create a debounce timer with a custom duration.
    pub fn with_duration(duration: Duration) -> Self {
        Self {
            last_edit: None,
            duration,
            skipped: false,
        }
    }

    /// Record an edit event.
    pub fn record_edit(&mut self) {
        self.last_edit = Some(Instant::now());
    }

    /// Check if the debounce period has elapsed since the last edit.
    /// Returns true if enough time has passed to proceed with processing.
    pub fn is_ready(&self) -> bool {
        match self.last_edit {
            Some(last) => last.elapsed() >= self.duration,
            None => true,
        }
    }

    /// Check if there is a pending edit (timer has been started but not yet ready).
    pub fn is_pending(&self) -> bool {
        self.last_edit.is_some() && !self.is_ready()
    }

    /// Mark that analysis was skipped for this debounce window.
    /// Called when `is_pending()` returns true and the handler returns early.
    pub fn mark_skipped(&mut self) {
        self.skipped = true;
    }

    /// Check whether a skipped edit needs to be flushed.
    ///
    /// Returns `true` when:
    /// - Analysis was previously skipped (`mark_skipped` was called), AND
    /// - The debounce window has since expired (`is_ready()` is true)
    ///
    /// After calling this and re-running analysis, call `clear_skipped()`
    /// to reset the flag.
    pub fn needs_flush(&self) -> bool {
        self.skipped && self.is_ready()
    }

    /// Clear the skipped flag after a flush has been processed.
    pub fn clear_skipped(&mut self) {
        self.skipped = false;
    }
}

impl Default for DebounceTimer {
    fn default() -> Self {
        Self::new()
    }
}
