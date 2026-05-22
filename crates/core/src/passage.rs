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

/// The ownership layer of a special passage.
///
/// Special passages come from different sources and must be tracked
/// separately to maintain format isolation:
///
/// - **TwineCore**: Compiler constructs defined by the Twee 3 specification
///   that exist regardless of the story format. Includes both name-matched
///   passages (StoryTitle, StoryData, Start) and tag-matched passages
///   (`script`, `stylesheet`). These are format-agnostic.
///
/// - **LegacyCore**: Twine 1 passage names that predate the format system
///   ("stylesheet", "script" as passage NAMES, not tags). Recognized for
///   import/migration compatibility only.
///
/// - **StoryFormat**: Format-specific special passages and tags defined by
///   the active format plugin. SugarCube registers name-matched code passages
///   (StoryInit, PassageHeader) and tag-matched code tags (init, widget).
///   Harlowe registers tag-matched passages (header, footer, startup).
///   The core never hardcodes format-specific names or tags.
///
/// - **UserDefined**: User-created special passages (reserved for future use).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecialPassageLayer {
    /// Twee 3 specification / Twine compiler constructs.
    /// Name-matched: StoryTitle, StoryData, Start.
    /// Tag-matched: script, stylesheet.
    /// Format-agnostic — every story format must handle these.
    TwineCore,
    /// Twine 1 legacy passage names ("stylesheet", "script" as NAMES).
    /// Recognized for import/migration compatibility only.
    LegacyCore,
    /// Format-specific special passages and tags (StoryInit, PassageHeader,
    /// [init], [widget] for SugarCube; [header], [footer], [startup]
    /// for Harlowe). Defined by the active format plugin.
    StoryFormat,
    /// User-defined special passages (not yet implemented).
    UserDefined,
}

impl Default for SpecialPassageLayer {
    fn default() -> Self {
        SpecialPassageLayer::StoryFormat
    }
}

/// The classification category of a passage within the priority hierarchy.
///
/// This enum explicitly represents the 6-level priority order that the
/// classification system uses when matching passages against special passage
/// definitions. Each variant corresponds to a distinct priority level, making
/// it possible to inspect and log classification decisions for debugging.
///
/// ## Priority Order (highest to lowest)
///
/// 1. **CoreMetadata** — StoryData (format detection) and StoryTitle.
///    These are TwineCore name-matched passages with `Metadata` behavior.
///    StoryData must be identified first because it determines which format
///    plugin is active, which in turn affects all subsequent classification.
///
/// 2. **CoreNamed** — Other Twine-core name-matched passages (Start).
///    These are always recognized by name regardless of the active format.
///    A passage named "Start" with `[widget]` is still Start, not a widget.
///
/// 3. **CoreTagged** — Twine-core tag-matched passages ([script], [stylesheet],
///    [style]). These are compiler constructs that apply across all formats.
///    Checked after core name matches so that a passage named "StoryInit"
///    tagged [script] is classified as StoryInit (CoreNamed/FormatNamed),
///    not as a script passage.
///
/// 4. **CoreLegacy** — Twine 1 legacy name-matched passages ("script" and
///    "stylesheet" as passage NAMES, not tags). Import/migration only.
///
/// 5. **FormatNamed** — Format-specific name-matched passages (StoryInit,
///    PassageHeader, StoryCaption, etc.). Singleton passages identified by
///    exact name. The specific set depends on the active format plugin.
///
/// 6. **FormatTagged** — Format-specific tag-matched passages ([init],
///    [widget] for SugarCube; [header], [footer], [startup] for
///    Harlowe). Multiple passages can share a tag.
///
/// 7. **Regular** — User-defined passages with no special definition match.
///    Tag checking happens BEFORE classifying as Regular, ensuring that
///    passages with special tags are never missed.
///
/// ## Usage
///
/// The `Passage::category()` method derives the category from the passage's
/// `special_def` field. The `FormatPlugin::classify_passage_category()`
/// method performs the full priority cascade and returns both the
/// `SpecialPassageDef` (if matched) and the `PassageCategory`.
///
/// ## Downstream Impact
///
/// Handlers that need to distinguish passage types should prefer
/// `passage.category()` over raw `is_special` checks. The category
/// provides more granular information for diagnostics, graph construction,
/// and semantic token generation:
///
/// - **Diagnostics**: `CoreMetadata` and `CoreNamed` passages are always
///   exempt from broken-link, orphan, and dead-end diagnostics.
///   `CoreTagged` passages are also exempt. `FormatTagged` passages with
///   `participates_in_graph: false` should be excluded from graph analysis.
/// - **Graph**: Only passages with `participates_in_graph: true` get graph
///   nodes. The category helps determine which implicit edges to add.
/// - **Semantic tokens**: The category determines the token type
///   (SpecialPassageHeader vs PassageHeader) and layer modifier
///   (TwineCore, StoryFormat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PassageCategory {
    /// Core metadata passages: StoryData, StoryTitle.
    /// Name-matched, TwineCore layer, Metadata behavior.
    /// StoryData is the format-detection entry point.
    CoreMetadata,
    /// Core name-matched passages (non-metadata): Start.
    /// Always recognized by name regardless of format.
    CoreNamed,
    /// Core tag-matched passages: [script], [stylesheet], [style].
    /// Format-agnostic compiler constructs.
    CoreTagged,
    /// Legacy core name-matched: "script"/"stylesheet" as passage NAMES.
    /// Twine 1 import/migration compatibility only.
    CoreLegacy,
    /// Format-specific name-matched: StoryInit, PassageHeader, etc.
    /// Singleton passages. Specific set depends on active format plugin.
    FormatNamed,
    /// Format-specific tag-matched: [init], [widget], [header], etc.
    /// Multiple passages can share a tag.
    FormatTagged,
    /// Regular user-defined passage. No special definition matched.
    /// Tags were checked before this classification, so no special
    /// passages are missed.
    Regular,
}

