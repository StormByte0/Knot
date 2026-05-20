//! Passage model — the fundamental unit of narrative structure.
//!
//! A passage represents a single named section of a Twine story. Passages
//! contain text blocks, links to other passages, and variable operations.

use serde::{Deserialize, Serialize};
use std::ops::Range;

/// A story format identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StoryFormat {
    SugarCube,
    Harlowe,
    Chapbook,
    Snowman,
}

impl StoryFormat {
    /// Returns the default format when none is specified.
    pub fn default_format() -> Self {
        StoryFormat::SugarCube
    }

    /// Whether cross-passage variable tracking is fully supported.
    #[deprecated(
        since = "2.0.0",
        note = "Use FormatPlugin::supports_full_variable_tracking() instead"
    )]
    pub fn supports_full_variable_tracking(&self) -> bool {
        matches!(self, StoryFormat::SugarCube | StoryFormat::Snowman)
    }

    /// Whether variable tracking is partially supported.
    #[deprecated(
        since = "2.0.0",
        note = "Use FormatPlugin::supports_partial_variable_tracking() instead"
    )]
    pub fn supports_partial_variable_tracking(&self) -> bool {
        matches!(self, StoryFormat::Harlowe)
    }
}

impl std::fmt::Display for StoryFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoryFormat::SugarCube => write!(f, "SugarCube"),
            StoryFormat::Harlowe => write!(f, "Harlowe"),
            StoryFormat::Chapbook => write!(f, "Chapbook"),
            StoryFormat::Snowman => write!(f, "Snowman"),
        }
    }
}

impl std::str::FromStr for StoryFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sugarcube" => Ok(StoryFormat::SugarCube),
            "harlowe" => Ok(StoryFormat::Harlowe),
            "chapbook" => Ok(StoryFormat::Chapbook),
            "snowman" => Ok(StoryFormat::Snowman),
            other => Err(format!("Unsupported story format: {}", other)),
        }
    }
}

/// A link from one passage to another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// The display text of the link (may differ from target passage name).
    pub display_text: Option<String>,
    /// The target passage name this link points to.
    pub target: String,
    /// The byte range of this link in the source text.
    pub span: Range<usize>,
}

/// The kind of variable operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VarKind {
    /// Variable is being read.
    Read,
    /// Variable is being initialized/assigned.
    Init,
}

/// A variable operation within a passage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarOp {
    /// The variable name, including its format-specific sigil
    /// (e.g., `$gold` for SugarCube story variables, `gold` for Snowman).
    pub name: String,
    /// Whether this is a read or write operation.
    pub kind: VarKind,
    /// The byte range of this operation in the source text.
    pub span: Range<usize>,
    /// Whether this is a temporary/scratch variable that does not persist
    /// across passage transitions. Format plugins set this flag based on
    /// their own variable scoping rules (e.g., SugarCube's `_temp` convention).
    /// Temporary variables are excluded from cross-passage dataflow analysis
    /// since they only exist within a single passage/moment.
    #[serde(default)]
    pub is_temporary: bool,
}

/// A content block within a passage body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Block {
    /// Plain text content.
    Text { content: String, span: Range<usize> },
    /// A macro invocation (format-specific).
    Macro { name: String, args: String, span: Range<usize> },
    /// An inline expression.
    Expression { content: String, span: Range<usize> },
    /// A heading or section divider.
    Heading { content: String, span: Range<usize> },
    /// An incomplete or malformed block (excluded from graph analysis).
    Incomplete { content: String, span: Range<usize> },
}

/// Behavior definition for a special passage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecialPassageBehavior {
    /// The passage runs at story startup before the first passage.
    Startup,
    /// The passage runs each time any passage is rendered.
    PassageReady,
    /// The passage provides UI chrome (excluded from reachability).
    Chrome,
    /// The passage provides metadata only.
    Metadata,
    /// Custom behavior defined by the format plugin.
    Custom(String),
}

/// Definition of a format-specific special passage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialPassageDef {
    /// The canonical passage name (e.g., "StoryInit").
    pub name: String,
    /// The behavior of this special passage.
    pub behavior: SpecialPassageBehavior,
    /// Whether this passage contributes variables to the state.
    pub contributes_variables: bool,
    /// Whether this passage participates in the narrative graph.
    pub participates_in_graph: bool,
    /// Execution priority relative to other special passages (lower = earlier).
    pub execution_priority: Option<i32>,
}

/// A passage — the fundamental unit of narrative structure in a Twine story.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Passage {
    /// The passage name (used as its identifier and link target).
    pub name: String,
    /// Tags assigned to this passage.
    pub tags: Vec<String>,
    /// The byte range of the entire passage in the source document.
    pub span: Range<usize>,
    /// Content blocks within the passage body.
    pub body: Vec<Block>,
    /// Links from this passage to other passages.
    pub links: Vec<Link>,
    /// Variable operations within this passage.
    pub vars: Vec<VarOp>,
    /// Whether this passage is a format-specific special passage.
    pub is_special: bool,
    /// If this is a special passage, its definition from the format plugin.
    pub special_def: Option<SpecialPassageDef>,
    /// The (x, y) position of this passage in the Twine editor canvas.
    ///
    /// When a Twine story is saved, each passage records its canvas
    /// position. This is parsed from the passage header metadata JSON
    /// block (e.g., `:: Name [tags] {"position":"100,200"}`) or from
    /// the `StoryData` JSON `position` field. If no position is recorded,
    /// this is `None` and the graph view will use an automatic layout.
    #[serde(default)]
    pub position: Option<(f64, f64)>,
}

