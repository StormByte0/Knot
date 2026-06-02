//! Shared types for format plugins.
//!
//! These types are used by both format plugin implementations and the LSP
//! server handlers. They define the data structures for macro catalogs,
//! variable sigils, implicit passage patterns, state variable tracking,
//! and other format-specific behavioral data that handlers need to query
//! through the FormatPlugin trait.
//!
//! ## Variable Tracking Architecture
//!
//! SugarCube variables (`$var`) are NOT traditional scoped variables — they
//! are persistent entries in `SugarCube.State.variables` that survive for the
//! entire game session. Once a variable is written (via `<<set>>`,
//! `State.variables.x =`, or a JS alias), it remains in the state collection
//! indefinitely. The `<<unset>>` macro can remove a variable, but this is rare.
//!
//! This means the traditional "uninitialized variable" / "definite assignment
//! analysis" approach is **wrong** for SugarCube. Instead, we use a
//! **state variable registry** with **graph-BFS availability computation**:
//!
//! 1. **Registry**: Collect all `$var` / `State.variables.*` references across
//!    the workspace into a `StateVariable` registry.
//! 2. **Availability**: Use the passage graph to compute which passages can
//!    reach a variable's first write. If a read occurs in a passage that is
//!    NOT reachable from any write (via graph traversal), it's flagged as
//!    a **hint** (not an error), since the variable might come from a saved
//!    game state.
//! 3. **Properties**: Dot-notation paths (`$player.name`) are tracked as
//!    first-class properties of their base state variable.
//! 4. **JS Aliasing**: `State.variables.x` and `var v = State.variables; v.x`
//!    are unified with `$x` references.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

// ---------------------------------------------------------------------------
// Virtual document types
// ---------------------------------------------------------------------------

/// The kind of a virtual document section.
///
/// Script passages are concatenated into a single unified section (they share
/// both scope and deterministic execution order at startup). Macro passages
/// are translated to JS but kept as individual sections (they share scope
/// with scripts but execute non-deterministically based on player choices).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualSectionKind {
    /// All `[script]` passages concatenated in SugarCube execution order.
    /// These execute at startup in a deterministic sequence, sharing a single
    /// JS scope. This section is the source of startup alias definitions.
    UnifiedScript,
    /// A single macro passage translated to JS via `OperatorNormalization`.
    /// These execute when the player visits the passage — non-deterministic
    /// order, but they share the same JS scope as the script section.
    MacroTranslated {
        /// The original passage name (before translation to JS).
        passage_name: String,
    },
}

/// A section within a virtual document.
///
/// Each section corresponds to one or more source passages and contains
/// JavaScript text (either original JS from `[script]` passages, or
/// translated JS from macro passages). The section tracks which passage
/// and which original line each virtual line came from.
#[derive(Debug, Clone)]
pub struct VirtualSection {
    /// The kind of this section (unified script or translated macro).
    pub kind: VirtualSectionKind,
    /// The JavaScript source text of this section.
    pub source_text: String,
    /// Line-level mapping from virtual line number (0-based, relative to
    /// this section's start) to the original source passage and line.
    pub line_map: Vec<LineMapping>,
}

/// Maps a virtual document line back to its original source location.
///
/// This is the critical piece that enables "go to definition" and hover
/// from virtual document analysis results back to the actual source files.
#[derive(Debug, Clone)]
pub struct LineMapping {
    /// The passage name where this line originated.
    pub passage_name: String,
    /// The file URI where this passage lives.
    pub file_uri: String,
    /// The 0-based line number within the original source file.
    pub original_line: u32,
}

/// An alias extracted from the startup script section.
///
/// In SugarCube, JavaScript in `[script]` passages can create aliases to
/// `State.variables` or to specific state properties. These aliases persist
/// for the entire game session and are used by both script and macro passages.
/// The virtual document's startup alias table captures these so that macro
/// sections can resolve them.
#[derive(Debug, Clone)]
pub struct StartupAlias {
    /// The alias identifier (e.g., `g` for `var g = gs()`).
    pub alias_name: String,
    /// What this alias resolves to.
    pub resolution: AliasResolution,
    /// The virtual line number (0-based) where this alias is defined.
    pub defined_at_line: u32,
}

