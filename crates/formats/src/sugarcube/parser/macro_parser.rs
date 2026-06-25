//! Macro parsing for `<< >>` syntax, including `<<set>>` assignment parsing
//! and raw-body extraction for `<<script>>`/`<<style>>` macros.
//!
//! ## Flat parse approach
//!
//! The parser emits a **flat** list of AST nodes — macros are never nested
//! at parse time. Block macros like `<<if>>` are emitted as a `Macro` node
//! with `children: None`, and close tags like `<</if>>` are emitted as
//! `MacroClose` nodes. The tree builder (Phase 1.5) pairs these and
//! establishes nesting.
//!
//! The only exception is `<<script>>`/`<<style>>` — these capture their body
//! as raw text (not parsed for SugarCube constructs) and emit with children
//! already populated, since the body is opaque JS/CSS that must not be
//! parsed for SugarCube syntax.

use crate::sugarcube::ast::*;
use super::predicates::{is_ident_char, is_ident_start, is_var_ident_char};
use super::variable_scan::scan_inline_vars;

/// Parse a macro starting after `<<`.
///
/// `i` points to the first character after `<<`.
/// On return, `i` points past the closing `>>` (or end of text).
///
/// `offset` is the base byte offset for the body text being parsed
/// (0 for top-level, nonzero for nested block content).
/// `macro_start` is the position of `<<` in `text`.
pub(super) fn parse_macro(text: &str, i: &mut usize, offset: usize, macro_start: usize) -> AstNode {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Skip whitespace after <<
    while *i < len && bytes[*i] == b' ' {
        *i += 1;
    }

    // Check for close tag: <</name>>
    if *i < len && bytes[*i] == b'/' {
        *i += 1;
        // Skip optional whitespace after /
        while *i < len && bytes[*i] == b' ' {
            *i += 1;
        }
        // Scan the close tag name
        let name_start = *i;
        while *i < len && is_ident_char(bytes[*i]) {
            *i += 1;
        }
        let name = text[name_start..*i].to_string();
        let name_end = *i;
        // Skip to >>
        skip_to_macro_close(text, i);
        let close_span_end = *i;
        return AstNode::MacroClose {
            name,
            name_span: offset + name_start..offset + name_end,
            span: offset + macro_start..offset + close_span_end,
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
        let var_refs = scan_inline_vars(&content, offset + content_start);
        return AstNode::Expression {
            kind,
            content,
            var_refs,
            js_analysis: None,
            span: offset + macro_start..offset + *i,
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

    let var_refs = scan_inline_vars(&args, offset + args_start);

    // For <<set>> macros: parse the assignment structure so that only
    // the RHS expression goes to oxc (not the target + operator).
    let set_assignment = if name.eq_ignore_ascii_case("set") {
        parse_set_assignment(&args, offset + args_start)
    } else {
        None
    };

    // For <<widget>> macros: extract the span of the name being defined.
    // The widget name is the first identifier in the args, e.g.,
    // `<<widget myHelper>>` → definition_name_span covers "myHelper".
    let definition_name_span = if name.eq_ignore_ascii_case("widget") {
        parse_definition_name_span(&args, offset + args_start)
    } else {
        None
    };

    // For <<capture>> macros: extract the target variable reference.
    // e.g., `<<capture $target>>` → capture_target = VarRef { name: "$target", ... }
    let capture_target = if name.eq_ignore_ascii_case("capture") {
        parse_capture_target(&args, offset + args_start)
    } else {
        None
    };

    // For <<for>> macros: extract loop variables from the simplified form.
    // e.g., `<<for _i, $array>>` → for_loop_vars with index_var and iterated_var.
    // The C-style form (`<<for _i to 0; _i lt 10; _i++>>`) returns None.
    let for_loop_vars = if name.eq_ignore_ascii_case("for") {
        parse_for_loop_vars(&args, offset + args_start)
    } else {
        None
    };

    // Phase 6: Structured args from catalog — classify each argument
    // position based on the catalog's MacroArgDef declarations.
    // Only macros with declared args in the catalog get structured extraction.
    let structured_args = parse_structured_args(&name, &args, offset + args_start);

    // ── Raw-body macros (catalog-driven) ─────────────────────────────
    //
    // Macros with `body_is_raw: true` in the catalog contain opaque body
    // content (JS, CSS, etc.) that must NOT be parsed for SugarCube syntax.
    // The parser scans for the close tag, captures the raw body as a Text
    // child, and emits the Macro with children already populated. This is
    // the one case where the flat parse pre-nests content — the body is
    // opaque and the tree builder should not attempt to pair it.
    //
    // Previously this was hardcoded to `script`/`style`/`css`. Now it's
    // catalog-driven (plan.md §7a). `<<style>>` and `<<css>>` have been
    // removed from the catalog because they don't exist in SugarCube.
    let is_raw_body = crate::sugarcube::macros::find_macro(&name)
        .map(|def| def.body_is_raw)
        .unwrap_or(false);
    if is_raw_body {
        let body_text = &text[open_end..];
        let (children, close_offset) = parse_raw_body(body_text, &name, offset + open_end);

        let close_span = if let Some(co) = close_offset {
            // Scan past the close tag to find its full extent
            let mut ci = co;
            while ci < body_text.len() && body_text.as_bytes()[ci] != b'>' {
                ci += 1;
            }
            if ci < body_text.len() && body_text.as_bytes()[ci] == b'>' {
                ci += 1;
                if ci < body_text.len() && body_text.as_bytes()[ci] == b'>' {
                    ci += 1;
                }
            }
            *i = open_end + ci;
            Some(offset + open_end + co..offset + open_end + ci)
        } else {
            // Unclosed raw-body macro — rest of text is body
            *i = len;
            None
        };

        let full_end = close_span.as_ref().map_or(offset + *i, |s| s.end);

        return AstNode::Macro {
            name,
            args,
            var_refs,
            js_analysis: None,
            children: Some(children),
            name_span: offset + name_start..offset + name_start + name_len,
            open_span: offset + macro_start..offset + open_end,
            close_span,
            full_span: offset + macro_start..full_end.max(offset + *i),
            set_assignment,
            definition_name_span,
            close_name_span: None,
            capture_target,
            for_loop_vars,
            structured_args,
        };
    }

    // ── All other macros: flat emission ─────────────────────────────
    //
    // The macro is emitted with children: None and close_span: None.
    // The tree builder (Phase 1.5) will pair it with a MacroClose node
    // if one exists, and populate children/close_span accordingly.
    // If no MacroClose is found, the tree builder consults the catalog's
    // BodyRequirement to decide whether this is an inline macro or an
    // unclosed block.
    AstNode::Macro {
        name,
        args,
        var_refs,
        js_analysis: None,
        children: None,
        name_span: offset + name_start..offset + name_start + name_len,
        open_span: offset + macro_start..offset + open_end,
        close_span: None,
        full_span: offset + macro_start..offset + open_end,
        set_assignment,
        definition_name_span,
        close_name_span: None,
        capture_target,
        for_loop_vars,
        structured_args,
    }
}

/// Scan macro arguments, handling nested `<<`/`>>`, strings, and comments.
///
/// Returns the byte position where args end (before `>>`).
/// Advances `i` past the closing `>>`.
///
/// ## Comment handling
///
/// SugarCube macro args can contain JS expressions with C-style comments
/// (`/* ... */` and `// ...`). A `>>` inside a comment must NOT be treated
/// as the macro closing delimiter. For example:
///
/// ```text
/// <<set $x = [
///   /* comment with >> inside */
///   1, 2, 3
/// ]>>
/// ```
///
/// Without comment awareness, the scanner would find `>>` inside the
/// `/* */` comment and incorrectly close the macro, truncating the args.
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
            // Advance by full UTF-8 character to avoid landing inside
            // a multi-byte sequence, which would cause a panic on slicing.
            *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
            continue;
        }

        // C-style block comment: /* ... */
        // Skip over it entirely — a >> inside the comment must NOT
        // be treated as a macro close delimiter.
        if b == b'/' && *i + 1 < len && bytes[*i + 1] == b'*' {
            *i += 2; // Skip /*
            while *i + 1 < len {
                if bytes[*i] == b'*' && bytes[*i + 1] == b'/' {
                    *i += 2; // Skip */
                    break;
                }
                // Advance by full UTF-8 character to avoid mid-char slicing.
                *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
            }
            continue;
        }

        // JS line comment: // ... (to end of line)
        // Skip to end of line — a >> on this commented-out line must NOT
        // be treated as a macro close delimiter.
        if b == b'/' && *i + 1 < len && bytes[*i + 1] == b'/' {
            *i += 2; // Skip //
            while *i < len && bytes[*i] != b'\n' {
                // Advance by full UTF-8 character to avoid mid-char slicing.
                *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
            }
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

        // Advance by full UTF-8 character to avoid mid-char slicing.
        *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
    }

    // Unclosed macro — everything is args
    *i = len;
    len
}

// ---------------------------------------------------------------------------
// <<widget>> definition name parser
// ---------------------------------------------------------------------------

/// Parse the definition name span from a `<<widget>>` macro's args.
///
/// For `<<widget myHelper>>`, extracts the span of "myHelper".
/// The name is the first identifier in the args string, after optional
/// leading whitespace.
///
/// Returns `None` if no valid identifier is found (e.g., empty args).
fn parse_definition_name_span(args: &str, args_offset: usize) -> Option<std::ops::Range<usize>> {
    let bytes = args.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return None;
    }

    // Skip leading whitespace
    let mut i = 0usize;
    while i < len && bytes[i] == b' ' {
        i += 1;
    }
    if i >= len {
        return None;
    }

    // Scan the identifier
    let name_start = i;
    while i < len && is_ident_char(bytes[i]) {
        i += 1;
    }
    if i == name_start {
        return None;
    }

    Some(args_offset + name_start..args_offset + i)
}

