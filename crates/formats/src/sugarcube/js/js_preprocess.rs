//! SugarCube `$var` and operator preprocessor for oxc JS parsing.
//!
//! SugarCube's expression syntax extends JavaScript with several keyword
//! operators that oxc cannot parse natively. This module handles all the
//! pre-processing needed before passing expressions to oxc:
//!
//! 1. **$var substitution**: Replace `$var` / `_var` with valid JS identifiers
//!    (`State_variables_varName` / `State_temporary_varName`) so oxc can parse
//!    them. The mapping is tracked for position back-mapping.
//!
//! 2. **SugarCube operator normalization**: Replace SugarCube keyword operators
//!    with their JavaScript equivalents. The full set of operators (from
//!    `macros::operator_normalization()`) is:
//!
//!    | SugarCube | JS   | Context          |
//!    |-----------|------|------------------|
//!    | `to`      | `=`  | Assignment       |
//!    | `eq`      | `===`| Equality         |
//!    | `neq`     | `!==`| Inequality       |
//!    | `is`      | `===`| Equality         |
//!    | `isnot`   | `!==`| Inequality       |
//!    | `gt`      | `>`  | Comparison       |
//!    | `gte`     | `>=` | Comparison       |
//!    | `lt`      | `<`  | Comparison       |
//!    | `lte`     | `<=` | Comparison       |
//!    | `and`     | `&&` | Logical AND      |
//!    | `or`      | `||` | Logical OR       |
//!    | `not`     | `!`  | Logical NOT      |
//!
//! ## Parsing boundary
//!
//! The SugarCube parser and oxc have a clear division of responsibility:
//!
//! - **SugarCube parser owns**: `<<` macro-name operator `>>` (macro structure)
//! - **oxc owns**: the expression content between the structural delimiters
//!
//! For `<<set $x to 5>>`, the SugarCube parser extracts `target=$x`,
//! `operator=To`, `expression="5"`, and only `"5"` goes to oxc.
//!
//! For `<<if $x gt 5 and $y lt 10>>`, the SugarCube parser extracts the
//! condition string `"$x gt 5 and $y lt 10"`, which goes through this
//! preprocessor to become `State_variables_x > 5 && State_variables_y < 10`
//! before oxc sees it.
//!
//! ## String literal handling
//!
//! All substitution and normalization is **disabled inside string literals**
//! (both `"..."` and `'...'`). This prevents:
//! - `$var` inside strings from being substituted (it's literal text)
//! - Keyword operators inside strings from being replaced
//!
//! ## Known limitations
//!
//! The `not` operator's precedence differs between SugarCube and JS.
//! SugarCube's `not` has lower precedence than comparison operators, so
//! `not $x gt 5` means `!($x > 5)`. But after text replacement, oxc sees
//! `!State_variables_x > 5`, which parses as `(!State_variables_x) > 5`.
//! Users should use explicit parentheses for such expressions.

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
    /// Offset to add to all `map_to_original()` results.
    ///
    /// For script passages, this is `0` because the preprocessor operates on
    /// the full passage body text, so `map_to_original()` already returns
    /// passage-body-relative positions.
    ///
    /// For inline JS snippets (extracted from `<<run>>`, `<<set>>`, etc.),
    /// the preprocessor operates on the snippet text, so `map_to_original()`
    /// returns snippet-relative positions. Setting `origin_offset` to the
    /// snippet's `body_offset` shifts results to passage-body-relative.
    pub origin_offset: usize,
    /// Number of characters prepended by `parse_js` before the actual source.
    ///
    /// When oxc parses in Expression mode, `parse_js` wraps the source as
    /// `(source)`, adding 1 character of prefix. All oxc AST spans are
    /// relative to this wrapped source, so we must subtract `wrapping_offset`
    /// before mapping positions back to the preprocessed source.
    ///
    /// - Module mode / StatementList: `0` (no wrapping)
    /// - Expression mode: `1` (the opening paren)
    pub wrapping_offset: usize,
}

