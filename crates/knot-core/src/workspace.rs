//! Workspace Model
//!
//! Knot is designed around a single-project workspace model. Each VS Code
//! workspace is expected to contain exactly one Twine project and exactly
//! one authoritative StoryData passage.
//!
//! This module handles:
//! - StoryData discovery and validation
//! - Format resolution
//! - Workspace indexing
//! - Project configuration loading

use crate::document::Document;
use crate::graph::{DiagnosticKind, GraphDiagnostic, PassageGraph};
use crate::passage::{Block, StoryFormat};
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;

/// Knot-specific workspace configuration.
/// Loaded from `.vscode/knot.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnotConfig {
    /// Path to the Tweego compiler binary.
    #[serde(default)]
    pub compiler_path: Option<PathBuf>,
    /// Build configuration.
    #[serde(default)]
    pub build: BuildConfig,
    /// Diagnostic severity overrides.
    #[serde(default)]
    pub diagnostics: HashMap<String, DiagnosticSeverity>,
    /// Files/patterns to ignore during indexing.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Story format override. When set, this takes priority over StoryData
    /// as the resolved format (Priority 2 in the architecture).
    #[serde(default)]
    pub format: Option<String>,
}

impl Default for KnotConfig {
    fn default() -> Self {
        Self {
            compiler_path: None,
            build: BuildConfig::default(),
            diagnostics: HashMap::new(),
            ignore: Vec::new(),
            format: None,
        }
    }
}

/// Build configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    /// Output directory for compiled HTML.
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
    /// Additional compiler flags.
    #[serde(default)]
    pub flags: Vec<String>,
}

fn default_output_dir() -> String {
    "build".to_string()
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            output_dir: default_output_dir(),
            flags: Vec::new(),
        }
    }
}

/// Configurable diagnostic severity level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
    Off,
}

/// The resolved story format and metadata, extracted from StoryData.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryMetadata {
    /// The story format declared in StoryData.
    pub format: StoryFormat,
    /// The format version (e.g., "2.36.1").
    pub format_version: Option<String>,
    /// The entry point passage name (from "start" field, defaults to "Start").
    pub start_passage: String,
    /// The IFID (Interactive Fiction IDentifier).
    pub ifid: Option<String>,
}

impl Default for StoryMetadata {
    fn default() -> Self {
        Self {
            format: StoryFormat::default_format(),
            format_version: None,
            start_passage: "Start".to_string(),
            ifid: None,
        }
    }
}

/// Intermediate JSON struct for deserializing the StoryData passage body.
/// The Twee 3 StoryData body is a JSON object with optional fields.
#[derive(Debug, Deserialize)]
struct StoryDataJson {
    ifid: Option<String>,
    format: Option<String>,
    #[serde(rename = "format-version")]
    format_version: Option<String>,
    start: Option<String>,
}

/// The workspace — represents a single Twine project.
#[derive(Debug)]
pub struct Workspace {
    /// The root URI of the workspace.
    pub root_uri: Url,
    /// All documents in the workspace, indexed by URI.
    documents: HashMap<Url, Document>,
    /// The passage graph for this workspace.
    pub graph: PassageGraph,
    /// The resolved story metadata (from StoryData).
    pub metadata: Option<StoryMetadata>,
    /// The Knot configuration (from .vscode/knot.json).
    pub config: KnotConfig,
    /// Whether the workspace has been fully indexed.
    pub indexed: bool,
}

impl Workspace {
    /// Create a new workspace rooted at the given URI.
    pub fn new(root_uri: Url) -> Self {
        Self {
            root_uri,
            documents: HashMap::new(),
            graph: PassageGraph::new(),
            metadata: None,
            config: KnotConfig::default(),
            indexed: false,
        }
    }

    /// Add or replace a document in the workspace.
    /// Returns the previous document if one existed at the same URI.
    pub fn insert_document(&mut self, doc: Document) -> Option<Document> {
        self.documents.insert(doc.uri.clone(), doc)
    }

    /// Remove a document from the workspace by URI.
    pub fn remove_document(&mut self, uri: &Url) -> Option<Document> {
        self.documents.remove(uri)
    }

    /// Get a document by URI.
    pub fn get_document(&self, uri: &Url) -> Option<&Document> {
        self.documents.get(uri)
    }

    /// Get a document by URI, mutably.
    pub fn get_document_mut(&mut self, uri: &Url) -> Option<&mut Document> {
        self.documents.get_mut(uri)
    }