// ---------------------------------------------------------------------------
// <<capture>> target parser
// ---------------------------------------------------------------------------

/// Parse the target variable from a `<<capture>>` macro's args.
///
/// For `<<capture $target>>`, extracts a `VarRef` for `$target`.
/// For `<<capture _temp>>`, extracts a `VarRef` for `_temp`.
///
/// The capture target is the first `$var` or `_var` reference in the args.
/// Returns `None` if no valid variable reference is found.
fn parse_capture_target(args: &str, args_offset: usize) -> Option<VarRef> {
    let bytes = args.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return None;
    }

    // Skip leading whitespace
    let mut i = 0usize;
    while i < len && bytes[i] == b' ' {
        i += 1;
    }
    if i >= len {
        return None;
    }

    // Must start with $ or _
    let is_story_var = bytes[i] == b'$' && i + 1 < len && is_ident_start(bytes[i + 1]);
    let is_temp_var = bytes[i] == b'_' && i + 1 < len && is_ident_start(bytes[i + 1]);

    if !is_story_var && !is_temp_var {
        return None;
    }

    let sigil_start = i;
    i += 1; // Skip $ or _

    // Scan the variable name
    while i < len && is_var_ident_char(bytes[i]) {
        i += 1;
    }

    // Scan dot-notation property path
    let path_start = i;
    let mut property_path = String::new();
    while i < len && bytes[i] == b'.' {
        i += 1; // Skip the dot
        let prop_start = i;
        while i < len && is_var_ident_char(bytes[i]) {
            i += 1;
        }
        if i > prop_start {
            if !property_path.is_empty() {
                property_path.push('.');
            }
            property_path.push_str(&args[prop_start..i]);
        }
    }

    let var_name = args[sigil_start..path_start].to_string();

    Some(VarRef {
        name: var_name,
        property_path,
        is_temporary: is_temp_var,
        is_write: true, // In <<capture>>, the target is captured (written)
        span: args_offset + sigil_start..args_offset + i,
    })
}

// ---------------------------------------------------------------------------
// <<for>> loop variable parser
// ---------------------------------------------------------------------------

