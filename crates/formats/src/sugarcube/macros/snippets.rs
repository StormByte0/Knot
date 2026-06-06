//! Per-macro VS Code snippet definitions and snippet builder.
//!
//! Provides completion snippet templates for SugarCube macros, including
//! per-macro overrides and a generic fallback for unknown macros.

use super::classifiers::block_macro_names;

/// A per-macro snippet body for completion.
///
/// The snippet is inserted AFTER `<<` and BEFORE the auto-closed `>>`.
///
/// Tabstop conventions:
/// - `$1`, `$2` … — positional tab stops
/// - `${1:placeholder}` — tab stop with placeholder text
/// - `$0` — final cursor position
pub fn macro_snippet(name: &str) -> Option<&'static str> {
    match name {
        // ── Variables ─────────────────────────────────────────────────────
        "set"     => Some(r#"set ${1:\$var} to ${2:value}"#),
        "unset"   => Some(r#"unset ${1:\$var}"#),
        "run"     => Some(r#"run ${1:expression}"#),
        "capture" => Some(r#"capture ${1:\$var}>>\n$2\n<</capture"#),

        // ── Output ────────────────────────────────────────────────────────
        "print"    => Some(r#"print ${1:expression}"#),
        "="        => Some(r#"= ${1:expression}"#),
        "-"        => Some(r#"- ${1:expression}"#),
        "type"     => Some(r#"type ${1:speed}>>\n$2\n<</type"#),
        "nobr"     => Some(r#"nobr>>\n$2\n<</nobr"#),
        "silently" => Some(r#"silently>>\n$2\n<</silently"#),

        // ── Control flow ──────────────────────────────────────────────────
        "if"      => Some(r#"if ${1:condition}>>\n$2\n<</if"#),
        "elseif"  => Some(r#"elseif ${1:condition}"#),
        "for"     => Some(r#"for ${1:_i}, ${2:\$array}>>\n$3\n<</for"#),
        "switch"  => Some(r#"switch ${1:\$var}>>\n<<case ${2:value}>>\n$3\n<</switch"#),

        // ── Links / interaction ───────────────────────────────────────────
        "link"        => Some(r#"link "${1:label}" "${2:passage}">>\n$3\n<</link"#),
        "button"      => Some(r#"button "${1:label}" "${2:passage}">>\n$3\n<</button"#),
        "linkappend"  => Some(r#"linkappend "${1:label}">>\n$2\n<</linkappend"#),
        "linkprepend" => Some(r#"linkprepend "${1:label}">>\n$2\n<</linkprepend"#),
        "linkreplace" => Some(r#"linkreplace "${1:label}">>\n$2\n<</linkreplace"#),

        // ── Navigation ────────────────────────────────────────────────────
        "goto"    => Some(r#"goto "${1:passage}""#),
        "include" => Some(r#"include "${1:passage}""#),
        "back"    => Some(r#"back"#),
        "return"  => Some(r#"return"#),

        // ── DOM ───────────────────────────────────────────────────────────
        "append"      => Some(r#"append "${1:#selector}">>\n$2\n<</append"#),
        "prepend"     => Some(r#"prepend "${1:#selector}">>\n$2\n<</prepend"#),
        "replace"     => Some(r#"replace "${1:#selector}">>\n$2\n<</replace"#),
        "remove"      => Some(r#"remove "${1:#selector}""#),
        "addclass"    => Some(r#"addclass "${1:#selector}" "${2:class}""#),
        "removeclass" => Some(r#"removeclass "${1:#selector}" "${2:class}""#),
        "toggleclass" => Some(r#"toggleclass "${1:#selector}" "${2:class}""#),

        // ── Widgets / scripting ───────────────────────────────────────────
        "widget" => Some(r#"widget "${1:name}">>\n$2\n<</widget"#),
        "script" => Some(r#"script>>\n$1\n<</script"#),
        "done"   => Some(r#"done>>\n$1\n<</done"#),

        // ── Timing ────────────────────────────────────────────────────────
        "timed"  => Some(r#"timed ${1:2s}>>\n$2\n<</timed"#),
        "repeat" => Some(r#"repeat ${1:2s}>>\n$2\n<</repeat"#),

        // ── Forms ─────────────────────────────────────────────────────────
        "checkbox"    => Some(r#"checkbox "${1:label}" ${2:\$var} "${3:checked}" "${4:unchecked}""#),
        "radiobutton" => Some(r#"radiobutton "${1:label}" ${2:\$var} "${3:value}""#),
        "textbox"     => Some(r#"textbox ${1:\$var} "${2:placeholder}""#),
        "textarea"    => Some(r#"textarea ${1:\$var} "${2:placeholder}""#),
        "numberbox"   => Some(r#"numberbox ${1:\$var} ${2:0}"#),

        _ => None,
    }
}

/// Build a snippet for a macro, using the per-macro override or a generic fallback.
pub fn build_macro_snippet(name: &str, has_body: bool) -> String {
    if let Some(custom) = macro_snippet(name) {
        return custom.to_string();
    }

    // Generic fallback
    let is_block = has_body || block_macro_names().contains(name);
    if is_block {
        format!("{name} $1>>\n$2\n<</{name}")
    } else {
        format!("{name} $1")
    }
}
