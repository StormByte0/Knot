//! Macro handler: classification, completion delegation, and hover delegation.
//!
//! This module provides the central macro classification system that all
//! LSP handlers (completion, hover, semantic tokens, diagnostics) can
//! query to determine the semantic role of any macro in a format-agnostic
//! way.
//!
//! ## Macro Classification
//!
//! Macros are classified along a `MacroKind` axis that determines how they
//! appear in the IDE:
//!
//! - **Keyword**: Symbol-like macros that act as operators or shorthand
//!   (e.g., SugarCube `=`, `-`). These get `Keyword` completion kind and
//!   `Keyword` semantic token type.
//!
//! - **ControlFlow**: Macros that alter execution flow — branching, looping,
//!   or jumping. Includes both block openers (`if`, `for`, `switch`) and
//!   standalone statements (`goto`, `break`, `continue`). These get
//!   `ControlFlow` semantic token modifier and are sorted first in completion.
//!
//! - **Block**: Macros that open a closeable body section (e.g., `if`,
//!   `for`, `link`, `append`, `widget`). These generate close-tag snippets
//!   in completion and get folding ranges.
//!
//! - **Modifier**: Structural modifiers that appear inside a parent block
//!   without opening their own closeable body (e.g., `else`, `elseif`,
//!   `case`, `default`). These get `ControlFlow` modifier but do NOT
//!   generate close tags.
//!
//! - **Statement**: Standalone macros that do not open a body and are not
//!   control-flow (e.g., `set`, `print`, `audio`, `remove`). These get
//!   `FUNCTION` completion kind.
//!
//! - **Identifier**: Macros whose primary purpose is to define or reference
//!   an identifier (e.g., `widget`, `capture`). These get `FUNCTION`
//!   completion kind with identifier-specific documentation.
//!
//! ## Usage by Handlers
//!
//! - **completion.rs**: Use `classify()` to determine `CompletionItemKind`,
//!   sort priority, and snippet generation (blocks get close tags).
//! - **hover.rs**: Use `classify()` to add kind-specific labels and
//!   semantic context to hover documentation.
//! - **semantic.rs**: Use `classify()` to apply `ControlFlow` modifier
//!   and `Keyword` token type where appropriate.
//! - **diagnostics**: Use `classify()` to validate nesting constraints
//!   (modifiers must be inside their parent blocks).
//!
//! ## Format Isolation
//!
//! All classification is derived from the `FormatPlugin` trait methods —
//! `builtin_macros()`, `block_macro_names()`, `folding_modifier_names()`,
//! `dynamic_navigation_macros()`, etc. No format-specific data is imported
//! directly.

use knot_formats::types::{MacroCategory, MacroDef};
use knot_formats::plugin::FormatPlugin;

// ---------------------------------------------------------------------------
// MacroKind — the unified classification enum
// ---------------------------------------------------------------------------

/// The semantic role of a macro, used to determine IDE presentation.
///
/// This classification is derived from the combination of `MacroDef` fields
/// and `FormatPlugin` trait methods. It provides a single, queryable
/// classification that replaces the need to check multiple independent
/// axes (category, has_body, block_macro_names, etc.) separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacroKind {
    /// A symbol-like macro that acts as an operator or shorthand
    /// (e.g., SugarCube `=` for `<<= expr>>`, `-` for `<<- expr>>`).
    /// Semantic token type: `Keyword`. Completion kind: `KEYWORD`.
    Keyword,

    /// A macro that alters execution flow — branching, looping, or jumping.
    /// Includes both block openers (`if`, `for`, `switch`) and standalone
    /// navigation macros (`goto`, `return`, `back`).
    /// Semantic token modifier: `ControlFlow`. Completion sort: first.
    ControlFlow,

    /// A macro that opens a closeable body section (e.g., `if`, `for`,
    /// `link`, `append`, `widget`). Completion generates close-tag snippets.
    /// Also gets `ControlFlow` modifier if it's in the Control category.
    Block,

    /// A structural modifier that appears inside a parent block without
    /// opening its own closeable body (e.g., `else`, `elseif`, `case`,
    /// `default`). Gets `ControlFlow` modifier. Does NOT get close tags.
    Modifier,

    /// A standalone macro that does not open a body and is not primarily
    /// control-flow (e.g., `set`, `print`, `audio`, `remove`, `css`).
    /// Completion kind: `FUNCTION`.
    Statement,

    /// A macro whose primary purpose is to define or reference an identifier
    /// (e.g., `widget` defines a new macro, `capture` assigns to a variable).
    /// Completion kind: `FUNCTION` with identifier-specific docs.
    Identifier,
}

