//! Macro parsing for `<< >>` syntax, including `<<set>>` assignment parsing
//! and block body parsing.

use crate::sugarcube::ast::*;
use super::predicates::{is_ident_char, is_ident_start, is_var_ident_char, is_block_macro, is_block_modifier};
use super::variable_scan::scan_inline_vars;

/// Parse a macro starting after `<<`.
///
/// `i` points to the first character after `<<`.
/// On return, `i` points past the closing `>>` (or end of text).
pub(super) fn parse_macro(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Skip whitespace after <<
    while *i < len && bytes[*i] == b' ' {
        *i += 1;
    }

    // Check for close tag: <</name>>
    if *i < len && bytes[*i] == b'/' {
        *i += 1;
        // Scan the close tag name
        let name_start = *i;
        while *i < len && is_ident_char(bytes[*i]) {
            *i += 1;
        }
        let name = text[name_start..*i].to_string();
        // Skip to >>
        skip_to_macro_close(text, i);
        return AstNode::Macro {
            name,
            args: String::new(),
            var_refs: Vec::new(),
            children: None, // close tags have no children
            name_span: span_start + 2..span_start + 2 + (*i - name_start),
            open_span: span_start..span_start + *i,
            close_span: None,
            full_span: span_start..span_start + *i,
            set_assignment: None, // close tags have no assignment
        };
    }

    // Scan the macro name
    let name_start = *i;
    // Expression macros: <<=>> and <<->>
    //
    // SugarCube's expression macros use a simplified close rule: the FIRST
    // `>>` after the `=` or `-` sigil closes the macro. This is different
    // from regular macros which use depth-tracked `<<`/`>>` matching.
    if *i < len && (bytes[*i] == b'=' || bytes[*i] == b'-') {
        let kind = if bytes[*i] == b'=' { ExprKind::Print } else { ExprKind::Silent };
        *i += 1;
        // Skip to the first >> (expression macros close at the first >>)
        let content_start = *i;
        let content_end = skip_to_first_macro_close(text, i);
        let content = text[content_start..content_end].to_string();
        let var_refs = scan_inline_vars(&content, span_start + content_start);
        return AstNode::Expression {
            kind,
            content,
            var_refs,
            span: span_start..span_start + *i,
        };
    }

    // Regular macro name
    while *i < len && is_ident_char(bytes[*i]) {
        *i += 1;
    }
    let name = text[name_start..*i].to_string();
    let name_len = name.len();

    // Skip space between name and args
    while *i < len && bytes[*i] == b' ' {
        *i += 1;
    }

    // Scan args to matching >>
    let args_start = *i;
    let args_end = scan_macro_args(text, i);
    let args = text[args_start..args_end].to_string();

    let open_end = *i; // past the >>

    // Determine if this is a block macro (needs a close tag).
    //
    // Block modifiers (<<else>>, <<elseif>>, <<case>>, <<default>>) are
    // excluded — they're clause markers within a parent block, not
    // standalone blocks with their own close tags.
    let is_block = is_block_macro(&name) && !is_block_modifier(&name);

    let var_refs = scan_inline_vars(&args, span_start + args_start);

    // For <<set>> macros: parse the assignment structure so that only
    // the RHS expression goes to oxc (not the target + operator).
    let set_assignment = if name.eq_ignore_ascii_case("set") {
        parse_set_assignment(&args, span_start + args_start)
    } else {
        None
    };

    if is_block {
        // Parse the body until <</name>>
        let body_text = &text[open_end..];
        let (children, close_offset) = parse_block_body(body_text, &name, span_start + open_end);

        let close_span = if let Some(co) = close_offset {
            // The close tag was found at body_text[co..]
            // Scan the close tag to find its full extent
            let mut ci = co;
            // Skip past <</name>>
            while ci < body_text.len() && body_text.as_bytes()[ci] != b'>' {
                ci += 1;
            }
            if ci < body_text.len() && body_text.as_bytes()[ci] == b'>' {
                ci += 1;
                // Check for >>
                if ci < body_text.len() && body_text.as_bytes()[ci] == b'>' {
                    ci += 1;
                }
            }
            *i = open_end + ci;
            Some(span_start + open_end + co..span_start + open_end + ci)
        } else {
            // Unclosed block macro — the rest of the text is the body
            *i = len;
            None
        };

        let full_end = close_span.as_ref().map_or(span_start + *i, |s| s.end);

        AstNode::Macro {
            name,
            args,
            var_refs,
            children: Some(children),
            name_span: span_start + 2..span_start + 2 + name_len,
            open_span: span_start..span_start + open_end,
            close_span,
            full_span: span_start..full_end.max(span_start + *i),
            set_assignment,
        }
    } else {
        AstNode::Macro {
            name,
            args,
            var_refs,
            children: None,
            name_span: span_start + 2..span_start + 2 + name_len,
            open_span: span_start..span_start + open_end,
            close_span: None,
            full_span: span_start..span_start + open_end,
            set_assignment,
        }
    }
}

