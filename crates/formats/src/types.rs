//! Shared types for format plugins.
//!
//! These types are used by both format plugin implementations and the LSP
//! server handlers. They define the data structures for macro catalogs,
//! variable sigils, implicit passage patterns, and other format-specific
//! behavioral data that handlers need to query through the FormatPlugin trait.

use std::collections::HashMap;

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

/// A format-specific builtin global object definition.
#[derive(Debug, Clone)]
pub struct GlobalDef {
    pub name: &'static str,
    pub description: &'static str,
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
}