/// Parse loop variables from a `<<for>>` macro's args (simplified iteration form).
///
/// SugarCube's simplified `<<for>>` syntax: `<<for _i, $array>>`
///
/// This form is detected by the comma separator between the index variable
/// and the iterated variable. The index variable (`_i`) is always a temporary
/// variable that receives each element during iteration. The iterated variable
/// (`$array`) is the collection being read.
///
/// Returns `None` if the args don't match the simplified iteration pattern
/// (e.g., C-style for loops like `<<for _i to 0; _i lt 10; _i++>>`).
fn parse_for_loop_vars(args: &str, args_offset: usize) -> Option<ForLoopVars> {
    let bytes = args.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return None;
    }

    // Skip leading whitespace
    let mut i = 0usize;
    while i < len && bytes[i] == b' ' {
        i += 1;
    }
    if i >= len {
        return None;
    }

    // Must start with _ (temporary variable for the loop index)
    if bytes[i] != b'_' || i + 1 >= len || !is_ident_start(bytes[i + 1]) {
        return None;
    }

    // Scan the index variable name
    let index_sigil_start = i;
    i += 1; // Skip _
    while i < len && is_var_ident_char(bytes[i]) {
        i += 1;
    }
    let index_name = args[index_sigil_start..i].to_string();
    let index_end = i;

    // Skip whitespace after index variable
    while i < len && bytes[i] == b' ' {
        i += 1;
    }

    // Must have a comma separator — this distinguishes the simplified form
    // from the C-style form (which would have `to` or `=` next)
    if i >= len || bytes[i] != b',' {
        return None;
    }
    i += 1; // Skip comma

    // Skip whitespace after comma
    while i < len && bytes[i] == b' ' {
        i += 1;
    }
    if i >= len {
        return None;
    }

    // Must start with $ (story variable for the collection) or _ (temp var)
    let is_story_var = bytes[i] == b'$' && i + 1 < len && is_ident_start(bytes[i + 1]);
    let is_temp_var = bytes[i] == b'_' && i + 1 < len && is_ident_start(bytes[i + 1]);

    if !is_story_var && !is_temp_var {
        return None;
    }

    let iter_sigil_start = i;
    i += 1; // Skip $ or _

    // Scan the iterated variable name
    while i < len && is_var_ident_char(bytes[i]) {
        i += 1;
    }

    // Scan dot-notation property path
    let iter_path_start = i;
    let mut iter_property_path = String::new();
    while i < len && bytes[i] == b'.' {
        i += 1;
        let prop_start = i;
        while i < len && is_var_ident_char(bytes[i]) {
            i += 1;
        }
        if i > prop_start {
            if !iter_property_path.is_empty() {
                iter_property_path.push('.');
            }
            iter_property_path.push_str(&args[prop_start..i]);
        }
    }

    let iter_name = args[iter_sigil_start..iter_path_start].to_string();

    Some(ForLoopVars {
        index_var: VarRef {
            name: index_name,
            property_path: String::new(),
            is_temporary: true,
            is_write: true, // The index var receives each element
            span: args_offset + index_sigil_start..args_offset + index_end,
        },
        iterated_var: VarRef {
            name: iter_name,
            property_path: iter_property_path,
            is_temporary: is_temp_var,
            is_write: false, // The iterated var is being read
            span: args_offset + iter_sigil_start..args_offset + i,
        },
    })
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
/// - `<<set 100 into $hp>>`   → target=$hp, op=Into,   expr="100"
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

    // Check for reverse-assignment form: `<<set expr into $var>>`
    // In this form, `into` is a keyword and the target variable is on the RIGHT.
    if let Some(into_pos) = find_into_keyword(args) {
        let expr_part = args[..into_pos].trim();
        let after_into = args[into_pos + 4..].trim(); // skip "into"
        if let Some(target) = scan_set_target(after_into, args_offset + into_pos + 4 + (args[into_pos + 4..].len() - after_into.len())) {
            if !expr_part.is_empty() {
                return Some(SetAssignment {
                    target,
                    operator: SetOperator::Into,
                    expression: Some(expr_part.to_string()),
                    expression_span: Some(args_offset..args_offset + into_pos),
                });
            }
        }
        // `into` found but no valid target on the right — fall through
        // to standard parsing (might be a false positive match).
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
        // Property names can contain hyphens (e.g. $obj.my-prop-name).
        // A hyphen is part of the property name if it's immediately followed
        // by an identifier char (no space). This distinguishes:
        //   $obj.my-prop   → property "my-prop"
        //   $obj.my - prop → property "my", then " - prop" is an expression
        //   $obj.my--      → property "my", then "--" is a decrement operator
        while vi < len {
            if is_var_ident_char(bytes[vi]) {
                vi += 1;
            } else if bytes[vi] == b'-' && vi + 1 < len && is_var_ident_char(bytes[vi + 1]) {
                vi += 1; // Include the hyphen as part of the property name
            } else {
                break;
            }
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

/// Find the `into` keyword in a `<<set>>` args string.
///
/// Searches for `into` as a standalone keyword (word-boundary delimited)
/// preceded by at least one whitespace character. Returns the byte position
/// of the `i` in `into`, or `None` if not found.
///
/// This is used for the reverse-assignment form: `<<set 100 into $hp>>`.
fn find_into_keyword(args: &str) -> Option<usize> {
    let bytes = args.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    // Track whether we're inside a string literal
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i + 4 <= len {
        let b = bytes[i];

        // ── String tracking: skip content inside string literals ──
        if b == b'\\' && i + 1 < len {
            i += 2; // skip escaped char
            continue;
        }
        if b == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            i += 1;
            continue;
        }
        if b == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            i += 1;
            continue;
        }
        if in_single_quote || in_double_quote {
            // Advance by full UTF-8 character
            i += args[i..].chars().next().map_or(1, |c| c.len_utf8());
            continue;
        }

        // ── Block comment: /* ... */ — skip entirely ──
        // "into" inside a comment must NOT be matched as a keyword.
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i += 2; // skip /*
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2; // skip */
                    break;
                }
                // Advance by full UTF-8 character
                i += args[i..].chars().next().map_or(1, |c| c.len_utf8());
            }
            continue;
        }

        // ── Line comment: // ... — skip to end of line ──
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2; // skip //
            while i < len && bytes[i] != b'\n' {
                // Advance by full UTF-8 character
                i += args[i..].chars().next().map_or(1, |c| c.len_utf8());
            }
            continue;
        }

        // Only attempt the match at char boundaries where the byte is 'i'.
        // Since "into" starts with an ASCII char, it can only begin at a
        // char boundary. Use `starts_with` so the comparison is char-boundary
        // safe even if a multi-byte UTF-8 sequence happens to follow.
        if b == b'i' && args[i..].starts_with("into") {
            // Must be preceded by whitespace (or be at start — unlikely for `into`)
            let preceded_by_ws = i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t' || bytes[i - 1] == b'\n' || bytes[i - 1] == b'\r';
            // Must be followed by whitespace or end-of-args
            let followed_by_ws = i + 4 >= len || bytes[i + 4] == b' ' || bytes[i + 4] == b'\t' || bytes[i + 4] == b'\n' || bytes[i + 4] == b'\r';
            if preceded_by_ws && followed_by_ws {
                return Some(i);
            }
        }
        // Advance by full UTF-8 character to stay on char boundaries.
        i += args[i..].chars().next().map_or(1, |c| c.len_utf8());
    }
    None
}

/// Scan a set-assignment target variable from the start of a string.
///
/// Used by the `into` reverse-assignment parser to extract the target
/// variable on the right side. Returns `None` if the string doesn't
/// start with a valid `$var` or `_var`.
fn scan_set_target(text: &str, offset: usize) -> Option<VarRef> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return None;
    }

    let is_story_var = bytes[0] == b'$' && len > 1 && is_ident_start(bytes[1]);
    let is_temp_var = bytes[0] == b'_' && len > 1 && is_ident_start(bytes[1]);

    if !is_story_var && !is_temp_var {
        return None;
    }

    let mut vi = 1usize;
    while vi < len && is_var_ident_char(bytes[vi]) {
        vi += 1;
    }

    // Scan dot-notation property path
    let path_start = vi;
    let mut property_path = String::new();
    while vi < len && bytes[vi] == b'.' {
        vi += 1;
        let prop_start = vi;
        while vi < len && is_var_ident_char(bytes[vi]) {
            vi += 1;
        }
        if vi > prop_start {
            if !property_path.is_empty() {
                property_path.push('.');
            }
            property_path.push_str(&text[prop_start..vi]);
        }
    }

    Some(VarRef {
        name: text[..path_start].to_string(),
        property_path,
        is_temporary: is_temp_var,
        is_write: true,
        span: offset..offset + vi,
    })
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
        // Advance by full UTF-8 character to avoid mid-char slicing.
        *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
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
/// with important exceptions: `>>` inside string literals, `/* */` block
/// comments, and `//` line comments is NOT treated as a close delimiter.
/// This prevents premature closing in expressions like:
/// - `<<= "hello >>">>`  (string containing >>)
/// - `<<= /* >> */ 42>>` (block comment containing >>)
/// - `<<= 42 // >>` + newline + `>>` (line comment containing >>)
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
            // Advance by full UTF-8 character to avoid mid-char slicing.
            *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
            continue;
        }

        // C-style block comment: /* ... */
        // Skip over it — a >> inside the comment must NOT close the macro.
        if b == b'/' && *i + 1 < len && bytes[*i + 1] == b'*' {
            *i += 2; // Skip /*
            while *i + 1 < len {
                if bytes[*i] == b'*' && bytes[*i + 1] == b'/' {
                    *i += 2; // Skip */
                    break;
                }
                // Advance by full UTF-8 character to avoid mid-char slicing.
                *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
            }
            continue;
        }

        // JS line comment: // ... (to end of line)
        // Skip to end of line — a >> on this commented-out line must NOT
        // close the macro.
        if b == b'/' && *i + 1 < len && bytes[*i + 1] == b'/' {
            *i += 2; // Skip //
            while *i < len && bytes[*i] != b'\n' {
                // Advance by full UTF-8 character to avoid mid-char slicing.
                *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
            }
            continue;
        }

        // First >> outside of strings/comments closes the expression macro
        if b == b'>' && *i + 1 < len && bytes[*i + 1] == b'>' {
            let content_end = *i;
            *i += 2;
            return content_end;
        }
        // Advance by full UTF-8 character to avoid mid-char slicing.
        *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
    }

    // No closing >> found — everything is content
    *i = len;
    len
}