/// Scan macro arguments, handling nested `<<`/`>>` and strings.
///
/// Returns the byte position where args end (before `>>`).
/// Advances `i` past the closing `>>`.
pub(super) fn scan_macro_args(text: &str, i: &mut usize) -> usize {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let _start = *i;
    let mut depth = 1u32; // We're inside one <<
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while *i < len {
        let b = bytes[*i];

        // String tracking
        if b == b'\\' && *i + 1 < len {
            *i += 2; // Skip escaped char
            continue;
        }
        if b == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            *i += 1;
            continue;
        }
        if b == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            *i += 1;
            continue;
        }
        if in_single_quote || in_double_quote {
            *i += 1;
            continue;
        }

        // Nested <<
        if b == b'<' && *i + 1 < len && bytes[*i + 1] == b'<' {
            depth += 1;
            *i += 2;
            continue;
        }

        // >> or >>>
        if b == b'>' && *i + 1 < len && bytes[*i + 1] == b'>' {
            depth -= 1;
            if depth == 0 {
                let args_end = *i;
                *i += 2; // Skip >>

                // SugarCube treats >>> as >> + >. If the next char is >,
                // it's part of the next token, not part of this close.
                // But >>> is actually a single sugar syntax for >>
                // followed by a literal >. So we just consume the two >.
                return args_end;
            }
            *i += 2;
            continue;
        }

        *i += 1;
    }

    // Unclosed macro — everything is args
    *i = len;
    len
}

// ---------------------------------------------------------------------------
// <<set>> assignment parser
// ---------------------------------------------------------------------------

