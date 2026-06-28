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
use crate::editing::{UpdateResult, graph_surgery};
use crate::graph::{DiagnosticKind, EdgeType, GraphDiagnostic, PassageEdge, PassageGraph};
use crate::passage::{Block, Passage, StoryFormat};
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;

/// A user-defined special passage declaration from `.vscode/knot.json`.
///
/// This is the config-file representation, which gets converted to a
/// full `SpecialPassageDef` during classification. Only the essential
/// fields are exposed to keep the configuration simple.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSpecialPassageDef {
    /// The passage name to match (exact, case-sensitive).
    /// Mutually exclusive with `tag`; one must be set.
    #[serde(default)]
    pub name: Option<String>,
    /// The passage tag to match (e.g., "sidebar" for `[sidebar]` passages).
    /// Mutually exclusive with `name`; one must be set.
    #[serde(default)]
    pub tag: Option<String>,
    /// The behavior category for this special passage.
    /// Defaults to "Custom" if not specified.
    #[serde(default = "default_user_behavior")]
    pub behavior: String,
    /// Whether this passage contributes variables to the dataflow analysis.
    /// Defaults to true for startup/custom behaviors.
    #[serde(default = "default_true")]
    pub contributes_variables: bool,
    /// Whether this passage should appear in the passage graph.
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub participates_in_graph: bool,
}

fn default_user_behavior() -> String {
    "Custom".to_string()
}

fn default_true() -> bool {
    true
}

/// Knot-specific workspace configuration.
/// Loaded from `.vscode/knot.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KnotConfig {
    /// Path to the Tweego compiler binary.
    #[serde(default)]
    pub compiler_path: Option<PathBuf>,
    /// Path to the directory containing installed story formats.
    ///
    /// When set, this takes priority over all other storyformat discovery
    /// mechanisms (tweego binary's sibling dir, project-local `.storyformats`,
    /// knot-managed globalStorage). When unset, the server uses the layered
    /// discovery chain in `build.rs::resolve_storyformats_dir()`.
    ///
    /// This is mirrored by the VS Code setting `knot.build.storyformatsPath`,
    /// which is the recommended way for users to configure this — the
    /// setting is visible in the Settings UI as a folder picker.
    #[serde(default)]
    pub storyformats_path: Option<PathBuf>,
    /// Build configuration.
    #[serde(default)]
    pub build: BuildConfig,
    /// Diagnostic severity overrides.
    #[serde(default)]
    pub diagnostics: HashMap<String, DiagnosticSeverity>,
    /// Files/patterns to ignore during indexing.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Maximum number of files to index. If the workspace exceeds this
    /// limit, indexing stops and a warning is shown. Prevents the server
    /// from hanging on very large workspaces. When `None`, uses the
    /// default from the VS Code setting `knot.indexing.maxFiles` (or
    /// 1000 if unset).
    #[serde(default)]
    pub max_files: Option<usize>,
    /// Story format override. When set, this takes priority over StoryData
    /// as the resolved format (Priority 1 in the architecture). This allows
    /// developers to test their project with a different format without
    /// modifying source files.
    #[serde(default)]
    pub format: Option<String>,
    /// User-defined special passages. Each entry declares a passage that
    /// should be treated as special, with a matching strategy, behavior,
    /// and other properties. These are merged with the format plugin's
    /// built-in special passages during classification.
    ///
    /// Example `.vscode/knot.json`:
    /// ```json
    /// {
    ///   "special_passages": [
    ///     { "name": "MyInit", "behavior": "startup" },
    ///     { "tag": "sidebar", "behavior": "chrome" }
    ///   ]
    /// }
    /// ```
    #[serde(default)]
    pub special_passages: Vec<UserSpecialPassageDef>,
}

/// Build configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    /// Output directory for compiled HTML.
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
    /// Source directory for twee files, relative to the workspace root.
    ///
    /// When set, this is used instead of the workspace root as the source
    /// argument to tweego. This is essential for projects that bundle the
    /// tweego toolchain (or any other non-source files) in a subdirectory —
    /// without this setting, tweego recursively scans the entire workspace
    /// and picks up files like `tweego/storyformats/*/format.js` as source
    /// passages, causing "Replacing existing passage \"format.js\"" warnings
    /// and build failures.
    ///
    /// Example `.vscode/knot.json`:
    /// ```json
    /// { "build": { "source_dir": "src" } }
    /// ```
    ///
    /// When unset or empty, the workspace root is used (backward compat).
    #[serde(default)]
    pub source_dir: Option<String>,
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
            source_dir: None,
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

