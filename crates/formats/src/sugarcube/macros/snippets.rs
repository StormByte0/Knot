//! Per-macro VS Code snippet definitions and snippet builder.
//!
//! Provides completion snippet templates for SugarCube macros, including
//! per-macro overrides and a generic fallback for unknown macros.
//!
//! ## Snippet format
//!
//! The snippet text is the body that appears **after** the `<<` the user
//! already typed. Each snippet is **self-contained**: it includes the `>>`
//! closing delimiter (and for block macros, the `>>` on the closing tag too).
//! This ensures correct output whether or not VS Code auto-close is active.
//!
//! The `compute_macro_text_edit()` function handles consuming any auto-close
//! `>>` that VS Code may have inserted after the cursor, so there is no
//! duplication.
//!
//!   - label:       `<<name>>`   (what's displayed in the completion list)
//!   - filterText:  `name`       (what the editor matches against)
//!   - insertText:  snippet      (replaces the word the editor detected)
//!
//! Tabstop conventions:
//! - `$1`, `$2` … — positional tab stops
//! - `${1:placeholder}` — tab stop with placeholder text
//! - `\$` — escaped dollar sign (literal `$` in the snippet output)
//! - `$0` — final cursor position
//!
//! ## Newline handling
//!
//! Snippet definitions use raw strings (`r#"..."#`) to preserve the `\$`
//! escape sequences that VS Code snippet syntax requires for literal `$`
//! characters (e.g., `${1:\$var}` → tab stop 1 with placeholder `$var`).
//!
//! Since raw strings don't interpret `\n` as newlines, the `build_macro_snippet()`
//! function replaces literal `\n` sequences with actual newline characters
//! before returning the snippet. This keeps the definition readable while
//! producing correct multi-line snippets.

use crate::types::BodyRequirement;