/// What a startup alias resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasResolution {
    /// The alias points to the entire `State.variables` object.
    /// (e.g., `var v = State.variables` or `var g = gs()`)
    StateVariables,
    /// The alias points to a specific property of `State.variables`.
    /// (e.g., `var profiles = State.variables.uiProfiles`)
    StateVariableProperty {
        /// The base variable name without `$` sigil.
        base_name: String,
        /// Optional dot-path after the base name.
        property_path: Option<String>,
    },
    /// The alias points to a known SugarCube getter function.
    /// (e.g., `reg` from `var reg = State.variables.reg` or a custom
    /// `function(name) { return State.variables[name]; }` pattern)
    GetterFunction,
}

/// The virtual document — a unified JS representation of all passage code.
///
/// This is the foundation for cross-passage variable tracking. It contains:
/// - A unified script section (all `[script]` passages concatenated)
/// - Individual macro sections (each macro passage translated to JS)
/// - A startup alias table (extracted from the script section)
/// - Line mappings back to original source locations
///
/// The virtual document enables:
/// 1. **Deep alias resolution**: `gs()` in scripts → `State.variables` →
///    `g.x` in macros resolves to `State.variables.x`
/// 2. **Macro-to-JS translation**: `<<set $x to 5>>` → `State.variables.x = 5`
///    using `OperatorNormalization`
/// 3. **Unified path-centric analysis**: All sections produce normalized
///    access paths that feed into the same reference counter
#[derive(Debug, Clone)]
pub struct VirtualDocument {
    /// All sections in the virtual document, in order.
    /// The unified script section comes first (if any script passages exist),
    /// followed by individual macro sections.
    pub sections: Vec<VirtualSection>,
    /// The startup alias table, extracted from the unified script section.
    /// These aliases are available in ALL sections (both script and macro)
    /// because SugarCube's JS scope is shared across the entire game session.
    pub startup_aliases: Vec<StartupAlias>,
    /// User-defined callables (custom macros and widgets) extracted from
    /// the workspace. These are used by the translator to recognize
    /// `<<macroName args>>` invocations as function calls.
    pub user_callables: Vec<UserCallable>,
}

// ---------------------------------------------------------------------------
// User-defined callable types (custom macros & widgets)
// ---------------------------------------------------------------------------

/// The kind of a user-defined callable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCallableKind {
    /// Custom macro defined via `Macro.add('name', { handler: function() { ... } })`.
    CustomMacro,
    /// Widget defined via `<<widget name>>...<</widget>>` in a [widget]-tagged passage.
    Widget,
}

/// A user-defined callable (custom macro or widget) that can be invoked
/// like a function from macro passages.
///
/// Custom macros are defined in `[script]` passages using SugarCube's
/// `Macro.add()` API. Widgets are defined in `[widget]`-tagged passages
/// using `<<widget name>>...<</widget>>`.
///
/// When the translator encounters `<<useItem matchbox>>`, it looks up
/// `useItem` in the user callables and translates it to a function call:
/// `useItem(matchbox);`. The arguments are mapped positionally to the
/// `this.args[N]` references in the handler body (for custom macros) or
/// the `<<args>>` / `$args` references in the widget body.
#[derive(Debug, Clone)]
pub struct UserCallable {
    /// The callable name (e.g., "useItem" for `Macro.add('useItem', ...)`).
    pub name: String,
    /// The kind of callable (custom macro or widget).
    pub kind: UserCallableKind,
    /// Number of arguments this callable accepts, if known.
    /// For custom macros, derived from `this.args[N]` usage in the handler.
    /// For widgets, defaults to variadic (None) unless explicitly annotated.
    pub arg_count: Option<usize>,
    /// The passage name where this callable is defined.
    pub defined_in: String,
    /// The file URI where this callable is defined.
    pub file_uri: String,
    /// The 0-based line number where this callable is defined.
    pub defined_at_line: u32,
    /// The body of the callable's handler/widget code (for analysis of
    /// variable effects). For custom macros, this is the `handler` function
    /// body. For widgets, this is the content between `<<widget>>` and
    /// `<</widget>>`.
    pub body: Option<String>,
}