impl Default for PassageCategory {
    fn default() -> Self {
        PassageCategory::Regular
    }
}

/// Behavior definition for a special passage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecialPassageBehavior {
    /// The passage runs at story startup before the first passage.
    Startup,
    /// The passage runs each time any passage is rendered.
    PassageReady,
    /// The passage provides UI chrome — rendered in the story interface
    /// chrome area, not per-passage. Examples: StoryCaption, StoryBanner,
    /// StoryMenu. These are excluded from reachability analysis and receive
    /// no implicit graph edges. They may still have explicit links extracted
    /// by the format plugin's parser (e.g., `[[links]]` inside StoryCaption),
    /// but those are user-authored references, not structural edges.
    Chrome,
    /// The passage is a **rendering interceptor** — prepended or appended
    /// to every rendered passage body. Examples: PassageHeader (prepended),
    /// PassageFooter (appended). These wrap every user-defined passage
    /// during rendering but are NOT navigation targets. The graph does not
    /// create O(N) edges from interceptors to every user passage; instead,
    /// the analysis engine treats them as always-invoked at render time,
    /// similar to how Startup passages are always invoked at launch time.
    ///
    /// Variable flow: ChromeInterceptor passages can contribute variables
    /// and their variable context should be merged into every passage's
    /// entry state during dataflow analysis (just as Startup's variables
    /// are seeded into the start passage's entry state).
    ChromeInterceptor,
    /// The passage is a **structural template** that defines the HTML shell
    /// for the entire story. Unlike Chrome passages which render content in
    /// predefined slots, a StructureTemplate REPLACES the entire UI structure.
    ///
    /// Key characteristic: StructureTemplate passages can contain explicit
    /// references to user-defined passages through `data-passage` attributes,
    /// `Engine.play()` calls, or other format-specific navigation patterns.
    /// These references are extracted by the format plugin's parser as links
    /// and create graph edges, making the referenced passages reachable.
    ///
    /// Example (SugarCube StoryInterface):
    /// ```html
    /// <div id="story">
    ///   <div id="passage" data-passage></div>
    ///   <div id="sidebar">
    ///     <div data-passage="SidebarStats"></div>
    ///   </div>
    /// </div>
    /// ```
    ///
    /// Here `data-passage="SidebarStats"` creates an explicit edge from
    /// StoryInterface → SidebarStats in the graph, ensuring SidebarStats
    /// is not flagged as unreachable even though it has no `[[links]]`
    /// pointing to it.
    StructureTemplate,
    /// The passage provides metadata only.
    Metadata,
    /// The passage contains global JavaScript injected at startup.
    /// Twine-core concept: the compiled HTML includes this as a <script>
    /// element, not as a named passage in the format engine. However, in
    /// Twee source files, it appears as a tagged passage and the LSP needs
    /// to recognize it. StoryJavaScript contributes variables because
    /// SugarCube's State.variables and other format APIs are accessible
    /// from this context.
    ///
    /// ScriptInjection passages can also contain explicit passage references
    /// through `Engine.play()`, `Engine.goTo()`, or widget definitions that
    /// reference user-defined passages. These are extracted by the format
    /// plugin's `extract_implicit_passage_refs()` and create graph edges.
    ScriptInjection,
    /// The passage contains global CSS injected at startup.
    /// Twine-core concept: analogous to ScriptInjection but for styles.
    StyleInjection,
    /// Custom behavior defined by the format plugin.
    Custom(String),
}