impl PreprocessedJs {
    /// Map a byte position from the oxc AST back to the original source.
    ///
    /// This is needed for mapping oxc diagnostics and AST node positions
    /// back to the original SugarCube source.
    ///
    /// The input `oxc_pos` is a byte position from oxc's AST spans (which
    /// are relative to the source text passed to the parser, including any
    /// wrapping for Expression mode). This method:
    /// 1. Subtracts `wrapping_offset` to get a position in the preprocessed source
    /// 2. Applies substitution mapping to get a position in the original source
    /// 3. Adds `origin_offset` to shift from snippet-relative to passage-body-relative
    pub fn map_to_original(&self, oxc_pos: usize) -> usize {
        // Step 1: Remove wrapping offset (for Expression mode, oxc wraps as `(source)`)
        let processed_pos = oxc_pos.saturating_sub(self.wrapping_offset);

        // Step 2: Map through substitution table
        let mut offset = 0isize;
        for sub in &self.substitutions {
            if processed_pos >= sub.processed_range.end {
                // This substitution is entirely before our position
                offset += sub.original_text.len() as isize - sub.replacement.len() as isize;
            } else if processed_pos >= sub.processed_range.start {
                // Our position is inside this substitution — map to start
                return sub.original_range.start + self.origin_offset;
            } else {
                // Our position is before this substitution — no adjustment needed yet
                break;
            }
        }
        // Step 3: Apply origin_offset for passage-body-relative result.
        // The offset can be negative (when replacements shrink the source),
        // so we must guard against underflow. If the math would go negative,
        // we clamp to 0 — this indicates a bug in substitution tracking,
        // but it's better than panicking in production.
        let adjusted = (processed_pos as isize)
            .checked_add(offset)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or_else(|| {
                tracing::warn!(
                    "PreprocessedJs::map_to_original: position underflow (processed_pos={}, offset={}), clamping to 0",
                    processed_pos, offset
                );
                0
            });
        adjusted + self.origin_offset
    }

    /// Map a byte range from the oxc AST back to the original source.
    pub fn map_range_to_original(&self, oxc_range: Range<usize>) -> Range<usize> {
        let start = self.map_to_original(oxc_range.start);
        let end = self.map_to_original(oxc_range.end);
        start..end
    }
}

// ---------------------------------------------------------------------------
// Preprocessor
// ---------------------------------------------------------------------------

/// Preprocess a SugarCube JS expression for oxc parsing.
///
/// Handles in a single left-to-right pass:
/// - `$var` → `State_variables_varName` (valid JS identifier)
/// - `_var` → `State_temporary_varName` (valid JS identifier)
/// - SugarCube keyword operators → JS equivalents (gt→>, is→===, etc.)
/// - String literals are copied as-is (no substitution inside them)
///
/// Returns a `PreprocessedJs` with the transformed source and substitution
/// map for position mapping.
///
/// When `sugarcube_syntax` is `false`, the source is returned unchanged
/// with an empty substitution map. This is used for standalone `.js` files
/// where `$` is a valid JS identifier character (jQuery, etc.) and
/// SugarCube keyword operators (`to`, `is`, `eq`) would incorrectly
/// mangle valid JS identifiers.
pub fn preprocess_for_oxc(source: &str, sugarcube_syntax: bool) -> PreprocessedJs {
    if !sugarcube_syntax {
        // Pure JS mode — no SugarCube-specific preprocessing. Return the
        // source unchanged with an empty substitution map. The position
        // mapping in `PreprocessedJs` handles the no-substitution case
        // correctly (offsets are identity).
        return PreprocessedJs {
            source: source.to_string(),
            substitutions: Vec::new(),
            origin_offset: 0,
            wrapping_offset: 0,
        };
    }
    preprocess_for_oxc_sugarcube(source)
}