/// Minimal passage info passed to the `extract_user_callables` hook.
///
/// The core virtual document builder collects this information from all
/// passages and passes it to the format plugin so it can detect custom
/// macro definitions (in script passages) and widget definitions (in
/// widget-tagged passages).
#[derive(Debug, Clone)]
pub struct PassageInfo {
    /// The passage name.
    pub name: String,
    /// The file URI where this passage lives.
    pub file_uri: String,
    /// The passage tags (e.g., ["script"], ["widget"]).
    pub tags: Vec<String>,
    /// The passage body text.
    pub body_text: String,
}

// ---------------------------------------------------------------------------
// Macro catalog types
// ---------------------------------------------------------------------------

/// The kind of a macro argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroArgKind {
    Expression,
    String,
    Selector,
    Variable,
}

/// Describes a single argument in a macro's signature.
#[derive(Debug, Clone)]
pub struct MacroArgDef {
    /// Position (0-based) in the macro's argument list.
    pub position: usize,
    /// Display label for signature help.
    pub label: &'static str,
    /// Whether this argument accepts a passage name reference.
    pub is_passage_ref: bool,
    /// Whether this argument accepts a CSS selector.
    pub is_selector: bool,
    /// Whether this argument accepts a variable name ($var or _var).
    pub is_variable: bool,
    /// Whether this argument is required.
    pub is_required: bool,
    /// Argument kind: expression, string, selector, or variable.
    pub kind: MacroArgKind,
}

/// Macro category for filtering and organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroCategory {
    Control,
    Variables,
    Output,
    Dom,
    Links,
    Forms,
    Navigation,
    Timing,
    Widgets,
    Audio,
}

impl std::fmt::Display for MacroCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MacroCategory::Control => write!(f, "control"),
            MacroCategory::Variables => write!(f, "variables"),
            MacroCategory::Output => write!(f, "output"),
            MacroCategory::Dom => write!(f, "dom"),
            MacroCategory::Links => write!(f, "links"),
            MacroCategory::Forms => write!(f, "forms"),
            MacroCategory::Navigation => write!(f, "navigation"),
            MacroCategory::Timing => write!(f, "timing"),
            MacroCategory::Widgets => write!(f, "widgets"),
            MacroCategory::Audio => write!(f, "audio"),
        }
    }
}

/// A format-specific macro definition entry.
#[derive(Debug, Clone)]
pub struct MacroDef {
    pub name: &'static str,
    pub description: &'static str,
    pub has_body: bool,
    /// Argument signature definitions. If None, the macro takes arbitrary args.
    pub args: Option<&'static [MacroArgDef]>,
    /// Whether this macro is deprecated.
    pub deprecated: bool,
    /// Deprecation message if deprecated.
    pub deprecation_message: Option<&'static str>,
    /// Category for filtering.
    pub category: MacroCategory,
    /// If this macro must be inside a parent macro.
    pub container: Option<&'static str>,
    /// If this macro must be inside one of several parent macros.
    pub container_any_of: Option<&'static [&'static str]>,
}

/// A property or method of a builtin global object.
#[derive(Debug, Clone)]
pub struct GlobalProperty {
    /// The property/method name (e.g., "variables", "save()").
    pub name: &'static str,
    /// Human-readable description of the property/method.
    pub description: &'static str,
    /// Whether this is a method (ends with `()`) or a property.
    pub is_method: bool,
}

/// A format-specific builtin global object definition.
#[derive(Debug, Clone)]
pub struct GlobalDef {
    pub name: &'static str,
    pub description: &'static str,
    /// Properties/methods of this global object for dot-notation completion.
    pub properties: Option<&'static [GlobalProperty]>,
}

