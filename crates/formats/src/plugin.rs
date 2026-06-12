//! Format Plugin Trait
//!
//! Defines the interface that all format plugins must implement. The core engine
//! is format-agnostic and consumes only normalized data exposed through this trait.
//!
//! ## Architecture
//!
//! The trait has two categories of methods:
//!
//! 1. **Parsing methods** — Core parsing of source text into passages, tokens,
//!    and diagnostics. Every format must implement these.
//!
//! 2. **Behavioral methods** — Format-specific data for completion, hover,
//!    validation, dynamic navigation, and variable tracking. These have default
//!    (no-op) implementations so formats only need to override what they support.
//!
//! The behavioral methods are the Rust equivalent of the former TypeScript
//! format-specific adapter pattern. They ensure format isolation: handlers
//! query the active format plugin instead of hardcoding format-specific logic.

use crate::types::*;
use knot_core::passage::{
    Passage, PassageCategory, SpecialPassageDef, StoryFormat,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use url::Url;

// ---------------------------------------------------------------------------
// SourceTextProvider — allows format plugins to resolve byte offsets to lines
// ---------------------------------------------------------------------------

/// A trait that provides source text for documents by URI.
///
/// The server implements this using its `open_documents` cache, making
/// document text available to format plugins for byte-offset → line-number
/// resolution. Without this, format plugins could only return `line: 0`
/// for variable usage locations because the `Workspace` does not store
/// source text.
///
/// This is passed through `build_variable_tree()` so that variable usage
/// locations in the variable flow UI can navigate to exact source lines
/// instead of just the passage header.
pub trait SourceTextProvider {
    /// Look up the source text of a document by its URI string.
    /// Returns `None` if the document is not available (e.g., not yet indexed).
    fn get_source_text(&self, file_uri: &str) -> Option<&str>;
}

/// A no-op `SourceTextProvider` that always returns `None`.
///
/// Used when the caller doesn't have source text available (e.g., during
/// testing or when the format plugin is used outside the LSP server).
pub struct NoSourceText;

impl SourceTextProvider for NoSourceText {
    fn get_source_text(&self, _file_uri: &str) -> Option<&str> {
        None
    }
}

/// A semantic token produced by a format plugin.
///
/// The `start` field is a **passage-relative** byte offset: byte 0 is the
/// `::` prefix of the passage header. This design enables incremental
/// passage updates — when a single passage is edited, only that passage's
/// token group needs to be regenerated, and the passage's document offset
/// is applied at the LSP boundary to produce document-absolute positions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticToken {
    /// The byte offset where the token starts, **relative to the passage
    /// head** (the `::` prefix of the passage header).
    ///
    /// To convert to a document-absolute byte offset, add the
    /// `PassageTokenGroup::passage_offset` value.
    pub start: usize,
    /// The length of the token in bytes.
    pub length: usize,
    /// The token type (e.g., "macro", "variable", "link", "string").
    pub token_type: SemanticTokenType,
    /// Optional modifier (e.g., "deprecated", "definition").
    pub modifier: Option<SemanticTokenModifier>,
}

/// A group of semantic tokens for a single passage, with passage-relative
/// byte offsets.
///
/// All `SemanticToken::start` values in this group are relative to the
/// passage head (the `::` prefix of the passage header). To convert to
/// document-absolute byte offsets, add `passage_offset` to each token's
/// `start`.
///
/// This design enables incremental passage updates: when a single passage
/// is edited, only that passage's token group needs to be regenerated.
/// The `passage_offset` is updated when the passage's position in the
/// document changes (e.g., due to edits in a preceding passage).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassageTokenGroup {
    /// The passage name (for lookup during incremental updates).
    pub passage_name: String,
    /// Byte offset of the passage head (`::` prefix) in the document.
    ///
    /// Adding this to any token's `start` gives a document-absolute byte
    /// offset.
    pub passage_offset: usize,
    /// Semantic tokens with passage-relative byte offsets.
    pub tokens: Vec<SemanticToken>,
}

/// Types of semantic tokens a format plugin can produce.
///
/// Each variant maps to a distinct entry in the LSP semantic token legend,
/// giving themes fine-grained control over how each construct is colored.
/// The legend order is defined in `lifecycle.rs` and the mapping in
/// `semantic.rs` — all three must stay in sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticTokenType {
    // ── Passage structure ───────────────────────────────────────────
    /// The `::` prefix on a regular (user-defined) passage header.
    PassageHeader,
    /// The passage name on a regular (user-defined) passage header.
    /// Distinct from the `::` prefix so themes can color the name
    /// differently from the prefix.
    PassageName,
    /// The passage name inside a `[[link]]` construct.
    Link,
    /// An implicit passage reference inside a macro or API call
    /// (e.g., the passage name string in `<<goto "Forest">>`,
    /// `Engine.play("Forest")`, `data-passage="Forest"`).
    PassageRef,
    /// The `::` prefix on a special passage header.
    SpecialPassageHeader,
    /// The passage name on a special passage header
    /// (e.g., "StoryInit", "StoryCaption").
    SpecialPassage,
    /// A tag in a passage header `[tag1 tag2]`.
    Tag,

    // ── Code constructs ─────────────────────────────────────────────
    /// A macro invocation name (e.g., `if`, `set`, `link` in SugarCube).
    Macro,
    /// A widget/function definition name.
    Function,
    /// A variable reference (e.g., `$storyVar`, `_tempVar`).
    Variable,
    /// A format-specific keyword (e.g., `to`, `is`, `eq`, `neq`, `gt`,
    /// `lt`, `gte`, `lte`, `and`, `or`, `not`).
    Keyword,
    /// A boolean literal (`true`, `false`).
    Boolean,
    /// A number literal.
    Number,
    /// A string literal.
    String,
    /// A comment (block or line).
    Comment,
    /// A format-specific operator that is not a keyword
    /// (e.g., `+=`, `-=`, assignment shorthand).
    Operator,

    // ── Object model ────────────────────────────────────────────────
    /// A global object/namespace (e.g., `State`, `Engine`, `Story`,
    /// `Dialog`, `settings` in SugarCube).
    Namespace,
    /// A property access on an object (e.g., `.variables` on `State`,
    /// `.passage` on `Story`).
    Property,
}