/// How a special passage definition is matched against actual passages.
///
/// The Twee 3 specification distinguishes two matching strategies:
///
/// - **Name-matched**: The passage NAME must exactly match (e.g., "StoryTitle",
///   "StoryData", "StoryInit", "PassageHeader"). These are singleton passages —
///   only one passage with a given name can exist in a story.
///
/// - **Tag-matched**: The passage TAG must match (e.g., `[script]`, `[stylesheet]`,
///   `[init]`, `[widget]`, `[header]`). Multiple passages can share the same tag,
///   and the passage name can be anything. Tweego compiles them in alphabetical
///   order by passage name.
///
/// This distinction is critical for format isolation: SugarCube matches
/// PassageHeader by NAME, while Harlowe matches [header] by TAG. Both achieve
/// the same functional result (content prepended to every passage) but through
/// different mechanisms. The classification system must handle both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchStrategy {
    /// Match by exact passage name (case-sensitive for SugarCube).
    /// Examples: StoryTitle, StoryData, StoryInit, PassageHeader.
    Name,
    /// Match by passage tag (case-insensitive, per Twee 3 spec).
    /// Examples: script, stylesheet, init, widget, header, footer.
    /// Multiple passages can match the same tag.
    Tag,
}

impl Default for MatchStrategy {
    fn default() -> Self {
        MatchStrategy::Name
    }
}

/// Definition of a special passage.
///
/// Special passages have different ownership layers (TwineCore, LegacyCore,
/// StoryFormat, UserDefined) and different matching strategies (Name vs Tag)
/// that determine how they are identified in source files.
///
/// ## Matching Strategy
///
/// - `MatchStrategy::Name`: The `name` field is the canonical passage name
///   that must appear in the passage header (e.g., `:: StoryInit`).
///
/// - `MatchStrategy::Tag`: The `name` field is the canonical TAG name
///   that must appear in the passage's tag block (e.g., `:: MyJS [script]`).
///   The passage name is user-defined and irrelevant for matching.
///
/// ## Workspace Scaffolding
///
/// The `scaffold` field provides metadata for the "Create Workspace" command,
/// allowing the LSP to generate default project skeletons with the correct
/// passage structure for each story format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialPassageDef {
    /// The canonical name for matching.
    ///
    /// - For `MatchStrategy::Name`: the passage name (e.g., "StoryInit").
    /// - For `MatchStrategy::Tag`: the tag name (e.g., "script").
    pub name: String,
    /// How this definition is matched against actual passages.
    #[serde(default)]
    pub match_strategy: MatchStrategy,
    /// The behavior of this special passage.
    pub behavior: SpecialPassageBehavior,
    /// Whether this passage contributes variables to the state.
    pub contributes_variables: bool,
    /// Whether this passage participates in the narrative graph.
    pub participates_in_graph: bool,
    /// Execution priority relative to other special passages (lower = earlier).
    pub execution_priority: Option<i32>,
    /// The ownership layer of this special passage.
    ///
    /// This determines whether the passage is defined by Twine itself
    /// (TwineCore/LegacyCore) or by the active story format (StoryFormat).
    /// Format isolation requires that Twine-core passages are never mixed
    /// into format plugin definitions, and vice versa.
    #[serde(default)]
    pub layer: SpecialPassageLayer,
    /// Workspace scaffolding metadata.
    ///
    /// When present, this definition can be used by the "Create Workspace"
    /// command to generate a default project skeleton. The scaffold provides
    /// the file path convention, default passage name, and initial content.
    #[serde(default)]
    pub scaffold: Option<ScaffoldInfo>,
}