/// A lightweight completion/hover entry for macro signatures.
pub struct MacroSignature {
    pub name: &'static str,
    pub signature: String,
    pub description: &'static str,
    pub has_params: bool,
    pub deprecated: bool,
}

impl MacroSignature {
    /// Return the snippet portion after the macro name (for insertion).
    pub fn insert_snippet(&self) -> &'static str {
        if self.has_params {
            " ${1:args}"
        } else {
            ""
        }
    }

    /// Return parameter names for signature help.
    pub fn param_names(&self) -> Vec<String> {
        if self.signature.is_empty() {
            return vec![];
        }
        self.signature
            .split(',')
            .map(|s| s.trim().to_string())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Implicit passage reference pattern
// ---------------------------------------------------------------------------

/// A pattern for detecting implicit passage references in raw text/HTML/JS.
///
/// Different formats have different APIs that reference passages indirectly.
/// For example, SugarCube has `Engine.play("passage")` and `data-passage="passage"`.
#[derive(Debug, Clone)]
pub struct ImplicitPassagePattern {
    /// Human-readable description of the pattern.
    pub description: &'static str,
    /// The regex pattern string (compiled lazily by the format plugin).
    pub pattern: &'static str,
}

// ---------------------------------------------------------------------------
// Variable sigil info
// ---------------------------------------------------------------------------

/// Information about a variable sigil character (e.g., `$` or `_` in SugarCube).
#[derive(Debug, Clone)]
pub struct VariableSigilInfo {
    /// The sigil character.
    pub sigil: char,
    /// Human-readable description (e.g., "SugarCube story variable").
    pub description: &'static str,
}

// ---------------------------------------------------------------------------
// Operator normalization
// ---------------------------------------------------------------------------

/// A mapping from a format-specific operator to its JavaScript equivalent.
#[derive(Debug, Clone)]
pub struct OperatorNormalization {
    pub from: &'static str,
    pub to: &'static str,
}

// ---------------------------------------------------------------------------
// Format behavior query result
// ---------------------------------------------------------------------------

/// Result of querying format-specific behavioral data for variable string
/// map building. Each format knows how to extract "variable → known string
/// values" from its own syntax.
pub struct VarStringMapResult {
    /// Map of variable name → set of known string literal values.
    pub map: HashMap<String, Vec<String>>,
}

/// A resolved dynamic navigation link.
pub struct ResolvedNavLink {
    /// Display text for the edge (may include "via $var" info).
    pub display_text: Option<String>,
    /// The target passage name.
    pub target: String,
    /// A format-provided hint about the semantic edge type.
    /// Set during dynamic navigation link resolution when the format
    /// plugin knows the edge semantics (e.g., SugarCube's <<goto $var>>
    /// resolves to a Jump edge, <<include $var>> to an Include edge).
    pub edge_type_hint: Option<knot_core::graph::EdgeType>,
}

// ---------------------------------------------------------------------------
// State variable tracking types
// ---------------------------------------------------------------------------

/// The kind of access to a state variable.
///
/// This replaces the core `VarKind::Init/Read` with format-specific granularity
/// that captures property paths and the distinction between base-level and
/// property-level access.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VarAccessKind {
    /// Variable is being assigned/written (e.g., `<<set $hp to 100>>`,
    /// `State.variables.hp = 100`, `v.hp = 100` via alias).
    Assign,
    /// Variable is being read (e.g., `You have $gold coins.`,
    /// `State.variables.gold`, `v.gold` via alias).
    Read,
    /// A property of the variable is being read
    /// (e.g., `$player.name`, `State.variables.player.name`).
    /// The `path` is the dot-notation path after the base name (e.g., "name").
    PropertyRead { path: String },
    /// A property of the variable is being written
    /// (e.g., `<<set $player.name to "Alice">>`,
    /// `State.variables.player.name = "Alice"`).
    /// The `path` is the dot-notation path after the base name.
    PropertyWrite { path: String },
    /// Variable is being unset (e.g., `<<unset $hp>>`).
    /// This is rare but explicitly removes the variable from state.
    Unset,
}