impl Passage {
    /// Create a new regular (non-special) passage.
    pub fn new(name: String, span: Range<usize>) -> Self {
        Self {
            name,
            tags: Vec::new(),
            span,
            body: Vec::new(),
            links: Vec::new(),
            vars: Vec::new(),
            is_special: false,
            special_def: None,
            position: None,
        }
    }

    /// Create a new special passage with the given definition.
    pub fn new_special(name: String, span: Range<usize>, def: SpecialPassageDef) -> Self {
        Self {
            name,
            tags: Vec::new(),
            span,
            body: Vec::new(),
            links: Vec::new(),
            vars: Vec::new(),
            is_special: true,
            special_def: Some(def),
            position: None,
        }
    }

    /// Returns true if this passage participates in narrative flow (graph edges).
    pub fn participates_in_graph(&self) -> bool {
        if self.is_special {
            self.special_def
                .as_ref()
                .map(|d| d.participates_in_graph)
                .unwrap_or(false)
        } else {
            true
        }
    }

    /// Returns true if this passage contributes variable state.
    pub fn contributes_variables(&self) -> bool {
        if self.is_special {
            self.special_def
                .as_ref()
                .map(|d| d.contributes_variables)
                .unwrap_or(false)
        } else {
            !self.vars.is_empty()
        }
    }

    /// Returns the names of all passages this passage links to.
    pub fn link_targets(&self) -> impl Iterator<Item = &str> {
        self.links.iter().map(|l| l.target.as_str())
    }

    /// Returns all variable init operations in this passage.
    pub fn variable_inits(&self) -> impl Iterator<Item = &VarOp> {
        self.vars.iter().filter(|v| v.kind == VarKind::Init)
    }

    /// Returns all variable read operations in this passage.
    pub fn variable_reads(&self) -> impl Iterator<Item = &VarOp> {
        self.vars.iter().filter(|v| v.kind == VarKind::Read)
    }

    /// Returns all persistent (non-temporary) variable init operations.
    /// Temporary variables (those with `is_temporary: true`) are excluded
    /// because they do not survive passage transitions.
    pub fn persistent_variable_inits(&self) -> impl Iterator<Item = &VarOp> {
        self.vars.iter().filter(|v| v.kind == VarKind::Init && !v.is_temporary)
    }

    /// Returns all persistent (non-temporary) variable read operations.
    pub fn persistent_variable_reads(&self) -> impl Iterator<Item = &VarOp> {
        self.vars.iter().filter(|v| v.kind == VarKind::Read && !v.is_temporary)
    }

    /// Returns all variable operations sorted by source position (span start).
    /// This is essential for intra-passage dataflow analysis where the
    /// order of operations matters (e.g., write before read within a passage).
    pub fn vars_sorted_by_span(&self) -> Vec<&VarOp> {
        let mut sorted: Vec<&VarOp> = self.vars.iter().collect();
        sorted.sort_by_key(|v| v.span.start);
        sorted
    }

    /// Whether this is a universal metadata passage (StoryData or StoryTitle).
    pub fn is_metadata(&self) -> bool {
        self.name == "StoryData" || self.name == "StoryTitle"
    }

    /// Whether this passage is a script passage (contains JavaScript).
    ///
    /// Script passages are identified by their **tag** `[script]`, not by
    /// their passage name. In SugarCube/Twine 2, "Story JavaScript" is a
    /// Twine editor concept — the engine loads it via `<script>` elements
    /// in the compiled HTML, not as a named passage. In Twee source files,
    /// any passage tagged `[script]` is treated as JavaScript (e.g.,
    /// `:: MyScript[script]`).
    ///
    /// The tag comparison is case-insensitive to match Twine's behavior
    /// (Twine normalizes tags to lowercase). The passage name is **not**
    /// checked because SugarCube is case-sensitive about passage names —
    /// there is no canonical "Story JavaScript" passage in the engine.
    pub fn is_script_passage(&self) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case("script"))
    }

    /// Whether this passage is a stylesheet passage (contains CSS).
    ///
    /// Stylesheet passages are identified by their **tag** `[stylesheet]`,
    /// not by their passage name. Same reasoning as `is_script_passage()` —
    /// "Story Stylesheet" is a Twine editor concept, not a SugarCube
    /// engine passage name. In Twee source files, any passage tagged
    /// `[stylesheet]` is treated as CSS (e.g., `:: MyCSS[stylesheet]`).
    ///
    /// The tag comparison is case-insensitive to match Twine's behavior.
    pub fn is_stylesheet_passage(&self) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case("stylesheet"))
    }

    /// Whether this passage is an interface passage (contains HTML).
    ///
    /// Only the exact passage name "StoryInterface" qualifies. SugarCube
    /// is case-sensitive about passage names, so this uses exact matching
    /// (not case-insensitive).
    pub fn is_interface_passage(&self) -> bool {
        self.name == "StoryInterface"
    }
}

// NOTE: The `is_story_javascript()` and `is_story_stylesheet()` helper
// functions have been removed. These previously matched passage names
// case-insensitively with optional whitespace (e.g., "Story JavaScript",
// "StoryJavascript", "story javascript"). This was incorrect because:
//
// 1. SugarCube is case-sensitive — passage names must match exactly.
// 2. "Story JavaScript" and "Story Stylesheet" are Twine 2 editor
//    concepts, not SugarCube engine passage names. In the compiled HTML,
//    they become `<script>`/`<style>` elements, not named passages.
// 3. In Twee source files, script/stylesheet passages are identified by
//    their [script]/[stylesheet] tags, not by their passage name.
//
// Script/stylesheet detection is now handled entirely by tag matching
// in `is_script_passage()` and `is_stylesheet_passage()`.