/// Parse the raw body of a script/style macro until `<</name>>`.
///
/// Unlike the general flat-parse approach, script/style bodies are NOT parsed
/// for SugarCube constructs. The body is captured as a single Text node
/// containing the raw JS/CSS content. This prevents the parser from
/// misinterpreting `<<` in JS template literals or CSS selectors as macro
/// delimiters.
///
/// Returns (children_nodes, close_tag_offset).
/// `close_tag_offset` is the position of `<</name>>` in `text`,
/// or None if the block is unclosed.
fn parse_raw_body(text: &str, macro_name: &str, offset: usize) -> (Vec<AstNode>, Option<usize>) {
    // Find the matching <</name>> — no nesting tracking needed since
    // raw-body macros are opaque (we don't parse them for SugarCube syntax).
    let close_tag = format!("<</{}>>", macro_name);
    let close_tag_alt = format!("<</ {}>>", macro_name); // with space after /

    // Determine whether the raw body is prose (displayed to player) or code.
    // <<script>> body is code (is_prose: false).
    // <<code>> body is prose (is_prose: true) — displayed as literal text.
    // Default: false (safe for any future raw-body macros that are code-like).
    let is_prose = macro_name.eq_ignore_ascii_case("code");

    let len = text.len();

    // Scan for close tag at char boundaries only (no depth tracking —
    // raw-body macros can't nest). Iterating by char ensures we never
    // slice at a byte position that falls inside a multi-byte UTF-8
    // character, which would cause a panic.
    let mut search_from = 0usize;
    while search_from < len {
        // Only attempt the match at char boundaries. Since `close_tag`
        // starts with '<' (a single-byte ASCII char), it can only begin
        // at a char boundary. If we're inside a multi-byte char, its
        // leading byte is not '<', so we can safely skip.
        if text.as_bytes()[search_from] == b'<' {
            if text[search_from..].starts_with(&close_tag)
                || text[search_from..].starts_with(&close_tag_alt)
            {
                let body_content = &text[..search_from];
                let mut children = Vec::new();
                if !body_content.is_empty() {
                    children.push(AstNode::Text {
                        content: body_content.to_string(),
                        var_refs: Vec::new(),
                        span: offset..offset + search_from,
                        is_prose,
                    });
                }
                return (children, Some(search_from));
            }
        }
        // Advance by full UTF-8 character to stay on char boundaries.
        search_from += text[search_from..].chars().next().map_or(1, |c| c.len_utf8());
    }

    // Unclosed — the rest is raw body
    let mut children = Vec::new();
    if !text.is_empty() {
        children.push(AstNode::Text {
            content: text.to_string(),
            var_refs: Vec::new(),
            span: offset..offset + len,
            is_prose,
        });
    }
    (children, None)
}

// ---------------------------------------------------------------------------
// Structured args parsing — catalog-driven extraction
// ---------------------------------------------------------------------------

/// Parse structured macro arguments from the raw args string, using the
/// catalog's `MacroArgDef` declarations to classify each argument.
///
/// This is the Phase 6 implementation: it scans the args string for quoted
/// string tokens and bare passage names, then aligns them with the catalog's
/// declared argument positions and kinds to produce `StructuredMacroArg` entries.
///
/// **Conservative approach**: Only quoted string arguments and bare passage
/// names are extracted. Complex JS expressions (containing operators, function
/// calls, etc.) are left to oxc (Phase 2). This covers the most impactful
/// use cases: passage name references for link extraction, graph edges, and
/// go-to-definition.
///
/// Returns `None` if:
/// - The macro name is not in the catalog
/// - The catalog entry has no declared args (`args: None`)
/// - No structured tokens could be extracted
pub(super) fn parse_structured_args(
    name: &str,
    args: &str,
    args_offset: usize,
) -> Option<Vec<StructuredMacroArg>> {
    use crate::sugarcube::macros::find_macro;
    use crate::sugarcube::ast::{ParsedArgKind, StructuredMacroArg};
    use crate::types::MacroArgKind;

    // Look up the macro in the catalog
    let macro_def = find_macro(name)?;
    let arg_defs = macro_def.args?;

    // If no args string, nothing to extract
    if args.is_empty() {
        return None;
    }

    // Scan the args string for tokens: quoted strings, variable refs, and bare identifiers
    let tokens = scan_arg_tokens(args, args_offset);

    // If no tokens found, nothing to structure
    if tokens.is_empty() {
        return None;
    }

    // Classify each token against the catalog's arg declarations
    let mut structured = Vec::new();
    for (token_idx, token) in tokens.iter().enumerate() {
        // Find the catalog arg def for this position.
        // If we have more tokens than declared args, use the last declared arg
        // (some macros like <<actions>> accept variable numbers of the same arg type).
        let arg_def = arg_defs.iter().find(|a| a.position == token_idx)
            .or_else(|| arg_defs.last());

        let kind = if let Some(def) = arg_def {
            // Classify based on the catalog's declarations
            if def.is_passage_ref && token.is_string_like() {
                ParsedArgKind::PassageRef
            } else if def.is_selector {
                ParsedArgKind::Selector
            } else if def.is_variable && (token.is_variable_ref() || token.is_quoted_variable_name()) {
                // SugarCube form macros (checkbox, radiobutton, textbox, etc.)
                // take the receiver variable as a QUOTED name string (e.g.,
                // "$color"). Recognize both unquoted `$var` and quoted `"$var"`
                // forms as VariableRef so variable-write tracking works.
                // See plan.md §4.5.1, §4.5.2.
                ParsedArgKind::VariableRef
            } else if def.is_passage_ref && token.is_variable_ref() {
                // Variable used where a passage ref is expected — dynamic navigation
                ParsedArgKind::VariableRef
            } else {
                // Map the catalog's MacroArgKind to our parsed kind
                match def.kind {
                    MacroArgKind::Expression => ParsedArgKind::Expression,
                    MacroArgKind::String => {
                        // String args that aren't passage refs or selectors.
                        // "Label" is a special case: the first arg of link/button-style
                        // macros that have a second passage_ref arg (e.g., <<link "Talk" "Shop">>).
                        // For other macros with String args (e.g., <<timed "2s">>), it's just String.
                        if token.is_string_like() {
                            if def.position == 0 && !def.is_passage_ref && !def.is_selector {
                                // Check if there's a passage_ref arg later — if so,
                                // this first arg is a display label
                                let has_passage_ref_later = arg_defs.iter()
                                    .any(|a| a.is_passage_ref && a.position > 0);
                                if has_passage_ref_later {
                                    ParsedArgKind::Label
                                } else {
                                    ParsedArgKind::String
                                }
                            } else {
                                ParsedArgKind::String
                            }
                        } else {
                            ParsedArgKind::Expression
                        }
                    }
                    MacroArgKind::Selector => ParsedArgKind::Selector,
                    MacroArgKind::Variable => {
                        if token.is_variable_ref() || token.is_quoted_variable_name() {
                            ParsedArgKind::VariableRef
                        } else {
                            ParsedArgKind::Expression
                        }
                    }
                    MacroArgKind::Keyword => {
                        // Bareword keyword flag (autofocus, selected, keep, etc.)
                        if token.is_keyword_token() {
                            ParsedArgKind::Keyword
                        } else {
                            ParsedArgKind::Expression
                        }
                    }
                    MacroArgKind::Link => {
                        // Link markup ([[...]]). Currently the scanner skips
                        // bracketed content, so link markup falls through to
                        // Expression. When the scanner is updated to recognize
                        // [[...]] as a token, this will return LinkMarkup.
                        if token.is_link_markup() {
                            ParsedArgKind::LinkMarkup
                        } else {
                            ParsedArgKind::Expression
                        }
                    }
                    MacroArgKind::Image => {
                        // Image markup ([img[...]]). Same caveat as Link.
                        if token.is_image_markup() {
                            ParsedArgKind::ImageMarkup
                        } else {
                            ParsedArgKind::Expression
                        }
                    }
                    MacroArgKind::Number => {
                        // Numeric literal (100, 0.5, etc.)
                        if token.is_number_literal() {
                            ParsedArgKind::Number
                        } else {
                            ParsedArgKind::Expression
                        }
                    }
                }
            }
        } else {
            // No catalog entry for this position — infer from token type
            match token.kind {
                ArgTokenKind::QuotedString => ParsedArgKind::String,
                ArgTokenKind::VariableRef => ParsedArgKind::VariableRef,
                ArgTokenKind::BareName => ParsedArgKind::String,
            }
        };

        structured.push(StructuredMacroArg {
            kind,
            value: token.value.clone(),
            span: token.span.clone(),
        });
    }

    if structured.is_empty() {
        None
    } else {
        Some(structured)
    }
}

