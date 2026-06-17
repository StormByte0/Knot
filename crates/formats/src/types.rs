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

/// Whether a macro can have a body (content between open and close tags).
///
/// The tree builder uses this to determine how to handle an open macro tag
/// that has no matching close tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyRequirement {
    /// Always inline — no body, no close tag expected.
    /// Examples: `<<set>>`, `<<print>>`, `<<goto>>`, `<<run>>`, `<<unset>>`
    Never,

    /// Always block — body is required, close tag is expected.
    /// Unclosed blocks produce a diagnostic.
    /// Examples: `<<if>>`, `<<for>>`, `<<switch>>`, `<<widget>>`, `<<link>>`
    Required,
}

/// The structural kind of a macro — determines its role in the macro tree.
///
/// This classification drives completion filtering, close-tag behavior,
/// and sub-macro scope enforcement. It is orthogonal to `BodyRequirement`:
///
/// | MacroKind     | body        | container/any_of | Examples                        |
/// |---------------|-------------|------------------|---------------------------------|
/// | Container     | Required    | None             | `if`, `for`, `link`, `widget`   |
/// | Inline        | Never       | None             | `set`, `goto`, `print`, `audio` |
/// | SubMacro      | Never       | Some             | `else`, `break`, `case`, `next` |
///
/// Container macros always need a closing tag. Inline macros never need one.
/// Sub-macros are only valid inside their parent container(s) — they are
/// filtered from top-level completions when the cursor is outside a valid
/// parent block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroKind {
    /// A macro that opens a closeable body section — always needs `<</name>>`.
    /// Examples: `<<if>>`, `<<for>>`, `<<link>>`, `<<button>>`, `<<widget>>`
    Container,

    /// A standalone macro that never has a body or close tag.
    /// Examples: `<<set>>`, `<<goto>>`, `<<print>>`, `<<audio>>`, `<<remove>>`
    Inline,

    /// A macro only valid inside a specific parent container.
    /// The `container` / `container_any_of` field on `MacroDef` specifies
    /// which parent(s) are valid.
    /// Examples: `<<else>>` (inside `<<if>>`), `<<break>>` (inside `<<for>>`),
    /// `<<case>>` (inside `<<switch>>`), `<<next>>` (inside `<<timed>>`)
    SubMacro,
}