/// Modifiers for semantic tokens.
///
/// Each variant maps to a bit position in the LSP modifier bitset.
/// The legend order is defined in `lifecycle.rs` and the mapping in
/// `semantic.rs` — all three must stay in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticTokenModifier {
    /// This token is a definition (not just a reference).
    /// LSP modifier: `definition`
    Definition,
    /// This token is read-only (cannot be modified).
    /// LSP modifier: `readonly`
    ReadOnly,
    /// This token is deprecated.
    /// LSP modifier: `deprecated`
    Deprecated,
    /// This token represents a control flow construct.
    /// LSP modifier: `controlFlow`
    ControlFlow,
    /// This token is part of the Twine-core layer.
    /// LSP modifier: `static` (reused to indicate core/system scope)
    TwineCore,
    /// This token is part of the story-format layer.
    /// LSP modifier: `async` (reused to indicate format scope)
    StoryFormat,
    /// This token is a user-defined special passage.
    /// LSP modifier: `modification` (indicates user-level customization)
    UserDefined,
}

/// A diagnostic produced by a format plugin during parsing.
///
/// The `range` field is a **passage-relative** byte range: byte 0 is the
/// `::` prefix of the passage header. To convert to a document-absolute
/// byte range, add the `PassageDiagnosticGroup::passage_offset` value.
///
/// This design mirrors `SemanticToken` and enables incremental per-passage
/// diagnostic updates — when a single passage is edited, only that passage's
/// diagnostic group needs to be regenerated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatDiagnostic {
    /// The byte range of the issue, **relative to the passage head**
    /// (the `::` prefix of the passage header).
    ///
    /// To convert to a document-absolute byte range, add the
    /// `PassageDiagnosticGroup::passage_offset` value.
    pub range: std::ops::Range<usize>,
    /// The diagnostic message.
    pub message: String,
    /// The severity.
    pub severity: FormatDiagnosticSeverity,
    /// The diagnostic code (for suppression).
    pub code: String,
}

/// Severity levels for format diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormatDiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A group of diagnostics for a single passage, with passage-relative
/// byte offsets.
///
/// All `FormatDiagnostic::range` values in this group are relative to the
/// passage head (the `::` prefix of the passage header). To convert to
/// document-absolute byte ranges, add `passage_offset` to each diagnostic's
/// range start and end.
///
/// This design mirrors `PassageTokenGroup` and enables incremental passage
/// updates: when a single passage is edited, only that passage's diagnostic
/// group needs to be regenerated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassageDiagnosticGroup {
    /// The passage name (for lookup during incremental updates).
    pub passage_name: String,
    /// Byte offset of the passage head (`::` prefix) in the document.
    ///
    /// Adding this to any diagnostic's range start/end gives a
    /// document-absolute byte offset.
    pub passage_offset: usize,
    /// Diagnostics with passage-relative byte ranges.
    pub diagnostics: Vec<FormatDiagnostic>,
}

/// The result of parsing a document with a format plugin.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// The parsed passages.
    pub passages: Vec<Passage>,
    /// Semantic token groups, one per passage, with passage-relative byte
    /// offsets. Each group's `passage_offset` is the document-absolute byte
    /// position of that passage's `::` header prefix, enabling conversion
    /// to document-absolute positions at the LSP boundary.
    pub token_groups: Vec<PassageTokenGroup>,
    /// Diagnostic groups, one per passage, with passage-relative byte
    /// offsets. Each group's `passage_offset` is the document-absolute byte
    /// position of that passage's `::` header prefix, enabling conversion
    /// to document-absolute ranges at the LSP boundary.
    pub diagnostic_groups: Vec<PassageDiagnosticGroup>,
    /// Whether the parse was fully successful (no errors).
    pub is_complete: bool,
}

// ===========================================================================
// Syntax-detection types (used by handlers for format-aware dispatch)
// ===========================================================================

/// Information about a macro invocation found at a given cursor position.
///
/// Returned by `FormatPlugin::find_macro_at_position()` so that handlers
/// (hover, completion, signature_help) can detect and locate macros using
/// format-specific syntax without hardcoding delimiters like `<<>>`.
///
/// All byte offsets are relative to the start of the line.
#[derive(Debug, Clone)]
pub struct MacroAtPosition {
    /// The macro/command name (e.g., "set", "if", "link" in SugarCube;
    /// "set:", "if:", "link:" in Harlowe — without delimiters).
    pub name: String,
    /// Byte range of the entire macro construct, including delimiters
    /// (e.g., `<<if $x gt 0>>` or `(set: $x to 5)`).
    pub full_range: std::ops::Range<usize>,
    /// Byte range of just the name portion, excluding delimiters
    /// (e.g., just `if` or just `set`).
    pub name_range: std::ops::Range<usize>,
    /// Whether the macro construct is unclosed (cursor is inside an
    /// incomplete macro). This happens during live editing when the user
    /// has typed `<<if $x` but hasn't typed `>>` yet.
    pub is_unclosed: bool,
}

/// A macro block event detected on a single line of source text.
///
/// Returned by `FormatPlugin::scan_line_for_macro_events()` so that
/// the folding-range handler can detect open/close pairs using
/// format-specific syntax. The handler pairs these events into
/// folding ranges; the format plugin only reports what it sees.
#[derive(Debug, Clone)]
pub struct MacroBlockEvent {
    /// The macro name (e.g., "if", "for", "link").
    pub name: String,
    /// 0-based line number where this event occurs.
    pub line: u32,
    /// Whether this is an opening event (`<<if>>`, `(if:)`)
    /// or a closing event (`<</if>>`).
    pub is_open: bool,
}

// ===========================================================================
// FormatPlugin trait
// ===========================================================================

/// The format plugin trait — all format parsers must implement this.
///
/// ## Parsing methods (required)
///
/// These methods handle source text parsing and must be implemented by every
/// format plugin.
///
/// ## Behavioral methods (optional, default no-ops)
///
/// These methods provide format-specific data for IDE features like completion,
/// hover, validation, and navigation. The default implementations return empty
/// collections or `None`, acting as safe no-ops for formats that don't support
/// a given feature. This is the same pattern as the default no-op fallback
/// for unsupported format features.
///
/// Handlers must always query these methods through the active format plugin
/// obtained from `FormatRegistry::get()`. Never import format-specific data
/// directly from a format module.
pub trait FormatPlugin: Send + Sync {
    // -----------------------------------------------------------------------
    // Parsing methods (required)
    // -----------------------------------------------------------------------

    /// Returns the story format this plugin handles.
    fn format(&self) -> StoryFormat;

    /// Returns the name-matched special passage definitions for this format.
    ///
    /// These are passages identified by their exact passage name
    /// (e.g., SugarCube's "StoryInit", "PassageHeader"). Format plugins
    /// should NOT include Twine-core passages (StoryTitle, StoryData, Start)
    /// or core tags (script, stylesheet) — those are always provided by
    /// `twine_core_special_passages()`.
    fn special_passages(&self) -> Vec<SpecialPassageDef>;