    /// Iterate over all documents in the workspace.
    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }

    /// Iterate over all documents mutably.
    pub fn documents_mut(&mut self) -> impl Iterator<Item = &mut Document> {
        self.documents.values_mut()
    }

    /// Find a passage by name across all documents.
    pub fn find_passage(&self, name: &str) -> Option<(&Document, &crate::passage::Passage)> {
        for doc in self.documents.values() {
            if let Some(passage) = doc.find_passage(name) {
                return Some((doc, passage));
            }
        }
        None
    }

    /// Return all passage names across all documents.
    pub fn all_passage_names(&self) -> Vec<String> {
        self.documents
            .values()
            .flat_map(|doc| doc.passages.iter().map(|p| p.name.clone()))
            .collect()
    }

    /// Parse the StoryData passage body as JSON and populate `self.metadata`.
    ///
    /// Finds the first StoryData passage across all documents, extracts its
    /// body text, parses it as JSON, and constructs a `StoryMetadata` from
    /// the `format`, `format-version`, `start`, and `ifid` fields.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No StoryData passage is found in any document.
    /// - The StoryData body cannot be extracted (no text blocks).
    /// - The body text is not valid JSON.
    pub fn parse_story_data(&mut self) -> Result<(), String> {
        // Find the StoryData passage across all documents
        let story_data_passage = self
            .documents
            .values()
            .find_map(|doc| doc.story_data())
            .ok_or_else(|| "No StoryData passage found in workspace".to_string())?;

        // Extract the body text from blocks
        let body_text: String = story_data_passage
            .body
            .iter()
            .filter_map(|block| match block {
                Block::Text { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect();

        if body_text.trim().is_empty() {
            return Err("StoryData passage body is empty".to_string());
        }

        // Parse the JSON body
        let data: StoryDataJson =
            serde_json::from_str(&body_text).map_err(|e| format!("Failed to parse StoryData JSON: {}", e))?;

        // Resolve the story format from the parsed format string
        let format = self.resolve_format_from_storydata(data.format.as_deref());

        self.metadata = Some(StoryMetadata {
            format,
            format_version: data.format_version,
            start_passage: data.start.unwrap_or_else(|| "Start".to_string()),
            ifid: data.ifid,
        });

        Ok(())
    }

    /// Resolve the `StoryFormat` from the StoryData format string.
    ///
    /// Attempts to parse the format string using `FromStr`. If the format
    /// string is `None` or unrecognized, falls back to the default format
    /// (SugarCube).
    fn resolve_format_from_storydata(&self, format_str: Option<&str>) -> StoryFormat {
        match format_str {
            Some(s) => StoryFormat::from_str(s).unwrap_or_else(|_| StoryFormat::default_format()),
            None => StoryFormat::default_format(),
        }
    }

    /// Resolve the story format using the priority order:
    /// 1. StoryData passage
    /// 2. knot.json configuration
    /// 3. Heuristic scan
    /// 4. Default (SugarCube 2)
    pub fn resolve_format(&self) -> StoryFormat {
        // Priority 1: StoryData
        if let Some(metadata) = &self.metadata {
            return metadata.format.clone();
        }

        // Priority 2: knot.json configuration
        if let Some(format_str) = &self.config.format {
            if let Ok(format) = StoryFormat::from_str(format_str) {
                return format;
            }
        }

        // Priority 4: Default
        StoryFormat::default_format()
    }

    /// Validate StoryData and produce diagnostics.
    pub fn validate_story_data(&self) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        // Count StoryData passages
        let story_data_count: usize = self
            .documents
            .values()
            .map(|doc| doc.passages.iter().filter(|p| p.name == "StoryData").count())
            .sum();

        if story_data_count == 0 {
            diagnostics.push(GraphDiagnostic {
                passage_name: "StoryData".to_string(),
                file_uri: self.root_uri.to_string(),
                kind: DiagnosticKind::MissingStoryData,
                message: "Missing StoryData passage. Assuming SugarCube 2.".to_string(),
            });
        } else if story_data_count > 1 {
            diagnostics.push(GraphDiagnostic {
                passage_name: "StoryData".to_string(),
                file_uri: self.root_uri.to_string(),
                kind: DiagnosticKind::DuplicateStoryData,
                message: format!(
                    "Found {} StoryData passages; expected exactly 1",
                    story_data_count
                ),
            });
        }

        // Validate start passage exists
        if let Some(metadata) = &self.metadata {
            if self.find_passage(&metadata.start_passage).is_none() {
                diagnostics.push(GraphDiagnostic {
                    passage_name: "StoryData".to_string(),
                    file_uri: self.root_uri.to_string(),
                    kind: DiagnosticKind::MissingStartPassage,
                    message: format!(
                        "Start passage '{}' not found in workspace",
                        metadata.start_passage
                    ),
                });
            }
        }

        diagnostics
    }

    /// Mark the workspace as indexed.
    pub fn mark_indexed(&mut self) {
        self.indexed = true;
    }

    /// The number of documents in the workspace.
    pub fn document_count(&self) -> usize {
        self.documents.len()
    }

    /// The total number of passages across all documents.
    pub fn passage_count(&self) -> usize {
        self.documents.values().map(|d| d.passages.len()).sum()
    }

    /// Load workspace configuration from `.vscode/knot.json`.
    ///
    /// If the file doesn't exist or is invalid, the existing default config
    /// is retained and an optional warning is logged.
    pub fn load_config(&mut self, config_text: &str) -> Result<(), String> {
        let config: KnotConfig =
            serde_json::from_str(config_text).map_err(|e| format!("Failed to parse knot.json: {}", e))?;
        self.config = config;
        Ok(())
    }

    /// Find the file URI containing a passage by name.
    ///
    /// Returns the URI of the document that contains the passage, or `None`
    /// if no such passage exists.
    pub fn find_passage_file_uri(&self, passage_name: &str) -> Option<Url> {
        for doc in self.documents.values() {
            if doc.find_passage(passage_name).is_some() {
                return Some(doc.uri.clone());
            }
        }
        None
    }

    /// Remove a document and update the graph by removing all passages
    /// that belonged to it.
    ///
    /// Returns the removed document if it existed.
    pub fn remove_document_and_update_graph(&mut self, uri: &Url) -> Option<Document> {
        let doc = self.documents.remove(uri)?;
        // Remove all passages from this document from the graph
        for passage in &doc.passages {
            self.graph.remove_passage(&passage.name);
        }
        Some(doc)
    }

    /// Check if a document with the given URI exists in the workspace.
    pub fn contains_document(&self, uri: &Url) -> bool {
        self.documents.contains_key(uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::passage::{Block, Passage, StoryFormat};

    /// Helper to create a workspace with a single document containing a StoryData passage.
    fn workspace_with_story_data(json_body: &str) -> Workspace {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        let uri = Url::parse("file:///project/story.twee").unwrap();
        let mut doc = Document::new(uri.clone(), StoryFormat::SugarCube);
        let mut passage = Passage::new("StoryData".to_string(), 0..100);
        passage.body = vec![Block::Text {
            content: json_body.to_string(),
            span: 0..json_body.len(),
        }];
        doc.passages.push(passage);
        ws.insert_document(doc);
        ws
    }

    #[test]
    fn parse_story_data_full() {
        let json = r#"{
            "ifid": "A1B2C3D4-E5F6-7890-1234-567890ABCDEF",
            "format": "SugarCube",
            "format-version": "2.36.1",
            "start": "Prologue"
        }"#;
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().expect("parse should succeed");

        let meta = ws.metadata.as_ref().expect("metadata should be set");
        assert_eq!(meta.format, StoryFormat::SugarCube);
        assert_eq!(meta.format_version.as_deref(), Some("2.36.1"));
        assert_eq!(meta.start_passage, "Prologue");
        assert_eq!(
            meta.ifid.as_deref(),
            Some("A1B2C3D4-E5F6-7890-1234-567890ABCDEF")
        );
    }

    #[test]
    fn parse_story_data_harlowe() {
        let json = r#"{
            "ifid": "DEADBEEF-0000-0000-0000-000000000000",
            "format": "Harlowe",
            "format-version": "3.3.0",
            "start": "Intro"
        }"#;
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().expect("parse should succeed");

        let meta = ws.metadata.as_ref().expect("metadata should be set");
        assert_eq!(meta.format, StoryFormat::Harlowe);
        assert_eq!(meta.format_version.as_deref(), Some("3.3.0"));
        assert_eq!(meta.start_passage, "Intro");
    }

    #[test]
    fn parse_story_data_minimal() {
        // Only ifid, other fields missing — should use defaults
        let json = r#"{"ifid": "12345678-1234-1234-1234-123456789ABC"}"#;
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().expect("parse should succeed");

        let meta = ws.metadata.as_ref().expect("metadata should be set");
        assert_eq!(meta.format, StoryFormat::SugarCube); // default
        assert_eq!(meta.format_version, None);
        assert_eq!(meta.start_passage, "Start"); // default
        assert_eq!(
            meta.ifid.as_deref(),
            Some("12345678-1234-1234-1234-123456789ABC")
        );
    }

    #[test]
    fn parse_story_data_empty_object() {
        // Empty JSON object — all fields None, should use defaults
        let json = "{}";
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().expect("parse should succeed");

        let meta = ws.metadata.as_ref().expect("metadata should be set");
        assert_eq!(meta.format, StoryFormat::SugarCube);
        assert_eq!(meta.format_version, None);
        assert_eq!(meta.start_passage, "Start");
        assert_eq!(meta.ifid, None);
    }

    #[test]
    fn parse_story_data_no_passage() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        let uri = Url::parse("file:///project/story.twee").unwrap();
        let doc = Document::new(uri, StoryFormat::SugarCube);
        ws.insert_document(doc);

        let result = ws.parse_story_data();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("No StoryData passage found in workspace"));
    }

    #[test]
    fn parse_story_data_invalid_json() {
        let json = "this is not json";
        let mut ws = workspace_with_story_data(json);
        let result = ws.parse_story_data();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to parse StoryData JSON"));
    }

    #[test]
    fn parse_story_data_empty_body() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        let uri = Url::parse("file:///project/story.twee").unwrap();
        let mut doc = Document::new(uri, StoryFormat::SugarCube);
        let passage = Passage::new("StoryData".to_string(), 0..100);
        // No body blocks — empty body
        doc.passages.push(passage);
        ws.insert_document(doc);

        let result = ws.parse_story_data();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn parse_story_data_unsupported_format_falls_back() {
        let json = r#"{
            "format": "UnknownFormat",
            "ifid": "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"
        }"#;
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().expect("parse should succeed");

        let meta = ws.metadata.as_ref().expect("metadata should be set");
        // Unsupported format falls back to SugarCube default
        assert_eq!(meta.format, StoryFormat::SugarCube);
    }

    #[test]
    fn resolve_format_from_metadata() {
        let json = r#"{"format": "Harlowe", "ifid": "TEST"}"#;
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().unwrap();

        // Priority 1: StoryData metadata
        assert_eq!(ws.resolve_format(), StoryFormat::Harlowe);
    }

    #[test]
    fn resolve_format_from_config() {
        let json = r#"{"ifid": "TEST"}"#;
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().unwrap();

        // StoryData has no format, so metadata.format = SugarCube (default).
        // But since metadata IS set, Priority 1 wins.
        assert_eq!(ws.resolve_format(), StoryFormat::SugarCube);
    }

    #[test]
    fn resolve_format_config_no_metadata() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        // No metadata, config has format
        ws.config.format = Some("Harlowe".to_string());
        // Priority 2: knot.json config
        assert_eq!(ws.resolve_format(), StoryFormat::Harlowe);
    }

    #[test]
    fn resolve_format_default() {
        let ws = Workspace::new(Url::parse("file:///project/").unwrap());
        // No metadata, no config format — default
        assert_eq!(ws.resolve_format(), StoryFormat::SugarCube);
    }

    #[test]
    fn resolve_format_invalid_config_falls_back() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        ws.config.format = Some("InvalidFormat".to_string());
        // Invalid config format string should fall through to default
        assert_eq!(ws.resolve_format(), StoryFormat::SugarCube);
    }

    #[test]
    fn parse_story_data_format_version_rename() {
        // Verify that "format-version" (hyphenated) is correctly deserialized
        let json = r#"{
            "format": "Chapbook",
            "format-version": "1.2.3",
            "start": "Beginning"
        }"#;
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().expect("parse should succeed");

        let meta = ws.metadata.as_ref().expect("metadata should be set");
        assert_eq!(meta.format, StoryFormat::Chapbook);
        assert_eq!(meta.format_version.as_deref(), Some("1.2.3"));
        assert_eq!(meta.start_passage, "Beginning");
    }

    #[test]
    fn parse_story_data_whitespace_body() {
        // Body with only whitespace should be treated as empty
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        let uri = Url::parse("file:///project/story.twee").unwrap();
        let mut doc = Document::new(uri, StoryFormat::SugarCube);
        let mut passage = Passage::new("StoryData".to_string(), 0..100);
        passage.body = vec![Block::Text {
            content: "   \n  \t  ".to_string(),
            span: 0..10,
        }];
        doc.passages.push(passage);
        ws.insert_document(doc);

        let result = ws.parse_story_data();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }
}
