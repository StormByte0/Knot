//! Variable sigils, operator normalization, and operator precedence.
//!
//! Self-contained leaf module providing SugarCube-specific variable sigil
//! information, operator mappings, and precedence data.

use crate::types::{OperatorNormalization, VariableSigilInfo};

/// SugarCube variable sigils: `$` = story (persistent), `_` = temporary.
pub fn variable_sigils() -> Vec<VariableSigilInfo> {
    vec![
        VariableSigilInfo {
            sigil: '$',
            description: "SugarCube story variable — persists across passages",
        },
        VariableSigilInfo {
            sigil: '_',
            description: "SugarCube temporary variable — scoped to the current passage",
        },
    ]
}

/// Resolve a variable sigil character to its type name.
///
/// Returns `"story"` for `$`, `"temporary"` for `_`, or `None` for unknown sigils.
pub fn resolve_variable_sigil(sigil: char) -> Option<&'static str> {
    match sigil {
        '$' => Some("story"),
        '_' => Some("temporary"),
        _ => None,
    }
}

/// Describe a variable sigil for hover documentation.
///
/// Returns a human-readable description of the sigil's meaning in SugarCube.
pub fn describe_variable_sigil(sigil: char) -> Option<&'static str> {
    match sigil {
        '$' => Some("SugarCube story variable — persists across passages"),
        '_' => Some("SugarCube temporary variable — scoped to the current passage"),
        _ => None,
    }
}

/// SugarCube operator normalization mappings (for virtual JS generation).
///
/// Maps SugarCube's English-like operators to their JavaScript equivalents.
pub fn operator_normalization() -> Vec<OperatorNormalization> {
    vec![
        OperatorNormalization { from: "to",    to: "=" },
        OperatorNormalization { from: "into",  to: "=" },
        OperatorNormalization { from: "eq",    to: "===" },
        OperatorNormalization { from: "neq",   to: "!==" },
        OperatorNormalization { from: "is",    to: "===" },
        OperatorNormalization { from: "isnot", to: "!==" },
        OperatorNormalization { from: "gt",    to: ">" },
        OperatorNormalization { from: "gte",   to: ">=" },
        OperatorNormalization { from: "lt",    to: "<" },
        OperatorNormalization { from: "lte",   to: "<=" },
        OperatorNormalization { from: "and",   to: "&&" },
        OperatorNormalization { from: "or",    to: "||" },
        OperatorNormalization { from: "not",   to: "!" },
    ]
}

/// SugarCube operator precedence (lower number = lower precedence).
pub fn operator_precedence() -> Vec<(&'static str, u8)> {
    vec![
        ("to", 0),
        ("or", 1),
        ("and", 2),
        ("eq", 3),
        ("neq", 3),
        ("is", 3),
        ("isnot", 3),
        ("gt", 4),
        ("gte", 4),
        ("lt", 4),
        ("lte", 4),
    ]
}

/// SugarCube assignment operators.
pub fn assignment_operators() -> Vec<&'static str> {
    vec!["to", "into", "="]
}

/// SugarCube comparison operators.
pub fn comparison_operators() -> Vec<&'static str> {
    vec!["gt", "gte", "lt", "lte"]
}
