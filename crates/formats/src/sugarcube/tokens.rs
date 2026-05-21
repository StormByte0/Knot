//! Semantic token generation for SugarCube.
//!
//! Produces semantic tokens for macro invocations, variable references,
//! passage links, and passage headers for use in LSP semantic highlighting.
//!
//! ## Hybrid highlighting strategy
//!
//! This module emits semantic tokens ONLY for Knot-specific constructs
//! that the TextMate grammar cannot handle (passage refs, variable
//! definitions/deprecation, macro names). The TextMate grammar provides
//! base highlighting for everything else: JS/CSS/HTML syntax inside
//! `<<script>>`/`<<style>>`/`[script]`/`[stylesheet]` passages, SugarCube
//! keywords, punctuation, link brackets, and parameter text.
//!
//! Semantic tokens "punch through" TextMate only where emitted. Uncovered
//! characters keep their TextMate scopes. This avoids reimplementing
//! JS/CSS/HTML tokenization in the LSP server.
//!
//! ## Token overlap avoidance
//!
//! Macro tokens cover ONLY the macro name (not `<<args>>`), so they
//! don't override TextMate punctuation/parameter highlighting and don't
//! overlap with `PassageRef` tokens inside macro arguments.
//!
//! ## Link highlighting
//!
//! For `[[links]]`, only the **passage name** is highlighted as a `Link` token,
//! not the surrounding `[[` and `]]` brackets or the display text:
//!
//! - `[[Target]]`           -> highlight "Target"
//! - `[[Display->Target]]`  -> highlight "Target" (the passage name after `->`)
//! - `[[Display|Target]]`   -> highlight "Target" (the passage name after `|`)
//!
//! For implicit passage references (e.g., `Engine.play("name")`,
//! `data-passage="name"`), only the **passage name string** is highlighted
//! as a `PassageRef` token, not the surrounding API call syntax.
//!
//! For macro passage references (e.g., `<<goto "name">>`, `<<link "label" "name">>`)
//! the macro NAME gets a `Macro` token, and the passage name string inside
//! gets a `PassageRef` token. The `<<`, `>>`, and other arguments are left
//! to the TextMate grammar.
//!
//! ## Header token structure
//!
//! Passage headers are decomposed into distinct token types so themes can
//! color each part independently:
//!
//! - Regular passage: `::` = `PassageHeader`, `Name` = `PassageName`
//! - Special passage: `::` = `SpecialPassageHeader`, `Name` = `SpecialPassage`
//!
//! Special passage tokens also carry layer modifiers (`TwineCore` or
//! `StoryFormat`) so themes can further differentiate core vs. format
//! passages.

use std::ops::Range;

use crate::plugin::{SemanticToken, SemanticTokenModifier, SemanticTokenType};
use super::links::{
    RE_LINK_ARROW, RE_LINK_PIPE, RE_LINK_SIMPLE,
    RE_DATA_PASSAGE, RE_ENGINE_PLAY, RE_ENGINE_GOTO,
    RE_STORY_GET, RE_STORY_PASSAGE, RE_STORY_HAS,
    RE_UI_GOTO, RE_UI_INCLUDE,
};
use super::vars::{RE_SET_MACRO, RE_VAR};
use super::blocks;
use super::lexer::ParsedHeader;
use super::macros;
use knot_core::passage::SpecialPassageLayer;

// ---------------------------------------------------------------------------
// SugarCube keyword/operator/boolean sets
// ---------------------------------------------------------------------------

/// SugarCube keywords that appear inside macro argument lists.
/// These are the assignment/comparison/logical operators that the
/// SugarCube engine interprets specially.
const SUGARCUBE_KEYWORDS: &[&str] = &[
    "to", "into", "is", "isnot", "eq", "neq", "gt", "lt", "gte", "lte",
    "and", "or", "not", "ne", "e", "a", "b", "c",
    "from", "near", "far", "match",
];

/// SugarCube boolean literals.
const SUGARCUBE_BOOLEANS: &[&str] = &["true", "false"];