/// Workspace scaffolding metadata for a special passage definition.
///
/// This allows the "Create Workspace" command to generate default project
/// files for each special passage, producing a skeleton like:
///
/// ```text
/// project/
/// ├── story/
/// │   ├── _core_special_passages.twee   (StoryTitle, StoryData)
/// │   ├── _format_special_passages.twee (StoryInit, PassageHeader, etc.)
/// │   ├── script.twee                   (:: Script [script])
/// │   ├── style.twee                    (:: Style [stylesheet])
/// │   └── Start.twee                    (:: Start)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaffoldInfo {
    /// Suggested file name for this passage in a new project.
    /// This is a suggestion — users can organize files however they like.
    /// Example: "script.twee", "style.twee", "_core_special_passages.twee"
    pub file_name: String,
    /// Default passage name to use in the scaffold.
    /// For Name-matched passages, this equals the passage name (e.g., "StoryInit").
    /// For Tag-matched passages, this is a suggested name (e.g., "Script" for [script]).
    pub default_passage_name: String,
    /// Default content for the passage body.
    /// An empty string means the passage body is left empty for the user.
    #[serde(default)]
    pub default_content: String,
}

// ---------------------------------------------------------------------------
// Twine-core special passage definitions
// ---------------------------------------------------------------------------

/// Returns the Twine-core special passage definitions.
///
/// These are format-agnostic constructs defined by the Twee 3 specification
/// and the Twine 2 compiler, not by any story format engine. Every story
/// format must handle these passages — they are not optional.
///
/// ## Format Isolation
///
/// Format plugins must NOT include these passages in their own
/// `special_passages()` lists. The server merges Twine-core definitions
/// with format-specific ones when building the complete special passage
/// registry. This ensures that:
///
/// 1. Twine-core passages are always recognized regardless of format.
/// 2. Format plugins don't duplicate or misinterpret compiler constructs.
/// 3. Diagnostics and graph edges for core passages are consistent.
///
/// ## Matching Strategy
///
/// Core passages use BOTH matching strategies per the Twee 3 spec:
///
/// - **Name-matched** (`MatchStrategy::Name`): `StoryTitle`, `StoryData`,
///   `Start`. These are singleton passages — only one passage with each
///   name can exist in a story.
///
/// - **Tag-matched** (`MatchStrategy::Tag`): `script`, `stylesheet`.
///   Multiple passages can share these tags, and the passage name can be
///   anything. Tweego compiles them in alphabetical order by passage name.
///
/// ## Script & Stylesheet Passages
///
/// In the Twee 3 specification, `script` and `stylesheet` are defined as
/// **special tags**, not special passage names. Any passage tagged
/// `[script]` contains JavaScript; any passage tagged `[stylesheet]`
/// contains CSS. The passage name is user-defined and irrelevant for
/// matching. This is the canonical mechanism in Tweego-based workflows.
///
/// In the compiled HTML, script/stylesheet passages become `<script>` and
/// `<style>` children of `<tw-storydata>`, not named passages in any
/// format's passage store. SugarCube loads them as `tw-user-script-0`
/// and `tw-user-style-0`.
pub fn twine_core_special_passages() -> Vec<SpecialPassageDef> {
    vec![
        // ── Name-matched metadata passages ──────────────────────────────
        SpecialPassageDef {
            name: "StoryTitle".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Metadata,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::TwineCore,
            scaffold: Some(ScaffoldInfo {
                file_name: "_core_special_passages.twee".into(),
                default_passage_name: "StoryTitle".into(),
                default_content: String::new(),
            }),
        },
        SpecialPassageDef {
            name: "StoryData".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Metadata,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::TwineCore,
            scaffold: Some(ScaffoldInfo {
                file_name: "_core_special_passages.twee".into(),
                default_passage_name: "StoryData".into(),
                default_content: r#"{
    "ifid": "",
    "format": "SugarCube",
    "format-version": "2.36.0",
    "start": "Start"
}"#.into(),
            }),
        },
        SpecialPassageDef {
            name: "Start".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Custom("Start".into()),
            contributes_variables: false,
            participates_in_graph: true,
            execution_priority: Some(1000),
            layer: SpecialPassageLayer::TwineCore,
            scaffold: Some(ScaffoldInfo {
                file_name: "Start.twee".into(),
                default_passage_name: "Start".into(),
                default_content: String::new(),
            }),
        },

        // ── Tag-matched code passages ──────────────────────────────────────
        // The Twee 3 spec defines "script" and "stylesheet" as SPECIAL TAGS,
        // not special passage names. Any passage with [script] contains JS;
        // any passage with [stylesheet] contains CSS. The passage name is
        // user-defined and can be anything. Multiple passages can share the
        // same tag. Tweego compiles them in alphabetical order by name.
        SpecialPassageDef {
            name: "script".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::ScriptInjection,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(-1), // Runs before StoryInit
            layer: SpecialPassageLayer::TwineCore,
            scaffold: Some(ScaffoldInfo {
                file_name: "script.twee".into(),
                default_passage_name: "Script".into(),
                default_content: String::new(),
            }),
        },
        SpecialPassageDef {
            name: "stylesheet".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::StyleInjection,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::TwineCore,
            scaffold: Some(ScaffoldInfo {
                file_name: "style.twee".into(),
                default_passage_name: "Style".into(),
                default_content: String::new(),
            }),
        },
        // "style" is a Twee 3 / Tweego alias for "stylesheet". Tweego treats
        // [style] identically to [stylesheet] regardless of the story format.
        // Both tags produce <style> elements in the compiled HTML.
        // This is a core concept, not format-specific.
        SpecialPassageDef {
            name: "style".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::StyleInjection,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::TwineCore,
            scaffold: Some(ScaffoldInfo {
                file_name: "style.twee".into(),
                default_passage_name: "Style".into(),
                default_content: String::new(),
            }),
        },
    ]
}