/// A format-specific macro definition entry.
#[derive(Debug, Clone)]
pub struct MacroDef {
    pub name: &'static str,
    pub description: &'static str,
    /// Whether this macro can have a body (content between open and close tags).
    ///
    /// Determines how the tree builder handles an open macro with no close tag:
    /// - `Never`: inline macro, no children expected
    /// - `Required`: always block — unclosed blocks get a diagnostic
    pub body: BodyRequirement,
    /// The structural kind of this macro — Container, Inline, or SubMacro.
    ///
    /// This drives completion filtering (SubMacro items are hidden from
    /// top-level completions unless the cursor is inside a valid parent),
    /// close-tag behavior, and sort ordering.
    pub kind: MacroKind,
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
// Multi-form macro completion
// ---------------------------------------------------------------------------

/// A single completion form for a macro.
///
/// Many SugarCube macros are polymorphic — the same macro name can be used
/// with different argument counts and with/without a closing tag. Each form
/// produces a separate completion item so the user can choose the right variant.
///
/// For example, `<<link>>` has 5 forms:
/// 1. `<<link "label">>` — 1-arg, inline click handler (no navigation)
/// 2. `<<link "label">>…<</link>>` — 1-arg, block click handler
/// 3. `<<link "label" "passage">>` — 2-arg, inline navigation
/// 4. `<<link "label" "passage">>…<</link>>` — 2-arg, block navigation
/// 5. `<<link "label" "passage" "tooltip">>` — 3-arg, with tooltip
///
/// The `snippet` field contains the body **after** `<<` and is used as
/// `insertText` with `InsertTextFormat.Snippet`. For block macros, the
/// snippet includes the `>>`, body tabstop, and closing tag.
#[derive(Debug, Clone)]
pub struct MacroCompletionForm {
    /// Display label for the completion list (e.g., `<<link "label" "passage">>…<</link>>`).
    pub label: &'static str,
    /// Human-readable description of this specific form.
    pub detail: &'static str,
    /// Snippet body (placed after `<<`). For block macros, includes `>>`, newlines,
    /// body tabstop, and closing tag. Uses raw-string conventions from `snippets.rs`.
    pub snippet: &'static str,
    /// Sort priority within the same macro. Lower = higher priority (shown first).
    /// The final `sortText` is `1_{name}_{priority}` for builtins.
    pub sort_priority: u8,
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
    /// The byte span of the access within the passage body (passage-body-relative).
    /// `None` when span data is not available.
    pub span: Option<std::ops::Range<usize>>,
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
// Passage variable reference types (format-agnostic)
// ---------------------------------------------------------------------------

/// A variable reference (read or write) within a specific passage.
///
/// This is the format-agnostic representation produced by
/// `FormatPlugin::extract_passage_variable_refs()`. The server converts
/// these directly to LSP wire types without any format-specific logic.
///
/// The `line` field is computed from the passage tree walk, which maps
/// variable references back to their exact source line numbers — enabling
/// the UI to show precise read/write locations.
#[derive(Debug, Clone)]
pub struct PassageVarRef {
    /// The variable name in format-specific notation (e.g., "$gold",
    /// "$player.name" in SugarCube). The server passes this through
    /// without interpretation.
    pub variable_name: String,
    /// Whether this is a write (true) or read (false).
    pub is_write: bool,
    /// The 0-based line number within the original source file.
    /// Derived from the passage tree walk, which maps variable
    /// references to their exact source line numbers.
    pub line: u32,
    /// The file URI containing this reference.
    pub file_uri: String,
    /// The passage name where this reference occurs.
    pub passage_name: String,
}

// ---------------------------------------------------------------------------
// Format Registry trait (template for all format plugins)
// ---------------------------------------------------------------------------

/// Trait defining the standard registry interface that any story format
/// must provide for runtime-populated data.
///
/// This trait serves as the **functional template** for implementing
/// registries across all supported formats (SugarCube, Harlowe, Snowman,
/// Chapbook). Each format implements this trait with its own sub-registries,
/// but the interface remains consistent so that LSP handlers can query
/// any format's registry through the same methods.
///
/// ## Design Principles
///
/// 1. **Format isolation**: Handlers never import format-specific types.
///    They query registries through `FormatPlugin` trait methods, which
///    delegate to the active format's `FormatRegistry` implementation.
///
/// 2. **Categorized sub-registries**: Each format organizes its runtime
///    data into categories (variables, macros, functions, templates, etc.).
///    The format decides which categories it supports — not all formats
///    need all categories.
///
/// 3. **Incremental updates**: All registries support `remove_file()` and
///    `remove_passage()` for incremental re-parse without full workspace
///    re-indexing.
///
/// ## Implementing for a New Format
///
/// When adding Harlowe, Snowman, or Chapbook support:
///
/// 1. Identify the format's runtime categories (e.g., Harlowe has datatypes,
///    macros, and variables; Snowman has `window.*` globals and passages)
/// 2. Create a sub-registry for each category with this standard interface
/// 3. Compose them into a unified `FormatNameRegistry` hub
/// 4. Implement `FormatRegistry` on the hub
/// 5. Wire up through `FormatPlugin` trait accessor methods
///
/// ## Registry Categories (SugarCube example)
///
/// | Category | SugarCube Source | Harlowe Equivalent |
/// |----------|-----------------|-------------------|
/// | Variables | `$var`, `State.variables.*` | `$var` (datamap entries) |
/// | Custom Macros | `<<widget>>`, `Macro.add()` | Custom macro definitions |
/// | Functions | JS `function` in `[script]` | N/A (no JS scripting) |
/// | Templates | `Template.add()` | N/A |
pub trait FormatRegistry: Send + Sync {
    // ── Lifecycle ─────────────────────────────────────────────────────

    /// Remove all entries for a specific file from all sub-registries.
    fn remove_file(&mut self, file_uri: &str);

    /// Remove all entries for a specific passage from all sub-registries.
    fn remove_passage(&mut self, passage_name: &str);

    /// Clear all sub-registries (for full workspace re-parse).
    fn clear(&mut self);

    // ── Variable queries ──────────────────────────────────────────────

    /// Get all variable names across the workspace (for completion).
    fn variable_names(&self) -> HashSet<String>;

    /// Get known property paths for a variable (for dot-notation completion).
    fn variable_properties(&self, var_name: &str) -> HashSet<String>;

    // ── Custom definition queries ─────────────────────────────────────