/// Result of applying a document update to the workspace.
///
/// Returned by [`Workspace::apply_document_update`] so that callers
/// (server handlers) can decide what follow-up actions to take
/// (format detection notifications, semantic token refreshes, etc.)
/// without needing access to workspace internals.
#[derive(Debug)]
pub struct DocumentUpdateResult {
    /// The result of the incremental graph surgery.
    pub surgery_result: UpdateResult,
    /// The resolved format BEFORE this update was applied.
    pub format_before: Option<StoryFormat>,
    /// The resolved format AFTER this update was applied.
    pub format_after: StoryFormat,
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
    ///
    /// If a document with the same URI already exists, it is replaced.
    /// Additionally, this checks for URI-equivalent documents that may
    /// have different serializations (e.g., `file:///d:/path` vs
    /// `file:///d%3A/path` on Windows) and removes them to prevent
    /// duplicate passage entries.
    ///
    /// Returns the previous document if one existed at the same URI.
    pub fn insert_document(&mut self, doc: Document) -> Option<Document> {
        // Safety net: remove any URI-equivalent document that has a different
        // serialization. This handles edge cases where normalization might not
        // have been applied at the entry point.
        let incoming_path = doc.uri.to_file_path().ok();
        if let Some(ref path) = incoming_path {
            let equiv_keys: Vec<Url> = self
                .documents
                .keys()
                .filter(|existing_uri| {
                    **existing_uri != doc.uri && // Different serialization
                    existing_uri.to_file_path().is_ok_and(|p| p == *path)
                })
                .cloned()
                .collect();
            for key in equiv_keys {
                tracing::warn!(
                    "Removing URI-equivalent document: {} (canonical: {})",
                    key,
                    doc.uri
                );
                self.documents.remove(&key);
            }
        }

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
        let data: StoryDataJson = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse StoryData JSON: {}", e))?;

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
    /// (Core — base Twine engine, no format-specific features).
    fn resolve_format_from_storydata(&self, format_str: Option<&str>) -> StoryFormat {
        match format_str {
            Some(s) if !s.is_empty() => StoryFormat::from_str(s).unwrap_or_else(|_| {
                tracing::warn!("Unrecognized format string '{}', falling back to Core", s);
                StoryFormat::default_format()
            }),
            _ => StoryFormat::default_format(),
        }
    }

    /// Resolve the story format using the priority order:
    /// 1. knot.json configuration (explicit override — takes precedence when set)
    /// 2. StoryData passage (authoritative format declaration in source)
    /// 3. Default (Core — base Twine engine, no format-specific features)
    ///
    /// The config override exists so developers can test their project with a
    /// different format without modifying source files. When `config.format`
    /// is set, it wins over StoryData.
    pub fn resolve_format(&self) -> StoryFormat {
        // Priority 1: knot.json configuration (explicit override)
        if let Some(format_str) = &self.config.format
            && let Ok(format) = StoryFormat::from_str(format_str)
        {
            return format;
        }

        // Priority 2: StoryData passage
        if let Some(metadata) = &self.metadata {
            return metadata.format.clone();
        }

        // Priority 3: Default
        StoryFormat::default_format()
    }

    /// Validate StoryData and produce diagnostics.
    pub fn validate_story_data(&self) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();

        // Count StoryData passages
        let story_data_count: usize = self
            .documents
            .values()
            .map(|doc| {
                doc.passages
                    .iter()
                    .filter(|p| p.name == "StoryData")
                    .count()
            })
            .sum();

        if story_data_count == 0 {
            diagnostics.push(GraphDiagnostic {
                passage_name: "StoryData".to_string(),
                file_uri: self.root_uri.to_string(),
                kind: DiagnosticKind::MissingStoryData,
                message: "Missing StoryData passage. Falling back to core Twine engine (no format-specific features).".to_string(),
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
        if let Some(metadata) = &self.metadata
            && self.find_passage(&metadata.start_passage).is_none()
        {
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

        diagnostics
    }

    /// Mark the workspace as indexed and freeze the format.
    ///
    /// After indexing, the story format is **readonly** — no subsequent
    /// file open, edit, or watch event may re-trigger format detection
    /// or overwrite `metadata`. If the user edits StoryData, a server
    /// restart is required.
    pub fn mark_indexed(&mut self) {
        self.indexed = true;
        tracing::info!(
            format = ?self.resolve_format(),
            "Workspace indexing complete — format resolved"
        );
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
        let config: KnotConfig = serde_json::from_str(config_text)
            .map_err(|e| format!("Failed to parse knot.json: {}", e))?;
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

    /// Apply a parsed document update to the workspace.
    ///
    /// This is the **single authoritative entry point** for all document
    /// updates (did_open, did_change, did_change_watched_files). It
    /// atomically:
    ///
    /// 1. Captures the old passages from the existing document (if any)
    /// 2. Inserts the new document into the workspace
    /// 3. Performs incremental graph surgery (adds/removes/modifies nodes
    ///    and edges for the changed passages only)
    /// 4. Rechecks broken links across the entire graph
    /// 5. Rebuilds the upstream lifecycle edge chain (ScriptInjection →
    ///    Startup → Start)
    /// 6. Extracts StoryData metadata from the new document
    ///
    /// The caller is responsible for:
    /// - Parsing the document with the format plugin BEFORE calling this
    /// - Computing `extra_edges` (dynamic navigation links) from the
    ///   format plugin BEFORE calling this
    /// - Publishing diagnostics and sending notifications AFTER this
    ///   returns
    ///
    /// This method ensures the workspace document model and passage graph
    /// are always consistent — you cannot have a document inserted without
    /// the graph being updated, or vice versa.
    pub fn apply_document_update(
        &mut self,
        uri: &Url,
        doc: Document,
        extra_edges: &[(String, Option<String>, String, Option<EdgeType>)],
    ) -> DocumentUpdateResult {
        let format_before = self.metadata.as_ref().map(|m| m.format.clone());

        // 1. Capture old passages for graph surgery
        let old_passages: Vec<Passage> = self
            .get_document(uri)
            .map(|d| d.passages.clone())
            .unwrap_or_default();

        let new_passages = doc.passages.clone();
        let file_uri_str = uri.to_string();

        // 2. Insert the new document
        self.insert_document(doc);

        // 3. Graph surgery — incremental update
        let surgery_result = graph_surgery(
            &mut self.graph,
            &old_passages,
            &new_passages,
            &file_uri_str,
            extra_edges,
        );

        tracing::debug!(
            "apply_document_update: graph_surgery added={:?} removed={:?} modified={:?}, graph nodes={} edges={}",
            surgery_result.added,
            surgery_result.removed,
            surgery_result.modified,
            self.graph.passage_count(),
            self.graph.edge_count()
        );

        // 4. Recheck broken links after surgery
        self.graph.recheck_broken_links();

        // 5. Rebuild upstream lifecycle edges
        self.rebuild_upstream_edges();

        // 6. Format isolation: StoryData parsing is a CORE operation handled
        // exclusively by the two-pass indexing in index_workspace(). This
        // method must NOT re-extract StoryData metadata or trigger format
        // switches. Individual format plugins (SugarCube, etc.) treat
        // StoryData as a special passage name with JSON highlighting only.
        // See: Format Isolation (useinteraction.md §7).

        let format_after = self.resolve_format();

        DocumentUpdateResult {
            surgery_result,
            format_before,
            format_after,
        }
    }

    /// Rebuild the upstream lifecycle edge chain.
    ///
    /// After graph surgery or document removal, the implicit upstream edges
    /// among special passages may be missing. This method re-establishes:
    ///
    /// 1. **ScriptInjection → Startup**: Script injection passages run
    ///    before startup passages (e.g., "Story JavaScript" → "StoryInit").
    /// 2. **Startup → Start**: The last startup passage bridges into the
    ///    user-defined passage graph.
    ///
    /// This method queries the graph's `special_bundle`, which is
    /// maintained incrementally by `add_passage()` / `remove_passage()`,
    /// so it never needs to re-scan workspace documents.
    pub fn rebuild_upstream_edges(&mut self) {
        let graph = &mut self.graph;

        let script_injection = graph.special_bundle.script_injection.clone();
        let startup = graph.special_bundle.startup.clone();

        let start_passage_name: String = self
            .metadata
            .as_ref()
            .map(|m| m.start_passage.clone())
            .unwrap_or_else(|| "Start".into());

        // Upstream edge: ScriptInjection → Startup
        for script_name in &script_injection {
            for startup_name in &startup {
                let exists = graph
                    .outgoing_neighbors(script_name)
                    .iter()
                    .any(|n| n == startup_name);
                if !exists {
                    graph.add_edge(
                        script_name,
                        startup_name,
                        PassageEdge {
                            display_text: Some(format!(
                                "(upstream: {} → {})",
                                script_name, startup_name
                            )),
                            edge_type: EdgeType::Upstream,
                            pre_broken_type: None,
                        },
                    );
                }
            }
        }

        // Bridge edge: Startup → Start passage
        if !startup.is_empty() && graph.contains_passage(&start_passage_name) {
            let bridge_source = &startup[0];
            let exists = graph
                .outgoing_neighbors(bridge_source)
                .contains(&start_passage_name);
            if !exists {
                graph.add_edge(
                    bridge_source,
                    &start_passage_name,
                    PassageEdge {
                        display_text: Some(format!(
                            "(upstream: {} → {})",
                            bridge_source, start_passage_name
                        )),
                        edge_type: EdgeType::Upstream,
                        pre_broken_type: None,
                    },
                );
            }
        }
    }

    /// Check if a document with the given URI exists in the workspace.
    pub fn contains_document(&self, uri: &Url) -> bool {
        self.documents.contains_key(uri)
    }

    /// Generate a new IFID (Interactive Fiction IDentifier).
    ///
    /// IFIDs are UUIDs in uppercase, following the Twine/Twee specification.
    /// This can be used when creating a new project skeleton or when a
    /// StoryData passage is missing an IFID.
    pub fn generate_ifid() -> String {
        uuid::Uuid::new_v4().to_string().to_uppercase()
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
        assert_eq!(meta.format, StoryFormat::Core); // default — no format specified
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
        assert_eq!(meta.format, StoryFormat::Core); // default — no format specified
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
        assert!(
            result
                .unwrap_err()
                .contains("No StoryData passage found in workspace")
        );
    }

    #[test]
    fn parse_story_data_invalid_json() {
        let json = "this is not json";
        let mut ws = workspace_with_story_data(json);
        let result = ws.parse_story_data();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Failed to parse StoryData JSON")
        );
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
        // Unsupported format falls back to Core (base Twine engine)
        assert_eq!(meta.format, StoryFormat::Core);
    }

    #[test]
    fn resolve_format_from_metadata() {
        let json = r#"{"format": "Harlowe", "ifid": "TEST"}"#;
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().unwrap();

        // No config override set, so Priority 2 (StoryData) wins
        assert_eq!(ws.resolve_format(), StoryFormat::Harlowe);
    }

    #[test]
    fn resolve_format_config_overrides_metadata() {
        let json = r#"{"format": "SugarCube", "ifid": "TEST"}"#;
        let mut ws = workspace_with_story_data(json);
        ws.parse_story_data().unwrap();

        // Config override takes priority over StoryData
        ws.config.format = Some("Harlowe".to_string());
        assert_eq!(ws.resolve_format(), StoryFormat::Harlowe);
    }

    #[test]
    fn resolve_format_config_no_metadata() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        // No metadata, config has format
        ws.config.format = Some("Harlowe".to_string());
        // Priority 1: knot.json config override
        assert_eq!(ws.resolve_format(), StoryFormat::Harlowe);
    }

    #[test]
    fn resolve_format_default() {
        let ws = Workspace::new(Url::parse("file:///project/").unwrap());
        // No metadata, no config format — default to Core (base Twine engine)
        assert_eq!(ws.resolve_format(), StoryFormat::Core);
    }

    #[test]
    fn resolve_format_invalid_config_falls_back() {
        let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
        ws.config.format = Some("InvalidFormat".to_string());
        // Invalid config format string should fall through to default (Core)
        assert_eq!(ws.resolve_format(), StoryFormat::Core);
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