/// A location where a state variable is accessed.
#[derive(Debug, Clone)]
pub struct VarLocation {
    /// The passage name where this access occurs.
    pub passage_name: String,
    /// The file URI where this access occurs.
    pub file_uri: String,
    /// The byte range of the variable reference in the source text.
    pub span: Range<usize>,
    /// The kind of access (assign, read, property read/write, unset).
    pub kind: VarAccessKind,
}

/// A state variable tracked across the workspace.
///
/// In SugarCube, `$var` is syntactic sugar for `SugarCube.State.variables.var`.
/// These variables persist for the entire game session once written. This struct
/// tracks all known information about a single state variable across all passages.
#[derive(Debug, Clone)]
pub struct StateVariable {
    /// The base name without the `$` sigil (e.g., "hp" for `$hp`).
    pub base_name: String,
    /// The dollar-prefixed name (e.g., "$hp").
    pub dollar_name: String,
    /// Known dot-notation property paths seen on this variable
    /// (e.g., {"name", "health"} for `$player.name`, `$player.health`).
    pub known_properties: HashSet<String>,
    /// All locations where this variable is written/assigned.
    pub write_locations: Vec<VarLocation>,
    /// All locations where this variable is read.
    pub read_locations: Vec<VarLocation>,
    /// The passage name where this variable first becomes available
    /// (computed via graph-BFS from the start passage and special passages).
    /// `None` means availability hasn't been computed yet.
    pub first_available: Option<String>,
    /// Whether this variable is seeded by a special passage
    /// (e.g., StoryInit) or a script-tagged passage. Variables seeded by
    /// special/script passages are always available from the start of the game.
    pub seeded_by_special: bool,
}

/// A diagnostic produced by format-specific variable analysis.
///
/// These diagnostics use **hint** severity rather than error/warning, because
/// SugarCube variables are persistent game state — a "read before write" in
/// source order doesn't mean the variable is actually unavailable at runtime
/// (it could come from a saved game, a browser session, or a JS script that
/// the LSP doesn't fully model).
#[derive(Debug, Clone)]
pub struct VariableDiagnostic {
    /// The passage name this diagnostic is associated with.
    pub passage_name: String,
    /// The file URI where the diagnostic should be reported.
    pub file_uri: String,
    /// The diagnostic kind.
    pub kind: VariableDiagnosticKind,
    /// Human-readable message.
    pub message: String,
}

/// Kinds of variable diagnostics a format plugin can produce.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VariableDiagnosticKind {
    /// A variable is read in a passage that is not reachable from any
    /// passage that writes it via the narrative graph. This is a **hint**,
    /// not an error, because the variable might exist from a saved game.
    VariableAvailabilityHint,
    /// A variable is written but never read on any reachable path.
    /// This is a **hint** since "unused" state variables are common
    /// (e.g., debug variables, state saved for future use).
    UnusedVariableHint,
    /// A variable is assigned twice in the same passage without an
    /// intervening read. This is a **hint** — it's often intentional
    /// (e.g., overwriting a default).
    RedundantWriteHint,
    /// A property path is accessed that hasn't been seen written anywhere.
    /// (e.g., `$player.mana` is read but never written).
    UnknownPropertyHint,
}

// ---------------------------------------------------------------------------
// Shape-aware property map types
// ---------------------------------------------------------------------------

/// The structural kind of a variable or property, inferred from assignment
/// patterns and usage across the workspace.
///
/// This enables the completion handler to offer different completions for
/// arrays (`.length`, `.push()`) vs objects (child properties) vs scalars
/// (no dot completions).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PropertyKind {
    /// A scalar value (number, string, boolean). No child properties.
    Scalar,
    /// An object with named child properties (e.g., `$player` with `.name`, `.hp`).
    Object,
    /// An array with indexed elements. Element shape may be known via `element_shape`.
    Array,
    /// Kind could not be determined (no assignment patterns found).
    Unknown,
}