/// A per-macro snippet body for completion.
///
/// Defined in raw strings to preserve `\$` escapes. The `build_macro_snippet()`
/// function converts `\n` to actual newlines before returning the snippet.
pub fn macro_snippet(name: &str) -> Option<&'static str> {
    match name {
        // ── Variables ─────────────────────────────────────────────────────
        "set"     => Some(r#"set ${1:\$var} to ${2:value}>>"#),
        "unset"   => Some(r#"unset ${1:\$var}>>"#),
        "run"     => Some(r#"run ${1:expression}>>"#),
        "capture" => Some(r#"capture ${1:\$var}>>\n$2\n<</capture>>"#),

        // ── Output ────────────────────────────────────────────────────────
        "print"    => Some(r#"print ${1:expression}>>"#),
        "="        => Some(r#"= ${1:expression}>>"#),
        "-"        => Some(r#"- ${1:expression}>>"#),
        "type"     => Some(r#"type ${1:speed}>>\n$2\n<</type>>"#),
        "nobr"     => Some(r#"nobr>>\n$2\n<</nobr>>"#),
        "silent"   => Some(r#"silent>>\n$1\n<</silent>>"#),
        "silently" => Some(r#"silently>>\n$1\n<</silently>>"#),
        "do"       => Some(r#"do>>\n$1\n<</do>>"#),
        "redo"     => Some(r#"redo>>"#),

        // ── Control flow ──────────────────────────────────────────────────
        "if"      => Some(r#"if ${1:condition}>>\n$2\n<</if>>"#),
        "elseif"  => Some(r#"elseif ${1:condition}>>"#),
        "else"    => Some(r#"else>>"#),
        "for"     => Some(r#"for ${1:_i} range ${2:\$array}>>\n$3\n<</for>>"#),
        "switch"  => Some(r#"switch ${1:\$var}>>\n<<case ${2:value}>>\n$3\n<</switch>>"#),
        "case"    => Some(r#"case ${1:value}>>"#),
        "default" => Some(r#"default>>"#),
        "break"   => Some(r#"break>>"#),
        "continue"=> Some(r#"continue>>"#),
        "stop"    => Some(r#"stop>>"#),

        // ── Links / interaction ───────────────────────────────────────────
        "link"        => Some(r#"link "${1:label}" "${2:passage}">>\n$3\n<</link>>"#),
        "button"      => Some(r#"button "${1:label}" "${2:passage}">>\n$3\n<</button>>"#),
        "linkappend"  => Some(r#"linkappend "${1:label}">>\n$2\n<</linkappend>>"#),
        "linkprepend" => Some(r#"linkprepend "${1:label}">>\n$2\n<</linkprepend>>"#),
        "linkreplace" => Some(r#"linkreplace "${1:label}">>\n$2\n<</linkreplace>>"#),

        // ── Navigation ────────────────────────────────────────────────────
        "goto"    => Some(r#"goto "${1:passage}">>"#),
        "include" => Some(r#"include "${1:passage}" "${2:element}">>"#),
        "back"    => Some(r#"back>>"#),
        "return"  => Some(r#"return>>"#),

        // ── DOM ───────────────────────────────────────────────────────────
        "append"      => Some(r#"append "${1:#selector}">>\n$2\n<</append>>"#),
        "prepend"     => Some(r#"prepend "${1:#selector}">>\n$2\n<</prepend>>"#),
        "replace"     => Some(r#"replace "${1:#selector}">>\n$2\n<</replace>>"#),
        "remove"      => Some(r#"remove "${1:#selector}">>"#),
        "addclass"    => Some(r#"addclass "${1:#selector}" "${2:class}">>"#),
        "removeclass" => Some(r#"removeclass "${1:#selector}" "${2:class}">>"#),
        "toggleclass" => Some(r#"toggleclass "${1:#selector}" "${2:class}">>"#),
        "copy"        => Some(r#"copy "${1:#selector}">>"#),
        "css"         => Some(r#"css>>\n$1\n<</css>>"#),

        // ── Widgets / scripting ───────────────────────────────────────────
        "widget" => Some(r#"widget "${1:name}" ${2:container}>>\n$3\n<</widget>>"#),
        "script" => Some(r#"script ${1:language}>>\n$2\n<</script>>"#),
        "code"   => Some(r#"code>>\n$1\n<</code>>"#),
        "done"   => Some(r#"done>>\n$1\n<</done>>"#),

        // ── Deprecated (still need snippets for completeness) ────────────
        "actions" => Some(r#"actions "${1:passage}" "${2:passage}">>"#),
        "display" => Some(r#"display "${1:passage}">>"#),
        "remember"=> Some(r#"remember "${1:passage}">>"#),
        "forget"  => Some(r#"forget "${1:passage}">>"#),
        "click"   => Some(r#"click "${1:label}" "${2:passage}">>\n$3\n<</click>>"#),
        "choice"  => Some(r#"choice "${1:passage}" "${2:linkText}">>"#),
        "setplaylist" => Some(r#"setplaylist "${1:list_id}">>"#),
        "stopallaudio" => Some(r#"stopallaudio>>"#),

        // ── Timing ────────────────────────────────────────────────────────
        "timed"  => Some(r#"timed ${1:2s}>>\n$2\n<</timed>>"#),
        "repeat" => Some(r#"repeat ${1:2s}>>\n$2\n<</repeat>>"#),
        "next"   => Some(r#"next ${1:2s}>>"#),

        // ── Forms ─────────────────────────────────────────────────────────
        // SugarCube signatures (per plan.md §3.12):
        //   <<checkbox receiverName uncheckedValue checkedValue [autocheck|checked]>>
        //   <<radiobutton receiverName checkedValue [autocheck|checked]>>
        // NOTE: checkbox values were previously swapped (checked/unchecked).
        //       Correct order is unchecked THEN checked.
        "checkbox"    => Some(r#"checkbox "${1:\$var}" "${2:unchecked}" "${3:checked}">>"#),
        "radiobutton" => Some(r#"radiobutton "${1:\$var}" "${2:value}">>"#),
        "textbox"     => Some(r#"textbox "${1:\$var}" "${2:default}" "${3:passage}" ${4:autofocus}>>"#),
        "textarea"    => Some(r#"textarea "${1:\$var}" "${2:default}" ${3:autofocus}>>"#),
        "numberbox"   => Some(r#"numberbox "${1:\$var}" ${2:0} "${3:passage}" ${4:autofocus}>>"#),
        "listbox"     => Some(r#"listbox "${1:\$var}">>\n<<option "${2:label}" "${3:value}" ${4:selected}>\n<</listbox>>"#),
        "cycle"       => Some(r#"cycle "${1:\$var}">>\n<<option "${2:label}" "${3:value}" ${4:selected}>\n<</cycle>>"#),
        "option"      => Some(r#"option "${1:label}" "${2:value}" ${3:selected}>>"#),
        "optionsfrom" => Some(r#"optionsfrom ${1:\$collection}>>"#),

        // ── Audio ─────────────────────────────────────────────────────────
        "audio"             => Some(r#"audio "${1:track}" ${2:play}>>"#),
        "cacheaudio"        => Some(r#"cacheaudio "${1:track}" "${2:source}">>"#),
        "masteraudio"       => Some(r#"masteraudio ${1:stop}>>"#),
        "playlist"          => Some(r#"playlist "${1:list}" ${2:play}>>"#),
        "createplaylist"    => Some(r#"createplaylist "${1:list}">>\n<<track "${2:track}">>\n<</createplaylist>>"#),
        "createaudiogroup"  => Some(r#"createaudiogroup "${1::group}">>\n<<track "${2:track}">>\n<</createaudiogroup>>"#),
        "removeaudiogroup"  => Some(r#"removeaudiogroup "${1::group}">>"#),
        "removeplaylist"    => Some(r#"removeplaylist "${1:list}">>"#),
        "waitforaudio"      => Some(r#"waitforaudio>>"#),
        "track"             => Some(r#"track "${1:track}">>"#),

        _ => None,
    }
}

/// Build a snippet for a macro, using the per-macro override or a generic fallback.
///
/// Converts literal `\n` in raw-string definitions to actual newline characters,
/// matching the VS Code snippet format expected by LSP clients.
///
/// All snippets are **self-contained** — they include `>>` so the output is
/// valid SugarCube syntax whether or not VS Code auto-close is active.
pub fn build_macro_snippet(name: &str, body: BodyRequirement) -> String {
    if let Some(custom) = macro_snippet(name) {
        return convert_snippet_newlines(custom);
    }

    // Generic fallback: macros with Optional or Required body get a block snippet
    let is_block = body != BodyRequirement::Never;
    if is_block {
        convert_snippet_newlines(&format!("{name} $1>>\\n$2\\n<</{name}>>"))
    } else {
        format!("{name} $1>>")
    }
}

/// Convert literal `\n` sequences in a raw-string snippet to actual newline characters.
///
/// Snippet definitions use raw strings (`r#"..."#`) to preserve `\$` escape
/// sequences required by VS Code snippet syntax. However, raw strings also
/// prevent `\n` from being interpreted as newlines. This function converts
/// the two-character sequence `\n` to actual newlines so multi-line snippets
/// render correctly in the editor.
///
/// This is also used by `completion_forms.rs` to convert multi-form snippets.
pub fn convert_snippet_newlines(snippet: &str) -> String {
    snippet.replace("\\n", "\n")
}