/// The kind of token found when scanning macro arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgTokenKind {
    /// A quoted string literal: "hello" or 'world'
    QuotedString,
    /// A variable reference: $var or _var
    VariableRef,
    /// A bare identifier-like name (potential passage name)
    BareName,
}

/// A token extracted from macro arguments during structured scanning.
#[derive(Debug, Clone)]
struct ArgToken {
    /// The kind of token.
    kind: ArgTokenKind,
    /// The token value (string content without quotes for quoted strings).
    value: String,
    /// Byte range in the passage body (passage-body-relative).
    span: std::ops::Range<usize>,
}

impl ArgToken {
    fn is_string_like(&self) -> bool {
        matches!(self.kind, ArgTokenKind::QuotedString | ArgTokenKind::BareName)
    }

    fn is_variable_ref(&self) -> bool {
        matches!(self.kind, ArgTokenKind::VariableRef)
    }

    /// Returns true if this token is a quoted string whose content looks like
    /// a SugarCube variable name — i.e., it starts with `$` or `_` followed by
    /// an identifier-start character, optionally with dot-notation property
    /// access (e.g., `"$color"`, `"_counter"`, `"$foo.bar"`, `"$foo[0]"`).
    ///
    /// SugarCube form macros (`<<checkbox>>`, `<<radiobutton>>`, `<<textbox>>`,
    /// `<<numberbox>>`, `<<textarea>>`, `<<cycle>>`, `<<listbox>>`) take the
    /// receiver variable as a **quoted** name string (e.g., `"$color"`). The
    /// quotes are required because SugarCube auto-substitutes unquoted `$var`
    /// with its value. By quoting the name, the macro receives the literal
    /// string `"$color"` and uses it to identify which variable to modify.
    ///
    /// This helper lets the structured-args classifier recognize these quoted
    /// variable names as `VariableRef` rather than `String`/`Expression`,
    /// which is critical for correct variable-write tracking, hover, and
    /// completion (see plan.md §4.5.1, §4.5.2).
    fn is_quoted_variable_name(&self) -> bool {
        if self.kind != ArgTokenKind::QuotedString {
            return false;
        }
        let bytes = self.value.as_bytes();
        if bytes.is_empty() {
            return false;
        }
        // Must start with $ or _ followed by an identifier-start character.
        let first = bytes[0];
        if first != b'$' && first != b'_' {
            return false;
        }
        if bytes.len() < 2 {
            return false;
        }
        // The second character must be a valid identifier-start (letter or underscore).
        // (Digits are not valid as the first char of an identifier.)
        let second = bytes[1];
        second.is_ascii_alphabetic() || second == b'_'
    }

    /// Returns true if this token is a numeric literal (integer or float).
    ///
    /// e.g., `100`, `0.5`, `42`, `3.14`. Does NOT accept leading/trailing
    /// whitespace or signs (`-5`, `+3`) — those are expressions.
    fn is_number_literal(&self) -> bool {
        if self.kind != ArgTokenKind::BareName {
            return false;
        }
        let bytes = self.value.as_bytes();
        if bytes.is_empty() {
            return false;
        }
        let mut has_digit = false;
        let mut has_dot = false;
        for &b in bytes {
            if b.is_ascii_digit() {
                has_digit = true;
            } else if b == b'.' && !has_dot {
                has_dot = true;
            } else {
                return false; // non-digit, non-dot (or second dot)
            }
        }
        has_digit // must have at least one digit
    }

    /// Returns true if this token looks like a bareword keyword.
    ///
    /// Keywords are bare names (unquoted identifiers) that appear as
    /// positional args. The classifier checks this when `def.kind == Keyword`.
    /// e.g., `autofocus`, `selected`, `keep`, `container`, `autocheck`,
    /// `checked`, `once`, `autoselect`, `play`, `pause`, `stop`, etc.
    fn is_keyword_token(&self) -> bool {
        matches!(self.kind, ArgTokenKind::BareName)
    }

    /// Returns true if this token is a link markup (`[[...]]`).
    ///
    /// The token value starts with `[[` (the scanner captures the full
    /// `[[...]]` construct as a single BareName token because `[` is a
    /// bracket-start char that gets skipped — BUT we need to check the
    /// scanner behavior. Actually, `[` triggers the bracket-skip path,
    /// so `[[...]]` would be skipped entirely. This means link markup
    /// args are NOT tokenized by `scan_arg_tokens` and fall through to
    /// the expression path.
    ///
    /// For now, this returns false — link markup detection requires
    /// teaching `scan_arg_tokens` to recognize `[[` as a link-start
    /// rather than a bracket-skip. That's a future enhancement; for
    /// now, link/image args are classified as Expression (which is
    /// safe — they just won't get special LinkMarkup/ImageMarkup
    /// classification until the scanner is updated).
    fn is_link_markup(&self) -> bool {
        false // TODO: teach scan_arg_tokens to recognize [[...]] as a token
    }

    /// Returns true if this token is an image markup (`[img[...]]`).
    ///
    /// Same caveat as `is_link_markup` — the scanner currently skips
    /// bracketed content, so image markup is not tokenized.
    fn is_image_markup(&self) -> bool {
        false // TODO: teach scan_arg_tokens to recognize [img[...]] as a token
    }
}