/// An entry in the shape-aware property map, describing the structural kind
/// and immediate children of a variable path.
///
/// This is produced by `FormatPlugin::build_shape_aware_property_map()` and
/// consumed by the completion handler for dot-notation completion. It enriches
/// the basic `HashSet<String>` from `build_object_property_map()` with type
/// information that allows distinguishing arrays from objects from scalars.
#[derive(Debug, Clone)]
pub struct PropertyMapEntry {
    /// The structural kind of this variable/property path.
    pub kind: PropertyKind,
    /// Immediate child property names (e.g., `["name", "hp"]` for `$player`).
    /// Empty for scalars and arrays (arrays use `element_shape` instead).
    pub children: Vec<String>,
    /// For arrays: the shape of each element, if known.
    /// Contains a virtual `PropertyMapEntry` representing `[*]` — the
    /// common structure across all observed array elements.
    /// `None` for non-array types or arrays with unknown element shape.
    pub element_shape: Option<Box<PropertyMapEntry>>,
}

// ---------------------------------------------------------------------------
// Variable tree types (format-agnostic)
// ---------------------------------------------------------------------------

/// A simplified usage location for tree output — just passage and file info.
///
/// This is the format-agnostic version of `VarLocation`. Format plugins
/// produce these from their format-specific internal types when building
/// the variable tree for the UI. The server translates these to LSP wire
/// types without needing to understand format-specific access kinds.
#[derive(Debug, Clone)]
pub struct VariableUsageLocation {
    /// The passage name where this usage occurs.
    pub passage_name: String,
    /// The file URI where this usage occurs.
    pub file_uri: String,
    /// Whether this is a write (true) or read (false).
    pub is_write: bool,
    /// The 0-based line number within the file where this usage occurs.
    /// Enables "goto" navigation to a specific line within a passage,
    /// not just the passage header. Defaults to 0 when not yet computed.
    pub line: u32,
}

/// A tree-structured variable node for display in the variable tracker UI.
///
/// Format plugins build these trees from their format-specific state models.
/// The tree structure mirrors the runtime state hierarchy of the format.
/// For example, SugarCube's `$player.hp` maps to `State.variables.player.hp`,
/// so `$player` becomes a `VariableTreeNode` with a `.hp` child property.
///
/// Other formats can produce their own tree structures that reflect their
/// runtime state model — the server and UI never need to know format-specific
/// details.
#[derive(Debug, Clone)]
pub struct VariableTreeNode {
    /// Display name (e.g., "$player", "$gold").
    pub name: String,
    /// State path for display (e.g., "State.variables.player").
    /// Format-specific — each format decides how to represent the path.
    pub state_path: String,
    /// Whether this variable is temporary (per-passage only).
    pub is_temporary: bool,
    /// Passages where this variable is written (base-level only, not properties).
    pub written_in: Vec<VariableUsageLocation>,
    /// Passages where this variable is read (base-level only, not properties).
    pub read_in: Vec<VariableUsageLocation>,
    /// Whether this variable is definitely initialized from the start
    /// (e.g., via StoryInit in SugarCube, or setup code in other formats).
    pub initialized_at_start: bool,
    /// Whether this variable is never read (unused write).
    pub is_unused: bool,
    /// Known child properties, forming a tree that mirrors the runtime
    /// state hierarchy (e.g., `$player.name`, `$player.hp` are children
    /// of `$player`). Each property may itself have sub-properties.
    pub properties: Vec<VariablePropertyNode>,
    /// The structural kind of this variable: scalar, object, array, or unknown.
    /// Inferred from assignment patterns (e.g., `<<set $var to {}>>` → Object).
    pub kind: PropertyKind,
    /// For Array-kind root variables: the shape of each array element.
    /// Contains a virtual `VariablePropertyNode` representing the `[*]` element
    /// structure. `None` for non-array root variables or arrays with unknown
    /// element shape.
    pub element_shape: Option<Box<VariablePropertyNode>>,
}