/// Returns the Twine 1 legacy special passage definitions.
///
/// These predate the Twine 2 format system. They are recognized for
/// import/migration compatibility (Twee imports, Twine archives, Tweego
/// conversions). In Twine 1, "stylesheet" and "script" were passage
/// NAMES (not tags), which is why they appear here as Name-matched
/// definitions rather than Tag-matched.
///
/// **Note**: These are Name-matched because in Twine 1, the passage was
/// literally named "script" or "stylesheet". This differs from Twee 3,
/// where `[script]` and `[stylesheet]` are tags. Both mechanisms are
/// supported — the LSP checks both Name and Tag matching.
pub fn legacy_core_special_passages() -> Vec<SpecialPassageDef> {
    vec![
        SpecialPassageDef {
            name: "stylesheet".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::StyleInjection,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::LegacyCore,
            scaffold: None,
        },
        SpecialPassageDef {
            name: "script".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::ScriptInjection,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(-1),
            layer: SpecialPassageLayer::LegacyCore,
            scaffold: None,
        },
    ]
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
    ///
    /// Uses the classification system as the single source of truth.
    /// Passages named "StoryData" or "StoryTitle" are ALWAYS classified
    /// as `CoreMetadata` by the classification system (they are TwineCore
    /// name-matched with `Metadata` behavior), so the fallback for
    /// unclassified passages is purely defensive.
    pub fn is_metadata(&self) -> bool {
        if let Some(ref def) = self.special_def {
            return matches!(def.behavior, SpecialPassageBehavior::Metadata);
        }
        // Defensive fallback for unclassified passages (should not happen
        // in normal operation — every passage goes through classify_passage()).
        self.name == "StoryData" || self.name == "StoryTitle"
    }

    /// Returns the ownership layer of this passage, if it is a special passage.
    ///
    /// Returns `None` for regular (non-special) passages.
    pub fn special_layer(&self) -> Option<&SpecialPassageLayer> {
        self.special_def.as_ref().map(|d| &d.layer)
    }

    /// Whether this passage is a Twine-core special passage.
    ///
    /// Twine-core passages (StoryTitle, StoryData, Story JavaScript,
    /// Story Stylesheet) are defined by the Twine 2 editor/compiler,
    /// not by any story format engine.
    pub fn is_twine_core(&self) -> bool {
        self.special_def
            .as_ref()
            .map(|d| matches!(d.layer, SpecialPassageLayer::TwineCore))
            .unwrap_or(false)
    }

    /// Whether this passage is a script passage (contains JavaScript).
    ///
    /// Uses the classification system as the single source of truth when
    /// `special_def` is available, falling back to raw tag matching only
    /// when the passage has not been classified (e.g., during incremental
    /// re-parse when tags are unavailable).
    ///
    /// This ensures that legacy name-matched passages (e.g., `:: script`
    /// from Twine 1) are correctly detected when the classification system
    /// identifies them as `ScriptInjection`, even though they lack the
    /// `[script]` tag.
    pub fn is_script_passage(&self) -> bool {
        if let Some(ref def) = self.special_def {
            return matches!(def.behavior, SpecialPassageBehavior::ScriptInjection);
        }
        // Fallback for unclassified passages (e.g., during incremental
        // re-parse when tags are unavailable or not yet classified).
        self.tags.iter().any(|t| t.eq_ignore_ascii_case("script"))
    }

    /// Whether this passage is a stylesheet passage (contains CSS).
    ///
    /// Uses the classification system as the single source of truth when
    /// `special_def` is available, falling back to raw tag matching only
    /// when the passage has not been classified.
    ///
    /// This ensures consistency with the classification system and handles
    /// legacy name-matched passages (e.g., `:: stylesheet` from Twine 1).
    ///
    /// The fallback checks both "stylesheet" and "style" — the latter is
    /// a Twee 3 / Tweego alias that Tweego treats identically regardless
    /// of the story format.
    pub fn is_stylesheet_passage(&self) -> bool {
        if let Some(ref def) = self.special_def {
            return matches!(def.behavior, SpecialPassageBehavior::StyleInjection);
        }
        // Fallback for unclassified passages. Both "stylesheet" and "style"
        // are core Twee tags that mark CSS passages.
        self.tags.iter().any(|t| {
            t.eq_ignore_ascii_case("stylesheet") || t.eq_ignore_ascii_case("style")
        })
    }

    /// Whether this passage is an interface passage (contains HTML).
    ///
    /// Uses the classification system as the single source of truth when
    /// `special_def` is available, checking for `StructureTemplate` behavior.
    /// This replaces the previous hardcoded `self.name == "StoryInterface"`
    /// check, which bypassed the classification system and would incorrectly
    /// match a Harlowe passage named "StoryInterface" (Harlowe doesn't
    /// define StoryInterface as a special passage).
    ///
    /// The fallback to name matching is kept only for unclassified passages
    /// (e.g., during incremental re-parse when the format plugin is not yet
    /// available).
    pub fn is_interface_passage(&self) -> bool {
        if let Some(ref def) = self.special_def {
            return matches!(def.behavior, SpecialPassageBehavior::StructureTemplate);
        }
        // Fallback for unclassified passages
        self.name == "StoryInterface"
    }

    /// Returns the classification category of this passage.
    ///
    /// Derives the category from the `special_def` field, which is assigned
    /// by the format plugin's `classify_passage()` method during parsing.
    /// If no special definition exists, returns `PassageCategory::Regular`.
    ///
    /// This is the preferred way to inspect a passage's classification
    /// for diagnostics, graph construction, and semantic tokens. It provides
    /// more granular information than the boolean `is_special` field.
    pub fn category(&self) -> PassageCategory {
        match &self.special_def {
            None => PassageCategory::Regular,
            Some(def) => {
                match (&def.layer, &def.match_strategy, &def.behavior) {
                    // Core metadata: StoryData, StoryTitle
                    (SpecialPassageLayer::TwineCore, MatchStrategy::Name, SpecialPassageBehavior::Metadata) => {
                        PassageCategory::CoreMetadata
                    }
                    // Core name-matched (non-metadata): Start
                    (SpecialPassageLayer::TwineCore, MatchStrategy::Name, _) => {
                        PassageCategory::CoreNamed
                    }
                    // Core tag-matched: [script], [stylesheet], [style]
                    (SpecialPassageLayer::TwineCore, MatchStrategy::Tag, _) => {
                        PassageCategory::CoreTagged
                    }
                    // Legacy core name-matched: "script"/"stylesheet" as passage NAMES
                    (SpecialPassageLayer::LegacyCore, MatchStrategy::Name, _) => {
                        PassageCategory::CoreLegacy
                    }
                    // Legacy core tag-matched (unlikely but handle it)
                    (SpecialPassageLayer::LegacyCore, MatchStrategy::Tag, _) => {
                        PassageCategory::CoreTagged
                    }
                    // Format-specific name-matched: StoryInit, PassageHeader, etc.
                    (SpecialPassageLayer::StoryFormat, MatchStrategy::Name, _) => {
                        PassageCategory::FormatNamed
                    }
                    // Format-specific tag-matched: [init], [widget], [header], etc.
                    (SpecialPassageLayer::StoryFormat, MatchStrategy::Tag, _) => {
                        PassageCategory::FormatTagged
                    }
                    // User-defined special passages (future)
                    (SpecialPassageLayer::UserDefined, _, _) => {
                        PassageCategory::FormatNamed // Treat as format-level for now
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── "style" tag classification regression tests ────────────────────

    #[test]
    fn test_style_tag_classified_as_style_injection() {
        // Verify that the "style" tag is in the core special passages
        let core = twine_core_special_passages();
        let style_def = core.iter().find(|d| d.name == "style" && d.match_strategy == MatchStrategy::Tag);
        assert!(style_def.is_some(), "[style] tag should be in twine_core_special_passages()");
        let def = style_def.unwrap();
        assert_eq!(def.behavior, SpecialPassageBehavior::StyleInjection);
        assert_eq!(def.layer, SpecialPassageLayer::TwineCore);
    }

    #[test]
    fn test_stylesheet_tag_classified_as_style_injection() {
        let core = twine_core_special_passages();
        let def = core.iter().find(|d| d.name == "stylesheet" && d.match_strategy == MatchStrategy::Tag);
        assert!(def.is_some(), "[stylesheet] tag should be in twine_core_special_passages()");
        assert_eq!(def.unwrap().behavior, SpecialPassageBehavior::StyleInjection);
    }

    #[test]
    fn test_is_stylesheet_passage_with_style_tag() {
        // A passage with [style] tag should be detected as stylesheet
        let def = SpecialPassageDef {
            name: "MyCSS".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::StyleInjection,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::TwineCore,
            scaffold: None,
        };
        let mut passage = Passage::new_special("MyCSS".into(), 0..100, def);
        passage.tags = vec!["style".into()];
        assert!(passage.is_stylesheet_passage(), "Passage with [style] tag should be stylesheet");
    }

    #[test]
    fn test_is_stylesheet_passage_with_stylesheet_tag() {
        let def = SpecialPassageDef {
            name: "MyCSS".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::StyleInjection,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::TwineCore,
            scaffold: None,
        };
        let mut passage = Passage::new_special("MyCSS".into(), 0..100, def);
        passage.tags = vec!["stylesheet".into()];
        assert!(passage.is_stylesheet_passage(), "Passage with [stylesheet] tag should be stylesheet");
    }

    #[test]
    fn test_is_stylesheet_passage_fallback_style_tag() {
        // When special_def is None (unclassified), fallback should recognize "style"
        let mut passage = Passage::new("MyCSS".into(), 0..100);
        passage.tags = vec!["style".into()];
        assert!(passage.is_stylesheet_passage(), "Fallback should recognize [style] tag");
    }

    #[test]
    fn test_is_stylesheet_passage_fallback_stylesheet_tag() {
        let mut passage = Passage::new("MyCSS".into(), 0..100);
        passage.tags = vec!["stylesheet".into()];
        assert!(passage.is_stylesheet_passage(), "Fallback should recognize [stylesheet] tag");
    }

    #[test]
    fn test_is_stylesheet_passage_fallback_case_insensitive() {
        let mut passage = Passage::new("MyCSS".into(), 0..100);
        passage.tags = vec!["STYLE".into()];
        assert!(passage.is_stylesheet_passage(), "Fallback should be case-insensitive for [STYLE] tag");

        passage.tags = vec!["StyleSheet".into()];
        assert!(passage.is_stylesheet_passage(), "Fallback should be case-insensitive for [StyleSheet] tag");
    }

    #[test]
    fn test_is_script_passage_with_script_tag() {
        let def = SpecialPassageDef {
            name: "MyJS".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::ScriptInjection,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(-1),
            layer: SpecialPassageLayer::TwineCore,
            scaffold: None,
        };
        let mut passage = Passage::new_special("MyJS".into(), 0..100, def);
        passage.tags = vec!["script".into()];
        assert!(passage.is_script_passage(), "Passage with [script] tag should be script");
    }

    #[test]
    fn test_normal_passage_not_stylesheet() {
        let passage = Passage::new("Forest".into(), 0..100);
        assert!(!passage.is_stylesheet_passage(), "Normal passage should not be stylesheet");
        assert!(!passage.is_script_passage(), "Normal passage should not be script");
    }

    #[test]
    fn test_style_and_stylesheet_are_distinct_entries() {
        // Both "style" and "stylesheet" should exist as separate entries
        let core = twine_core_special_passages();
        let style_count = core.iter()
            .filter(|d| d.match_strategy == MatchStrategy::Tag && d.behavior == SpecialPassageBehavior::StyleInjection)
            .count();
        assert_eq!(style_count, 2, "Should have both [style] and [stylesheet] tag entries");
    }

    // ── PassageCategory tests ──────────────────────────────────────────

    #[test]
    fn test_regular_passage_category() {
        let passage = Passage::new("Forest".into(), 0..100);
        assert_eq!(passage.category(), PassageCategory::Regular);
    }

    #[test]
    fn test_core_metadata_category_storydata() {
        let def = SpecialPassageDef {
            name: "StoryData".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Metadata,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::TwineCore,
            scaffold: None,
        };
        let passage = Passage::new_special("StoryData".into(), 0..100, def);
        assert_eq!(passage.category(), PassageCategory::CoreMetadata);
        assert!(passage.is_metadata());
    }

    #[test]
    fn test_core_metadata_category_storytitle() {
        let def = SpecialPassageDef {
            name: "StoryTitle".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Metadata,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::TwineCore,
            scaffold: None,
        };
        let passage = Passage::new_special("StoryTitle".into(), 0..100, def);
        assert_eq!(passage.category(), PassageCategory::CoreMetadata);
        assert!(passage.is_metadata());
    }

    #[test]
    fn test_core_named_category_start() {
        let def = SpecialPassageDef {
            name: "Start".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Custom("Start".into()),
            contributes_variables: false,
            participates_in_graph: true,
            execution_priority: Some(1000),
            layer: SpecialPassageLayer::TwineCore,
            scaffold: None,
        };
        let passage = Passage::new_special("Start".into(), 0..100, def);
        assert_eq!(passage.category(), PassageCategory::CoreNamed);
        assert!(!passage.is_metadata());
    }

    #[test]
    fn test_core_tagged_category_script() {
        let def = SpecialPassageDef {
            name: "MyJS".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::ScriptInjection,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(-1),
            layer: SpecialPassageLayer::TwineCore,
            scaffold: None,
        };
        let passage = Passage::new_special("MyJS".into(), 0..100, def);
        assert_eq!(passage.category(), PassageCategory::CoreTagged);
        assert!(passage.is_script_passage());
    }

    #[test]
    fn test_core_tagged_category_stylesheet() {
        let def = SpecialPassageDef {
            name: "MyCSS".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::StyleInjection,
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::TwineCore,
            scaffold: None,
        };
        let passage = Passage::new_special("MyCSS".into(), 0..100, def);
        assert_eq!(passage.category(), PassageCategory::CoreTagged);
        assert!(passage.is_stylesheet_passage());
    }

    #[test]
    fn test_format_named_category_storyinit() {
        let def = SpecialPassageDef {
            name: "StoryInit".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::Startup,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(0),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        };
        let passage = Passage::new_special("StoryInit".into(), 0..100, def);
        assert_eq!(passage.category(), PassageCategory::FormatNamed);
    }

    #[test]
    fn test_format_tagged_category_widget() {
        let def = SpecialPassageDef {
            name: "MyWidget".into(),
            match_strategy: MatchStrategy::Tag,
            behavior: SpecialPassageBehavior::Custom("Widget".into()),
            contributes_variables: false,
            participates_in_graph: false,
            execution_priority: None,
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        };
        let passage = Passage::new_special("MyWidget".into(), 0..100, def);
        assert_eq!(passage.category(), PassageCategory::FormatTagged);
    }

    #[test]
    fn test_is_interface_passage_uses_classification() {
        // StoryInterface with StructureTemplate behavior
        let def = SpecialPassageDef {
            name: "StoryInterface".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::StructureTemplate,
            contributes_variables: false,
            participates_in_graph: true,
            execution_priority: Some(107),
            layer: SpecialPassageLayer::StoryFormat,
            scaffold: None,
        };
        let passage = Passage::new_special("StoryInterface".into(), 0..100, def);
        assert!(passage.is_interface_passage());
        assert_eq!(passage.category(), PassageCategory::FormatNamed);
    }

    #[test]
    fn test_is_interface_passage_fallback() {
        // Without special_def, fallback to name check
        let passage = Passage::new("StoryInterface".into(), 0..100);
        assert!(passage.is_interface_passage());
    }

    #[test]
    fn test_legacy_core_category() {
        let def = SpecialPassageDef {
            name: "script".into(),
            match_strategy: MatchStrategy::Name,
            behavior: SpecialPassageBehavior::ScriptInjection,
            contributes_variables: true,
            participates_in_graph: false,
            execution_priority: Some(-1),
            layer: SpecialPassageLayer::LegacyCore,
            scaffold: None,
        };
        let passage = Passage::new_special("script".into(), 0..100, def);
        assert_eq!(passage.category(), PassageCategory::CoreLegacy);
    }
}