/// Parse a `<<set>>` macro's arguments into a structured assignment.
///
/// SugarCube's `<<set>>` syntax: `<<set operator {expression}>>`
///
/// Where `<<set operator` is SugarCube-owned (target variable + assignment
/// operator), and `{expression}` is the ONLY part oxc parses.
///
/// Supported patterns:
/// - `<<set $hp to 100>>`     → target=$hp, op=To,     expr="100"
/// - `<<set $hp = 100>>`      → target=$hp, op=Eq,     expr="100"
/// - `<<set $hp += 10>>`      → target=$hp, op=PlusEq, expr="10"
/// - `<<set $hp -= 5>>`       → target=$hp, op=MinusEq,expr="5"
/// - `<<set $hp *= 2>>`       → target=$hp, op=StarEq, expr="2"
/// - `<<set $hp /= 2>>`       → target=$hp, op=SlashEq,expr="2"
/// - `<<set $hp %= 3>>`       → target=$hp, op=PercentEq,expr="3"
/// - `<<set $hp++>>`          → target=$hp, op=PostfixPlus, expr=None
/// - `<<set $hp-->>`          → target=$hp, op=PostfixMinus,expr=None
/// - `<<set $arr.push(1)>>`   → None (not a simple assignment)
///
/// Returns `None` if the args don't match a simple assignment pattern
/// (e.g., method calls like `$arr.push(1)`), in which case the entire
/// args string is treated as a JS expression (same as `<<run>>`).
pub(super) fn parse_set_assignment(args: &str, args_offset: usize) -> Option<SetAssignment> {
    let bytes = args.as_bytes();
    let len = bytes.len();

    if len == 0 {
        return None;
    }

    // Must start with a variable reference ($var or _var)
    let is_story_var = bytes[0] == b'$' && len > 1 && is_ident_start(bytes[1]);
    let is_temp_var = bytes[0] == b'_' && len > 1 && is_ident_start(bytes[1]);

    if !is_story_var && !is_temp_var {
        return None;
    }

    // Scan the target variable. We use a dedicated scan here (not scan_variable)
    // because the main scan_variable includes `-` as an ident char (for macro
    // names like <<link-replace>>), but variable names do NOT include `-`.
    // This is critical for postfix operators: `$hp--` must be parsed as
    // variable `$hp` + operator `--`, NOT as variable `$hp--`.
    let mut vi = 1usize;
    while vi < len && is_var_ident_char(bytes[vi]) {
        vi += 1;
    }

    // Scan dot-notation property path
    let path_start = vi;
    let mut property_path = String::new();
    while vi < len && bytes[vi] == b'.' {
        vi += 1; // Skip the dot
        let prop_start = vi;
        while vi < len && is_var_ident_char(bytes[vi]) {
            vi += 1;
        }
        if vi > prop_start {
            if !property_path.is_empty() {
                property_path.push('.');
            }
            property_path.push_str(&args[prop_start..vi]);
        }
    }

    let var_end = vi;
    let var_name = args[..path_start].to_string();

    let target = VarRef {
        name: var_name,
        property_path,
        is_temporary: is_temp_var,
        is_write: true, // In <<set>>, the target is always a write
        span: args_offset..args_offset + var_end,
    };

    // Skip whitespace after variable
    let mut i = var_end;
    while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }

    // Check for postfix ++ or --
    if i + 1 < len && bytes[i] == b'+' && bytes[i + 1] == b'+' {
        return Some(SetAssignment {
            target,
            operator: SetOperator::PostfixPlus,
            expression: None,
            expression_span: None,
        });
    }
    if i + 1 < len && bytes[i] == b'-' && bytes[i + 1] == b'-' {
        return Some(SetAssignment {
            target,
            operator: SetOperator::PostfixMinus,
            expression: None,
            expression_span: None,
        });
    }

    // Check for assignment operators
    let (operator, op_len) = if i + 2 < len && bytes[i] == b'+' && bytes[i + 1] == b'=' {
        (SetOperator::PlusEq, 2)
    } else if i + 2 < len && bytes[i] == b'-' && bytes[i + 1] == b'=' {
        (SetOperator::MinusEq, 2)
    } else if i + 2 < len && bytes[i] == b'*' && bytes[i + 1] == b'=' {
        (SetOperator::StarEq, 2)
    } else if i + 2 < len && bytes[i] == b'/' && bytes[i + 1] == b'=' {
        (SetOperator::SlashEq, 2)
    } else if i + 2 < len && bytes[i] == b'%' && bytes[i + 1] == b'=' {
        (SetOperator::PercentEq, 2)
    } else if i < len && bytes[i] == b'=' {
        // Simple = (but not ==)
        if i + 1 < len && bytes[i + 1] == b'=' {
            // == is comparison, not assignment — not a simple set
            return None;
        }
        (SetOperator::Eq, 1)
    } else if i + 2 < len && bytes[i] == b't' && bytes[i + 1] == b'o' && is_to_keyword_boundary(args, i) {
        // SugarCube `to` keyword
        (SetOperator::To, 2)
    } else {
        // Not a recognized assignment operator — this is likely a
        // method call like $arr.push(1) or some other expression.
        // Return None so the entire args string goes to oxc.
        return None;
    };

    // Skip past the operator and any whitespace
    i += op_len;
    while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }

    // Everything after the operator is the RHS expression
    let expression = if i < len {
        Some(args[i..].to_string())
    } else {
        None
    };

    Some(SetAssignment {
        target,
        operator,
        expression,
        expression_span: if i < len {
            Some(args_offset + i..args_offset + len)
        } else {
            None
        },
    })
}

/// Check that `to` at position `pos` in `args` is a keyword boundary.
///
/// SugarCube's `to` must be at a word boundary — preceded by whitespace
/// (already guaranteed by caller) and followed by whitespace or end-of-args.
/// This prevents matching `to` inside identifiers like `$total`.
fn is_to_keyword_boundary(args: &str, pos: usize) -> bool {
    let bytes = args.as_bytes();
    let len = bytes.len();
    let after_to = pos + 2;
    if after_to >= len {
        return true; // `to` at end of args
    }
    // `to` must be followed by whitespace
    bytes[after_to] == b' ' || bytes[after_to] == b'\t' || bytes[after_to] == b'\n'
}