    /// Returns the tag-matched special passage definitions for this format.
    ///
    /// These are passages identified by their TAG (e.g., SugarCube's
    /// `[init]`, `[widget]`; Harlowe's `[header]`, `[footer]`,
    /// `[startup]`). The passage name is user-defined and irrelevant for
    /// matching. Multiple passages can share the same tag.
    ///
    /// Format plugins should NOT include core tags (`script`, `stylesheet`)
    /// — those are always provided by `twine_core_special_passages()`.
    ///
    /// The default implementation returns an empty list. Format plugins
    /// that have tag-matched special passages should override this method.
    fn tag_matched_special_passages(&self) -> Vec<SpecialPassageDef> {
        Vec::new()
    }

    /// Returns whether the given passage name is a known special passage.
    ///
    /// Checks against ALL name-matched special passages
    /// (TwineCore + LegacyCore + StoryFormat) so that Twine-core passages
    /// like StoryTitle and StoryData are always recognized regardless of
    /// the active format.
    ///
    /// **Note**: This only checks NAME-matched passages. To check if a
    /// passage matches a TAG-matched definition, use
    /// `classify_passage()` or check `tag_matched_special_passages()`.
    fn is_special_passage(&self, name: &str) -> bool {
        self.all_name_matched_passages().iter().any(|d| d.name == name)
    }

    /// Returns ALL name-matched special passage definitions applicable to
    /// this format, including Twine-core and legacy-core passages merged
    /// with the format-specific ones.
    ///
    /// The default implementation merges `twine_core_special_passages()`,
    /// `legacy_core_special_passages()`, and the format plugin's own
    /// `special_passages()`. Format plugins should NOT override this
    /// method — override `special_passages()` instead.
    fn all_name_matched_passages(&self) -> Vec<SpecialPassageDef> {
        let mut all = knot_core::passage::twine_core_special_passages();
        all.extend(knot_core::passage::legacy_core_special_passages());
        all.extend(self.special_passages());
        all
    }

    /// Returns ALL special passage definitions (both name-matched and
    /// tag-matched) applicable to this format.
    ///
    /// This is the union of `all_name_matched_passages()` and
    /// `tag_matched_special_passages()`. Used by handlers that need
    /// the complete set of special passage definitions.
    fn all_special_passages(&self) -> Vec<SpecialPassageDef> {
        let mut all = self.all_name_matched_passages();
        all.extend(self.tag_matched_special_passages());
        all
    }

    /// Classify a passage against all known special passage definitions.
    ///
    /// This is the primary classification entry point. The priority order
    /// ensures that core passages are never misclassified by a tag:
    ///
    /// 1. **Core name-matched** (StoryTitle, StoryData, Start) — always
    ///    recognized regardless of tags. A passage named "StoryTitle" with
    ///    `[widget]` is still StoryTitle, not a widget passage.
    ///
    /// 2. **Core tag-matched** ([script], [stylesheet], [style]) — Twine
    ///    compiler constructs that apply to all formats.
    ///
    /// 3. **Format name-matched** (StoryInit, PassageHeader, etc.) —
    ///    format-specific singleton passages.
    ///
    /// 4. **Format tag-matched** ([init], [widget], etc.) —
    ///    format-specific tagged passages.
    ///
    /// 5. **Legacy name-matched** ("script"/"stylesheet" as passage names
    ///    from Twine 1) — import/migration compatibility only.
    ///
    /// Returns `Some(SpecialPassageDef)` if the passage matches a known
    /// definition, or `None` if it is a regular user-defined passage.
    ///
    /// **Format isolation**: This method delegates to the format plugin
    /// for both name-matched and tag-matched definitions, ensuring that
    /// format-specific logic (e.g., Harlowe's [header] tag vs SugarCube's
    /// PassageHeader name) is handled correctly.
    fn classify_passage(
        &self,
        passage_name: &str,
        passage_tags: &[String],
    ) -> Option<SpecialPassageDef> {
        let all_defs = self.all_special_passages();

        // ── Step 1: Core name-matched definitions (HIGHEST PRIORITY) ───
        // Core passages like StoryTitle and StoryData must ALWAYS be
        // recognized by name, even if they also have tags. A passage
        // named "StoryData" with [widget] is still StoryData, not a
        // widget passage. This prevents core passages from being
        // misclassified by tag-matching.
        for def in &all_defs {
            if def.match_strategy == knot_core::passage::MatchStrategy::Name
                && def.name == passage_name
                && matches!(def.layer, knot_core::passage::SpecialPassageLayer::TwineCore | knot_core::passage::SpecialPassageLayer::LegacyCore)
            {
                return Some(def.clone());
            }
        }

        // ── Step 2: Core tag-matched definitions ──────────────────────
        // [script], [stylesheet], [style] — Twine compiler constructs
        // that apply across all formats. Checked before format-specific
        // tags so that format plugins don't duplicate or override them.
        for tag in passage_tags {
            for def in &all_defs {
                if def.match_strategy == knot_core::passage::MatchStrategy::Tag
                    && tag.eq_ignore_ascii_case(&def.name)
                    && matches!(def.layer, knot_core::passage::SpecialPassageLayer::TwineCore | knot_core::passage::SpecialPassageLayer::LegacyCore)
                {
                    let mut matched = def.clone();
                    matched.name = passage_name.to_string();
                    return Some(matched);
                }
            }
        }

        // ── Step 3: Format tag-matched definitions ────────────────────
        // [init], [widget] for SugarCube; [header], [footer],
        // [startup] for Harlowe. The passage name is user-defined.
        //
        // Tags take priority over format names for two reasons:
        // 1. Consistency: core tags already take priority over format names
        //    (step 2 before step 4). Format tags should follow the same
        //    pattern so the classification rule is uniform.
        // 2. Grouping: tags like [script] and [stylesheet] group passages
        //    by behavior regardless of name. Format tags like [startup]
        //    and [widget] serve the same purpose — a passage tagged
        //    [startup] is a startup passage regardless of its name.
        //    E.g., `:: PassageHeader [startup]` is a startup passage,
        //    not a chrome interceptor.
        for tag in passage_tags {
            for def in &all_defs {
                if def.match_strategy == knot_core::passage::MatchStrategy::Tag
                    && tag.eq_ignore_ascii_case(&def.name)
                    && matches!(def.layer, knot_core::passage::SpecialPassageLayer::StoryFormat)
                {
                    let mut matched = def.clone();
                    matched.name = passage_name.to_string();
                    return Some(matched);
                }
            }
        }

        // ── Step 4: Format name-matched definitions ───────────────────
        // SugarCube's StoryInit, PassageHeader, etc. These are singleton
        // passages identified by exact name. Checked AFTER format tags
        // so that tag-based grouping takes priority over name matching.
        for def in &all_defs {
            if def.match_strategy == knot_core::passage::MatchStrategy::Name
                && def.name == passage_name
                && matches!(def.layer, knot_core::passage::SpecialPassageLayer::StoryFormat)
            {
                return Some(def.clone());
            }
        }

        None
    }