    /// Get all custom macro/definition names (for completion).
    fn custom_definition_names(&self) -> Vec<String>;

    /// Get all function names discovered in script passages (for completion).
    fn function_names(&self) -> Vec<String> {
        Vec::new() // Default: not all formats have JS functions
    }

    /// Get all template names (for completion, format-specific prefix included).
    fn template_names(&self) -> Vec<String> {
        Vec::new() // Default: not all formats have templates
    }
}

// ---------------------------------------------------------------------------
// Function and Template discovery types (format-agnostic)
// ---------------------------------------------------------------------------

/// A function definition discovered during JS analysis, in format-agnostic form.
///
/// Produced by format plugins that support JS scripting (SugarCube, Snowman)
/// for completion and hover. Formats without JS scripting return empty lists.
#[derive(Debug, Clone)]
pub struct FunctionDefInfo {
    /// The function name.
    pub name: String,
    /// The passage where this function is defined.
    pub defined_in: String,
    /// The file URI where this function is defined.
    pub file_uri: String,
    /// The byte offset of the definition within the file.
    pub defined_at_offset: usize,
    /// The number of parameters (if known).
    pub param_count: Option<usize>,
}

/// A template definition discovered during JS analysis, in format-agnostic form.
///
/// Produced by format plugins that support template systems (SugarCube's
/// `Template.add()` API) for completion and hover.
#[derive(Debug, Clone)]
pub struct TemplateDefInfo {
    /// The template name (without the invocation prefix).
    pub name: String,
    /// The passage where this template is defined.
    pub defined_in: String,
    /// The file URI where this template is defined.
    pub file_uri: String,
    /// The byte offset of the definition within the file.
    pub defined_at_offset: usize,
}

// ---------------------------------------------------------------------------
// Completion context (format-owned)
// ---------------------------------------------------------------------------

/// The kind of completion context at a cursor position, as determined by the
/// active format plugin.
///
/// This enum is the **single source of truth** for what completions to offer.
/// The completion handler does NOT hardcode trigger-character routing — it
/// calls `FormatPlugin::resolve_completion_context()` which returns one of
/// these variants, and the handler simply maps the variant to the appropriate
/// LSP completion items.
///
/// ## Design principle
///
/// All format-specific context detection lives in the format plugin. The
/// handler is a thin dispatcher that:
/// 1. Calls `plugin.resolve_completion_context()` with the cursor position
///    and workspace data
/// 2. Matches on the returned `CompletionContext` variant
/// 3. Builds LSP `CompletionItem` lists using only format-agnostic data
///    (workspace passage names, plugin-provided variable lists, etc.)
///
/// This ensures that adding a new format (Harlowe, Chapbook, Snowman) only
/// requires implementing `resolve_completion_context()` — the handler code
/// doesn't change.
#[derive(Debug, Clone)]
pub enum CompletionContext {
    /// Cursor is on a variable reference or just typed a variable sigil.
    ///
    /// - `$` trigger in SugarCube → variables starting with `$`
    /// - `_` trigger in SugarCube → temporary variables starting with `_`
    /// - Cursor on an existing variable span
    Variable {
        /// The variable name at the cursor (including sigil), or empty if
        /// the user just typed the sigil character.
        name: String,
        /// Whether this is a temporary/scratch variable.
        is_temporary: bool,
    },

    /// Cursor is on a passage link target (e.g., `[[Forest]]`).
    Link {
        /// The current link target text, or empty if the user just typed `[`.
        target: String,
    },

    /// Cursor is on a passage-ref arg inside a macro
    /// (e.g., `"Shop"` in `<<link "Talk" "Shop">>`).
    MacroPassageRef {
        /// The current passage name in the arg, or empty.
        target: String,
        /// The macro name containing this passage-ref arg.
        macro_name: String,
        /// Whether this macro invocation has a body block.
        has_body: bool,
    },

    /// Cursor is on a macro name or typing a new macro name
    /// (e.g., `if` in `<<if ...>>`, or typing after `<<`).
    MacroName {
        /// The partial macro name typed so far, or empty if just after `<<`.
        name: String,
    },

    /// Cursor is inside a macro opening tag but not on the name or a
    /// specific passage-ref arg.
    MacroInterior {
        /// The macro name whose interior the cursor is in.
        name: String,
    },

    /// Cursor is on a global object namespace (e.g., `State`, `Story`).
    Namespace {
        /// The namespace name.
        name: String,
    },