/// SugarCube-specific preprocessor — the original implementation, now
/// called only when `sugarcube_syntax: true`.
fn preprocess_for_oxc_sugarcube(source: &str) -> PreprocessedJs {
    let mut result = String::with_capacity(source.len() + 64);
    let mut substitutions = Vec::new();
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut result_offset = 0usize;

    while i < len {
        let b = bytes[i];

        // ── Block comments: /* ... */ ───────────────────────────────
        // Copy as-is with no substitution. This prevents $var and
        // keyword operators inside comments from being processed.
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            result.push_str("/*");
            i += 2;
            result_offset += 2;
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    result.push_str("*/");
                    i += 2;
                    result_offset += 2;
                    break;
                }
                if bytes[i] < 0x80 {
                    result.push(bytes[i] as char);
                    i += 1;
                    result_offset += 1;
                } else {
                    let consumed = push_utf8_char(source, bytes, i, &mut result);
                    i += consumed;
                    result_offset += consumed;
                }
            }
            continue;
        }

        // ── Line comments: // ... ───────────────────────────────────
        // Copy as-is with no substitution until end of line.
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            result.push_str("//");
            i += 2;
            result_offset += 2;
            while i < len {
                if bytes[i] == b'\n' {
                    result.push('\n');
                    i += 1;
                    result_offset += 1;
                    break;
                }
                if bytes[i] < 0x80 {
                    result.push(bytes[i] as char);
                    i += 1;
                    result_offset += 1;
                } else {
                    let consumed = push_utf8_char(source, bytes, i, &mut result);
                    i += consumed;
                    result_offset += consumed;
                }
            }
            continue;
        }

        // ── String literals: copy as-is (no substitution) ──────────────
        // Both $var substitution and operator normalization must skip
        // string content to avoid corrupting literal text like "$hp" or
        // "greater than five" where keyword fragments appear inside strings.
        if b == b'"' || b == b'\'' {
            let quote = b;
            result.push(b as char); // opening quote is always ASCII
            i += 1;
            result_offset += 1;
            while i < len {
                if bytes[i] == b'\\' && i + 1 < len {
                    // Escaped character — copy the backslash (ASCII) ...
                    result.push('\\');
                    i += 1;
                    result_offset += 1;
                    // ... then copy the next character (may be multi-byte UTF-8)
                    let consumed = push_utf8_char(source, bytes, i, &mut result);
                    i += consumed;
                    result_offset += consumed;
                    continue;
                }
                let cb = bytes[i];
                if cb < 0x80 {
                    // ASCII character — can check for closing quote
                    result.push(cb as char);
                    i += 1;
                    result_offset += 1;
                    if cb == quote {
                        break; // Closing quote found
                    }
                } else {
                    // Multi-byte UTF-8 — copy full character, advance properly
                    let consumed = push_utf8_char(source, bytes, i, &mut result);
                    i += consumed;
                    result_offset += consumed;
                    // Multi-byte chars can never be the closing quote (which is ASCII)
                }
            }
            continue;
        }

        // ── $$ escaped dollar ──────────────────────────────────────────
        if b == b'$' && i + 1 < len && bytes[i + 1] == b'$' {
            result.push_str("$$");
            i += 2;
            result_offset += 2;
            continue;
        }

        // ── $var story variable ────────────────────────────────────────
        if b == b'$' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            let start = i;
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

            let original_text = source[start..i].to_string();
            // Replace both `.` and `-` in the property path with `_` so the
            // result is a valid JS identifier. SugarCube allows hyphens in
            // property names (e.g. $obj.my-prop), but JS identifiers can't
            // contain hyphens — so we normalize them for oxc.
            let normalized_path = property_path.replace('.', "_").replace('-', "_");
            let replacement =
                format!("State_variables_{}{}", name, normalized_path);

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

        // ── _var temporary variable ────────────────────────────────────
        if b == b'_' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            let is_word_boundary = i == 0
                || (!bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_');
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
                // Just a bare underscore — reset and fall through
                i = name_start;
            }
        }

        // ── SugarCube keyword operators ────────────────────────────────
        // Replace SugarCube-specific operators with their JavaScript
        // equivalents. Only matches at word boundaries to avoid false
        // positives inside identifiers (e.g., "gt" in "$target").
        if let Some((_keyword, js_equiv, kw_len)) =
            match_sugarcube_operator(&source[i..], i, bytes, len)
        {
            let original_text = source[i..i + kw_len].to_string();
            let original_range = i..i + kw_len;
            let processed_start = result_offset;
            result.push_str(js_equiv);
            result_offset += js_equiv.len();

            substitutions.push(Substitution {
                original_range,
                processed_range: processed_start..result_offset,
                original_text,
                replacement: js_equiv.to_string(),
            });
            i += kw_len;
            continue;
        }

        // ── Regular character (possibly multi-byte UTF-8) ───────────
        if b < 0x80 {
            // ASCII — fast path
            result.push(b as char);
            i += 1;
            result_offset += 1;
        } else {
            // Multi-byte UTF-8 character — must advance by full char width
            // to keep `i` on a char boundary for subsequent &source[i..] slices.
            let consumed = push_utf8_char(source, bytes, i, &mut result);
            i += consumed;
            result_offset += consumed;
        }
    }

    PreprocessedJs {
        source: result,
        substitutions,
        origin_offset: 0,
        wrapping_offset: 0,
    }
}

