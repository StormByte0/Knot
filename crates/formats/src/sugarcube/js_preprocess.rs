//! SugarCube `$var` preprocessor for oxc JS parsing.
//!
//! SugarCube's `$var` syntax is NOT valid JavaScript — `$hp` is a valid
//! JS identifier, but `$player.name` works fine, and `$_temp` is fine too.
//! The main problem is that in macro args like `<<set $hp to 100>>`, the
//! `to` keyword is SugarCube syntax, not JS. And in expressions like
//! `$hp + $gold`, the variables are fine JS but the semantics differ.
//!
//! This module handles the pre-processing needed before passing JS to oxc:
//!
//! 1. **$var substitution**: Replace `$var` with a valid JS identifier
//!    so oxc can parse it. The mapping is tracked so we can map positions
//!    back to the original source.
//! 2. **`to` → `=` replacement**: SugarCube's `<<set $x to 5>>` uses
//!    `to` as the assignment operator. We replace it with `=` for oxc.
//!
//! ## Design
//!
//! The preprocessor is intentionally simple — it does NOT try to fully
//! understand SugarCube's expression syntax. It only does enough
//! substitution to make oxc produce a valid AST that we can then walk
//! for variable/macro/function extraction.

use std::ops::Range;

// ---------------------------------------------------------------------------
// Substitution map — tracks replacements for position mapping
// ---------------------------------------------------------------------------

/// A single substitution made during preprocessing.
#[derive(Debug, Clone)]
pub struct Substitution {
    /// Byte range in the ORIGINAL source text that was replaced.
    pub original_range: Range<usize>,
    /// Byte range in the PREPROCESSED source text (after substitution).
    pub processed_range: Range<usize>,
    /// What the original text was.
    pub original_text: String,
    /// What it was replaced with.
    pub replacement: String,
}

/// The result of preprocessing a JS snippet.
#[derive(Debug, Clone)]
pub struct PreprocessedJs {
    /// The preprocessed JS source text (safe to pass to oxc).
    pub source: String,
    /// All substitutions made, in source order.
    pub substitutions: Vec<Substitution>,
}

impl PreprocessedJs {
    /// Map a byte position from the preprocessed text back to the original.
    ///
    /// This is needed for mapping oxc diagnostics and AST node positions
    /// back to the original SugarCube source.
    pub fn map_to_original(&self, processed_pos: usize) -> usize {
        let mut offset = 0isize;
        for sub in &self.substitutions {
            if processed_pos >= sub.processed_range.end {
                // This substitution is entirely before our position
                offset += sub.original_text.len() as isize - sub.replacement.len() as isize;
            } else if processed_pos >= sub.processed_range.start {
                // Our position is inside this substitution — map to start
                return sub.original_range.start;
            } else {
                // Our position is before this substitution — no adjustment needed yet
                break;
            }
        }
        (processed_pos as isize + offset) as usize
    }

    /// Map a byte range from the preprocessed text back to the original.
    pub fn map_range_to_original(&self, processed_range: Range<usize>) -> Range<usize> {
        let start = self.map_to_original(processed_range.start);
        let end = self.map_to_original(processed_range.end);
        start..end
    }
}

// ---------------------------------------------------------------------------
// Preprocessor
// ---------------------------------------------------------------------------

/// Preprocess a SugarCube JS expression for oxc parsing.
///
/// Handles:
/// - `$var` → `State_variables_varName` (valid JS identifier)
/// - `_var` → `State_temporary_varName` (valid JS identifier)
/// - `to` assignment → `=` (in specific contexts)
///
/// Returns a `PreprocessedJs` with the transformed source and substitution
/// map for position mapping.
pub fn preprocess_for_oxc(source: &str) -> PreprocessedJs {
    let mut result = String::with_capacity(source.len() + 64);
    let mut substitutions = Vec::new();
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut result_offset = 0usize;

    while i < len {
        // Check for $$ (escaped dollar — not a variable)
        if bytes[i] == b'$' && i + 1 < len && bytes[i + 1] == b'$' {
            result.push_str("$$");
            i += 2;
            result_offset += 2;
            continue;
        }

        // Check for $var (story variable)
        if bytes[i] == b'$' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            let start = i;
            let var_start = i;
            i += 1; // skip $
            let name_start = i;
            while i < len && is_ident_char(bytes[i]) {
                i += 1;
            }
            let name = &source[name_start..i];

            // Scan dot-notation property path
            let mut property_path = String::new();
            while i < len && bytes[i] == b'.' {
                i += 1;
                let prop_start = i;
                while i < len && is_ident_char(bytes[i]) {
                    i += 1;
                }
                if i > prop_start {
                    property_path.push('.');
                    property_path.push_str(&source[prop_start..i]);
                }
            }

            let original_text = source[var_start..i].to_string();
            let replacement = format!("State_variables_{}{}", name, property_path.replace('.', "_"));

            let original_range = start..i;
            let processed_start = result_offset;
            result.push_str(&replacement);
            result_offset += replacement.len();

            substitutions.push(Substitution {
                original_range,
                processed_range: processed_start..result_offset,
                original_text,
                replacement,
            });
            continue;
        }

        // Check for _var (temporary variable) at word boundary
        if bytes[i] == b'_' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            let is_word_boundary = i == 0
                || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_';
            if is_word_boundary {
                let start = i;
                i += 1; // skip _
                let name_start = i;
                while i < len && is_ident_char(bytes[i]) {
                    i += 1;
                }
                // Must have at least one char after _
                if i > name_start {
                    let name = &source[name_start..i];
                    let original_text = source[start..i].to_string();
                    let replacement = format!("State_temporary_{}", name);

                    let original_range = start..i;
                    let processed_start = result_offset;
                    result.push_str(&replacement);
                    result_offset += replacement.len();

                    substitutions.push(Substitution {
                        original_range,
                        processed_range: processed_start..result_offset,
                        original_text,
                        replacement,
                    });
                    continue;
                }
                // If it's just a bare underscore, fall through
                i = name_start; // reset
            }
        }

        // Regular character — copy through
        result.push(bytes[i] as char);
        i += 1;
        result_offset += 1;
    }

    // Replace `to` with `=` in assignment contexts
    let result = replace_to_with_equals(&result, &mut substitutions);

    PreprocessedJs {
        source: result,
        substitutions,
    }
}