impl std::fmt::Display for MacroKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MacroKind::Keyword => write!(f, "keyword"),
            MacroKind::ControlFlow => write!(f, "control-flow"),
            MacroKind::Block => write!(f, "block"),
            MacroKind::Modifier => write!(f, "modifier"),
            MacroKind::Statement => write!(f, "statement"),
            MacroKind::Identifier => write!(f, "identifier"),
        }
    }
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Classify a macro by its semantic role, using the format plugin's
/// behavioral data.
///
/// The classification logic:
///
/// 1. **Modifier** check: If the macro name is in `folding_modifier_names()`,
///    it's a modifier (e.g., `else`, `elseif`, `case`, `default`).
///
/// 2. **Keyword** check: If the macro name is a non-alphabetic operator
///    (e.g., `=`, `-`), it's a keyword.
///
/// 3. **Block** check: If the macro name is in `block_macro_names()` AND
///    has `has_body == true`, it's a block macro. If it's also in the
///    Control or Navigation category, it additionally gets ControlFlow
///    semantics (returned as `Block`, but callers should check category).
///
/// 4. **ControlFlow** check: If the macro is in the Control category and
///    NOT a block or modifier, it's a standalone control-flow statement
///    (e.g., `break`, `continue`, `goto`).
///
/// 5. **Identifier** check: If the macro is in `macro_definition_macros()`
///    or `variable_assignment_macros()`, it's an identifier-defining macro.
///
/// 6. **Statement**: Default for any macro that doesn't fit the above.
///
/// # Arguments
///
/// * `name` - The bare macro name (e.g., "if", "set", "=")
/// * `mdef` - The macro definition from the format plugin catalog
/// * `plugin` - The format plugin (for block_macro_names, etc.)
pub fn classify(
    name: &str,
    mdef: &MacroDef,
    plugin: &dyn FormatPlugin,
) -> MacroKind {
    // 1. Modifier check — structural modifiers like else, elseif, case, default
    if plugin.folding_modifier_names().contains(name) {
        return MacroKind::Modifier;
    }

    // 2. Keyword check — non-alphabetic operator macros like =, -
    if !name.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) {
        return MacroKind::Keyword;
    }

    // 3. Block check — macros with closeable body sections
    let is_block = plugin.block_macro_names().contains(name);
    if is_block && mdef.has_body {
        // Control category blocks (if, for, switch) and Navigation blocks
        // (link, button with goto) are semantically control-flow blocks.
        // Callers can check the category for additional semantics.
        return MacroKind::Block;
    }

    // 4. ControlFlow check — standalone control-flow statements
    //    (goto, break, continue, return, back — not block openers)
    if mdef.category == MacroCategory::Control {
        return MacroKind::ControlFlow;
    }

    // Navigation macros that are NOT blocks are also control-flow
    // (e.g., goto, return, back — they redirect execution)
    if mdef.category == MacroCategory::Navigation && !is_block {
        return MacroKind::ControlFlow;
    }

    // 5. Identifier check — macros that define identifiers
    if plugin.macro_definition_macros().contains(name) {
        return MacroKind::Identifier;
    }
    if plugin.variable_assignment_macros().contains(name) {
        return MacroKind::Identifier;
    }

    // 6. Default: statement
    MacroKind::Statement
}

/// Classify a macro by name only, looking it up in the format plugin catalog.
///
/// Returns `None` if the macro name is not found in the catalog.
pub fn classify_by_name(
    name: &str,
    plugin: &dyn FormatPlugin,
) -> Option<MacroKind> {
    let mdef = plugin.find_macro(name)?;
    Some(classify(name, mdef, plugin))
}

// ---------------------------------------------------------------------------
// Semantic token helpers
// ---------------------------------------------------------------------------

/// Determine whether a macro should receive the `ControlFlow` semantic
/// token modifier based on its classification.
///
/// Control-flow macros (if, for, switch, goto, break, continue) and
/// modifiers (else, elseif, case, default) both get the `ControlFlow`
/// modifier so that themes can highlight them distinctly from regular
/// macros.
pub fn is_control_flow(kind: MacroKind) -> bool {
    matches!(kind, MacroKind::ControlFlow | MacroKind::Modifier)
        || kind == MacroKind::Block // Block macros in Control/Navigation categories
}

/// Determine whether a macro should use the `Keyword` semantic token type
/// (rather than `Macro`) based on its classification.
///
/// Keyword macros (e.g., `=`, `-`) get `Keyword` token type. All other
/// macros get `Macro` token type, possibly with modifiers.
pub fn uses_keyword_token_type(kind: MacroKind) -> bool {
    matches!(kind, MacroKind::Keyword)
}

// ---------------------------------------------------------------------------
// Completion helpers
// ---------------------------------------------------------------------------

/// Determine the LSP `CompletionItemKind` for a macro based on its classification.
///
/// - `Keyword` → `CompletionItemKind::KEYWORD`
/// - `ControlFlow`, `Block`, `Modifier` → `CompletionItemKind::SNIPPET`
///   (because they benefit from placeholder-rich snippets)
/// - `Identifier`, `Statement` → `CompletionItemKind::FUNCTION`
pub fn completion_item_kind(kind: MacroKind) -> lsp_types::CompletionItemKind {
    use lsp_types::CompletionItemKind;
    match kind {
        MacroKind::Keyword => CompletionItemKind::KEYWORD,
        MacroKind::ControlFlow | MacroKind::Block | MacroKind::Modifier => {
            CompletionItemKind::SNIPPET
        }
        MacroKind::Identifier | MacroKind::Statement => CompletionItemKind::FUNCTION,
    }
}