// ---------------------------------------------------------------------------
// SugarCube operator matching
// ---------------------------------------------------------------------------

/// Try to match a SugarCube keyword operator at position `i` in the source.
///
/// Returns `Some((keyword, js_equivalent, keyword_length))` if a SugarCube
/// operator is found at a word boundary. Returns `None` otherwise.
///
/// Operators are checked longest-first to avoid partial matches (e.g.,
/// `isnot` before `is`, `gte` before `gt`, `lte` before `lt`).
///
/// Word boundary rules:
/// - **Before**: the character before position `i` must NOT be an ident char
///   (or `i` must be 0). This prevents matching "gt" inside "$target".
/// - **After**: the character after the keyword must NOT be an ident char
///   (or the keyword must end at the source boundary). This prevents
///   matching "is" inside "island" or "or" inside "order".
fn match_sugarcube_operator(
    source_from_i: &str,
    i: usize,
    bytes: &[u8],
    len: usize,
) -> Option<(&'static str, &'static str, usize)> {
    // SugarCube keyword operators sorted by length (longest first).
    // This ensures that longer operators are matched before their
    // shorter prefixes (e.g., "isnot" before "is", "gte" before "gt").
    //
    // These are the same mappings defined in macros::operator_normalization(),
    // but inlined here as a static list for efficient matching in the
    // preprocessor's hot loop.
    const OPERATORS: &[(&str, &str)] = &[
        ("isnot", "!=="),
        ("and", "&&"),
        ("not", "!"),
        ("gte", ">="),
        ("lte", "<="),
        ("neq", "!=="),
        ("eq", "==="),
        ("is", "==="),
        ("gt", ">"),
        ("lt", "<"),
        ("or", "||"),
        ("to", "="),
    ];

    // Quick check: the first character of any SugarCube keyword operator
    // is always a letter (a-z). If the current byte isn't a letter, we
    // can skip the full scan.
    let first = bytes[i];
    if !first.is_ascii_alphabetic() {
        return None;
    }

    // Check word boundary before: previous character must not be an ident char.
    // This prevents matching "gt" inside "$target" or "is" inside "this".
    if i > 0 && is_ident_char(bytes[i - 1]) {
        return None;
    }

    for &(keyword, js_equiv) in OPERATORS {
        if source_from_i.starts_with(keyword) {
            let kw_len = keyword.len();
            // Check word boundary after: next character must not be an ident char.
            // This prevents matching "is" inside "island" or "or" inside "order".
            if i + kw_len >= len || !is_ident_char(bytes[i + kw_len]) {
                return Some((keyword, js_equiv, kw_len));
            }
        }
    }

    None
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

/// Return the expected byte length of the UTF-8 character starting at `bytes[i]`.
///
/// Uses the leading byte pattern per RFC 3629:
/// - `0xxxxxxx` → 1 byte (ASCII)
/// - `110xxxxx` → 2 bytes
/// - `1110xxxx` → 3 bytes
/// - `11110xxx` → 4 bytes
///
/// The result is clamped to the remaining bytes in `bytes` from position `i`
/// to avoid out-of-bounds access on malformed input.
fn utf8_char_len(bytes: &[u8], i: usize) -> usize {
    let b = bytes[i];
    let expected = if b < 0x80 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    };
    expected.min(bytes.len() - i)
}

