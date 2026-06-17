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

/// Human-readable description for a SugarCube operator.
///
/// Returns `None` for unknown operators. Covers assignment, comparison,
/// equality, and logical operators — the ones a user might hover over
/// in a `<<set>>` or `<<if>>` expression.
pub fn describe_operator(op: &str) -> Option<&'static str> {
    match op {
        // Assignment
        "to" | "into" | "=" => Some("assignment — assigns the value on the right to the variable on the left"),
        // Comparison
        "gt" => Some("greater than — true if the left value is strictly greater than the right"),
        "gte" => Some("greater than or equal — true if the left value is greater than or equal to the right"),
        "lt" => Some("less than — true if the left value is strictly less than the right"),
        "lte" => Some("less than or equal — true if the left value is less than or equal to the right"),
        // Equality
        "eq" | "is" => Some("equal — true if both values are strictly equal (===)"),
        "neq" | "isnot" => Some("not equal — true if the values are not strictly equal (!==)"),
        // Logical
        "and" => Some("logical AND — true if both operands are true"),
        "or" => Some("logical OR — true if either operand is true"),
        "not" => Some("logical NOT — inverts the boolean value of the operand"),
        _ => None,
    }
}