/// Replace `to` keyword with `=` for assignment contexts.
///
/// SugarCube uses `<<set $x to 5>>` where `to` is the assignment operator.
/// In JS, this would be `$x = 5`. We replace `to` with `=` when it appears
/// between a variable reference and a value expression.
///
/// This is a simplified heuristic — it looks for patterns like:
/// - `State_variables_x to ` → `State_variables_x = `
/// - `State_temporary_x to ` → `State_temporary_x = `
fn replace_to_with_equals(source: &str, substitutions: &mut Vec<Substitution>) -> String {
    // Look for " to " that follows a State_variables_ or State_temporary_ reference
    // This is intentionally conservative to avoid false positives
    let mut result = source.to_string();

    // Simple pattern: " to " preceded by a variable-like identifier
    // and followed by a value expression
    let pattern = " to ";
    let mut offset = 0isize;

    while let Some(pos) = result[(offset.max(0) as usize)..].find(pattern) {
        let actual_pos = (offset.max(0) as usize) + pos;

        // Check if preceded by something that looks like a variable reference
        let before = &result[..actual_pos];
        let looks_like_assignment = before.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_');

        if looks_like_assignment {
            // Check if followed by a value expression (not another keyword)
            let after = &result[actual_pos + pattern.len()..];
            let followed_by_value = after.starts_with(|c: char| {
                c.is_ascii_alphanumeric() || c == '"' || c == '\'' || c == '(' || c == '{' || c == '[' || c == '!'
            });

            if followed_by_value {
                // Replace " to " with " = "
                let original_range = actual_pos..actual_pos + pattern.len();
                let replacement = " = ";
                result.replace_range(original_range.clone(), replacement);

                substitutions.push(Substitution {
                    original_range: original_range.start..original_range.end,
                    processed_range: original_range.start..original_range.start + replacement.len(),
                    original_text: " to ".to_string(),
                    replacement: " = ".to_string(),
                });

                // Adjust offset for the length change
                offset = (actual_pos + replacement.len()) as isize;
                continue;
            }
        }

        offset = (actual_pos + pattern.len()) as isize;
    }

    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_variable_substitution() {
        let result = preprocess_for_oxc("$hp + $gold");
        assert!(result.source.contains("State_variables_hp"));
        assert!(result.source.contains("State_variables_gold"));
    }

    #[test]
    fn property_path_substitution() {
        let result = preprocess_for_oxc("$player.name");
        assert!(result.source.contains("State_variables_player_name"));
    }

    #[test]
    fn temporary_variable_substitution() {
        let result = preprocess_for_oxc("_i + _count");
        assert!(result.source.contains("State_temporary_i"));
        assert!(result.source.contains("State_temporary_count"));
    }

    #[test]
    fn escaped_dollar_not_substituted() {
        let result = preprocess_for_oxc("$$notavar");
        assert!(result.source.contains("$$notavar"));
        assert!(!result.source.contains("State_variables_"));
    }

    #[test]
    fn to_replacement() {
        let result = preprocess_for_oxc("State_variables_hp to 100");
        assert!(result.source.contains("= 100"));
        assert!(!result.source.contains(" to "));
    }

    #[test]
    fn position_mapping() {
        let result = preprocess_for_oxc("$hp to 5");
        // $hp is replaced with State_variables_hp (longer)
        // "to" is replaced with "=" (shorter)
        // The substitution map should let us map back
        assert!(!result.substitutions.is_empty());
    }

    #[test]
    fn mixed_content() {
        let result = preprocess_for_oxc("$x + 1 + $y.name");
        assert!(result.source.contains("State_variables_x"));
        assert!(result.source.contains("State_variables_y_name"));
    }
}