    /// Cursor is on a property access (e.g., `.variables` in `State.variables`).
    Property {
        /// The namespace/object name (e.g., "State").
        object_name: String,
        /// The property name being accessed, if the cursor is past the dot.
        property_name: Option<String>,
    },

    /// Cursor is on a passage header name.
    PassageHeader {
        /// The passage name.
        name: String,
    },

    /// Cursor is in a context where close-tag completion is appropriate
    /// (e.g., just typed `<</`).
    CloseTag {
        /// The partial close-tag name typed so far (may be empty).
        partial: String,
    },

    /// Cursor is on a variable with a dot trigger, requesting dot-notation
    /// property completion (e.g., `$player.` → offer `.name`, `.hp`).
    VariableDot {
        /// The variable path up to the dot (e.g., "$player" or "$player.state").
        path: String,
    },

    /// No specific context recognized. The handler should offer a sensible
    /// default (e.g., passage names) or return no completions.
    Other,
}

// ---------------------------------------------------------------------------
// Format completion types
// ---------------------------------------------------------------------------

/// A completion item kind, independent of the LSP type system.
///
/// Maps to `lsp_types::CompletionItemKind` by the handler. Using our own
/// enum keeps `knot-formats` independent of `lsp_types`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatCompletionKind {
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

/// Insert text format (plain text or snippet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatInsertTextFormat {
    PlainText,
    Snippet,
}

/// A text edit for a completion item, specifying a range to replace.
#[derive(Debug, Clone)]
pub struct FormatTextEdit {
    /// Start line (0-based).
    pub start_line: u32,
    /// Start character (0-based, UTF-16).
    pub start_character: u32,
    /// End line (0-based).
    pub end_line: u32,
    /// End character (0-based, UTF-16).
    pub end_character: u32,
    /// The new text to insert.
    pub new_text: String,
}

/// A format-agnostic completion item.
///
/// The format plugin builds these directly — it owns ALL context detection
/// and completion item construction. The handler just maps them to
/// `lsp_types::CompletionItem`. This follows the legacy TypeScript adapter
/// pattern where `provideFormatCompletions()` returns `CompletionItem[]`.
#[derive(Debug, Clone)]
pub struct FormatCompletionItem {
    /// The label (display text) for this completion.
    pub label: String,
    /// The kind of completion item.
    pub kind: FormatCompletionKind,
    /// Human-readable detail text (shown in the completion popup).
    pub detail: Option<String>,
    /// Sort text for ordering in the completion list.
    pub sort_text: Option<String>,
    /// Filter text for matching against user input.
    pub filter_text: Option<String>,
    /// Text to insert (may be a snippet if `insert_text_format` is Snippet).
    pub insert_text: Option<String>,
    /// Insert text format (plain text or snippet).
    pub insert_text_format: FormatInsertTextFormat,
    /// Optional text edit that replaces a specific range.
    ///
    /// When present, the handler converts this to a `lsp_types::TextEdit`
    /// and sets it as the completion item's `text_edit`. This is more
    /// reliable than relying on the editor's word-boundary detection,
    /// especially for `<<macro` patterns where the `<<` delimiters
    /// confuse word boundaries.
    pub text_edit: Option<FormatTextEdit>,
    /// Whether this item is deprecated.
    pub deprecated: bool,
    /// Whether to preselect this item in the completion list.
    pub preselect: bool,
    /// Opaque data for the resolve handler (JSON value).
    pub data: Option<serde_json::Value>,
    /// Commit characters that trigger acceptance of this completion.
    pub commit_characters: Vec<String>,
}

// ---------------------------------------------------------------------------
// Hover type (format-owned, mirroring FormatCompletionItem)
// ---------------------------------------------------------------------------

/// A hover result produced by a format plugin.
///
/// Mirrors `FormatCompletionItem` in spirit: the format plugin owns all
/// hover-content construction, and the server handler is a thin dispatcher
/// that maps this to `lsp_types::Hover`.
///
/// The `range` is a document-absolute byte range that the server handler
/// converts to an LSP `Range` via `helpers::byte_range_to_lsp_range`.
#[derive(Debug, Clone)]
pub struct FormatHover {
    /// Markdown content of the hover popup.
    pub contents: String,
    /// Document-absolute byte range that the hover applies to.
    /// When `None`, the editor uses the word at the cursor position.
    pub range: Option<std::ops::Range<usize>>,
}