    /// Classify a passage and return both the definition and its category.
    ///
    /// This is the full classification entry point that returns the
    /// `PassageCategory` alongside the optional `SpecialPassageDef`.
    /// The category explicitly represents which priority level matched,
    /// making classification decisions inspectable and debuggable.
    ///
    /// Use this method when you need to log or inspect the classification
    /// decision. For simple "is this special?" checks, `classify_passage()`
    /// is sufficient. For diagnostics and graph construction that need to
    /// know the classification tier, use this method.
    ///
    /// The returned `PassageCategory` matches the priority hierarchy:
    /// 1. `CoreMetadata` — StoryData, StoryTitle (format detection)
    /// 2. `CoreNamed` — Start (core name-matched non-metadata)
    /// 3. `CoreTagged` — [script], [stylesheet], [style] (core tags)
    /// 4. `CoreLegacy` — "script"/"stylesheet" as names (Twine 1)
    /// 5. `FormatTagged` — [init], [widget], [startup], etc. (format tags)
    /// 6. `FormatNamed` — StoryInit, PassageHeader, etc. (format names)
    /// 7. `Regular` — No match
    fn classify_passage_category(
        &self,
        passage_name: &str,
        passage_tags: &[String],
    ) -> (Option<SpecialPassageDef>, PassageCategory) {
        let all_defs = self.all_special_passages();

        // ── Step 1: Core name-matched definitions (HIGHEST PRIORITY) ───
        for def in &all_defs {
            if def.match_strategy == knot_core::passage::MatchStrategy::Name
                && def.name == passage_name
                && matches!(def.layer, knot_core::passage::SpecialPassageLayer::TwineCore | knot_core::passage::SpecialPassageLayer::LegacyCore)
            {
                let category = if matches!(def.behavior, knot_core::passage::SpecialPassageBehavior::Metadata) {
                    PassageCategory::CoreMetadata
                } else if matches!(def.layer, knot_core::passage::SpecialPassageLayer::LegacyCore) {
                    PassageCategory::CoreLegacy
                } else {
                    PassageCategory::CoreNamed
                };
                return (Some(def.clone()), category);
            }
        }

        // ── Step 2: Core tag-matched definitions ──────────────────────
        for tag in passage_tags {
            for def in &all_defs {
                if def.match_strategy == knot_core::passage::MatchStrategy::Tag
                    && tag.eq_ignore_ascii_case(&def.name)
                    && matches!(def.layer, knot_core::passage::SpecialPassageLayer::TwineCore | knot_core::passage::SpecialPassageLayer::LegacyCore)
                {
                    let mut matched = def.clone();
                    matched.name = passage_name.to_string();
                    return (Some(matched), PassageCategory::CoreTagged);
                }
            }
        }

        // ── Step 3: Format tag-matched definitions ────────────────────
        // Tags take priority over format names (same pattern as core tags
        // taking priority over format names in step 2). See classify_passage()
        // for the detailed rationale.
        for tag in passage_tags {
            for def in &all_defs {
                if def.match_strategy == knot_core::passage::MatchStrategy::Tag
                    && tag.eq_ignore_ascii_case(&def.name)
                    && matches!(def.layer, knot_core::passage::SpecialPassageLayer::StoryFormat)
                {
                    let mut matched = def.clone();
                    matched.name = passage_name.to_string();
                    return (Some(matched), PassageCategory::FormatTagged);
                }
            }
        }

        // ── Step 4: Format name-matched definitions ───────────────────
        // Checked AFTER format tags so tag-based grouping takes priority.
        for def in &all_defs {
            if def.match_strategy == knot_core::passage::MatchStrategy::Name
                && def.name == passage_name
                && matches!(def.layer, knot_core::passage::SpecialPassageLayer::StoryFormat)
            {
                return (Some(def.clone()), PassageCategory::FormatNamed);
            }
        }

        (None, PassageCategory::Regular)
    }
    fn display_name(&self) -> &str;

    // -----------------------------------------------------------------------
    // Tag classification (semantic token modifiers)
    // -----------------------------------------------------------------------

    /// Classify a tag against known special tag definitions.
    ///
    /// Returns the appropriate `SemanticTokenModifier` if the tag matches a
    /// known special tag, or `None` for custom (user-defined) tags.
    ///
    /// This is used by tag semantic token generation to visually distinguish
    /// special tags like `[script]`, `[widget]`, `[startup]` from custom tags
    /// like `[dark]`, `[forest]`, `[myTag]`. Themes can then color special
    /// tags differently from custom tags.
    ///
    /// ## Priority (same as `classify_passage`)
    ///
    /// 1. Core tags (`[script]`, `[stylesheet]`, `[style]`) → `TwineCore`
    /// 2. Format-specific tags (`[init]`, `[widget]`, `[startup]`, etc.)
    ///    → `StoryFormat`
    /// 3. Custom tags → `None`
    ///
    /// ## Format Isolation
    ///
    /// This method checks core tags first (from `twine_core_special_passages()`)
    /// then delegates to the format plugin's `tag_matched_special_passages()`
    /// for format-specific tags. Format plugins should NOT override this
    /// method — the default implementation is format-isolation-compliant.
    fn classify_tag(&self, tag: &str) -> Option<SemanticTokenModifier> {
        // Step 1: Core tags — [script], [stylesheet], [style]
        for def in knot_core::passage::twine_core_special_passages() {
            if def.match_strategy == knot_core::passage::MatchStrategy::Tag
                && tag.eq_ignore_ascii_case(&def.name)
            {
                return Some(SemanticTokenModifier::TwineCore);
            }
        }

        // Step 2: Legacy core tags (same as core for modifier purposes)
        for def in knot_core::passage::legacy_core_special_passages() {
            if def.match_strategy == knot_core::passage::MatchStrategy::Tag
                && tag.eq_ignore_ascii_case(&def.name)
            {
                return Some(SemanticTokenModifier::TwineCore);
            }
        }

        // Step 3: Format-specific tags
        for def in self.tag_matched_special_passages() {
            if tag.eq_ignore_ascii_case(&def.name) {
                return Some(SemanticTokenModifier::StoryFormat);
            }
        }

        // Not a known special tag — custom/user-defined
        None
    }

    // -----------------------------------------------------------------------
    // Macro catalog (optional)
    // -----------------------------------------------------------------------