/// Skip to the closing `>>` of a macro (for close tags and simple cases).
pub(super) fn skip_to_macro_close(text: &str, i: &mut usize) {
    let bytes = text.as_bytes();
    let len = bytes.len();

    while *i < len {
        if bytes[*i] == b'>' && *i + 1 < len && bytes[*i + 1] == b'>' {
            *i += 2;
            return;
        }
        *i += 1;
    }
}

/// Skip to the FIRST `>>` that closes an expression macro (`<<=>>` / `<<->>`).
///
/// Unlike `skip_to_macro_close` (which just advances `i`), this function
/// returns the byte position **before** the closing `>>`, so the caller
/// can extract the expression content without including the `>>` delimiters.
///
/// After this call, `*i` points past the `>>`.
///
/// Expression macros close at the **first** `>>` after the `=`/`-` sigil,
/// with one important exception: `>>` inside string literals is NOT treated
/// as a close delimiter. This prevents `<<= "hello >>">>` from closing early.
pub(super) fn skip_to_first_macro_close(text: &str, i: &mut usize) -> usize {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while *i < len {
        let b = bytes[*i];

        // String tracking — don't match >> inside strings
        if b == b'\\' && *i + 1 < len {
            *i += 2;
            continue;
        }
        if b == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            *i += 1;
            continue;
        }
        if b == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            *i += 1;
            continue;
        }
        if in_single_quote || in_double_quote {
            *i += 1;
            continue;
        }

        // First >> outside of strings closes the expression macro
        if b == b'>' && *i + 1 < len && bytes[*i + 1] == b'>' {
            let content_end = *i;
            *i += 2;
            return content_end;
        }
        *i += 1;
    }

    // No closing >> found — everything is content
    *i = len;
    len
}

