//! SugarCube-specific JavaScript pre-processing before Oxc parsing.
//!
//! SugarCube's JS contexts contain syntax that is NOT valid JavaScript:
//! - `$varName` variable references (dollar-prefixed identifiers)
//! - Operator aliases: `to` → `=`, `eq` → `===`, `neq` → `!==`, etc.
//!
//! Before sending JS snippets to `knot_core::oxc::parse_js()`, we must
//! transform these into valid JavaScript so Oxc can parse them.
//!
//! ## Variable substitution
//!
//! `$varName` → `State_variables_varName`
//!
//! This produces valid JS identifiers. The offset mapping is tracked so
//! diagnostics from Oxc can be mapped back to the original SugarCube
//! source positions.
//!
//! ## Operator substitution
//!
//! SugarCube operator aliases are replaced with their JS equivalents:
//! - `to` → `=`
//! - `eq` → `===`
//! - `neq` → `!==`
//! - `gt` → `>`
//! - `gte` → `>=`
//! - `lt` → `<`
//! - `lte` → `<=`
//! - `and` → `&&`
//! - `or` → `||`
//! - `is` → `===`
//!
//! This is a best-effort transformation — it handles the most common cases
//! where these tokens appear as standalone words outside string literals.

use std::ops::Range;

// ---------------------------------------------------------------------------
// Variable substitution: $var → State_variables_varName
// ---------------------------------------------------------------------------

/// Result of substituting SugarCube `$var` references with valid JS identifiers.
pub(crate) struct SubstitutedSource {
    /// The substituted JavaScript source text.
    pub js_source: String,
    /// Mapping from substituted text offsets back to original offsets.
    /// Each entry is (substituted_offset, original_offset).
    pub offset_map: Vec<(usize, usize)>,
}

impl SubstitutedSource {
    /// Substitute `$var` references in a SugarCube JS snippet.
    ///
    /// `$varName` → `State_variables_varName` (valid JS identifier).
    /// `_varName` → `_varName` (already valid JS, no change).
    ///
    /// Also tracks the offset mapping for diagnostic position mapping.
    pub fn new(source: &str, base_offset: usize) -> Self {
        let mut js_source = String::with_capacity(source.len() + 64);
        let mut offset_map = Vec::new();
        let mut src_pos = 0;

        let bytes = source.as_bytes();
        while src_pos < bytes.len() {
            if bytes[src_pos] == b'$' && src_pos + 1 < bytes.len() {
                let next = bytes[src_pos + 1];
                if is_js_ident_start(next) {
                    // Record the mapping for the start of this variable reference
                    offset_map.push((js_source.len(), base_offset + src_pos));

                    // Replace $ with "State_variables_"
                    js_source.push_str("State_variables_");

                    // Copy the variable name characters
                    let name_start = src_pos + 1;
                    let mut name_end = name_start;
                    while name_end < bytes.len() && is_js_ident_char(bytes[name_end]) {
                        name_end += 1;
                    }

                    let var_name = &source[name_start..name_end];
                    for (i, ch) in var_name.char_indices() {
                        offset_map.push((js_source.len(), base_offset + name_start + i));
                        js_source.push(ch);
                    }

                    src_pos = name_end;
                    continue;
                }
            }

            // Regular character — just copy with offset mapping
            offset_map.push((js_source.len(), base_offset + src_pos));
            let ch = source[src_pos..].chars().next().unwrap();
            js_source.push(ch);
            src_pos += ch.len_utf8();
        }

        Self { js_source, offset_map }
    }

    /// Map an offset in the substituted text back to the original document offset.
    pub fn map_offset_back(&self, substituted_offset: usize) -> usize {
        let mut result = substituted_offset;
        for &(sub_off, orig_off) in &self.offset_map {
            if sub_off <= substituted_offset {
                result = orig_off + (substituted_offset - sub_off);
            } else {
                break;
            }
        }
        result
    }

    /// Map a byte range from substituted text back to original document range.
    pub fn map_range_back(&self, substituted_range: &Range<usize>) -> Range<usize> {
        self.map_offset_back(substituted_range.start)..self.map_offset_back(substituted_range.end)
    }
}

fn is_js_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn is_js_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

// ---------------------------------------------------------------------------
// Operator substitution: SugarCube aliases → JS operators
// ---------------------------------------------------------------------------

/// Result of substituting SugarCube operator aliases with JS operators.
pub(crate) struct OperatorSubstitution {
    /// The source with operators substituted.
    pub js_source: String,
    /// Whether any substitution was made (for diagnostic messages).
    #[allow(dead_code)] // for future precise diagnostic ranges
    pub substitutions: Vec<(Range<usize>, String)>,
}