    /// Returns the builtin macro definitions for this format.
    ///
    /// Used by completion, hover, validation, and signature help.
    fn builtin_macros(&self) -> &'static [MacroDef] {
        &[]
    }

    /// Returns the set of macro names that can have a body (block or polymorphic macros).
    ///
    /// Derived from the catalog's `BodyRequirement`: macros with `Required` or
    /// `Optional` body can appear as block macros with close tags.
    ///
    /// Used by close-tag completion, folding region detection, and structural validation.
    fn body_macro_names(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that modify block structure for folding
    /// (e.g., "else", "elseif" in SugarCube). These are not block openers
    /// (they don't get their own close tag) but they create folding
    /// subdivisions within a block macro.
    ///
    /// Used by the folding range handler to detect intermediate modifiers
    /// that should split a macro block into sub-folds.
    fn folding_modifier_names(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that accept passage name arguments.
    ///
    /// Used by passage-in-quote completion and link extraction.
    fn passage_arg_macro_names(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names where the first string arg is a label
    /// and the second is a passage reference (e.g., `<<link "label" "passage">>`).
    fn label_then_passage_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that assign/write variables
    /// (e.g., `set`, `capture` in SugarCube).
    fn variable_assignment_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that define new macros
    /// (e.g., `widget` in SugarCube).
    fn macro_definition_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that contain inline scripts
    /// (e.g., `script` in SugarCube).
    fn inline_script_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that can navigate to other passages
    /// via variable arguments (e.g., `goto`, `include`, `link`, `button`).
    fn dynamic_navigation_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Look up a macro definition by name.
    fn find_macro(&self, _name: &str) -> Option<&'static MacroDef> {
        None
    }

    /// Returns the structural parent constraints: maps child macro name →
    /// set of valid parent macro names.
    ///
    /// For example, in SugarCube: `elseif` must be inside `if` or `elseif`.
    fn macro_parent_constraints(&self) -> HashMap<&'static str, HashSet<&'static str>> {
        HashMap::new()
    }

    /// Given a macro name and the number of args provided so far, returns
    /// the 0-based index of the argument that is a passage reference.
    /// Returns -1 if no passage-ref arg at that position.
    fn get_passage_arg_index(&self, _macro_name: &str, _arg_count: usize) -> i32 {
        -1
    }

    // -----------------------------------------------------------------------
    // Syntax detection (optional — format-aware handler dispatch)
    //
    // These methods replace hardcoded SugarCube <<>> detection in handlers.
    // Every handler that searches for macro syntax MUST use these methods
    // instead of hardcoding delimiters. This is the format-isolation
    // guarantee for the handler layer.
    // -----------------------------------------------------------------------

    /// Find the macro invocation at the given cursor position on a line.
    ///
    /// Returns `Some(MacroAtPosition)` if the cursor (at byte offset
    /// `byte_pos` within `line`) is inside a macro construct, along with
    /// the macro name and byte ranges needed for hover, completion, and
    /// signature-help responses.
    ///
    /// The `byte_pos` parameter is a byte offset into `line`. The handler
    /// must convert the LSP UTF-16 position to a byte offset before calling
    /// this method, using `helpers::utf16_to_byte_offset()`.
    ///
    /// The returned `full_range` and `name_range` are also byte offsets
    /// into `line`. The handler must convert these to UTF-16 for LSP
    /// responses using `helpers::utf16_len_up_to()`.
    ///
    /// - SugarCube: searches for `<<name ...>>` and `<</name>>`
    /// - Harlowe:   searches for `(name:...)`
    /// - Chapbook:  searches for `[name]...[/name]` special blocks
    /// - Snowman:   searches for `<%= ... %>` and `<% ... %>`
    ///
    /// The default implementation returns `None` (no macro detection),
    /// which is appropriate for formats that don't have macros.
    fn find_macro_at_position(
        &self,
        _line: &str,
        _byte_pos: usize,
    ) -> Option<MacroAtPosition> {
        None
    }

    /// Scan a single line for macro block open/close events.
    ///
    /// Used by the folding-range handler to detect macro block structure.
    /// The handler collects events across all lines, then pairs them into
    /// folding ranges using a stack-based algorithm. The format plugin
    /// only reports what it sees on each line — the pairing logic is
    /// format-agnostic.
    ///
    /// - SugarCube: detects `<<name>>` (open) and `<</name>>` (close)
    /// - Harlowe:   detects `(name:)` (open, if block) — no close tags
    /// - Chapbook:  detects `[name]` (open) and `[/name]` (close)
    /// - Snowman:   no block macro structure
    ///
    /// The default implementation returns an empty vector.
    fn scan_line_for_macro_events(
        &self,
        _line: &str,
        _line_idx: u32,
    ) -> Vec<MacroBlockEvent> {
        Vec::new()
    }

    /// Format a macro name for display in hover text, completion labels,
    /// and documentation.
    ///
    /// - SugarCube: `<<name>>`
    /// - Harlowe:   `(name:)`
    /// - Chapbook:  `[name]`
    /// - Snowman:   `<%= name %>` (rarely applicable)
    fn format_macro_label(&self, name: &str) -> String {
        // Default: return the bare name. Format plugins MUST override this
        // to add their own delimiters (e.g., <<name>>, (name:), [name]).
        name.to_string()
    }

    /// Format a macro signature for display (name + parameter list).
    ///
    /// Used by signature-help to show the full call syntax.
    /// - SugarCube: `<<name params>>`
    /// - Harlowe:   `(name: params)`
    fn format_macro_signature_label(&self, name: &str, params: &str) -> String {
        // Default: return the bare name + params. Format plugins MUST override.
        if params.is_empty() {
            name.to_string()
        } else {
            format!("{} {}", name, params)
        }
    }

    /// Format a closing macro tag for display.
    ///
    /// - SugarCube: `<</name>>`
    /// - Harlowe:   not applicable (returns empty string by default)
    fn format_close_macro_label(&self, _name: &str) -> String {
        // Default: empty — formats without close tags should not produce one.
        // SugarCube overrides this to return `<</name>>`.
        String::new()
    }

    /// Build an insertion snippet for a macro.
    ///
    /// Override this in format plugins that use different delimiter syntax.
    /// The default implementation produces bare-name snippets (no delimiters).
    fn build_macro_snippet(&self, name: &str, body: BodyRequirement) -> String {
        // Default: bare name + placeholder. Format plugins MUST override this
        // to add their own delimiters and body structure.
        if body != BodyRequirement::Never {
            format!("{} $1\n$2\n", name)
        } else {
            format!("{} $1", name)
        }
    }

    /// Detect close-tag context and return the partial name typed so far.
    ///
    /// Used by close-tag completion. Returns `Some(partial_name)` when the
    /// cursor is in a close-tag context (e.g., after `<</` in SugarCube).
    ///
    /// - SugarCube: detects `<</` prefix and extracts the partial name
    /// - Harlowe:   not applicable (no close tags)
    fn detect_close_tag_context(
        &self,
        _before_cursor: &str,
    ) -> Option<String> {
        None
    }

    /// Whether this format has block macros with close tags.
    ///
    /// Handlers use this to decide whether to offer close-tag completion
    /// and macro-block folding. Formats without close tags (Harlowe, Snowman)
    /// return `false`.
    fn has_block_macros_with_close_tags(&self) -> bool {
        false // Default: no close tags. SugarCube overrides to `true`.
    }

    // -----------------------------------------------------------------------
    // Special passages (extended)
    // -----------------------------------------------------------------------

    /// Returns the set of special passage names (e.g., "StoryInit", "PassageHeader").
    ///
    /// The default implementation returns an empty set. Format plugins that
    /// have special passages should override this method (and typically will,
    /// since they have `&'static str` names available at compile time).
    fn special_passage_names(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of system/metadata passage names
    /// (e.g., "StoryData", "StoryTitle", "Story JavaScript", "Story Stylesheet").
    fn system_passage_names(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    // -----------------------------------------------------------------------
    // Variable tracking (optional)
    // -----------------------------------------------------------------------

    /// Returns the variable sigils this format uses (e.g., `$` and `_` for SugarCube).
    fn variable_sigils(&self) -> Vec<VariableSigilInfo> {
        Vec::new()
    }

    /// Describe a variable sigil character (e.g., `$` → "SugarCube story variable").
    fn describe_variable_sigil(&self, _sigil: char) -> Option<&'static str> {
        None
    }

    /// Resolve a variable sigil character to a human-readable type name.
    fn resolve_variable_sigil(&self, _sigil: char) -> Option<&'static str> {
        None
    }

    /// Returns the assignment operators this format uses
    /// (e.g., `to`, `=` for SugarCube).
    fn assignment_operators(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Returns the format-specific snippet for initializing a variable in the
    /// startup passage.
    ///
    /// This is used by the "Initialize variable" code action to insert the
    /// correct assignment syntax for the detected story format:
    /// - SugarCube: `<<set $var to 0>>`
    /// - Harlowe: `(set: $var to 0)`
    /// - Chapbook: `{_var: 0}` (or equivalent)
    /// - Snowman: `<% s.var = 0 %>` (or equivalent)
    ///
    /// The default implementation returns `None`, which signals that the
    /// format does not support variable initialization via code actions.
    /// Callers should fall back to a plain comment or skip the action.
    fn variable_assignment_snippet(&self, _var_name: &str, _value: &str) -> Option<String> {
        None
    }

    /// Returns the comparison operators this format uses.
    fn comparison_operators(&self) -> Vec<&'static str> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Implicit passage references (optional)
    // -----------------------------------------------------------------------

    /// Returns the patterns for detecting implicit passage references in
    /// raw text/HTML/JS (e.g., `data-passage="..."`, `Engine.play("...")`).
    fn implicit_passage_patterns(&self) -> Vec<ImplicitPassagePattern> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Dynamic navigation resolution (optional)
    // -----------------------------------------------------------------------

    /// Build a map of variable name → set of known string literal values
    /// from format-specific assignment syntax.
    ///
    /// This is used to resolve dynamic passage references like
    /// `<<goto $dest>>` into concrete passage names.
    ///
    /// The default implementation returns an empty map. SugarCube overrides
    /// this to scan `<<set $var to "literal">>` patterns.
    fn build_var_string_map(&self, _workspace: &knot_core::Workspace) -> HashMap<String, Vec<String>> {
        HashMap::new()
    }

    /// Resolve dynamic navigation links from a passage using format-specific
    /// patterns and the variable string map.
    ///
    /// Returns a list of (display_text, target_passage) pairs for links that
    /// were resolved from variable references.
    fn resolve_dynamic_navigation_links(
        &self,
        _passage: &Passage,
        _var_string_map: &HashMap<String, Vec<String>>,
    ) -> Vec<ResolvedNavLink> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Edge classification (optional — format-aware edge typing)
    // -----------------------------------------------------------------------

    /// Classify the edge type for a link from the given source passage.
    ///
    /// Each format plugin overrides this method to classify edges as
    /// navigation, call, include, or jump based on the format-specific
    /// syntax used in the source passage. This is the format-isolation-
    /// correct way to handle edge types — handlers never hardcode
    /// format-specific edge logic.
    ///
    /// ## Default Classification Rules
    ///
    /// - `[[link]]` → `Navigation` (player choice — Twine core)
    /// - Broken targets → `Broken` (set by graph engine, not plugins)
    /// - Upstream lifecycle → `Upstream` (set by graph engine, not plugins)
    ///
    /// ## Format-Specific Overrides
    ///
    /// - **SugarCube**: `<<widget>>` → `Call`, `<<include>>` → `Include`,
    ///   `<<goto>>` → `Jump`, `<<link>>`/`<<button>>` → `Navigation`
    /// - **Harlowe**: `(display:)` → `Include`, `(go-to:)` → `Jump`,
    ///   `(redirect:)` → `Jump`, `(link-goto:)` → `Navigation`
    /// - **Chapbook**: `{{> partial}}` → `Include`
    /// - **Snowman**: `include()` → `Include`
    ///
    /// The `display_text` and `target` identify the specific link being
    /// classified. If the plugin returns `None`, the default `Navigation`
    /// type is used.
    fn classify_edge(
        &self,
        _source_passage: &Passage,
        _display_text: Option<&str>,
        _target: &str,
    ) -> Option<knot_core::graph::EdgeType> {
        None
    }

    // -----------------------------------------------------------------------
    // Hover / documentation (optional)
    // -----------------------------------------------------------------------

    /// Returns hover text for a global object name (e.g., "State", "Engine").
    fn global_hover_text(&self, _name: &str) -> Option<&'static str> {
        None
    }

    /// Returns the builtin global object definitions for this format.
    fn builtin_globals(&self) -> &'static [GlobalDef] {
        &[]
    }

    /// Returns the set of known global object names (e.g., "State", "Engine").
    fn global_object_names(&self) -> HashSet<&'static str> {
        self.builtin_globals().iter().map(|g| g.name).collect()
    }

    // -----------------------------------------------------------------------
    // Operator normalization (optional)
    // -----------------------------------------------------------------------

    /// Returns the operator normalization mappings for this format
    /// (e.g., SugarCube `to` → JS `=`, `is` → JS `===`).
    fn operator_normalization(&self) -> Vec<OperatorNormalization> {
        Vec::new()
    }

    /// Returns the operator precedence table for this format.
    fn operator_precedence(&self) -> Vec<(&'static str, u8)> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Variable tracking capability (optional)
    // -----------------------------------------------------------------------

    /// Whether cross-passage variable tracking is fully supported.
    ///
    /// Formats that return `true` have a complete variable dataflow model
    /// that supports cross-passage tracking of variable initialization,
    /// reads, and writes. The variable flow UI and diagnostics are fully
    /// available for these formats.
    ///
    /// The default implementation returns `false`. SugarCube and Snowman
    /// override this to return `true`.
    fn supports_full_variable_tracking(&self) -> bool {
        false
    }

    /// Whether variable tracking is partially supported.
    ///
    /// Formats that return `true` support some variable tracking but lack
    /// a complete cross-passage dataflow model. Variable highlighting and
    /// per-passage extraction work, but cross-passage diagnostics may be
    /// limited.
    ///
    /// The default implementation returns `false`. Harlowe overrides this
    /// to return `true`.
    fn supports_partial_variable_tracking(&self) -> bool {
        false
    }

    // -----------------------------------------------------------------------
    // Macro snippet mapping (optional)
    // -----------------------------------------------------------------------

    /// Returns a per-macro snippet override for completion.
    /// If None is returned for a macro name, the default snippet is used.
    fn macro_snippet(&self, _name: &str) -> Option<&'static str> {
        None
    }

    // -----------------------------------------------------------------------
    // Dot-notation completion (optional)
    // -----------------------------------------------------------------------

    /// Build a map of variable dot-path → set of immediate child property names.
    ///
    /// Used for dot-notation completion (e.g., `$item.` → suggest "sword", "shield").
    /// The default implementation returns an empty map.
    fn build_object_property_map(&self, _workspace: &knot_core::Workspace) -> HashMap<String, HashSet<String>> {
        HashMap::new()
    }

    /// Build a shape-aware property map for dot-notation completion.
    ///
    /// This enriches the basic `build_object_property_map()` with structural
    /// type information (`PropertyKind`: Scalar, Object, Array, Unknown) and
    /// array element shapes. The completion handler uses this to offer:
    /// - Array methods (`.length`, `.push()`) for Array-kind variables
    /// - Child properties for Object-kind variables
    /// - No completions for Scalar-kind variables
    /// - Element property completions via `[0].prop` for arrays with known element shape
    ///
    /// The default implementation returns an empty map (no shape-aware completion).
    /// Format plugins that support dot-notation completion should override this.
    fn build_shape_aware_property_map(&self, _workspace: &knot_core::Workspace) -> HashMap<String, crate::types::PropertyMapEntry> {
        HashMap::new()
    }

    // -----------------------------------------------------------------------
    // State variable registry & diagnostics (optional)
    // -----------------------------------------------------------------------

    /// Build a registry of all state variables across the workspace.
    ///
    /// This is the format-specific replacement for the core's `detect_uninitialized_reads()`.
    /// Format plugins that support persistent state variables (like SugarCube's
    /// `State.variables`) should override this to collect all `$var` /
    /// `State.variables.*` references into a `StateVariable` registry.
    ///
    /// The registry tracks:
    /// - All write and read locations for each variable
    /// - Known dot-notation properties
    /// - Whether the variable is seeded by a special passage
    ///
    /// The default implementation returns an empty registry.
    fn build_state_variable_registry(
        &self,
        _workspace: &knot_core::Workspace,
    ) -> HashMap<String, crate::types::StateVariable> {
        HashMap::new()
    }

    /// Compute variable-related diagnostics using the format's state model
    /// and the passage graph.
    ///
    /// This replaces the core's `detect_uninitialized_reads()`,
    /// `detect_unused_variables()`, and `detect_redundant_writes()` with
    /// format-aware analysis. For SugarCube, this uses graph-BFS to compute
    /// variable availability rather than traditional definite-assignment analysis.
    ///
    /// The diagnostics produced are **hints** rather than errors/warnings,
    /// because persistent state variables may exist from saved games or
    /// scripts that the LSP cannot fully model.
    ///
    /// The default implementation returns an empty list (no diagnostics).
    fn compute_variable_diagnostics(
        &self,
        _workspace: &knot_core::Workspace,
        _start_passage: &str,
        _registry: &HashMap<String, crate::types::StateVariable>,
    ) -> Vec<crate::types::VariableDiagnostic> {
        Vec::new()
    }

    /// Return the set of variable names that are initialized by special
    /// passages (e.g., `StoryInit`, `Story JavaScript`, `PassageDone`).
    ///
    /// This supplements the core engine's `collect_special_passage_initializers()`
    /// which only scans passages in the indexed workspace. If a special passage
    /// is defined in an unindexed file, its initializers won't appear in
    /// `passage_data`. By also querying the format plugin's variable registry,
    /// which builds `seeded_by_special` from ALL parsed documents, we close
    /// this gap and avoid false "uninitialized variable" diagnostics for
    /// variables that are actually seeded at game start.
    ///
    /// The default implementation uses `build_state_variable_registry()` and
    /// filters for variables where `seeded_by_special` is true.
    fn special_passage_seed_variables(
        &self,
        workspace: &knot_core::Workspace,
    ) -> HashSet<String> {
        self.build_state_variable_registry(workspace)
            .into_iter()
            .filter(|(_, sv)| sv.seeded_by_special)
            .map(|(name, _)| name)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Variable tree (format-agnostic UI representation)
    // -----------------------------------------------------------------------

    /// Build a tree-structured representation of all state variables for
    /// display in the variable tracker UI.
    ///
    /// This method returns format-agnostic `VariableTreeNode` instances that
    /// the server translates directly to LSP wire types without any
    /// format-specific logic. The tree structure mirrors the runtime state
    /// hierarchy of the format.
    ///
    /// For SugarCube, `$player.hp` maps to `State.variables.player.hp`, so
    /// `$player` becomes a `VariableTreeNode` with a `.hp` child property.
    /// Other formats can produce their own tree structures that reflect their
    /// runtime state model — the server and UI never need to know
    /// format-specific details.
    ///
    /// **Format isolation guarantee**: The server's `knot/variableFlow` handler
    /// calls this method and performs only a mechanical translation to LSP
    /// wire types. It never inspects format-specific enums like `VarAccessKind`
    /// or hardcodes format-specific strings like `"State.variables"`. All
    /// format-specific logic lives here, in the format plugin.
    ///
    /// The default implementation returns an empty list (no variables).
    ///
    /// The `source_text` parameter provides access to document source text
    /// for computing accurate line numbers from byte offsets. Without it,
    /// all usage locations would report `line: 0` (passage header).
    fn build_variable_tree(
        &self,
        _workspace: &knot_core::Workspace,
        _source_text: &dyn SourceTextProvider,
    ) -> Vec<crate::types::VariableTreeNode> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Passage variable references (optional)
    // -----------------------------------------------------------------------

    /// Extract variable references for a specific passage.
    ///
    /// This is the format-agnostic entry point used by passage diagnostics to
    /// show which variables are read/written in a passage, with exact line
    /// numbers mapped back to the original source.
    ///
    /// The default implementation:
    /// 1. Extracts all variable accesses from the passage tree
    /// 2. Filters for the requested passage
    /// 3. Returns format-agnostic `PassageVarRef` instances with line numbers
    ///
    /// Format plugins that need custom extraction logic (e.g., SugarCube's
    /// alias resolution) should override this method. Format plugins that
    /// don't support variable tracking should use the default (empty) path.
    ///
    /// Returns an empty Vec if the format doesn't support variable tracking
    /// or the passage has no variable references.
    fn extract_passage_variable_refs(
        &self,
        workspace: &knot_core::Workspace,
        source_text: &dyn SourceTextProvider,
        passage_name: &str,
    ) -> Vec<crate::types::PassageVarRef> {
        // Default: no variable references (formats must override to provide them)
        let _ = (workspace, source_text, passage_name);
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Registry accessors (Phase C — format-owned side tables)
    // -----------------------------------------------------------------------

    /// Get all workspace variable names for completion.
    ///
    /// Returns the set of variable names known to the format's side table.
    /// The default implementation returns an empty set.
    fn workspace_variable_names(&self) -> HashSet<String> {
        HashSet::new()
    }

    /// Get known property paths for a variable (for dot-notation completion).
    ///
    /// Returns the set of known property paths (e.g., `{"name", "hp"}` for
    /// `$player`) from the format's variable side table.
    /// The default implementation returns an empty set.
    fn variable_properties(&self, _var_name: &str) -> HashSet<String> {
        HashSet::new()
    }

    /// Get all custom macro names for completion.
    ///
    /// Returns names of user-defined macros (widgets and `Macro.add()` calls)
    /// from the format's macro registry. The default returns an empty list.
    fn custom_macro_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// Look up a custom macro definition for hover/go-to-def.
    ///
    /// Returns `(passage_name, file_uri, offset)` if the macro is found,
    /// or `None`. The default returns `None`.
    fn find_custom_macro(&self, _name: &str) -> Option<(String, String, usize)> {
        None
    }

    /// Check if a macro name is a known custom macro.
    ///
    /// Returns `true` if the name matches a widget or `Macro.add()` definition.
    fn is_custom_macro(&self, _name: &str) -> bool {
        false
    }

    // -----------------------------------------------------------------------
    // Function registry (optional — formats with JS scripting)
    // -----------------------------------------------------------------------

    /// Get all JS function names discovered in script passages (for completion).
    ///
    /// Formats that support JS scripting (SugarCube, Snowman) can override this
    /// to provide function name completion in JS contexts. Harlowe and Chapbook
    /// return empty vectors by default.
    fn function_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// Look up a function definition for hover/go-to-definition.
    ///
    /// Returns `(passage_name, file_uri, defined_at_offset)` if found.
    fn find_function(&self, _name: &str) -> Option<FunctionDefInfo> {
        None
    }

    // -----------------------------------------------------------------------
    // Template registry (optional — formats with template systems)
    // -----------------------------------------------------------------------

    /// Get all template names for completion (with format-specific prefix).
    ///
    /// SugarCube returns names with `?` prefix (e.g., `?heal`).
    fn template_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// Look up a template definition for hover/go-to-definition.
    fn find_template(&self, _name: &str) -> Option<TemplateDefInfo> {
        None
    }

    // -----------------------------------------------------------------------
    // Registry lifecycle (optional — incremental re-parse support)
    // -----------------------------------------------------------------------

}

// ===========================================================================
// FormatPluginMut trait
// ===========================================================================

/// Mutable operations on a format plugin.
///
/// All methods take `&mut self`, meaning the caller MUST hold exclusive access
/// to the plugin. In the server, this means holding the write lock on
/// `ServerStateInner`.
///
/// Read-only operations remain on the `FormatPlugin` trait with `&self`.
pub trait FormatPluginMut: FormatPlugin {
    /// Parse an entire file and return structured results.
    /// The plugin MUST call `registry.remove_file(uri)` before populating.
    fn parse_mut(&mut self, uri: &Url, text: &str) -> ParseResult;

    /// Re-parse a single passage incrementally.
    /// The plugin MUST call `registry.remove_passage(name, uri)` before populating.
    fn parse_passage_mut(&mut self, passage_name: &str, passage_tags: &[String], passage_text: &str, file_uri: &str) -> Option<Passage>;

    /// Remove all registry entries for a file.
    fn remove_file_from_registries(&mut self, file_uri: &str);

    /// Remove all registry entries for a single passage.
    fn remove_passage_from_registries(&mut self, passage_name: &str, file_uri: &str);
}

// ===========================================================================
// FormatRegistry
// ===========================================================================

/// Registry of available format plugins.
pub struct FormatRegistry {
    plugins: Vec<Box<dyn FormatPluginMut>>,
}

impl FormatRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a format plugin.
    pub fn register(&mut self, plugin: Box<dyn FormatPluginMut>) {
        self.plugins.push(plugin);
    }

    /// Get the plugin for a given story format (read-only access).
    pub fn get(&self, format: &StoryFormat) -> Option<&dyn FormatPlugin> {
        self.plugins
            .iter()
            .find(|p| &p.format() == format)
            .map(|p| p.as_ref() as &dyn FormatPlugin)
    }

    /// Get the plugin for a given story format (mutable access).
    ///
    /// Use this when calling `parse_mut()`, `parse_passage_mut()`,
    /// `remove_file_from_registries()`, or `remove_passage_from_registries()`.
    pub fn get_mut(&mut self, format: &StoryFormat) -> Option<&mut dyn FormatPluginMut> {
        self.plugins
            .iter_mut()
            .find(|p| p.format() == *format)
            .map(|p| p.as_mut() as &mut dyn FormatPluginMut)
    }

    /// Get all registered formats.
    pub fn formats(&self) -> Vec<StoryFormat> {
        self.plugins.iter().map(|p| p.format()).collect()
    }

    /// Create a registry with all built-in format plugins.
    ///
    /// The `Core` plugin is registered first as the lowest-priority fallback.
    /// It provides base Twine engine behavior (passage headers, links, core
    /// special passages) with no format-specific features. When format
    /// detection fails, `resolve_format()` returns `Core`, and the registry
    /// finds this plugin.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(crate::twine_core::TwineCorePlugin::new()));
        registry.register(Box::new(crate::sugarcube::SugarCubePlugin::new()));
        registry.register(Box::new(crate::harlowe::HarlowePlugin::new()));
        registry.register(Box::new(crate::chapbook::ChapbookPlugin::new()));
        registry.register(Box::new(crate::snowman::SnowmanPlugin::new()));
        registry
    }
}

impl Default for FormatRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}