/// Parse the body of a block macro until `<</name>>`.
///
/// Returns (children_nodes, close_tag_offset).
/// `close_tag_offset` is the position of `<</name>>` in `text`,
/// or None if the block is unclosed.
pub(super) fn parse_block_body(text: &str, macro_name: &str, offset: usize) -> (Vec<AstNode>, Option<usize>) {
    // Find the matching <</name>>
    let close_tag = format!("<</{}>>", macro_name);
    let close_tag_alt = format!("<</ {}>>", macro_name); // with space after /

    let mut search_from = 0usize;
    let mut depth = 1u32;
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Scan for the matching close tag, tracking nesting depth
    while search_from < len {
        // Check for nested opening tag of the same macro.
        //
        // We look for `<<name` at the current position and verify:
        // 1. It's not a close tag (`<</name>>`) — check for `/` after `<<`
        // 2. The character after the name is NOT an ident char — this
        //    distinguishes `<<if>>` / `<<if $cond>>` from `<<if2>>` or
        //    `<<if_something>>` (which are different macros).
        let open_tag = format!("<<{}", macro_name);
        if search_from + open_tag.len() <= len
            && text[search_from..].starts_with(&open_tag)
        {
            let after_name = search_from + open_tag.len();
            // Skip close tags (<</name>> or <</ name>>)
            if after_name < len && bytes[after_name] == b'/' {
                // This starts with <</, so it's a close tag — don't count
                // as a nested open. The close-tag check below will handle it.
            } else if after_name >= len || !is_ident_char(bytes[after_name]) {
                // The character after the name is not an ident char,
                // so this is a genuine opening tag (e.g., <<if>> or
                // <<if $cond>>), not a different macro like <<if2>>.
                depth += 1;
                search_from += open_tag.len();
                continue;
            }
        }

        // Check for close tag
        if text[search_from..].starts_with(&close_tag)
            || text[search_from..].starts_with(&close_tag_alt)
        {
            depth -= 1;
            if depth == 0 {
                // Found the matching close tag
                let body_content = &text[..search_from];
                let children = super::core::parse_body(body_content, offset);
                return (children, Some(search_from));
            }
            search_from += close_tag.len();
            continue;
        }

        search_from += 1;
    }

    // Unclosed — parse the rest as body
    let children = super::core::parse_body(text, offset);
    (children, None)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::{AstNode, ParseMode, ExprKind, SetOperator, LinkSource};
    use crate::sugarcube::parser::parse_passage_body;

    #[test]
    fn parse_inline_macro() {
        let ast = parse_passage_body("<<set $hp to 100>>", 0, ParseMode::Normal);
        assert!(ast.var_ops.len() >= 1);
        assert_eq!(ast.var_ops[0].name, "$hp");
        assert!(ast.var_ops[0].is_write);
    }

    #[test]
    fn parse_block_macro() {
        let ast = parse_passage_body("<<if $alive>>You live!<</if>>", 0, ParseMode::Normal);
        // Should have: Text("You live!") inside the if block
        let macros = collect_macros(&ast.nodes);
        assert!(macros.len() >= 1);
        match macros[0] {
            AstNode::Macro { name, children, .. } => {
                assert_eq!(name, "if");
                assert!(children.is_some());
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn parse_if_else_block() {
        // Realistic <<if>> block with nested <<if>>, <<=>>, <<else>>, and <</if>>
        let input = r#"<<if _parts.length > 0>>
  <<= _parts[0] >>
  <<if _parts.length > 1>> &#43;<<= _parts.length - 1 >><</if>>
<<else>>
  &mdash;
<</if>>"#;

        let ast = parse_passage_body(input, 0, ParseMode::Normal);

        // There should be exactly 1 top-level node: the outer <<if>>
        assert_eq!(ast.nodes.len(), 1, "Expected 1 top-level node, got {}", ast.nodes.len());

        // Verify the outer <<if>> macro
        let outer_if = &ast.nodes[0];
        match outer_if {
            AstNode::Macro { name, args, children, close_span, .. } => {
                assert_eq!(name, "if");
                assert_eq!(args.trim(), "_parts.length > 0");
                assert!(close_span.is_some(), "Outer <<if>> should be properly closed");
                let ch = children.as_ref().expect("Outer <<if>> should have children");

                // Verify the inner <<if>> exists in children
                let inner_if = ch.iter().find(|n| matches!(n, AstNode::Macro { name, .. } if name == "if"));
                assert!(inner_if.is_some(), "Should find inner <<if>> macro");

                // Verify <<else>> is a child (inline macro, no children)
                let else_macro = ch.iter().find(|n| matches!(n, AstNode::Macro { name, .. } if name == "else"));
                assert!(else_macro.is_some(), "Should find <<else>> macro as child");
                if let Some(AstNode::Macro { name, children, .. }) = else_macro {
                    assert_eq!(name, "else");
                    assert!(children.is_none(), "<<else>> should NOT have children (inline clause marker)");
                }

                // Verify the <<=>> expression doesn't include >> in its content
                let print_expr = ch.iter().find(|n| matches!(n, AstNode::Expression { .. }));
                if let Some(AstNode::Expression { content, .. }) = print_expr {
                    assert!(
                        !content.contains(">>"),
                        "<<=>> content should NOT include >>: got '{:?}'",
                        content
                    );
                    assert!(
                        content.trim().contains("_parts[0]"),
                        "<<=>> content should contain _parts[0]: got '{:?}'",
                        content
                    );
                }
            }
            other => panic!("Expected Macro node, got {:?}", other),
        }
    }

    #[test]
    fn parse_if_with_elseif() {
        // <<if>> with <<elseif>> clause
        let input = "<<if $x gt 5>>big<<elseif $x gt 3>>medium<<else>>small<</if>>";
        let ast = parse_passage_body(input, 0, ParseMode::Normal);

        assert_eq!(ast.nodes.len(), 1, "Expected 1 top-level node");
        match &ast.nodes[0] {
            AstNode::Macro { name, args, children, close_span, .. } => {
                assert_eq!(name, "if");
                assert_eq!(args.trim(), "$x gt 5");
                assert!(close_span.is_some(), "<<if>> should be properly closed");
                let ch = children.as_ref().unwrap();

                // Find <<elseif>> — should be inline (no children)
                let elseif = ch.iter().find(|n| matches!(n, AstNode::Macro { name, .. } if name == "elseif"));
                assert!(elseif.is_some(), "Should find <<elseif>> as child");
                if let Some(AstNode::Macro { name, args, children, .. }) = elseif {
                    assert_eq!(name, "elseif");
                    assert_eq!(args.trim(), "$x gt 3");
                    assert!(children.is_none(), "<<elseif>> should NOT have children (inline clause marker)");
                }

                // Find <<else>> — should be inline (no children)
                let else_node = ch.iter().find(|n| matches!(n, AstNode::Macro { name, .. } if name == "else"));
                assert!(else_node.is_some(), "Should find <<else>> as child");
            }
            other => panic!("Expected Macro node, got {:?}", other),
        }
    }

    #[test]
    fn parse_nested_macros() {
        let ast = parse_passage_body(
            "<<if $alive>><<set $msg to \"yes\">>You live<</if>>",
            0,
            ParseMode::Normal,
        );
        let macros = collect_macros(&ast.nodes);
        // Should find both <<if>> and <<set>>
        assert!(macros.len() >= 2);
    }

    #[test]
    fn parse_unclosed_macro() {
        let ast = parse_passage_body("<<if $alive>>never closed", 0, ParseMode::Normal);
        // Should not panic, should produce an AST with the unclosed block
        let macros = collect_macros(&ast.nodes);
        assert!(macros.len() >= 1);
        match macros[0] {
            AstNode::Macro { name, close_span, .. } => {
                assert_eq!(name, "if");
                assert!(close_span.is_none()); // Unclosed
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn expression_macro() {
        let ast = parse_passage_body("<<= $hp>>", 0, ParseMode::Normal);
        let has_expr = ast.nodes.iter().any(|n| matches!(n, AstNode::Expression { kind: ExprKind::Print, .. }));
        assert!(has_expr);
    }

    #[test]
    fn print_expression_closes_at_first_close() {
        // <<=>> should close at the first >>, not consume subsequent >>
        let input = "<<= 1 + 2 >> more text";
        let ast = parse_passage_body(input, 0, ParseMode::Normal);

        assert_eq!(ast.nodes.len(), 2, "Expected 2 nodes (Expression + Text)");
        match &ast.nodes[0] {
            AstNode::Expression { content, kind, .. } => {
                assert_eq!(*kind, ExprKind::Print);
                assert_eq!(content.trim(), "1 + 2", "Content should be '1 + 2', got '{:?}'", content);
            }
            other => panic!("Expected Expression node, got {:?}", other),
        }
    }

    #[test]
    fn print_expression_with_string_containing_gt_gt() {
        // <<=>> with a string literal containing >> should NOT close early
        let input = r#"<<= "hello >>" >>"#;
        let ast = parse_passage_body(input, 0, ParseMode::Normal);

        assert_eq!(ast.nodes.len(), 1, "Expected 1 node");
        match &ast.nodes[0] {
            AstNode::Expression { content, .. } => {
                assert!(
                    content.trim().contains("hello"),
                    "Content should contain 'hello': got '{:?}'",
                    content
                );
            }
            other => panic!("Expected Expression node, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // <<set>> macro structured assignment tests
    // -----------------------------------------------------------------------

    #[test]
    fn set_macro_to_keyword() {
        let ast = parse_passage_body("<<set $hp to 100>>", 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        assert_eq!(macros.len(), 1);
        let sa = macros[0].as_ref().unwrap();
        assert_eq!(sa.target.name, "$hp");
        assert_eq!(sa.operator, SetOperator::To);
        assert_eq!(sa.expression.as_deref(), Some("100"));
    }

    #[test]
    fn set_macro_eq_operator() {
        let ast = parse_passage_body("<<set $hp = 100>>", 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        let sa = macros[0].as_ref().unwrap();
        assert_eq!(sa.operator, SetOperator::Eq);
        assert_eq!(sa.expression.as_deref(), Some("100"));
    }

    #[test]
    fn set_macro_compound_operators() {
        let ast = parse_passage_body("<<set $hp += 10>>", 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        let sa = macros[0].as_ref().unwrap();
        assert_eq!(sa.operator, SetOperator::PlusEq);
        assert_eq!(sa.expression.as_deref(), Some("10"));

        let ast = parse_passage_body("<<set $hp -= 5>>", 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        let sa = macros[0].as_ref().unwrap();
        assert_eq!(sa.operator, SetOperator::MinusEq);
        assert_eq!(sa.expression.as_deref(), Some("5"));
    }

    #[test]
    fn set_macro_postfix() {
        let ast = parse_passage_body("<<set $hp++>>", 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        let sa = macros[0].as_ref().unwrap();
        assert_eq!(sa.target.name, "$hp");
        assert_eq!(sa.operator, SetOperator::PostfixPlus);
        assert!(sa.expression.is_none());

        let ast = parse_passage_body("<<set $hp-->>", 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        assert!(!macros.is_empty(), "Expected to find a 'set' macro");
        let sa = macros[0].as_ref().unwrap();
        assert_eq!(sa.operator, SetOperator::PostfixMinus);
        assert!(sa.expression.is_none());
    }

    #[test]
    fn set_macro_method_call_no_assignment() {
        // <<set $arr.push("item")>> is NOT a simple assignment —
        // set_assignment should be None, and the whole args go to oxc
        let ast = parse_passage_body(r#"<<set $arr.push("item")>>"#, 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        assert_eq!(macros.len(), 1);
        assert!(macros[0].is_none()); // Not a simple assignment
    }

    #[test]
    fn set_macro_complex_expression() {
        // The RHS expression can contain other variables
        let ast = parse_passage_body("<<set $hp to $gold + 10>>", 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        let sa = macros[0].as_ref().unwrap();
        assert_eq!(sa.target.name, "$hp");
        assert_eq!(sa.operator, SetOperator::To);
        // The expression contains the RHS: "$gold + 10"
        assert!(sa.expression.as_ref().unwrap().contains("$gold"));
    }

    #[test]
    fn set_macro_only_rhs_goes_to_oxc() {
        // Verify that collect_js_snippets only gets the RHS for <<set>>
        let ast = parse_passage_body("<<set $hp to 100>>", 0, ParseMode::Normal);
        let snippets = collect_js_snippets(&ast.nodes);
        // Only one snippet, and it should be just the RHS "100"
        let set_snippets: Vec<_> = snippets.iter().filter(|s| s.macro_name == "set").collect();
        assert_eq!(set_snippets.len(), 1);
        assert_eq!(set_snippets[0].source.trim(), "100");
    }

    #[test]
    fn set_macro_postfix_no_js_snippet() {
        // Postfix ++ and -- should NOT produce JS snippets
        let ast = parse_passage_body("<<set $hp++>>", 0, ParseMode::Normal);
        let snippets = collect_js_snippets(&ast.nodes);
        let set_snippets: Vec<_> = snippets.iter().filter(|s| s.macro_name == "set").collect();
        assert!(set_snippets.is_empty());
    }

    #[test]
    fn set_macro_method_call_full_args_to_oxc() {
        // Method calls should send full args to oxc (like <<run>>)
        let ast = parse_passage_body(r#"<<set $arr.push("item")>>"#, 0, ParseMode::Normal);
        let snippets = collect_js_snippets(&ast.nodes);
        let set_snippets: Vec<_> = snippets.iter().filter(|s| s.macro_name == "set").collect();
        assert_eq!(set_snippets.len(), 1);
        assert!(set_snippets[0].source.contains("push"));
    }

    #[test]
    fn set_macro_temp_variable() {
        let ast = parse_passage_body("<<set _i to 0>>", 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        let sa = macros[0].as_ref().unwrap();
        assert_eq!(sa.target.name, "_i");
        assert!(sa.target.is_temporary);
        assert_eq!(sa.operator, SetOperator::To);
    }

    // -----------------------------------------------------------------------
    // Navigation macro link extraction tests
    // -----------------------------------------------------------------------

    #[test]
    fn goto_macro_extracts_link() {
        let ast = parse_passage_body(r#"<<goto "Forest">>"#, 0, ParseMode::Normal);
        let goto_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Goto).collect();
        assert_eq!(goto_links.len(), 1);
        assert_eq!(goto_links[0].target, "Forest");
        assert_eq!(goto_links[0].source, LinkSource::Goto);
        assert!(!goto_links[0].is_dynamic);
    }

    #[test]
    fn include_macro_extracts_link() {
        let ast = parse_passage_body(r#"<<include "Header">>"#, 0, ParseMode::Normal);
        let include_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Include).collect();
        assert_eq!(include_links.len(), 1);
        assert_eq!(include_links[0].target, "Header");
        assert_eq!(include_links[0].source, LinkSource::Include);
    }

    #[test]
    fn button_macro_extracts_link() {
        let ast = parse_passage_body(r#"<<button "Enter cave" "Cave">>"#, 0, ParseMode::Normal);
        let nav_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::NavigationMacro).collect();
        assert_eq!(nav_links.len(), 1);
        assert_eq!(nav_links[0].target, "Cave");
    }

    #[test]
    fn actions_macro_extracts_multiple_links() {
        let ast = parse_passage_body(r#"<<actions "Forest" "Cave" "Village">>"#, 0, ParseMode::Normal);
        let action_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Actions).collect();
        assert_eq!(action_links.len(), 3);
        assert_eq!(action_links[0].target, "Forest");
        assert_eq!(action_links[1].target, "Cave");
        assert_eq!(action_links[2].target, "Village");
    }

    #[test]
    fn goto_dynamic_variable() {
        let ast = parse_passage_body("<<goto $dest>>", 0, ParseMode::Normal);
        let goto_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Goto).collect();
        assert_eq!(goto_links.len(), 1);
        assert!(goto_links[0].is_dynamic);
        assert_eq!(goto_links[0].target, "$dest");
    }

    #[test]
    fn return_macro_extracts_link() {
        // <<return "Town">> — "Town" is display text, not a passage name.
        // With one arg, the target is dynamic (browser history).
        let ast = parse_passage_body(r#"<<return "Town">>"#, 0, ParseMode::Normal);
        let ret_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Return).collect();
        assert_eq!(ret_links.len(), 1);
        assert_eq!(ret_links[0].display.as_deref(), Some("Town"));
        assert!(ret_links[0].is_dynamic, "<<return>> with one arg should be dynamic");
        assert!(ret_links[0].target.is_empty(), "<<return>> with one arg should have no fixed target");
    }

    #[test]
    fn return_macro_two_args() {
        // <<return "Go back" "Town">> — display text + specific passage
        let ast = parse_passage_body(r#"<<return "Go back" "Town">>"#, 0, ParseMode::Normal);
        let ret_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Return).collect();
        assert_eq!(ret_links.len(), 1);
        assert_eq!(ret_links[0].display.as_deref(), Some("Go back"));
        assert_eq!(ret_links[0].target, "Town");
        assert!(!ret_links[0].is_dynamic, "<<return>> with two args should have a fixed target");
    }

    #[test]
    fn back_macro_extracts_link() {
        // <<back "Start">> — "Start" is display text, not a passage name.
        // With one arg, the target is dynamic (browser history).
        let ast = parse_passage_body(r#"<<back "Start">>"#, 0, ParseMode::Normal);
        let back_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Back).collect();
        assert_eq!(back_links.len(), 1);
        assert_eq!(back_links[0].display.as_deref(), Some("Start"));
        assert!(back_links[0].is_dynamic, "<<back>> with one arg should be dynamic");
        assert!(back_links[0].target.is_empty(), "<<back>> with one arg should have no fixed target");
    }

    #[test]
    fn back_macro_two_args() {
        // <<back "Flee" "Forest">> — display text + specific passage
        let ast = parse_passage_body(r#"<<back "Flee" "Forest">>"#, 0, ParseMode::Normal);
        let back_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Back).collect();
        assert_eq!(back_links.len(), 1);
        assert_eq!(back_links[0].display.as_deref(), Some("Flee"));
        assert_eq!(back_links[0].target, "Forest");
        assert!(!back_links[0].is_dynamic, "<<back>> with two args should have a fixed target");
    }

    #[test]
    fn back_macro_no_args() {
        // <<back>> — no args, fully dynamic (previous passage)
        let ast = parse_passage_body("<<back>>", 0, ParseMode::Normal);
        let back_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::Back).collect();
        // With no args at all, there's no string arg to extract — no link
        assert_eq!(back_links.len(), 0, "<<back>> with no args produces no extractable link");
    }
}