/// Scan macro argument tokens: quoted strings, variable references, and bare names.
///
/// This produces a sequence of top-level tokens from the args string, respecting
/// string quoting, nested parentheses/brackets, and commas (as separators for
/// multi-arg macros like `<<link "Talk" "Shop">>`).
///
/// Tokens inside parentheses, brackets, or braces are NOT extracted — those are
/// JS expression contexts that oxc handles. Only top-level quoted strings, `$var`
/// references, and bare identifier-like tokens are extracted.
fn scan_arg_tokens(args: &str, args_offset: usize) -> Vec<ArgToken> {
    let mut tokens = Vec::new();
    let bytes = args.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    // Helper: advance by full UTF-8 character
    let advance = |pos: usize| -> usize {
        args[pos..].chars().next().map_or(1, |c| c.len_utf8())
    };

    while i < len {
        // Skip whitespace and commas (arg separators)
        while i < len && (bytes[i] == b' ' || bytes[i] == b',') {
            i += 1;
        }
        if i >= len {
            break;
        }

        let b = bytes[i];

        // Quoted string: "..." or '...'
        if b == b'"' || b == b'\'' {
            let quote = b;
            let _token_start = i;
            i += 1; // skip opening quote
            let content_start = i;
            while i < len && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2; // skip escaped char
                } else {
                    i += advance(i);
                }
            }
            let content_end = i;
            let value = args[content_start..content_end].to_string();
            if i < len {
                i += 1; // skip closing quote
            }
            tokens.push(ArgToken {
                kind: ArgTokenKind::QuotedString,
                value,
                span: args_offset + content_start..args_offset + content_end,
            });
            continue;
        }

        // Variable reference: $var or _var (word boundary)
        if (b == b'$' || b == b'_') && i + 1 < len && is_ident_start(bytes[i + 1]) {
            let token_start = i;
            i += 1; // skip sigil
            while i < len && is_var_ident_char(bytes[i]) {
                i += 1;
            }
            // Also scan dot-notation properties
            while i < len && bytes[i] == b'.' {
                i += 1; // skip dot
                while i < len && is_var_ident_char(bytes[i]) {
                    i += 1;
                }
            }
            let value = args[token_start..i].to_string();
            tokens.push(ArgToken {
                kind: ArgTokenKind::VariableRef,
                value,
                span: args_offset + token_start..args_offset + i,
            });
            continue;
        }

        // Parenthesized/bracketed expression: skip entirely
        // These are JS expression contexts that oxc handles.
        if b == b'(' || b == b'[' || b == b'{' {
            let open = b;
            let mut depth = 1u32;
            i += 1;
            while i < len && depth > 0 {
                // Skip strings inside brackets
                if bytes[i] == b'"' || bytes[i] == b'\'' {
                    let q = bytes[i];
                    i += 1;
                    while i < len && bytes[i] != q {
                        if bytes[i] == b'\\' && i + 1 < len {
                            i += 2;
                        } else {
                            i += advance(i);
                        }
                    }
                    if i < len {
                        i += 1;
                    }
                    continue;
                }
                if bytes[i] == open {
                    depth += 1;
                } else if bytes[i] == b')' && open == b'(' {
                    depth -= 1;
                } else if bytes[i] == b']' && open == b'[' {
                    depth -= 1;
                } else if bytes[i] == b'}' && open == b'{' {
                    depth -= 1;
                }
                // Advance by full UTF-8 character to avoid mid-char slicing.
                i += advance(i);
            }
            continue;
        }

        // Operator characters: skip (these are JS operators that oxc handles)
        if b == b'=' || b == b'+' || b == b'-' || b == b'*' || b == b'/' || b == b'%' || b == b'!' || b == b'<' || b == b'>' || b == b'&' || b == b'|' || b == b'?' || b == b':' {
            // Skip to next space or end of operator sequence
            while i < len && !bytes[i].is_ascii_whitespace() && bytes[i] != b',' && bytes[i] != b'"' && bytes[i] != b'\'' && bytes[i] != b'(' && bytes[i] != b')' && bytes[i] != b'[' && bytes[i] != b']' && bytes[i] != b'{' && bytes[i] != b'}' && bytes[i] != b'$' && bytes[i] != b'_' {
                // Advance by full UTF-8 character to avoid mid-char slicing.
                i += advance(i);
            }
            continue;
        }

        // Bare name: identifier-like token (potential passage name, keyword, etc.)
        // Also scan numeric literals (100, 0.5, 42) as BareName tokens so the
        // classifier can recognize them as Number args.
        if is_ident_start(b) || b.is_ascii_digit() {
            let token_start = i;
            if b.is_ascii_digit() {
                // Numeric literal: scan digits and at most one decimal point.
                let mut has_dot = false;
                while i < len {
                    if bytes[i].is_ascii_digit() {
                        i += 1;
                    } else if bytes[i] == b'.' && !has_dot {
                        has_dot = true;
                        i += 1;
                    } else {
                        break;
                    }
                }
            } else {
                // Identifier-like token.
                while i < len && is_bare_name_char(bytes[i]) {
                    i += 1;
                }
            }
            let value = args[token_start..i].to_string();
            // Only include bare names that look like passage names
            // (not JS keywords, not numbers, not operators)
            if is_bare_passage_name_candidate(&value) {
                tokens.push(ArgToken {
                    kind: ArgTokenKind::BareName,
                    value,
                    span: args_offset + token_start..args_offset + i,
                });
            }
            continue;
        }

        // Anything else: skip by full UTF-8 character
        i += advance(i);
    }

    tokens
}