impl OperatorSubstitution {
    /// Substitute SugarCube operator aliases in the source text.
    ///
    /// Handles: `to`, `eq`, `neq`, `gt`, `gte`, `lt`, `lte`, `and`, `or`, `is`.
    /// Only replaces tokens that appear as standalone words outside string
    /// literals.
    pub fn new(source: &str) -> Self {
        let mut result = source.to_string();
        let mut substitutions = Vec::new();

        // SugarCube operator aliases → JS operators.
        // Order matters: "gte" before "gt", "lte" before "lt", etc.
        let replacements: &[(&str, &str)] = &[
            (" to ", " = "),
            (" eq ", " === "),
            (" neq ", " !== "),
            (" gte ", " >= "),
            (" lte ", " <= "),
            (" gt ", " > "),
            (" lt ", " < "),
            (" and ", " && "),
            (" or ", " || "),
            (" is ", " === "),
        ];

        for (sc_op, js_op) in replacements {
            // Simple whole-word replacement — not perfect but handles
            // the vast majority of real usage.
            if let Some(idx) = find_operator(&result, sc_op) {
                let before = result[..idx].to_string();
                let after = result[idx + sc_op.len()..].to_string();
                let orig_range = idx..(idx + sc_op.len());
                substitutions.push((orig_range, js_op.trim().to_string()));
                result = format!("{}{}{}", before, js_op, after);
            }
        }

        Self {
            js_source: result,
            substitutions,
        }
    }
}

/// Find a SugarCube operator in the source, skipping string literals.
fn find_operator(source: &str, op: &str) -> Option<usize> {
    let mut in_string: Option<u8> = None;
    let mut i = 0;
    let bytes = source.as_bytes();

    while i + op.len() <= source.len() {
        let b = bytes[i];

        // Handle string delimiters
        if in_string.is_none() && (b == b'"' || b == b'\'' || b == b'`') {
            in_string = Some(b);
            i += 1;
            continue;
        }
        if let Some(quote) = in_string {
            if b == b'\\' {
                i += 2; // skip escaped char
                continue;
            }
            if b == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        // Check for operator match
        if source[i..].starts_with(op) {
            return Some(i);
        }
        i += 1;
    }

    None
}

// ---------------------------------------------------------------------------
// Full pre-processing pipeline
// ---------------------------------------------------------------------------

/// Pre-process a SugarCube JS snippet for Oxc parsing.
///
/// Applies both variable substitution and operator substitution,
/// returning the final JS source and an offset mapper.
pub(crate) fn preprocess_sugarcube_js(source: &str, base_offset: usize) -> PreprocessedJs {
    // Step 1: Substitute $var references
    let var_subst = SubstitutedSource::new(source, base_offset);

    // Step 2: Substitute SugarCube operator aliases
    let op_subst = OperatorSubstitution::new(&var_subst.js_source);

    PreprocessedJs {
        js_source: op_subst.js_source,
        offset_mapper: var_subst,
    }
}

/// The result of SugarCube JS pre-processing.
pub(crate) struct PreprocessedJs {
    /// The final JavaScript source ready for Oxc parsing.
    pub js_source: String,
    /// The offset mapper from the variable substitution step.
    /// Can be used to map Oxc diagnostic positions back to original
    /// SugarCube source positions.
    pub offset_mapper: SubstitutedSource,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitution_simple_var() {
        let sub = SubstitutedSource::new("$x + 1", 10);
        assert_eq!(sub.js_source, "State_variables_x + 1");
    }

    #[test]
    fn test_substitution_multiple_vars() {
        let sub = SubstitutedSource::new("$x + $y", 10);
        assert_eq!(sub.js_source, "State_variables_x + State_variables_y");
    }

    #[test]
    fn test_substitution_dot_path() {
        let sub = SubstitutedSource::new("$item.sword.damage", 10);
        assert_eq!(sub.js_source, "State_variables_item.sword.damage");
    }

    #[test]
    fn test_substitution_temp_var() {
        let sub = SubstitutedSource::new("_temp + $x", 10);
        // _temp stays as-is, $x gets substituted
        assert_eq!(sub.js_source, "_temp + State_variables_x");
    }

    #[test]
    fn test_substitution_no_vars() {
        let sub = SubstitutedSource::new("1 + 2 * 3", 10);
        assert_eq!(sub.js_source, "1 + 2 * 3");
    }

    #[test]
    fn test_offset_mapping() {
        let sub = SubstitutedSource::new("$x + 1", 100);
        // "State_variables_x" is 19 chars, replacing "$x" (2 chars)
        // The "+ 1" part should still map back to the original offset
        let plus_offset = sub.js_source.find('+').unwrap();
        let mapped = sub.map_offset_back(plus_offset);
        // '+' was at position 3 in original, so mapped should be 100 + 3 = 103
        assert_eq!(mapped, 103);
    }

    #[test]
    fn test_operator_substitution() {
        let sub = OperatorSubstitution::new("$x to 5");
        assert!(sub.js_source.contains('='));
        assert!(!sub.js_source.contains(" to "));
    }

    #[test]
    fn test_full_preprocess() {
        let result = preprocess_sugarcube_js("$x to 5", 0);
        assert!(result.js_source.contains("State_variables_x"));
        assert!(result.js_source.contains('='));
    }
}