/// Determine the sort priority for a macro completion item.
///
/// Lower numbers appear first. Control-flow and block macros should appear
/// before statements because they are more likely to be what the user wants
/// when typing a `<` trigger.
pub fn sort_priority(kind: MacroKind) -> u8 {
    match kind {
        MacroKind::ControlFlow => 0,
        MacroKind::Block => 1,
        MacroKind::Modifier => 2,
        MacroKind::Keyword => 3,
        MacroKind::Identifier => 4,
        MacroKind::Statement => 5,
    }
}

/// Build the sort text for a macro completion item, incorporating
/// the kind-based priority and the macro name for alphabetical tiebreaking.
pub fn sort_text(kind: MacroKind, name: &str) -> String {
    format!("{}_{}", sort_priority(kind), name)
}

/// Determine whether a macro completion should generate a close-tag snippet.
///
/// Only `Block` macros get close-tag snippets. `Modifier` macros (else,
/// elseif) do NOT get close tags — they appear inside a parent block.
pub fn should_generate_close_tag(kind: MacroKind) -> bool {
    matches!(kind, MacroKind::Block)
}

// ---------------------------------------------------------------------------
// Hover helpers
// ---------------------------------------------------------------------------

/// Build the kind label for hover documentation.
///
/// Returns a human-readable string like "Control-flow macro", "Block macro",
/// "Statement macro", etc.
pub fn hover_kind_label(kind: MacroKind) -> &'static str {
    match kind {
        MacroKind::Keyword => "Keyword macro",
        MacroKind::ControlFlow => "Control-flow macro",
        MacroKind::Block => "Block macro",
        MacroKind::Modifier => "Modifier macro",
        MacroKind::Statement => "Statement macro",
        MacroKind::Identifier => "Identifier macro",
    }
}

/// Build the full hover header for a macro, including its kind label
/// and format-specific label.
///
/// Example output: `**Block macro** \`<<if>>\``
pub fn hover_header(kind: MacroKind, format_label: &str) -> String {
    format!("**{}** `{}`", hover_kind_label(kind), format_label)
}

/// Build additional hover context based on macro classification.
///
/// Adds kind-specific notes:
/// - Block: "Opens a closeable body section. Use `<</name>>` to close."
/// - Modifier: "Must appear inside a parent block macro."
/// - ControlFlow: "Alters the execution flow of the story."
/// - Identifier: "Defines a new identifier accessible in the story."
pub fn hover_kind_note(kind: MacroKind, macro_name: &str, plugin: &dyn FormatPlugin) -> Option<String> {
    match kind {
        MacroKind::Block => Some(format!(
            "Opens a closeable body section. Close with `{}`.",
            plugin.format_close_macro_label(macro_name)
        )),
        MacroKind::Modifier => Some(
            "Must appear inside a parent block macro.".to_string()
        ),
        MacroKind::ControlFlow => Some(
            "Alters the execution flow of the story.".to_string()
        ),
        MacroKind::Identifier => Some(
            "Defines a new identifier accessible in the story.".to_string()
        ),
        MacroKind::Keyword | MacroKind::Statement => None,
    }
}

// ---------------------------------------------------------------------------
// Graph/Navigation helpers
// ---------------------------------------------------------------------------

/// Determine whether a macro contributes a navigation edge in the passage
/// graph. Navigation macros (`goto`, `return`, `back`, `include`) and
/// link-like macros (`link`, `button`) produce edges.
///
/// This delegates to the format plugin's `dynamic_navigation_macros()` and
/// `passage_arg_macro_names()` sets, plus checks the Navigation and Links
/// categories.
pub fn is_navigation_macro(
    name: &str,
    mdef: &MacroDef,
    plugin: &dyn FormatPlugin,
) -> bool {
    // Dynamic navigation macros (variable-arg versions of goto, link, etc.)
    if plugin.dynamic_navigation_macros().contains(name) {
        return true;
    }

    // Macros with passage-name arguments
    if plugin.passage_arg_macro_names().contains(name) {
        return true;
    }

    // Navigation and Links categories
    matches!(mdef.category, MacroCategory::Navigation | MacroCategory::Links)
}

/// Determine whether a macro's navigation is "dynamic" (resolved at runtime
/// via a variable) vs. "static" (a hardcoded passage name).
///
/// Dynamic navigation macros like `<<goto $target>>` need special handling
/// in the passage graph — they produce conditional/virtual edges.
pub fn is_dynamic_navigation(
    name: &str,
    plugin: &dyn FormatPlugin,
) -> bool {
    plugin.dynamic_navigation_macros().contains(name)
}
