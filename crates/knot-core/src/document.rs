//! Unified Document Model
//!
//! Every file is normalized into a format-agnostic internal representation.
//! Format plugins are responsible only for parsing source text into this structure.
//! The core engine owns global graph construction, workspace indexing,
//! cross-file diagnostics, and dataflow analysis.

use crate::passage::{Passage, StoryFormat};
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use url::Url;

/// A normalized, format-agnostic representation of a Twee source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// The URI of this document.
    pub uri: Url,
    /// The story format used to parse this document.
    pub format: StoryFormat,
    /// The passages contained in this document.
    pub passages: Vec<Passage>,
    /// The version number of this document (increments on each change).
    pub version: i32,
}

impl Document {
    /// Create a new empty document.
    pub fn new(uri: Url, format: StoryFormat) -> Self {
        Self {
            uri,
            format,
            passages: Vec::new(),
            version: 0,
        }
    }

    /// Find a passage by name.
    pub fn find_passage(&self, name: &str) -> Option<&Passage> {
        self.passages.iter().find(|p| p.name == name)
    }

    /// Find a passage by name, mutably.
    pub fn find_passage_mut(&mut self, name: &str) -> Option<&mut Passage> {
        self.passages.iter_mut().find(|p| p.name == name)
    }

    /// Find the StoryData passage, if present.
    pub fn story_data(&self) -> Option<&Passage> {
        self.passages.iter().find(|p| p.name == "StoryData")
    }

    /// Find the StoryTitle passage, if present.
    pub fn story_title(&self) -> Option<&Passage> {
        self.passages.iter().find(|p| p.name == "StoryTitle")
    }

    /// Return all links from all passages in this document.
    pub fn all_links(&self) -> impl Iterator<Item = &crate::passage::Link> {
        self.passages.iter().flat_map(|p| p.links.iter())
    }

    /// Return all variable operations from all passages.
    pub fn all_var_ops(&self) -> impl Iterator<Item = &crate::passage::VarOp> {
        self.passages.iter().flat_map(|p| p.vars.iter())
    }

    /// Increment the document version.
    pub fn bump_version(&mut self) {
        self.version += 1;
    }

    /// Remove a passage by name and return it.
    pub fn remove_passage(&mut self, name: &str) -> Option<Passage> {
        let idx = self.passages.iter().position(|p| p.name == name)?;
        Some(self.passages.remove(idx))
    }

    /// Replace a passage by name, or add it if it doesn't exist.
    pub fn upsert_passage(&mut self, passage: Passage) {
        if let Some(existing) = self.find_passage_mut(&passage.name) {
            *existing = passage;
        } else {
            self.passages.push(passage);
        }
    }
}

/// A snapshot of a document at a particular version, used for incremental
/// updates and change tracking.
#[derive(Debug, Clone)]
pub struct DocumentSnapshot {
    /// The document URI.
    pub uri: Url,
    /// The version this snapshot represents.
    pub version: i32,
    /// The full text content as a Rope for efficient incremental editing.
    pub rope: Rope,
    /// Names of passages that existed at this version.
    pub passage_names: Vec<String>,
}

impl DocumentSnapshot {
    /// Create a snapshot from a document and its text content.
    pub fn from_document(doc: &Document, text: &str) -> Self {
        Self {
            uri: doc.uri.clone(),
            version: doc.version,
            rope: Rope::from_str(text),
            passage_names: doc.passages.iter().map(|p| p.name.clone()).collect(),
        }
    }

    /// Apply a range of text changes to the rope.
    pub fn apply_change(&mut self, range: Range<usize>, new_text: &str) {
        self.rope.remove(range.start..range.end);
        self.rope.insert(range.start, new_text);
        self.version += 1;
    }
}