/// Check if a character can be part of a bare name token (passage names allow
/// hyphens and underscores in addition to alphanumeric characters).
fn is_bare_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Check if a bare name token looks like a passage name candidate.
///
/// This filters out JS keywords and operators that might be mistakenly
/// classified as bare names. Only alphanumeric strings (possibly with
/// hyphens/underscores) that don't look like JS keywords are included.
fn is_bare_passage_name_candidate(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Filter out common JS/SugarCube keywords that appear as bare identifiers
    // in macro args but are NOT passage names
    match s {
        "to" | "is" | "isnot" | "eq" | "neq" | "gt" | "gte" | "lt" | "lte"
        | "and" | "or" | "not" | "true" | "false" | "null" | "undefined"
        | "new" | "typeof" | "instanceof" | "in" | "of" | "delete"
        | "var" | "let" | "const" | "function" | "return" | "if" | "else"
        | "for" | "while" | "do" | "switch" | "case" | "default" | "break"
        | "continue" | "try" | "catch" | "finally" | "throw" | "class"
        | "extends" | "import" | "export" | "from" | "as" | "this"
        | "void" | "with" | "yield" | "async" | "await" => false,
        _ => {
            // Must start with a letter (for passage names/keywords) OR a
            // digit (for numeric literals like 100, 0.5). Sigils ($, _) are
            // handled by the VariableRef scanner, not here.
            let first = s.as_bytes()[0];
            first.is_ascii_alphabetic() || first.is_ascii_digit()
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::{AstNode, ParseMode, ExprKind, SetOperator, LinkSource, ParsedArgKind};
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

    // -----------------------------------------------------------------------
    // Comment-aware macro scanning tests
    // -----------------------------------------------------------------------

    #[test]
    fn set_macro_with_block_comment_containing_gt_gt() {
        // A >> inside a /* */ comment must NOT close the macro.
        // Without comment awareness, the scanner would find >> inside
        // the comment and truncate the args.
        let input = r#"<<set $x = [1, /* >> */ 2, 3]>>"#;
        let ast = parse_passage_body(input, 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, args, .. } if name == "set" => Some(args.clone()),
            _ => None,
        }).collect();
        assert_eq!(macros.len(), 1);
        // The args should contain the full expression including the
        // commented-out >> and the 2, 3 after it.
        assert!(macros[0].contains("2, 3"), "Args should contain '2, 3' after the block comment, got: {:?}", macros[0]);
    }

    #[test]
    fn set_macro_with_line_comment_containing_gt_gt() {
        // A >> inside a // comment must NOT close the macro.
        let input = "<<set $x = [1, // >> close\n2]>>";
        let ast = parse_passage_body(input, 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, args, .. } if name == "set" => Some(args.clone()),
            _ => None,
        }).collect();
        assert_eq!(macros.len(), 1);
        assert!(macros[0].contains("2]"), "Args should contain '2]' after the line comment, got: {:?}", macros[0]);
    }

    #[test]
    fn set_macro_multiline_with_comments() {
        // Realistic multi-line <<set>> with C-style comments,
        // mimicking the user's $UI_PROFILES pattern.
        let input = r#"<<set $arr = [
  /* Block comment */
  {
    id: "base",
    // Line comment
    value: 42
  }
]>>"#;
        let ast = parse_passage_body(input, 0, ParseMode::Normal);
        let macros: Vec<_> = ast.nodes.iter().filter_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => Some(set_assignment.clone()),
            _ => None,
        }).collect();
        assert_eq!(macros.len(), 1);
        let sa = macros[0].as_ref().unwrap();
        assert_eq!(sa.target.name, "$arr");
        assert_eq!(sa.operator, SetOperator::Eq);
        // The expression should contain the full array literal with comments
        let expr = sa.expression.as_ref().unwrap();
        assert!(expr.contains("id: \"base\""), "Expression should contain 'id: \"base\"', got: {:?}", expr);
        assert!(expr.contains("value: 42"), "Expression should contain 'value: 42', got: {:?}", expr);
    }

    #[test]
    fn expression_macro_with_block_comment_containing_gt_gt() {
        // <<=>> with a >> inside a /* */ comment should not close early
        let input = r#"<<= 1 + /* >> */ 2>>"#;
        let ast = parse_passage_body(input, 0, ParseMode::Normal);
        match &ast.nodes[0] {
            AstNode::Expression { content, kind, .. } => {
                assert_eq!(*kind, ExprKind::Print);
                // The content should include the full expression
                assert!(content.contains("1 +"), "Content should contain '1 +', got: {:?}", content);
                assert!(content.contains("2"), "Content should contain '2', got: {:?}", content);
            }
            other => panic!("Expected Expression node, got {:?}", other),
        }
    }

    #[test]
    fn expression_macro_with_line_comment_containing_gt_gt() {
        // <<=>> with a >> inside a // comment should not close early
        let input = "<<= 1 + // >> close\n2>>";
        let ast = parse_passage_body(input, 0, ParseMode::Normal);
        match &ast.nodes[0] {
            AstNode::Expression { content, kind, .. } => {
                assert_eq!(*kind, ExprKind::Print);
                assert!(content.contains("2"), "Content should contain '2' after line comment, got: {:?}", content);
            }
            other => panic!("Expected Expression node, got {:?}", other),
        }
    }

    // ── Phase 5: capture_target and for_loop_vars ──────────────────────

    #[test]
    fn parse_capture_target() {
        let ast = parse_passage_body("<<capture $target>>Captured!<</capture>>", 0, ParseMode::Normal);
        let macros = collect_macros(&ast.nodes);
        assert!(!macros.is_empty(), "Should have at least one macro");
        match macros.iter().find(|m| matches!(m, AstNode::Macro { name, .. } if name == "capture")) {
            Some(AstNode::Macro { capture_target, .. }) => {
                assert!(capture_target.is_some(), "<<capture>> should have capture_target");
                let ct = capture_target.as_ref().unwrap();
                assert_eq!(ct.name, "$target");
                assert!(ct.is_write, "Capture target should be marked as write");
                assert!(!ct.is_temporary, "Story var should not be temporary");
            }
            _ => panic!("Expected <<capture>> macro"),
        }
    }

    #[test]
    fn parse_capture_temp_var() {
        let ast = parse_passage_body("<<capture _temp>>Captured!<</capture>>", 0, ParseMode::Normal);
        let macros = collect_macros(&ast.nodes);
        match macros.iter().find(|m| matches!(m, AstNode::Macro { name, .. } if name == "capture")) {
            Some(AstNode::Macro { capture_target, .. }) => {
                assert!(capture_target.is_some());
                let ct = capture_target.as_ref().unwrap();
                assert_eq!(ct.name, "_temp");
                assert!(ct.is_temporary, "_temp should be marked as temporary");
            }
            _ => panic!("Expected <<capture>> macro"),
        }
    }

    #[test]
    fn parse_for_loop_simplified() {
        let ast = parse_passage_body("<<for _i, $items>>Item<</for>>", 0, ParseMode::Normal);
        let macros = collect_macros(&ast.nodes);
        match macros.iter().find(|m| matches!(m, AstNode::Macro { name, .. } if name == "for")) {
            Some(AstNode::Macro { for_loop_vars, .. }) => {
                assert!(for_loop_vars.is_some(), "<<for _i, $items>> should have for_loop_vars");
                let fl = for_loop_vars.as_ref().unwrap();
                assert_eq!(fl.index_var.name, "_i");
                assert_eq!(fl.iterated_var.name, "$items");
                assert!(fl.index_var.is_temporary, "Index var should be temporary");
                assert!(fl.index_var.is_write, "Index var should be write");
                assert!(!fl.iterated_var.is_write, "Iterated var should be read");
                assert!(!fl.iterated_var.is_temporary, "$items should not be temporary");
            }
            _ => panic!("Expected <<for>> macro"),
        }
    }

    #[test]
    fn parse_for_loop_c_style_no_for_loop_vars() {
        let ast = parse_passage_body("<<for _i to 0; _i lt 10; _i++>>Loop<</for>>", 0, ParseMode::Normal);
        let macros = collect_macros(&ast.nodes);
        match macros.iter().find(|m| matches!(m, AstNode::Macro { name, .. } if name == "for")) {
            Some(AstNode::Macro { for_loop_vars, .. }) => {
                assert!(for_loop_vars.is_none(),
                    "C-style <<for>> should NOT have for_loop_vars");
            }
            _ => panic!("Expected <<for>> macro"),
        }
    }

    #[test]
    fn parse_for_loop_with_property_path() {
        let ast = parse_passage_body("<<for _item, $player.inventory>>Item<</for>>", 0, ParseMode::Normal);
        let macros = collect_macros(&ast.nodes);
        match macros.iter().find(|m| matches!(m, AstNode::Macro { name, .. } if name == "for")) {
            Some(AstNode::Macro { for_loop_vars, .. }) => {
                assert!(for_loop_vars.is_some());
                let fl = for_loop_vars.as_ref().unwrap();
                assert_eq!(fl.iterated_var.name, "$player");
                assert_eq!(fl.iterated_var.property_path, "inventory");
            }
            _ => panic!("Expected <<for>> macro"),
        }
    }

    #[test]
    fn parse_capture_with_property_path() {
        let ast = parse_passage_body("<<capture $target.name>>Captured!<</capture>>", 0, ParseMode::Normal);
        let macros = collect_macros(&ast.nodes);
        match macros.iter().find(|m| matches!(m, AstNode::Macro { name, .. } if name == "capture")) {
            Some(AstNode::Macro { capture_target, .. }) => {
                assert!(capture_target.is_some());
                let ct = capture_target.as_ref().unwrap();
                assert_eq!(ct.name, "$target");
                assert_eq!(ct.property_path, "name");
            }
            _ => panic!("Expected <<capture>> macro"),
        }
    }

    // -----------------------------------------------------------------------
    // Phase 6: Structured args from catalog tests
    // -----------------------------------------------------------------------

    #[test]
    fn structured_args_goto_quoted_passage() {
        let ast = parse_passage_body(r#"<<goto "Cave">>"#, 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "goto" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].kind, ParsedArgKind::PassageRef);
        assert_eq!(args[0].value, "Cave");
    }

    #[test]
    fn structured_args_include_quoted_passage() {
        let ast = parse_passage_body(r#"<<include "Header">>"#, 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "include" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].kind, ParsedArgKind::PassageRef);
        assert_eq!(args[0].value, "Header");
    }

    #[test]
    fn structured_args_link_label_and_passage() {
        let ast = parse_passage_body(r#"<<link "Talk" "Shop">>"#, 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "link" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].kind, ParsedArgKind::Label);
        assert_eq!(args[0].value, "Talk");
        assert_eq!(args[1].kind, ParsedArgKind::PassageRef);
        assert_eq!(args[1].value, "Shop");
    }

    #[test]
    fn structured_args_button_label_and_passage() {
        let ast = parse_passage_body(r#"<<button "Enter cave" "Cave">>"#, 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "button" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].kind, ParsedArgKind::Label);
        assert_eq!(args[0].value, "Enter cave");
        assert_eq!(args[1].kind, ParsedArgKind::PassageRef);
        assert_eq!(args[1].value, "Cave");
    }

    #[test]
    fn structured_args_goto_variable_target() {
        let ast = parse_passage_body("<<goto $dest>>", 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "goto" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].kind, ParsedArgKind::VariableRef);
        assert_eq!(args[0].value, "$dest");
    }

    #[test]
    fn structured_args_actions_multiple_passages() {
        let ast = parse_passage_body(r#"<<actions "Forest" "Cave" "Village">>"#, 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "actions" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0].kind, ParsedArgKind::PassageRef);
        assert_eq!(args[0].value, "Forest");
        assert_eq!(args[1].kind, ParsedArgKind::PassageRef);
        assert_eq!(args[1].value, "Cave");
        assert_eq!(args[2].kind, ParsedArgKind::PassageRef);
        assert_eq!(args[2].value, "Village");
    }

    #[test]
    fn structured_args_set_expression_args() {
        // <<set>> has declared args (Expression kind), so structured extraction
        // does run, but the extracted tokens are Expression kind (not PassageRef)
        let ast = parse_passage_body("<<set $hp to 100>>", 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "set" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        // <<set>> has Expression args — the structured extraction may produce
        // tokens, but they should be VariableRef/Expression, not PassageRef
        if let Some(args) = macro_node {
            for arg in &args {
                assert!(!matches!(arg.kind, ParsedArgKind::PassageRef | ParsedArgKind::Label | ParsedArgKind::Selector));
            }
        }
        // Either no structured args, or args that aren't passage refs — both OK
    }

    #[test]
    fn structured_args_if_no_catalog_args() {
        // <<if>> has args: None in catalog (undeclared JS expression args),
        // so no structured extraction — it falls through to oxc
        let ast = parse_passage_body("<<if $hp gte 50>>", 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "if" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        assert!(macro_node.is_none());
    }

    #[test]
    fn structured_args_goto_bare_name() {
        // <<goto Forest>> — bare name should be extracted as a PassageRef
        let ast = parse_passage_body("<<goto Forest>>", 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "goto" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].kind, ParsedArgKind::PassageRef);
        assert_eq!(args[0].value, "Forest");
    }

    #[test]
    fn structured_args_remove_selector() {
        // <<remove "#hp-bar">> — selector argument
        let ast = parse_passage_body("<<remove \"#hp-bar\">>", 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "remove" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].kind, ParsedArgKind::Selector);
        assert_eq!(args[0].value, "#hp-bar");
    }

    #[test]
    fn structured_args_display_deprecated_passage_ref() {
        // <<display>> is deprecated but still has a passage ref arg
        let ast = parse_passage_body(r#"<<display "OldPassage">>"#, 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "display" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].kind, ParsedArgKind::PassageRef);
        assert_eq!(args[0].value, "OldPassage");
    }

    #[test]
    fn structured_args_timed_speed_string() {
        // <<timed "2s">> — generic string (speed value), not a passage ref
        let ast = parse_passage_body(r#"<<timed "2s">>"#, 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "timed" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 1);
        // "2s" is a speed value, classified as String (not PassageRef)
        assert_eq!(args[0].kind, ParsedArgKind::String);
        assert_eq!(args[0].value, "2s");
    }

    #[test]
    fn structured_args_link_single_arg_label() {
        // <<link "Talk">> — single string arg is both label and target
        let ast = parse_passage_body(r#"<<link "Talk">>"#, 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "link" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args.len(), 1);
        // First arg of link is the label (position 0, not is_passage_ref)
        assert_eq!(args[0].kind, ParsedArgKind::Label);
        assert_eq!(args[0].value, "Talk");
    }

    #[test]
    fn structured_args_span_correctness() {
        // Verify that spans point to the correct positions in the passage body
        // <<goto "Cave">>
        // 0123456789012345
        //   args start at position 6 (after "<<goto ")
        //   "Cave" content starts at position 7
        let ast = parse_passage_body(r#"<<goto "Cave">>"#, 0, ParseMode::Normal);
        let macro_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, structured_args, .. } if name == "goto" => Some(structured_args.clone()),
            _ => None,
        }).unwrap();

        let args = macro_node.unwrap();
        assert_eq!(args[0].value, "Cave");
        // The span should cover "Cave" (4 chars), starting after the opening quote
        assert_eq!(args[0].span.end - args[0].span.start, 4);
    }

    #[test]
    fn set_into_assignment() {
        // <<set 100 into $hp>> — reverse-assignment form
        let ast = parse_passage_body("<<set 100 into $hp>>", 0, ParseMode::Normal);
        let sa = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => set_assignment.clone(),
            _ => None,
        }).expect("should find <<set>> with set_assignment");

        assert_eq!(sa.target.name, "$hp");
        assert_eq!(sa.operator, SetOperator::Into);
        assert_eq!(sa.expression.as_deref(), Some("100"));
        assert!(sa.target.is_write, "into target should be marked as write");
    }

    #[test]
    fn set_into_keyword_boundary() {
        // `intogether` should NOT match as `into` keyword
        // This is an edge case — `intogether` is not valid SugarCube but
        // we must not accidentally parse it as `into`.
        let ast = parse_passage_body("<<set $x intogether $y>>", 0, ParseMode::Normal);
        let sa = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => set_assignment.clone(),
            _ => None,
        });
        // Should NOT parse as Into — `intogether` is not a keyword boundary
        assert!(sa.is_none() || sa.as_ref().map(|s| s.operator != SetOperator::Into).unwrap_or(true),
            "intogether should not be parsed as 'into' keyword");
    }

    #[test]
    fn set_into_with_expression() {
        // <<set $base + $bonus into $total>> — complex expression on the left
        let ast = parse_passage_body("<<set $base + $bonus into $total>>", 0, ParseMode::Normal);
        let sa = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, set_assignment, .. } if name == "set" => set_assignment.clone(),
            _ => None,
        }).expect("should find <<set>> with set_assignment");

        assert_eq!(sa.target.name, "$total");
        assert_eq!(sa.operator, SetOperator::Into);
        assert_eq!(sa.expression.as_deref(), Some("$base + $bonus"));
    }
}