/// A property node in the variable tree, reflecting the hierarchical structure
/// of the format's runtime state model.
///
/// For SugarCube, `$player.inventory.sword` would be represented as:
/// - `$player` (VariableTreeNode)
///   - `.inventory` (VariablePropertyNode)
///     - `.sword` (VariablePropertyNode, child of inventory)
///
/// Each format produces its own property tree structure. The server and UI
/// are completely format-agnostic — they just render the tree.
#[derive(Debug, Clone)]
pub struct VariablePropertyNode {
    /// The property name without the parent path (e.g., "name", "hp").
    pub name: String,
    /// The full display name (e.g., "$player.name", "$player.hp").
    pub full_name: String,
    /// The full state path (e.g., "State.variables.player.name").
    /// Format-specific — each format decides how to represent the path.
    pub state_path: String,
    /// The 0-based line number within the file where this property usage occurs.
    /// Enables "goto" navigation to a specific line within a passage.
    /// Defaults to 0 when not yet computed.
    pub line: u32,
    /// Passages where this property is written.
    pub written_in: Vec<VariableUsageLocation>,
    /// Passages where this property is read.
    pub read_in: Vec<VariableUsageLocation>,
    /// Sub-properties (e.g., for `$player.inventory.sword`, the `inventory`
    /// property would have `sword` as a sub-property).
    pub properties: Vec<VariablePropertyNode>,
    /// The structural kind of this property: scalar, object, array, or unknown.
    /// Inferred from assignment patterns.
    pub kind: PropertyKind,
    /// For array-kind properties: the shape of each array element.
    /// Contains a virtual `VariablePropertyNode` representing the `[*]` element
    /// structure. `None` for non-array properties or arrays with unknown element shape.
    pub element_shape: Option<Box<VariablePropertyNode>>,
    /// For `[*]` property nodes: coverage annotation for irregular arrays.
    /// Format: "present_in/total" (e.g., "3/5" means property exists in 3 of 5 elements).
    /// `None` for non-array properties or regular arrays (100% coverage).
    /// Only set when coverage < 100%.
    pub coverage: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-passage virtual doc types (Phase C)
// ---------------------------------------------------------------------------

/// A single entry in the virtual document's line map, mapping a JS output
/// line back to its original source position within a passage body.
///
/// This is the format-agnostic version of `ExactLineMapping` from
/// `walk_translate.rs`. It is produced by `FormatPlugin::virtual_doc_line_map()`
/// and consumed by the LSP handler for diagnostic routing.
#[derive(Debug, Clone)]
pub struct VirtualDocLineMapEntry {
    /// The passage name that this line belongs to.
    /// For preamble lines (JSDoc + const declaration), this is empty.
    pub passage_name: String,
    /// The file URI where this passage lives.
    pub file_uri: String,
    /// The 0-based line number within the original passage body
    /// (offset from passage header, NOT global). 0 for preamble lines
    /// and function header/footer lines that have no direct source mapping.
    pub original_line: u32,
}

// ---------------------------------------------------------------------------
// Passage variable reference types (format-agnostic)
// ---------------------------------------------------------------------------

/// A variable reference (read or write) within a specific passage.
///
/// This is the format-agnostic representation produced by
/// `FormatPlugin::extract_passage_variable_refs()`. The server converts
/// these directly to LSP wire types without any format-specific logic.
///
/// The `line` field comes from the virtual document's `LineMapping`,
/// which maps virtual document line numbers back to original source file
/// line numbers — this is the "deref index" from virtual docs to normal
/// files that enables showing exact read/write lines in the UI.
#[derive(Debug, Clone)]
pub struct PassageVarRef {
    /// The variable name in format-specific notation (e.g., "$gold",
    /// "$player.name" in SugarCube). The server passes this through
    /// without interpretation.
    pub variable_name: String,
    /// Whether this is a write (true) or read (false).
    pub is_write: bool,
    /// The 0-based line number within the original source file.
    /// Derived from the virtual document's line map, which maps
    /// virtual line positions back to original source locations.
    pub line: u32,
    /// The file URI containing this reference.
    pub file_uri: String,
    /// The passage name where this reference occurs.
    pub passage_name: String,
}
