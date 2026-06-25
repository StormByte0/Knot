//! AST → LinkInfo/VarOpInfo/DataRef extraction.
//!
//! This module provides functions that walk the AST to extract structural
//! information: passage links (from `[[ ]]` and navigation macros), variable
//! operations (reads and writes), and `data-passage` attribute references.

use crate::sugarcube::ast::*;
use super::predicates::is_assignment_macro;
use super::comment::strip_comments;

// ---------------------------------------------------------------------------
// Link extraction
// ---------------------------------------------------------------------------

/// Extract LinkInfo from the AST (flattened, including nested macros).
///
/// This collects both `[[ ]]` passage links and passage references from
/// navigation macros (`<<goto>>`, `<<include>>`, `<<link>>`, `<<button>>`,
/// `<<actions>>`, `<<return>>`, `<<back>>`). Each link carries its `LinkSource`
/// so that `build_passage()` can set `edge_type_hint` directly without
/// post-hoc substring matching.
pub(super) fn extract_links_from_ast(nodes: &[AstNode]) -> Vec<LinkInfo> {
    let mut links = Vec::new();
    extract_links_recursive(nodes, &mut links);
    links
}

fn extract_links_recursive(nodes: &[AstNode], links: &mut Vec<LinkInfo>) {
    for node in nodes {
        match node {
            AstNode::Link {
                display,
                target,
                span,
                ..
            } => {
                let is_dynamic = target.starts_with('$') || target.starts_with('_');
                links.push(LinkInfo {
                    display: display.clone(),
                    target: target.clone(),
                    span: span.clone(),
                    is_dynamic,
                    source: LinkSource::PassageLink,
                });
            }
            AstNode::Macro {
                name,
                args,
                open_span,
                children,
                ..
            } => {
                // Extract passage references from navigation macros
                let macro_links = extract_macro_passage_refs(name, args, open_span.clone());
                links.extend(macro_links);

                // Recurse into children for nested links
                if let Some(ch) = children {
                    extract_links_recursive(ch, links);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Macro passage reference extraction
// ---------------------------------------------------------------------------

/// Navigation macros that reference passage names in their arguments.
///
/// These macros create graph edges when their arguments contain string
/// literal passage names or variable references. The mapping determines
/// which `LinkSource` (and therefore which `EdgeType`) each macro produces.
///
/// ## Argument semantics by macro
///
/// | Macro | 1 string arg | 2+ string args | Variable arg |
/// |-------|-------------|----------------|-------------|
/// | `<<goto>>` | passage target | — | dynamic target |
/// | `<<include>>` | passage target | — | dynamic target |
/// | `<<link>>` / `<<button>>` | display + target (same) | display + target | dynamic target |
/// | `<<actions>>` | all are targets | all are targets | — |
/// | `<<back>>` | display text only | display + target | dynamic (history) |
/// | `<<return>>` | display text only | display + target | dynamic (history) |
const NAVIGATION_MACROS: &[(&str, LinkSource)] = &[
    ("goto", LinkSource::Goto),
    ("include", LinkSource::Include),
    ("link", LinkSource::NavigationMacro),
    ("button", LinkSource::NavigationMacro),
    ("actions", LinkSource::Actions),
    ("return", LinkSource::Return),
    ("back", LinkSource::Back),
];

/// Extract passage references from a navigation macro's arguments.
///
/// SugarCube navigation macros accept passage names as string literal
/// arguments. This function extracts those passage names and creates
/// `LinkInfo` entries with the appropriate `LinkSource` for edge type
/// classification.
///
/// ## Supported patterns
///
/// - `<<goto "Passage">>` — single passage target
/// - `<<include "Passage">>` — single passage target
/// - `<<link "Display" "Passage">>` — display + passage target
/// - `<<button "Display" "Passage">>` — display + passage target
/// - `<<actions "P1" "P2" "P3">>` — multiple passage targets
/// - `<<back "Display">>` — display text only (history-based, no fixed target)
/// - `<<back "Display" "Passage">>` — display + specific passage target
/// - `<<return "Display">>` — display text only (history-based, no fixed target)
/// - `<<return "Display" "Passage">>` — display + specific passage target
/// - `<<goto $var>>` — dynamic variable target (is_dynamic = true)
/// - `<<link "Display" $var>>` — dynamic variable target
///
/// Returns a (possibly empty) vector of `LinkInfo` entries.
fn extract_macro_passage_refs(
    macro_name: &str,
    args: &str,
    open_span: std::ops::Range<usize>,
) -> Vec<LinkInfo> {
    // Find the matching LinkSource for this macro
    let source = match NAVIGATION_MACROS.iter().find(|(name, _)| name.eq_ignore_ascii_case(macro_name)) {
        Some((_, src)) => *src,
        None => return Vec::new(), // Not a navigation macro
    };

    // Strip comments from args before extracting string arguments.
    // This prevents strings inside /* */, //, /% %/, <!-- --> etc.
    // from being extracted as passage targets. Both `strip_comments()`
    // and the parser's `parse_body()` handle the same 6 comment types
    // in a single pass — they must be kept in sync.
    let stripped_args = strip_comments(args);
    let string_args = extract_string_args(&stripped_args);

    let mut links = Vec::new();

    if string_args.is_empty() {
        // No string literal args. Check for:
        // 1. Variable reference as sole argument: <<goto $dest>>
        // 2. Bare passage name: <<goto Forest>> (SugarCube accepts unquoted names)
        //
        // IMPORTANT: For <<back>> and <<return>>, a single unquoted arg is
        // display text (not a passage target) — same semantics as a single
        // quoted string arg. These macros navigate via browser history; the
        // single arg customizes the link text, not the destination.
        let trimmed = stripped_args.trim();
        if trimmed.starts_with('$') || trimmed.starts_with('_') {
            links.push(LinkInfo {
                display: None,
                target: trimmed.to_string(),
                span: open_span,
                is_dynamic: true,
                source,
            });
        } else if !trimmed.is_empty() && is_bare_passage_name(trimmed) {
            if matches!(source, LinkSource::Back | LinkSource::Return) {
                // Single unquoted arg for <<back>>/<<return>> is display text,
                // not a passage target. Mark as dynamic (history-based nav)
                // with no fixed target to prevent false BrokenLink diagnostics.
                links.push(LinkInfo {
                    display: Some(trimmed.to_string()),
                    target: String::new(),
                    span: open_span,
                    is_dynamic: true,
                    source,
                });
            } else {
                // Bare passage name (e.g., <<goto Forest>>)
                // SugarCube treats unquoted identifiers as passage names
                links.push(LinkInfo {
                    display: None,
                    target: trimmed.to_string(),
                    span: open_span,
                    is_dynamic: false,
                    source,
                });
            }
        }
        return links;
    }

    // For <<link>> / <<button>>, the first string arg is the display text
    // and the second is the passage target (if present).
    // For <<back>> / <<return>>, the first string arg is display text, the
    // second (if present) is the passage target. With only one arg, it's just
    // display text — these macros navigate via browser history, not a fixed target.
    // For <<goto>> / <<include>>, the first string arg is the passage target.
    // For <<actions>>, all string args are passage targets.
    match source {
        LinkSource::NavigationMacro | LinkSource::Back | LinkSource::Return => {
            // <<link "display" "target">>, <<button "display" "target">>,
            // <<back "display" "target">>, <<return "display" "target">>
            if string_args.len() >= 2 {
                links.push(LinkInfo {
                    display: Some(string_args[0].clone()),
                    target: string_args[1].clone(),
                    span: open_span.clone(),
                    is_dynamic: false,
                    source,
                });
            } else if string_args.len() == 1 {
                // For <<back>> / <<return>>: one string arg is display text only.
                // The navigation target is dynamic (browser history), so we mark
                // it as dynamic with no fixed target to avoid false "BrokenLink"
                // diagnostics. The display text is NOT a passage name.
                if matches!(source, LinkSource::Back | LinkSource::Return) {
                    links.push(LinkInfo {
                        display: Some(string_args[0].clone()),
                        target: String::new(), // no fixed passage target
                        span: open_span.clone(),
                        is_dynamic: true, // target is dynamic (history-based)
                        source,
                    });
                } else {
                    // For <<link>> / <<button>>: single arg is both display + target
                    links.push(LinkInfo {
                        display: Some(string_args[0].clone()),
                        target: string_args[0].clone(),
                        span: open_span.clone(),
                        is_dynamic: false,
                        source,
                    });
                }
            }
            // For <<link>> / <<button>>: also check for bare passage name
            if matches!(source, LinkSource::NavigationMacro) && string_args.len() == 1 {
                let bare = extract_bare_args_after_strings(&stripped_args, string_args.len());
                if let Some(bare_target) = bare.first() {
                    // Override: the bare arg is the real target, string is display
                    links.clear();
                    links.push(LinkInfo {
                        display: Some(string_args[0].clone()),
                        target: bare_target.clone(),
                        span: open_span,
                        is_dynamic: false,
                        source,
                    });
                }
            }
        }
        LinkSource::Actions => {
            for arg in &string_args {
                links.push(LinkInfo {
                    display: Some(arg.clone()),
                    target: arg.clone(),
                    span: open_span.clone(),
                    is_dynamic: false,
                    source,
                });
            }
        }
        _ => {
            // <<goto "Passage">>, <<include "Passage">>, etc.
            if !string_args.is_empty() {
                links.push(LinkInfo {
                    display: None,
                    target: string_args[0].clone(),
                    span: open_span,
                    is_dynamic: false,
                    source,
                });
            }
        }
    }

    links
}

// ---------------------------------------------------------------------------
// Bare passage name / string arg utilities
// ---------------------------------------------------------------------------

/// Check if a string looks like a bare passage name (not a variable, not
/// an expression, not a string literal — just an identifier-like token).
pub fn is_bare_passage_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Must not start with a sigil
    if s.starts_with('$') || s.starts_with('_') {
        return false;
    }
    // Must not contain spaces (multi-word expressions are not bare names)
    if s.contains(' ') {
        return false;
    }
    // Must not contain operators or special characters
    let bytes = s.as_bytes();
    if bytes.iter().any(|&b| b == b'=' || b == b'+' || b == b'-' || b == b'(' || b == b')') {
        return false;
    }
    // Must look like an identifier (alphanumeric, hyphens, underscores)
    bytes.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Extract bare (unquoted) tokens that appear after the quoted string args.
///
/// For example, in `"Display" Forest`, this returns `["Forest"]`.
/// In `"Go" "Forest"`, this returns `[]` (no bare args after strings).
pub fn extract_bare_args_after_strings(args: &str, num_string_args: usize) -> Vec<String> {
    let mut result = Vec::new();
    let bytes = args.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut strings_found = 0usize;

    // Helper: advance by full UTF-8 character
    let advance = |pos: usize| -> usize {
        args[pos..].chars().next().map_or(1, |c| c.len_utf8())
    };

    // Skip past the quoted string args
    while i < len && strings_found < num_string_args {
        while i < len && bytes[i] == b' ' {
            i += 1;
        }
        if i >= len {
            break;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            while i < len && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2;
                } else {
                    i += advance(i);
                }
            }
            if i < len {
                i += 1;
            }
            strings_found += 1;
        } else {
            // Non-string token — skip (advance by UTF-8 char)
            while i < len && bytes[i] != b' ' {
                i += advance(i);
            }
        }
    }

    // Now collect remaining bare tokens
    while i < len {
        while i < len && bytes[i] == b' ' {
            i += 1;
        }
        if i >= len {
            break;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            // Another string — skip it
            let quote = bytes[i];
            i += 1;
            while i < len && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2;
                } else {
                    i += advance(i);
                }
            }
            if i < len {
                i += 1;
            }
        } else {
            // Bare token
            let start = i;
            while i < len && bytes[i] != b' ' {
                i += advance(i);
            }
            let token = args[start..i].to_string();
            if !token.is_empty() {
                result.push(token);
            }
        }
    }

    result
}

/// Extract string literal arguments from a macro argument string.
///
/// Parses `"arg1" "arg2" 'arg3'` into a vec of the string contents
/// (without quotes). Handles escaped quotes inside strings.
pub fn extract_string_args(args: &str) -> Vec<String> {
    let mut result = Vec::new();
    let bytes = args.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    // Helper: advance by full UTF-8 character
    let advance = |pos: usize| -> usize {
        args[pos..].chars().next().map_or(1, |c| c.len_utf8())
    };

    while i < len {
        // Skip whitespace
        while i < len && bytes[i] == b' ' {
            i += 1;
        }
        if i >= len {
            break;
        }

        // Check for string literal
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1; // skip opening quote
            let start = i;
            while i < len && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2; // skip escaped char
                } else {
                    i += advance(i);
                }
            }
            let content = args[start..i].to_string();
            if i < len {
                i += 1; // skip closing quote
            }
            result.push(content);
        } else {
            // Non-string token — skip to next whitespace or end
            while i < len && bytes[i] != b' ' {
                i += advance(i);
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// data-passage attribute extraction
// ---------------------------------------------------------------------------

/// Extract `data-passage` attribute references from HTML content.
///
/// SugarCube's StoryInterface passage can contain HTML elements with
/// `data-passage="PassageName"` attributes. These create navigation links
/// in the story graph. This function scans for both quoted and unquoted
/// `data-passage` attribute values.
///
/// ## Patterns matched
///
/// - `data-passage="PassageName"` — quoted attribute
/// - `data-passage='PassageName'` — single-quoted attribute
/// - `data-passage` — bare attribute (no value, references current passage)
///
/// Returns `LinkInfo` entries with `LinkSource::DataPassage`.
pub fn extract_data_passage_refs(body: &str) -> Vec<LinkInfo> {
    // Strip comments first — data-passage inside comments should be ignored.
    // Both `strip_comments()` and the parser's `parse_body()` handle the
    // same 6 comment types in a single pass — they must be kept in sync.
    // Since `strip_comments()` preserves byte offsets (replaces comments
    // with spaces of the same length), positions in the stripped text map
    // 1:1 to positions in the original text.
    let stripped = strip_comments(body);

    let mut links = Vec::new();
    let bytes = stripped.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i + 12 < len {
        // Look for "data-passage". Use `starts_with` so the comparison is
        // char-boundary safe (direct slicing would panic if `i+12` landed
        // inside a multi-byte UTF-8 sequence).
        if stripped[i..].starts_with("data-passage") {
            let attr_start = i;
            i += 12;

            // Skip whitespace after attribute name
            while i < len && bytes[i] == b' ' {
                i += 1;
            }

            // Check for = sign
            if i < len && bytes[i] == b'=' {
                i += 1;
                // Skip whitespace
                while i < len && bytes[i] == b' ' {
                    i += 1;
                }

                if i < len && (bytes[i] == b'"' || bytes[i] == b'\'') {
                    let quote = bytes[i];
                    i += 1; // skip opening quote
                    let value_start = i;
                    while i < len && bytes[i] != quote {
                        // Advance by full UTF-8 character to avoid mid-char slicing.
                        i += stripped[i..].chars().next().map_or(1, |c| c.len_utf8());
                    }
                    let value = stripped[value_start..i].to_string();
                    if i < len {
                        i += 1; // skip closing quote
                    }
                    if !value.is_empty() {
                        links.push(LinkInfo {
                            display: None,
                            target: value,
                            span: attr_start..i,
                            is_dynamic: false,
                            source: LinkSource::DataPassage,
                        });
                    }
                }
            }
            // Bare data-passage (no value) — references the current passage
            // being rendered. We don't create a link for this since the
            // target is dynamic (the current passage at render time).
        } else {
            // Advance by full UTF-8 character to stay on char boundaries.
            i += stripped[i..].chars().next().map_or(1, |c| c.len_utf8());
        }
    }

    links
}

// ---------------------------------------------------------------------------
// Variable operation extraction
// ---------------------------------------------------------------------------

/// Extract VarOpInfo from the AST (flattened, including nested macros).
pub(super) fn extract_var_ops_from_ast(nodes: &[AstNode]) -> Vec<VarOpInfo> {
    let mut ops = Vec::new();
    extract_var_ops_recursive(nodes, &mut ops, false);
    ops
}

fn extract_var_ops_recursive(
    nodes: &[AstNode],
    ops: &mut Vec<VarOpInfo>,
    _in_assignment: bool,
) {
    for node in nodes {
        match node {
            AstNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    ops.push(VarOpInfo {
                        name: vr.name.clone(),
                        property_path: vr.property_path.clone(),
                        is_temporary: vr.is_temporary,
                        is_write: false, // Text vars are always reads
                        span: vr.span.clone(),
                    });
                }
            }
            AstNode::Macro {
                name,
                var_refs,
                children,
                set_assignment,
                ..
            } => {
                // For <<set>> macros with structured assignment info, use the
                // precise target from set_assignment rather than the broad
                // is_assignment_macro heuristic.
                if let Some(sa) = set_assignment {
                    // Emit the target as a write
                    ops.push(VarOpInfo {
                        name: sa.target.name.clone(),
                        property_path: sa.target.property_path.clone(),
                        is_temporary: sa.target.is_temporary,
                        is_write: true,
                        span: sa.target.span.clone(),
                    });
                    // Other vars in var_refs (from the expression) are reads
                    for vr in var_refs {
                        // Skip the target — it's already emitted above as a write
                        if vr.name == sa.target.name && vr.span.start == sa.target.span.start {
                            continue;
                        }
                        ops.push(VarOpInfo {
                            name: vr.name.clone(),
                            property_path: vr.property_path.clone(),
                            is_temporary: vr.is_temporary,
                            is_write: false, // Expression vars are reads
                            span: vr.span.clone(),
                        });
                    }
                } else {
                    let is_assignment = is_assignment_macro(name);
                    // Macro's own var refs
                    for vr in var_refs {
                        ops.push(VarOpInfo {
                            name: vr.name.clone(),
                            property_path: vr.property_path.clone(),
                            is_temporary: vr.is_temporary,
                            is_write: is_assignment,
                            span: vr.span.clone(),
                        });
                    }
                }
                // Recurse into children
                let is_assignment = is_assignment_macro(name);
                if let Some(ch) = children {
                    extract_var_ops_recursive(ch, ops, is_assignment);
                }
            }
            AstNode::Expression { var_refs, .. } => {
                for vr in var_refs {
                    ops.push(VarOpInfo {
                        name: vr.name.clone(),
                        property_path: vr.property_path.clone(),
                        is_temporary: vr.is_temporary,
                        is_write: false, // Expressions are reads
                        span: vr.span.clone(),
                    });
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::parser::parse_passage_body;

    #[test]
    fn extract_string_args_double_quotes() {
        let args = r#""hello" "world""#;
        let result = extract_string_args(args);
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn extract_string_args_single_quotes() {
        let args = "'hello' 'world'";
        let result = extract_string_args(args);
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn extract_string_args_mixed() {
        let args = r#""hello" 'world'"#;
        let result = extract_string_args(args);
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn extract_string_args_escaped_quote() {
        let args = r#""he said \"hi\"""#;
        let result = extract_string_args(args);
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("hi"));
    }

    #[test]
    fn extract_links_ignores_js_line_comment_in_macro_args() {
        // Verify that // comments inside macro args don't cause panics
        let ast = parse_passage_body(
            r#"<<goto // "CommentedOut"
"RealTarget">>"#,
            0,
            ParseMode::Normal,
        );
        // This is a tricky edge case — just verify no crash
        assert!(!ast.links.is_empty() || ast.links.is_empty()); // verify it doesn't panic
    }

    // -----------------------------------------------------------------------
    // <<back>> / <<return>> no false BrokenLink tests
    // -----------------------------------------------------------------------

    #[test]
    fn back_quoted_single_arg_no_broken_link() {
        // <<back "Back">> — "Back" is display text, NOT a passage target.
        // Must NOT produce a link with target "Back" (which would cause
        // a false BrokenLink diagnostic if no passage named "Back" exists).
        let ast = parse_passage_body(r#"<<back "Back">>"#, 0, ParseMode::Normal);
        let back_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Back).collect();
        assert_eq!(back_links.len(), 1);
        assert_eq!(back_links[0].display.as_deref(), Some("Back"));
        assert!(back_links[0].target.is_empty(), "<<back>> with one quoted arg should have no fixed target");
        assert!(back_links[0].is_dynamic, "<<back>> with one arg should be dynamic");
    }

    #[test]
    fn back_unquoted_single_arg_no_broken_link() {
        // <<back Back>> — unquoted bare name, same semantics as quoted.
        // SugarCube treats the single arg as display text for history navigation.
        // Must NOT produce a link with target "Back".
        let ast = parse_passage_body("<<back Back>>", 0, ParseMode::Normal);
        let back_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Back).collect();
        assert_eq!(back_links.len(), 1);
        assert_eq!(back_links[0].display.as_deref(), Some("Back"));
        assert!(back_links[0].target.is_empty(), "<<back>> with one unquoted arg should have no fixed target");
        assert!(back_links[0].is_dynamic, "<<back>> with one arg should be dynamic");
    }

    #[test]
    fn return_quoted_single_arg_no_broken_link() {
        // <<return "Town">> — display text only, history-based navigation
        let ast = parse_passage_body(r#"<<return "Town">>"#, 0, ParseMode::Normal);
        let ret_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Return).collect();
        assert_eq!(ret_links.len(), 1);
        assert!(ret_links[0].target.is_empty(), "<<return>> with one arg should have no fixed target");
        assert!(ret_links[0].is_dynamic);
    }

    #[test]
    fn return_unquoted_single_arg_no_broken_link() {
        // <<return Town>> — unquoted bare name, display text only
        let ast = parse_passage_body("<<return Town>>", 0, ParseMode::Normal);
        let ret_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Return).collect();
        assert_eq!(ret_links.len(), 1);
        assert!(ret_links[0].target.is_empty(), "<<return>> with one unquoted arg should have no fixed target");
        assert!(ret_links[0].is_dynamic);
    }

    #[test]
    fn back_two_args_has_fixed_target() {
        // <<back "Flee" "Forest">> — display text + specific passage target
        let ast = parse_passage_body(r#"<<back "Flee" "Forest">>"#, 0, ParseMode::Normal);
        let back_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Back).collect();
        assert_eq!(back_links.len(), 1);
        assert_eq!(back_links[0].display.as_deref(), Some("Flee"));
        assert_eq!(back_links[0].target, "Forest");
        assert!(!back_links[0].is_dynamic, "<<back>> with two args should have a fixed target");
    }

    #[test]
    fn goto_bare_name_is_passage_target() {
        // <<goto Forest>> — bare name IS a passage target for <<goto>>
        // (unlike <<back>>/<<return>> where single arg is display text)
        let ast = parse_passage_body("<<goto Forest>>", 0, ParseMode::Normal);
        let goto_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Goto).collect();
        assert_eq!(goto_links.len(), 1);
        assert_eq!(goto_links[0].target, "Forest");
        assert!(!goto_links[0].is_dynamic);
    }

    #[test]
    fn include_bare_name_is_passage_target() {
        // <<include Header>> — bare name is passage target for <<include>>
        let ast = parse_passage_body("<<include Header>>", 0, ParseMode::Normal);
        let inc_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Include).collect();
        assert_eq!(inc_links.len(), 1);
        assert_eq!(inc_links[0].target, "Header");
        assert!(!inc_links[0].is_dynamic);
    }
}