/// Push the next full UTF-8 character from `source` (starting at byte `i`)
/// onto `result`, and return the number of bytes consumed.
///
/// This is the UTF-8-safe replacement for `result.push(bytes[i] as char); i += 1`.
fn push_utf8_char(source: &str, bytes: &[u8], i: usize, result: &mut String) -> usize {
    if bytes[i] < 0x80 {
        // ASCII fast path — single byte, always a char boundary
        result.push(bytes[i] as char);
        1
    } else {
        // Multi-byte UTF-8 — must slice at char boundaries
        let char_len = utf8_char_len(bytes, i);
        let char_end = i + char_len;
        // source[i..char_end] is safe because utf8_char_len returns the
        // correct number of bytes for a valid UTF-8 sequence starting at i.
        result.push_str(&source[i..char_end]);
        char_len
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Variable substitution tests ────────────────────────────────────

    #[test]
    fn simple_variable_substitution() {
        let result = preprocess_for_oxc("$hp + $gold", true);
        assert!(result.source.contains("State_variables_hp"));
        assert!(result.source.contains("State_variables_gold"));
    }

    #[test]
    fn property_path_substitution() {
        let result = preprocess_for_oxc("$player.name", true);
        assert!(result.source.contains("State_variables_player_name"));
    }

    #[test]
    fn temporary_variable_substitution() {
        let result = preprocess_for_oxc("_i + _count", true);
        assert!(result.source.contains("State_temporary_i"));
        assert!(result.source.contains("State_temporary_count"));
    }

    #[test]
    fn escaped_dollar_not_substituted() {
        let result = preprocess_for_oxc("$$notavar", true);
        assert!(result.source.contains("$$notavar"));
        assert!(!result.source.contains("State_variables_"));
    }

    #[test]
    fn mixed_content() {
        let result = preprocess_for_oxc("$x + 1 + $y.name", true);
        assert!(result.source.contains("State_variables_x"));
        assert!(result.source.contains("State_variables_y_name"));
    }

    // ── Operator normalization tests ───────────────────────────────────

    #[test]
    fn to_replacement() {
        let result = preprocess_for_oxc("$hp to 100", true);
        assert!(
            result.source.contains("State_variables_hp = 100"),
            "expected 'State_variables_hp = 100', got '{}'",
            result.source
        );
    }

    #[test]
    fn gt_lt_replacement() {
        let result = preprocess_for_oxc("$x gt 5 and $y lt 10", true);
        assert!(
            result.source.contains("State_variables_x > 5"),
            "expected '> 5', got '{}'",
            result.source
        );
        assert!(
            result.source.contains("State_variables_y < 10"),
            "expected '< 10', got '{}'",
            result.source
        );
        assert!(
            result.source.contains("&&"),
            "expected '&&', got '{}'",
            result.source
        );
    }

    #[test]
    fn gte_lte_replacement() {
        let result = preprocess_for_oxc("$hp gte 1 and $mp lte 100", true);
        assert!(
            result.source.contains("State_variables_hp >= 1"),
            "expected '>= 1', got '{}'",
            result.source
        );
        assert!(
            result.source.contains("State_variables_mp <= 100"),
            "expected '<= 100', got '{}'",
            result.source
        );
    }

    #[test]
    fn eq_neq_replacement() {
        let result = preprocess_for_oxc("$x eq 5 or $y neq 0", true);
        assert!(
            result.source.contains("State_variables_x === 5"),
            "expected '=== 5', got '{}'",
            result.source
        );
        assert!(
            result.source.contains("State_variables_y !== 0"),
            "expected '!== 0', got '{}'",
            result.source
        );
    }

    #[test]
    fn is_isnot_replacement() {
        let result = preprocess_for_oxc("$name is \"Alice\" or $name isnot \"Bob\"", true);
        assert!(
            result.source.contains("State_variables_name === \"Alice\""),
            "expected '=== \"Alice\"', got '{}'",
            result.source
        );
        assert!(
            result.source.contains("State_variables_name !== \"Bob\""),
            "expected '!== \"Bob\"', got '{}'",
            result.source
        );
    }

    #[test]
    fn and_or_not_replacement() {
        let result = preprocess_for_oxc("$alive and not $poisoned or $cured", true);
        assert!(
            result.source.contains("State_variables_alive &&"),
            "expected '&&', got '{}'",
            result.source
        );
        assert!(
            result.source.contains("! State_variables_poisoned"),
            "expected '! State_variables_poisoned', got '{}'",
            result.source
        );
        assert!(
            result.source.contains("|| State_variables_cured"),
            "expected '|| State_variables_cured', got '{}'",
            result.source
        );
    }

    #[test]
    fn not_replacement_prefix() {
        let result = preprocess_for_oxc("not $alive", true);
        assert!(
            result.source.contains("! State_variables_alive"),
            "expected '! State_variables_alive', got '{}'",
            result.source
        );
    }

    #[test]
    fn complex_if_condition() {
        // Realistic <<if>> condition: <<if $hp gt 0 and $name is "hero">>
        let result = preprocess_for_oxc("$hp gt 0 and $name is \"hero\"", true);
        assert!(
            result.source.contains("State_variables_hp > 0 && State_variables_name === \"hero\""),
            "expected proper normalization, got '{}'",
            result.source
        );
    }

    // ── Word boundary protection tests ─────────────────────────────────

    #[test]
    fn operators_not_replaced_in_identifiers() {
        // "gt" inside "$target" should NOT be replaced — it's part of the
        // variable name, which gets substituted before we check operators.
        let result = preprocess_for_oxc("$target gt 5", true);
        assert!(
            result.source.contains("State_variables_target > 5"),
            "expected 'State_variables_target > 5', got '{}'",
            result.source
        );
        // No "gt" should remain in the output (the one in $target was consumed)
        assert!(
            !result.source.contains(" gt "),
            "expected no remaining 'gt' keyword, got '{}'",
            result.source
        );
    }

    #[test]
    fn operators_not_replaced_in_strings() {
        // "gt" inside a string literal should NOT be replaced
        let result = preprocess_for_oxc("$msg is \"greater than five\"", true);
        assert!(
            result.source.contains("State_variables_msg === \"greater than five\""),
            "expected string preserved, got '{}'",
            result.source
        );
        // The "gt" in "greater" should NOT be replaced with ">"
        assert!(
            !result.source.contains(">reater"),
            "expected 'greater' preserved in string, got '{}'",
            result.source
        );
    }

    #[test]
    fn or_not_replaced_in_string() {
        let result = preprocess_for_oxc("$label is \"door\"", true);
        assert!(
            result.source.contains("State_variables_label === \"door\""),
            "expected 'door' preserved in string, got '{}'",
            result.source
        );
        // "or" inside "door" should NOT be replaced with "||"
        assert!(
            !result.source.contains("d||"),
            "expected 'door' not mangled, got '{}'",
            result.source
        );
    }

    #[test]
    fn is_not_replaced_mid_word() {
        // "is" inside "this" should NOT be replaced
        let result = preprocess_for_oxc("this gt 5", true);
        assert!(
            result.source.contains("this > 5"),
            "expected 'this > 5', got '{}'",
            result.source
        );
        assert!(
            !result.source.contains("th=== "),
            "expected 'this' not mangled, got '{}'",
            result.source
        );
    }

    // ── Position mapping tests ─────────────────────────────────────────

    #[test]
    fn position_mapping() {
        let result = preprocess_for_oxc("$hp to 5", true);
        // $hp is replaced with State_variables_hp (longer)
        // "to" is replaced with "=" (shorter)
        // The substitution map should let us map back
        assert!(!result.substitutions.is_empty());
    }

    #[test]
    fn substitution_map_consistency() {
        let result = preprocess_for_oxc("$x gt 5 and $y lt 10", true);
        // Verify all substitutions have valid ranges
        for sub in &result.substitutions {
            assert!(
                sub.original_range.start <= sub.original_range.end,
                "invalid original range: {:?}",
                sub.original_range
            );
            assert!(
                sub.processed_range.start <= sub.processed_range.end,
                "invalid processed range: {:?}",
                sub.processed_range
            );
        }
    }

    // ── Multi-byte UTF-8 tests ──────────────────────────────────────────

    #[test]
    fn em_dash_in_expression() {
        // This is the exact case that caused the panic:
        // em-dash '—' is 3 bytes in UTF-8 (0xE2 0x80 0x94).
        // Byte-by-byte iteration would land on byte 2 of the char,
        // which is not a char boundary, causing &source[i..] to panic.
        let result = preprocess_for_oxc("$x — $y", true);
        assert!(
            result.source.contains("State_variables_x"),
            "expected variable substitution, got '{}'",
            result.source
        );
        assert!(
            result.source.contains("State_variables_y"),
            "expected variable substitution, got '{}'",
            result.source
        );
        assert!(
            result.source.contains("—"),
            "expected em-dash preserved, got '{}'",
            result.source
        );
    }

    #[test]
    fn cjk_characters_in_expression() {
        // CJK characters are 3 bytes each in UTF-8.
        // Common in game dialogue: $player名前 (variable + CJK property)
        let result = preprocess_for_oxc("$x + 日本語", true);
        assert!(
            result.source.contains("State_variables_x + 日本語"),
            "expected CJK preserved, got '{}'",
            result.source
        );
    }

    #[test]
    fn emoji_in_expression() {
        // Emoji are 4 bytes in UTF-8.
        let result = preprocess_for_oxc("$x + 🎮", true);
        assert!(
            result.source.contains("State_variables_x + 🎮"),
            "expected emoji preserved, got '{}'",
            result.source
        );
    }

    #[test]
    fn multibyte_in_string_literal() {
        // Multi-byte chars inside string literals should be preserved
        let result = preprocess_for_oxc("$msg is \"hello — world\"", true);
        assert!(
            result.source.contains("State_variables_msg === \"hello — world\""),
            "expected string with em-dash preserved, got '{}'",
            result.source
        );
    }

    #[test]
    fn mixed_multibyte_with_operators() {
        // Full realistic case: variables + operators + multi-byte text
        let result = preprocess_for_oxc("$name is \"Alice—\" and $hp gt 0", true);
        assert!(
            result.source.contains("State_variables_name === \"Alice—\" && State_variables_hp > 0"),
            "expected mixed content preserved, got '{}'",
            result.source
        );
    }

    #[test]
    fn multibyte_does_not_panic_on_slicing() {
        // Regression: previously, byte-by-byte iteration on multi-byte chars
        // would cause &source[i..] to panic when i landed inside a char.
        // Test various multi-byte chars to ensure no panics.
        let cases = vec![
            "café",           // 2-byte: é
            "naïve",          // 2-byte: ï
            "日本語",          // 3-byte CJK
            "🎮",             // 4-byte emoji
            "a—b",            // 3-byte em-dash
            "$x + café",      // variable + 2-byte
            "$y gt 日本語",    // variable + operator + 3-byte
        ];
        for case in cases {
            // Must not panic
            let result = preprocess_for_oxc(case, true);
            assert!(
                !result.source.is_empty(),
                "preprocess should produce output for: {}",
                case
            );
        }
    }
}