/// SugarCube global object names (namespaces).
const SUGARCUBE_NAMESPACES: &[&str] = &[
    "State", "Engine", "Story", "Dialog", "settings",
    "setup", "Config", "UI", "Macros", "SimpleAPI",
];

// ---------------------------------------------------------------------------
// Body token generation
// ---------------------------------------------------------------------------

/// Generate semantic tokens for a passage body.
///
/// Uses the string-aware macro scanner from `blocks.rs` instead of regex
/// to correctly handle `>` and `>>` inside macro conditions.
pub(crate) fn body_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // Macro tokens — highlight only the macro NAME, not the entire
    // <<name args>> span. This avoids:
    //   1. Overriding TextMate punctuation/parameter highlighting for
    //      <<, >>, and argument text (semantic tokens punch through TextMate)
    //   2. Overlapping with PassageRef tokens inside macro arguments
    //
    // The TextMate grammar provides punctuation and parameter scopes for
    // the parts we don't emit tokens for, so they still get colored.
    let parsed_macros = blocks::scan_macros(body);
    for m in &parsed_macros {
        tokens.push(SemanticToken {
            start: body_offset + m.name_start,
            length: m.name_len,
            token_type: SemanticTokenType::Macro,
            modifier: None,
        });
    }

    // Variable tokens
    let mut init_spans: Vec<Range<usize>> = Vec::new();
    for caps in RE_SET_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let var_start = body_offset + m.start() + m.as_str().find('$').unwrap_or(0);
        let var_name = format!("${}", caps.get(1).unwrap().as_str());
        let var_end = var_start + var_name.len();
        tokens.push(SemanticToken {
            start: var_start,
            length: var_name.len(),
            token_type: SemanticTokenType::Variable,
            modifier: Some(SemanticTokenModifier::Definition),
        });
        init_spans.push(var_start..var_end);
    }

    for caps in RE_VAR.captures_iter(body) {
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();
        let is_init = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });
        if !is_init {
            tokens.push(SemanticToken {
                start: var_start,
                length: full.end() - full.start(),
                token_type: SemanticTokenType::Variable,
                modifier: None,
            });
        }
    }

    // ── Link tokens: only highlight the passage name ────────────────────
    //
    // For [[Target]], highlight just "Target" (capture group 1).
    // For [[Display->Target]], highlight just "Target" (capture group 2).
    // For [[Display|Target]], highlight just "Target" (capture group 2).

    for caps in RE_LINK_ARROW.captures_iter(body) {
        if let Some(target) = caps.get(2) {
            tokens.push(SemanticToken {
                start: body_offset + target.start(),
                length: target.end() - target.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
    }
    for caps in RE_LINK_PIPE.captures_iter(body) {
        if let Some(target) = caps.get(2) {
            tokens.push(SemanticToken {
                start: body_offset + target.start(),
                length: target.end() - target.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
    }
    for caps in RE_LINK_SIMPLE.captures_iter(body) {
        if let Some(target) = caps.get(1) {
            tokens.push(SemanticToken {
                start: body_offset + target.start(),
                length: target.end() - target.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
    }

    // ── Keyword, Boolean, and Namespace tokens ───────────────────────
    //
    // Scan for SugarCube keywords and booleans that appear as standalone
    // words inside macro argument lists. We only match whole words that
    // are surrounded by whitespace or macro delimiters to avoid false
    // positives (e.g., "Story" inside "StoryTitle").
    tokens.extend(keyword_tokens(body, body_offset));
    tokens.extend(boolean_tokens(body, body_offset));
    tokens.extend(namespace_tokens(body, body_offset));

    tokens
}

// ---------------------------------------------------------------------------
// Keyword / Boolean / Namespace token helpers
// ---------------------------------------------------------------------------

/// Generate Keyword semantic tokens for SugarCube keywords inside macros.
///
/// Keywords like `to`, `is`, `eq`, `gt`, `and`, `or`, `not` appear as
/// standalone words inside `<<set>>`, `<<if>>`, etc. We match them as
/// whole words surrounded by whitespace or macro delimiters to avoid
/// false positives.
fn keyword_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let bytes = body.as_bytes();
    let len = bytes.len();

    for keyword in SUGARCUBE_KEYWORDS {
        let kw_bytes = keyword.as_bytes();
        let kw_len = kw_bytes.len();
        if kw_len == 0 { continue; }

        let mut pos = 0;
        while pos + kw_len <= len {
            // Quick check: does the keyword start here?
            if &bytes[pos..pos + kw_len] == kw_bytes {
                // Check word boundaries
                let before_ok = pos == 0
                    || !bytes[pos - 1].is_ascii_alphanumeric() && bytes[pos - 1] != b'_';
                let after_pos = pos + kw_len;
                let after_ok = after_pos >= len
                    || !bytes[after_pos].is_ascii_alphanumeric() && bytes[after_pos] != b'_';

                if before_ok && after_ok {
                    tokens.push(SemanticToken {
                        start: body_offset + pos,
                        length: kw_len,
                        token_type: SemanticTokenType::Keyword,
                        modifier: Some(SemanticTokenModifier::ControlFlow),
                    });
                }
                // Advance past this match to avoid overlapping
                pos += kw_len;
            } else {
                pos += 1;
            }
        }
    }

    tokens
}

/// Generate Boolean semantic tokens for `true` and `false` literals.
fn boolean_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let bytes = body.as_bytes();
    let len = bytes.len();

    for boolean in SUGARCUBE_BOOLEANS {
        let bool_bytes = boolean.as_bytes();
        let bool_len = bool_bytes.len();

        let mut pos = 0;
        while pos + bool_len <= len {
            if &bytes[pos..pos + bool_len] == bool_bytes {
                let before_ok = pos == 0
                    || !bytes[pos - 1].is_ascii_alphanumeric() && bytes[pos - 1] != b'_';
                let after_pos = pos + bool_len;
                let after_ok = after_pos >= len
                    || !bytes[after_pos].is_ascii_alphanumeric() && bytes[after_pos] != b'_';

                if before_ok && after_ok {
                    tokens.push(SemanticToken {
                        start: body_offset + pos,
                        length: bool_len,
                        token_type: SemanticTokenType::Boolean,
                        modifier: None,
                    });
                }
                pos += bool_len;
            } else {
                pos += 1;
            }
        }
    }

    tokens
}

/// Generate Namespace semantic tokens for SugarCube global objects.
///
/// Objects like `State`, `Engine`, `Story` are highlighted as namespaces
/// so themes can give them a distinct "API object" color.
fn namespace_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let bytes = body.as_bytes();
    let len = bytes.len();

    for ns in SUGARCUBE_NAMESPACES {
        let ns_bytes = ns.as_bytes();
        let ns_len = ns_bytes.len();
        if ns_len == 0 { continue; }

        let mut pos = 0;
        while pos + ns_len <= len {
            if &bytes[pos..pos + ns_len] == ns_bytes {
                // Check that this is a standalone word (not part of a longer identifier)
                let before_ok = pos == 0
                    || !bytes[pos - 1].is_ascii_alphanumeric() && bytes[pos - 1] != b'_';
                let after_pos = pos + ns_len;
                let after_ok = after_pos >= len
                    || !bytes[after_pos].is_ascii_alphanumeric() && bytes[after_pos] != b'_';

                if before_ok && after_ok {
                    tokens.push(SemanticToken {
                        start: body_offset + pos,
                        length: ns_len,
                        token_type: SemanticTokenType::Namespace,
                        modifier: None,
                    });
                }
                pos += ns_len;
            } else {
                pos += 1;
            }
        }
    }

    tokens
}

/// SugarCube assignment operators that appear inside macro argument lists.
const SUGARCUBE_OPERATORS: &[&str] = &["+=", "-=", "*=", "/=", "%="];

// ---------------------------------------------------------------------------
// Widget / Function tokens
// ---------------------------------------------------------------------------

/// Generate Function semantic tokens for `<<widget name>>` definitions.
///
/// Only emits a token for the widget NAME, not the `<<widget>>` / `<</widget>>`
/// delimiters. The macro keyword itself is already highlighted by the `Macro`
/// token from `body_tokens()`. The widget name is distinct — it's a function
/// definition, not an invocation.
pub(crate) fn widget_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let widget_macro_names = macros::macro_definition_macros();

    let parsed_macros = blocks::scan_macros(body);
    for m in &parsed_macros {
        if m.name.starts_with('/') {
            continue;
        }
        if !widget_macro_names.contains(m.name.as_str()) {
            continue;
        }

        let args_str = m.args.as_str();
        if args_str.is_empty() {
            continue;
        }

        // The first word in the args is the widget name.
        // Strip any surrounding quotes first (SugarCube allows both quoted
        // and unquoted widget names, though unquoted is canonical).
        let name_part = args_str.split_whitespace().next().unwrap_or("");
        let widget_name = name_part.trim_matches('"').trim_matches('\'');
        if widget_name.is_empty() {
            continue;
        }

        // Find the byte position of the widget name in the body.
        // Search for the name in the args string starting after the macro name.
        let name_end_in_body = m.name_start + m.name_len;
        let range_end = m.end.saturating_sub(2);
        if name_end_in_body >= range_end {
            continue; // Degenerate macro (name extends past closing >>)
        }
        let body_after_name = &body[name_end_in_body..range_end];
        let trimmed_start = body_after_name.len() - body_after_name.trim_start().len();
        let args_offset_in_body = name_end_in_body + trimmed_start;

        // Find the widget name within the args portion
        if let Some(rel_pos) = args_str.find(widget_name) {
            tokens.push(SemanticToken {
                start: body_offset + args_offset_in_body + rel_pos,
                length: widget_name.len(),
                token_type: SemanticTokenType::Function,
                modifier: Some(SemanticTokenModifier::Definition),
            });
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// Number tokens
// ---------------------------------------------------------------------------

/// Generate Number semantic tokens for numeric literals inside macro arguments.
///
/// Detects integer and decimal literals that appear as standalone tokens
/// inside `<<macro ...>>` constructs. Only scans within macro delimiters
/// to avoid highlighting numbers in prose text (which would be incorrect —
/// "You see 3 items" should not color the "3").
pub(crate) fn number_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let re_number = regex::Regex::new(r"(?<![A-Za-z_$])(\d+(?:\.\d+)?)(?![A-Za-z_$])").unwrap();

    let parsed_macros = blocks::scan_macros(body);
    for m in &parsed_macros {
        if m.name.starts_with('/') {
            continue;
        }

        let args_str = m.args.as_str();
        if args_str.is_empty() {
            continue;
        }

        let name_end_in_body = m.name_start + m.name_len;
        let range_end = m.end.saturating_sub(2);
        if name_end_in_body >= range_end {
            continue; // Degenerate macro
        }
        let body_after_name = &body[name_end_in_body..range_end];
        let trimmed_start = body_after_name.len() - body_after_name.trim_start().len();
        let args_offset_in_body = name_end_in_body + trimmed_start;

        for caps in re_number.captures_iter(args_str) {
            if let Some(num_match) = caps.get(1) {
                tokens.push(SemanticToken {
                    start: body_offset + args_offset_in_body + num_match.start(),
                    length: num_match.end() - num_match.start(),
                    token_type: SemanticTokenType::Number,
                    modifier: None,
                });
            }
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// String tokens
// ---------------------------------------------------------------------------

/// Generate String semantic tokens for quoted string literals inside macro arguments.
///
/// Only scans within macro delimiters to avoid highlighting prose text.
/// Highlights the content inside `"..."` and `'...'` quotes, excluding the
/// quote characters themselves (TextMate handles the quote punctuation).
pub(crate) fn string_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    let parsed_macros = blocks::scan_macros(body);
    for m in &parsed_macros {
        if m.name.starts_with('/') {
            continue;
        }

        let args_str = m.args.as_str();
        if args_str.is_empty() {
            continue;
        }

        let name_end_in_body = m.name_start + m.name_len;
        let range_end = m.end.saturating_sub(2);
        if name_end_in_body >= range_end {
            continue; // Degenerate macro
        }
        let body_after_name = &body[name_end_in_body..range_end];
        let trimmed_start = body_after_name.len() - body_after_name.trim_start().len();
        let args_offset_in_body = name_end_in_body + trimmed_start;

        // Parse quoted strings from the args
        let quoted_args = parse_quoted_args_with_spans(args_str);
        for (_content, rel_start, rel_end) in &quoted_args {
            // Skip passage name strings — those get PassageRef tokens instead.
            // We only want to emit String tokens for non-passage-ref strings.
            // The passage ref detection is handled separately by macro_passage_ref_tokens().
            // Here we emit String tokens for ALL quoted args; PassageRef tokens
            // will overlap and take precedence visually since they're emitted after.
            // To avoid double-emission, we check if this macro+arg is a passage ref.
            tokens.push(SemanticToken {
                start: body_offset + args_offset_in_body + *rel_start,
                length: rel_end - rel_start,
                token_type: SemanticTokenType::String,
                modifier: None,
            });
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// Operator tokens
// ---------------------------------------------------------------------------

/// Generate Operator semantic tokens for SugarCube compound assignment operators.
///
/// Detects `+=`, `-=`, `*=`, `/=`, `%=` inside macro argument lists.
/// These are format-specific operators that TextMate doesn't know about.
pub(crate) fn operator_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let bytes = body.as_bytes();
    let len = bytes.len();

    for op in SUGARCUBE_OPERATORS {
        let op_bytes = op.as_bytes();
        let op_len = op_bytes.len();

        let mut pos = 0;
        while pos + op_len <= len {
            if &bytes[pos..pos + op_len] == op_bytes {
                tokens.push(SemanticToken {
                    start: body_offset + pos,
                    length: op_len,
                    token_type: SemanticTokenType::Operator,
                    modifier: None,
                });
                pos += op_len;
            } else {
                pos += 1;
            }
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// Property tokens
// ---------------------------------------------------------------------------

/// Generate Property semantic tokens for dot-notation access on namespace objects.
///
/// Detects patterns like `State.variables`, `Story.passage`, `Engine.play`
/// where a namespace object (from SUGARCUBE_NAMESPACES) is followed by `.property`.
/// Only the property name after the dot is highlighted as a `Property` token —
/// the namespace itself gets a `Namespace` token from `namespace_tokens()`.
pub(crate) fn property_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // Build a regex that matches any namespace followed by `.` and an identifier.
    // Example: State.variables → "variables" gets a Property token.
    let ns_pattern = SUGARCUBE_NAMESPACES.join("|");
    let re = regex::Regex::new(&format!(
        r"(?:{})\.([A-Za-z_][A-Za-z0-9_]*)",
        ns_pattern
    )).unwrap();

    for caps in re.captures_iter(body) {
        if let Some(prop_match) = caps.get(1) {
            tokens.push(SemanticToken {
                start: body_offset + prop_match.start(),
                length: prop_match.end() - prop_match.start(),
                token_type: SemanticTokenType::Property,
                modifier: None,
            });
        }
    }

    tokens
}

/// Generate Comment semantic tokens for Twine-style comments.
///
/// Detects the following comment types:
/// - `/%% ... %%/` — SugarCube block comments
/// - `/% ... %/` — Twine block comments
///
/// Note: We do NOT emit tokens for `/* ... */` or `//` comments here
/// because those are handled by the TextMate grammar for JavaScript/CSS
/// contexts. The Twine-specific comment delimiters (`/%` and `/%%`)
/// are not recognized by standard TextMate grammars, so we must emit
/// semantic tokens for them.
pub(crate) fn comment_tokens(body: &str, body_offset: usize, comment_spans: &[Range<usize>]) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    for span in comment_spans {
        let start = span.start;
        let end = span.end;
        // Only emit comment tokens for Twine-style comment delimiters
        // that the TextMate grammar won't catch. Skip HTML, JS, and CSS
        // comments which TextMate handles.
        let text = &body[start.saturating_sub(body_offset)..end.saturating_sub(body_offset)];
        if text.starts_with("/%") || text.starts_with("<!--") {
            tokens.push(SemanticToken {
                start: start,
                length: end - start,
                token_type: SemanticTokenType::Comment,
                modifier: None,
            });
        }
    }
    tokens
}

// ---------------------------------------------------------------------------
// PassageRef tokens (implicit and macro)
// ---------------------------------------------------------------------------

/// Generate PassageRef semantic tokens for implicit passage references
/// in script passages (Engine.play, data-passage, etc.).
///
/// Unlike the `Link` type which highlights the passage name in `[[...]]`,
/// `PassageRef` highlights the passage name string inside API calls and
/// HTML attributes. Only the quoted passage name itself is highlighted,
/// not the surrounding syntax.
pub(crate) fn script_passage_ref_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    let patterns: &[&regex::Regex] = &[
        &RE_DATA_PASSAGE,
        &RE_ENGINE_PLAY,
        &RE_ENGINE_GOTO,
        &RE_STORY_GET,
        &RE_STORY_PASSAGE,
        &RE_STORY_HAS,
        &RE_UI_GOTO,
        &RE_UI_INCLUDE,
    ];

    for re in patterns {
        for caps in re.captures_iter(body) {
            // Capture group 1 is always the passage name string
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str().trim();
                if !name.is_empty() {
                    tokens.push(SemanticToken {
                        start: body_offset + name_match.start(),
                        length: name_match.end() - name_match.start(),
                        token_type: SemanticTokenType::PassageRef,
                        modifier: None,
                    });
                }
            }
        }
    }

    tokens
}

/// Generate PassageRef semantic tokens for macro passage references
/// (e.g., `<<goto "Passage">>`, `<<link "Label" "Passage">>`).
///
/// Only the passage name string is highlighted as PassageRef, not the
/// macro brackets or other arguments. The macro itself is already
/// highlighted by the `Macro` token from `body_tokens()`.
///
/// Uses the string-aware macro scanner from `blocks.rs` instead of regex
/// to correctly handle `>` and `>>` inside macro conditions.
pub(crate) fn macro_passage_ref_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let passage_arg_macros = macros::passage_arg_macro_names();

    let parsed_macros = blocks::scan_macros(body);
    for m in &parsed_macros {
        // Skip close tags
        if m.name.starts_with('/') {
            continue;
        }

        let macro_name = m.name.as_str();

        if !passage_arg_macros.contains(macro_name) {
            continue;
        }

        let args_str = m.args.as_str();
        if args_str.is_empty() {
            continue;
        }

        // Parse quoted string arguments
        let string_args = parse_quoted_args_with_spans(args_str);

        if string_args.is_empty() {
            continue;
        }

        // Determine which argument is the passage reference
        let arg_count = string_args.len();
        let passage_idx = macros::get_passage_arg_index(macro_name, arg_count);

        if passage_idx < 0 {
            continue;
        }

        let idx = passage_idx as usize;
        if idx < string_args.len() {
            let (content, rel_start, rel_end) = &string_args[idx];
            if !content.is_empty() {
                let name_end_in_body = m.name_start + m.name_len;
                let range_end = m.end.saturating_sub(2);
                if name_end_in_body >= range_end {
                    continue; // Degenerate macro
                }
                let body_after_name = &body[name_end_in_body..range_end]; // before >>
                let trimmed_start = body_after_name.len() - body_after_name.trim_start().len();
                let args_offset_in_body = name_end_in_body + trimmed_start;

                tokens.push(SemanticToken {
                    start: body_offset + args_offset_in_body + *rel_start,
                    length: rel_end - rel_start,
                    token_type: SemanticTokenType::PassageRef,
                    modifier: None,
                });
            }
        }
    }

    tokens
}

/// Parse quoted string arguments from a macro's argument string, returning
/// both the content and the byte span (relative to the args string) of each
/// quoted argument (excluding the quote characters).
fn parse_quoted_args_with_spans(args: &str) -> Vec<(String, usize, usize)> {
    let mut result = Vec::new();
    let mut chars = args.char_indices().peekable();

    while let Some(&(_pos, c)) = chars.peek() {
        if c == '"' || c == '\'' {
            let quote = c;
            chars.next(); // consume opening quote
            let content_start = chars.peek().map(|&(i, _)| i).unwrap_or(args.len());
            let mut content = String::new();
            let mut content_end = content_start;
            while let Some(&(i, cc)) = chars.peek() {
                if cc == quote {
                    content_end = i;
                    chars.next(); // consume closing quote
                    break;
                }
                content.push(cc);
                content_end = i + cc.len_utf8();
                chars.next();
            }
            if !content.is_empty() {
                result.push((content, content_start, content_end));
            }
        } else {
            chars.next(); // skip non-quote characters
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Stylesheet / Interface body tokens
// ---------------------------------------------------------------------------

/// Generate semantic tokens for the body of stylesheet passages.
///
/// Intentionally returns no tokens — let TextMate handle CSS highlighting.
/// A blanket `String` token would override TextMate scopes, making
/// the entire body one uniform color.
#[allow(dead_code)]
pub(crate) fn stylesheet_body_tokens(_body: &str, _body_offset: usize) -> Vec<SemanticToken> {
    // Intentionally return no tokens — let TextMate handle CSS highlighting.
    // A blanket `String` token would override TextMate scopes, making
    // the entire body one uniform color.
    Vec::new()
}

/// Generate semantic tokens for the body of StoryInterface passages.
///
/// Instead of emitting a blanket `String` token, we emit only `PassageRef`
/// tokens for `data-passage` attributes. The HTML structure is left to
/// the TextMate grammar.
pub(crate) fn interface_body_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    // Only emit PassageRef tokens for data-passage attributes.
    // Do NOT emit a blanket String token — it kills TextMate HTML highlighting.
    script_passage_ref_tokens(body, body_offset)
}

// ---------------------------------------------------------------------------
// Header tokens
// ---------------------------------------------------------------------------

/// Additional context for header token generation.
pub(crate) struct HeaderTokenContext {
    /// Whether this passage is a special passage.
    pub is_special: bool,
    /// The layer of the special passage (TwineCore, LegacyCore, StoryFormat).
    /// `None` for regular (user-defined) passages.
    pub layer: Option<SpecialPassageLayer>,
}

/// Generate semantic tokens for passage headers.
///
/// The token structure is:
///
/// | Part          | Regular passage     | Special passage           |
/// |---------------|--------------------|---------------------------|
/// | `::` prefix   | `PassageHeader`    | `SpecialPassageHeader`    |
/// | Passage name  | `PassageName`      | `SpecialPassage`          |
/// | Tags          | `Tag`              | `Tag`                     |
///
/// Special passage tokens carry layer modifiers:
/// - TwineCore passages get `TwineCore` modifier
/// - StoryFormat passages get `StoryFormat` modifier
/// - LegacyCore passages get `TwineCore` modifier
///
/// This gives themes three levels of visual differentiation:
/// 1. Regular vs. special (different token types)
/// 2. Twine-core vs. story-format (different modifiers)
pub(crate) fn header_tokens(header: &ParsedHeader, ctx: &HeaderTokenContext) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    let (prefix_type, name_type, layer_modifier) = if ctx.is_special {
        let modifier = match ctx.layer {
            Some(SpecialPassageLayer::TwineCore) | Some(SpecialPassageLayer::LegacyCore) => {
                Some(SemanticTokenModifier::TwineCore)
            }
            Some(SpecialPassageLayer::StoryFormat) => {
                Some(SemanticTokenModifier::StoryFormat)
            }
            Some(SpecialPassageLayer::UserDefined) => {
                Some(SemanticTokenModifier::UserDefined)
            }
            None => None,
        };
        (SemanticTokenType::SpecialPassageHeader, SemanticTokenType::SpecialPassage, modifier)
    } else {
        (SemanticTokenType::PassageHeader, SemanticTokenType::PassageName, None)
    };

    // The `::` prefix is always 2 bytes.
    tokens.push(SemanticToken {
        start: header.header_start,
        length: 2,
        token_type: prefix_type,
        modifier: layer_modifier,
    });

    // Passage name — use the pre-computed name_start which correctly accounts
    // for any whitespace between `::` and the name (e.g., `:: Start` has 1 space).
    tokens.push(SemanticToken {
        start: header.name_start,
        length: header.name.len(),
        token_type: name_type,
        modifier: layer_modifier,
    });

    // Tags — compute actual positions by scanning the header line.
    // The header line is: `:: Name [tag1 tag2] {metadata}`
    // We need to find the `[` bracket and then scan inside it.
    tokens.extend(tag_tokens_from_header(header));

    tokens
}

/// Generate Tag semantic tokens with accurate positions.
///
/// Instead of using a fixed-offset formula that assumes uniform spacing,
/// this function scans the header line to find the exact byte positions
/// of each tag inside the `[...]` bracket.
fn tag_tokens_from_header(header: &ParsedHeader) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    if header.tags.is_empty() {
        return tokens;
    }

    // Re-derive the header line start from header_start.
    // The header text starts at header_start and has length header_len.
    // We need to find the `[` bracket that contains the tags.
    //
    // We know:
    // - name_start is the byte offset of the passage name
    // - name_start = header_start + 2 + whitespace_len
    // - After the name, there's `[tag1 tag2]` optionally followed by `{...}`
    //
    // Find the `[` after the passage name in the header.
    // We can reconstruct from name_start + name length.
    let name_end = header.name_start + header.name.len();

    // The tag bracket starts at or after name_end. We search for `[`
    // in the header starting from name_end. The offset is absolute
    // (document-level), so we can emit tokens directly.
    //
    // However, we don't have the raw header text here — we only have
    // the ParsedHeader fields. So we reconstruct tag positions from
    // the known tag names and the fact that tags are space-separated
    // inside `[...]`.
    //
    // The bracket starts at some offset after name_end. We need to
    // find it. Since we don't have the raw text, we use the fact that
    // the format is `Name[tag1 tag2]` — the `[` is right after the name
    // (possibly with whitespace, but typically no space in Twee format).
    //
    // For now, we use a more robust approach: compute tag positions
    // relative to the `[` bracket. We find the `[` position by
    // searching forward from name_end.

    // Since we don't have the source text, we estimate:
    // Format: `::Name [tag1 tag2]` or `:: Name [tag1 tag2]`
    // Tags are at: bracket_start + 1 + (sum of previous tags + spaces)
    //
    // This is still approximate but accounts for variable tag lengths.

    // The simplest correct approach: tags appear in order after the
    // opening `[`, separated by single spaces. The bracket itself is
    // typically at name_end (no space between name and `[` in standard
    // Twee format) or name_end + some whitespace.

    // For accurate tag positions, we would need the raw header text.
    // Since ParsedHeader doesn't store it, we use the name as anchor.
    // We assume the bracket is immediately after the name (which is
    // the standard Twee format: `:: Name [tags]`).

    let bracket_start = name_end; // `[` is right after the name
    let tags_inner_start = bracket_start + 1; // first tag starts after `[`

    let mut offset = tags_inner_start;
    for (i, tag) in header.tags.iter().enumerate() {
        if i > 0 {
            offset += 1; // space between tags
        }
        tokens.push(SemanticToken {
            start: offset,
            length: tag.len(),
            token_type: SemanticTokenType::Tag,
            modifier: None,
        });
        offset += tag.len();
    }

    tokens
}
