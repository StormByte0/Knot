//! Multi-form completion definitions for SugarCube macros.
//!
//! Many SugarCube macros are polymorphic — the same macro name can be invoked
//! with different argument counts and with/without a closing tag. Each form
//! produces a separate completion item so the user can choose the right variant.
//!
//! ## Snippet conventions
//!
//! Snippets use the same raw-string + `convert_snippet_newlines()` system as
//! `snippets.rs`:
//! - `\$` → literal `$` (e.g., `${1:\$var}` tabstop with placeholder `$var`)
//! - `\n` → converted to actual newline by `convert_snippet_newlines()`
//! - `$1`, `$2` → positional tab stops
//! - `${1:placeholder}` → tab stop with placeholder text
//!
//! The snippet body is the text that appears **after** the `<<` the user
//! already typed. For block macros, the snippet includes `>>`, the body
//! tabstop, and the closing tag.

use crate::types::MacroCompletionForm;

/// Return the multi-form completion definitions for a builtin macro.
///
/// Returns `None` for macros that have only a single form (their single-form
/// snippet comes from `snippets::macro_snippet()`). Returns `Some(&[...])`
/// for polymorphic macros that need multiple completion items.
pub fn macro_completion_forms(name: &str) -> Option<&'static [MacroCompletionForm]> {
    match name {
        // ── Links / interaction (polymorphic: inline vs block, 1-arg vs 2-arg vs 3-arg) ──
        "link" => Some(&LINK_FORMS),
        "button" => Some(&BUTTON_FORMS),
        "click" => Some(&CLICK_FORMS), // deprecated but still offer forms

        // ── Link modifiers (inline vs block) ──
        "linkappend" => Some(&LINKAPPEND_FORMS),
        "linkprepend" => Some(&LINKPREPEND_FORMS),
        "linkreplace" => Some(&LINKREPLACE_FORMS),

        // ── Control flow ──
        "for" => Some(&FOR_FORMS),
        "if" => Some(&IF_FORMS),
        "switch" => Some(&SWITCH_FORMS),

        // ── Variables ──
        "set" => Some(&SET_FORMS),
        "capture" => Some(&CAPTURE_FORMS),

        // ── DOM ──
        "append" => Some(&APPEND_FORMS),
        "prepend" => Some(&PREPEND_FORMS),
        "replace" => Some(&REPLACE_FORMS),

        // ── Navigation ──
        "goto" => Some(&GOTO_FORMS),
        "include" => Some(&INCLUDE_FORMS),

        // ── Forms ──
        "checkbox" => Some(&CHECKBOX_FORMS),

        // ── Timing ──
        "timed" => Some(&TIMED_FORMS),
        "repeat" => Some(&REPEAT_FORMS),

        // ── Widgets / scripting ──
        "widget" => Some(&WIDGET_FORMS),

        _ => None,
    }
}

// ===========================================================================
// Per-macro completion form definitions
// ===========================================================================

// ── <<link>> ─────────────────────────────────────────────────────────────
//
// SugarCube `<<link>>` has 5 distinct invocation patterns:
//   1. <<link "label">>                  — click handler, no navigation
//   2. <<link "label">>…<</link>>        — click handler with body
//   3. <<link "label" "passage">>        — navigate to passage
//   4. <<link "label" "passage">>…<</link>> — navigate + execute body
//   5. <<link "label" "passage" "tooltip">> — navigate with tooltip

static LINK_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<link "label" "passage">>"#,
        detail: "Navigate to passage on click",
        snippet: r#"link "${1:label}" "${2:passage}">>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<link "label" "passage">>…<</link>>"#,
        detail: "Navigate to passage + execute body on click",
        snippet: r#"link "${1:label}" "${2:passage}">>\n$3\n<</link"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<link "label">>"#,
        detail: "Click handler — no navigation (use body for actions)",
        snippet: r#"link "${1:label}">>"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: r#"<<link "label">>…<</link>>"#,
        detail: "Click handler with body — execute content on click",
        snippet: r#"link "${1:label}">>\n$2\n<</link"#,
        sort_priority: 3,
    },
    MacroCompletionForm {
        label: r#"<<link "label" "passage" "tooltip">>"#,
        detail: "Navigate to passage on click with tooltip",
        snippet: r#"link "${1:label}" "${2:passage}" "${3:tooltip}">>"#,
        sort_priority: 4,
    },
];

static BUTTON_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<button "label" "passage">>"#,
        detail: "Button — navigate to passage on click",
        snippet: r#"button "${1:label}" "${2:passage}">>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<button "label" "passage">>…<</button>>"#,
        detail: "Button — navigate + execute body on click",
        snippet: r#"button "${1:label}" "${2:passage}">>\n$3\n<</button"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<button "label">>"#,
        detail: "Button — click handler, no navigation",
        snippet: r#"button "${1:label}">>"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: r#"<<button "label">>…<</button>>"#,
        detail: "Button — click handler with body",
        snippet: r#"button "${1:label}">>\n$2\n<</button"#,
        sort_priority: 3,
    },
    MacroCompletionForm {
        label: r#"<<button "label" "passage" "tooltip">>"#,
        detail: "Button — navigate to passage with tooltip",
        snippet: r#"button "${1:label}" "${2:passage}" "${3:tooltip}">>"#,
        sort_priority: 4,
    },
];

static CLICK_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<click "label" "passage">>"#,
        detail: "[Deprecated] Use <<link>> — navigate to passage on click",
        snippet: r#"click "${1:label}" "${2:passage}">>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<click "label" "passage">>…<</click>>"#,
        detail: "[Deprecated] Use <<link>> — navigate + execute body",
        snippet: r#"click "${1:label}" "${2:passage}">>\n$3\n<</click"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<click "label">>"#,
        detail: "[Deprecated] Use <<link>> — click handler, no navigation",
        snippet: r#"click "${1:label}">>"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: r#"<<click "label">>…<</click>>"#,
        detail: "[Deprecated] Use <<link>> — click handler with body",
        snippet: r#"click "${1:label}">>\n$2\n<</click"#,
        sort_priority: 3,
    },
];

// ── Link modifiers (always block, but 1-arg vs 2-arg) ────────────────

static LINKAPPEND_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<linkappend "label">>…<</linkappend>>"#,
        detail: "Append content after link text on click",
        snippet: r#"linkappend "${1:label}">>\n$2\n<</linkappend"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<linkappend "label" "transition">>…<</linkappend>>"#,
        detail: "Append content with transition on click",
        snippet: r#"linkappend "${1:label}" "${2:transition}">>\n$3\n<</linkappend"#,
        sort_priority: 1,
    },
];

static LINKPREPEND_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<linkprepend "label">>…<</linkprepend>>"#,
        detail: "Prepend content before link text on click",
        snippet: r#"linkprepend "${1:label}">>\n$2\n<</linkprepend"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<linkprepend "label" "transition">>…<</linkprepend>>"#,
        detail: "Prepend content with transition on click",
        snippet: r#"linkprepend "${1:label}" "${2:transition}">>\n$3\n<</linkprepend"#,
        sort_priority: 1,
    },
];

static LINKREPLACE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<linkreplace "label">>…<</linkreplace>>"#,
        detail: "Replace link text with content on click",
        snippet: r#"linkreplace "${1:label}">>\n$2\n<</linkreplace"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<linkreplace "label" "transition">>…<</linkreplace>>"#,
        detail: "Replace link text with transition on click",
        snippet: r#"linkreplace "${1:label}" "${2:transition}">>\n$3\n<</linkreplace"#,
        sort_priority: 1,
    },
];

// ── Control flow ──────────────────────────────────────────────────────

static FOR_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<for _i, $array>>…<</for>>",
        detail: "Iterate over array elements",
        snippet: r#"for ${1:_i}, ${2:\$array}>>\n$3\n<</for"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<for _i, _min, _max>>…<</for>>",
        detail: "Iterate over numeric range",
        snippet: r#"for ${1:_i}, ${2:0}, ${3:10}>>\n$4\n<</for"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: "<<for _i, _min, _max, _step>>…<</for>>",
        detail: "Iterate over numeric range with step",
        snippet: r#"for ${1:_i}, ${2:0}, ${3:10}, ${4:1}>>\n$5\n<</for"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: "<<for $condition>>…<</for>>",
        detail: "Conditional loop (while-style)",
        snippet: r#"for ${1:condition}>>\n$2\n<</for"#,
        sort_priority: 3,
    },
];

static IF_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<if condition>>…<</if>>",
        detail: "Conditional block with if/else",
        snippet: r#"if ${1:condition}>>\n$2\n<</if"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<if>>…<<else>>…<</if>>",
        detail: "Conditional block with else branch",
        snippet: r#"if ${1:condition}>>\n$2\n<<else>>\n$3\n<</if"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: "<<if>>…<<elseif>>…<</if>>",
        detail: "Conditional block with else-if branches",
        snippet: r#"if ${1:condition}>>\n$2\n<<elseif ${3:condition}>>\n$4\n<</if"#,
        sort_priority: 2,
    },
];

static SWITCH_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<switch $var>>…<<case>>…<</switch>>",
        detail: "Switch statement with case branches",
        snippet: r#"switch ${1:\$var}>>\n<<case ${2:value}>>\n$3\n<</switch"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<switch $var>>…<<case>>…<<default>>…<</switch>>",
        detail: "Switch statement with case and default",
        snippet: r#"switch ${1:\$var}>>\n<<case ${2:value}>>\n$3\n<<default>>\n$4\n<</switch"#,
        sort_priority: 1,
    },
];

// ── Variables ─────────────────────────────────────────────────────────

static SET_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<set $var to value>>",
        detail: "Set variable to value",
        snippet: r#"set ${1:\$var} to ${2:value}"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<set $var++>>",
        detail: "Increment variable by 1",
        snippet: r#"set ${1:\$var}++"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: "<<set $var += value>>",
        detail: "Add value to variable",
        snippet: r#"set ${1:\$var} += ${2:value}"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: "<<set $var-->>",
        detail: "Decrement variable by 1",
        snippet: r#"set ${1:\$var}--"#,
        sort_priority: 3,
    },
    MacroCompletionForm {
        label: "<<set $var -= value>>",
        detail: "Subtract value from variable",
        snippet: r#"set ${1:\$var} -= ${2:value}"#,
        sort_priority: 4,
    },
];

static CAPTURE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<capture $var>>…<</capture>>",
        detail: "Capture link interaction into variable",
        snippet: r#"capture ${1:\$var}>>\n$2\n<</capture"#,
        sort_priority: 0,
    },
];

// ── DOM ───────────────────────────────────────────────────────────────

static APPEND_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<append #selector>>…<</append>>",
        detail: "Append content to element(s) matching selector",
        snippet: r#"append "${1:#selector}">>\n$2\n<</append"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<append #selector>>…<</append>> (transition)",
        detail: "Append content with transition",
        snippet: r#"append "${1:#selector}" "${2:transition}">>\n$3\n<</append"#,
        sort_priority: 1,
    },
];

static PREPEND_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<prepend #selector>>…<</prepend>>",
        detail: "Prepend content to element(s) matching selector",
        snippet: r#"prepend "${1:#selector}">>\n$2\n<</prepend"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<prepend #selector>>…<</prepend>> (transition)",
        detail: "Prepend content with transition",
        snippet: r#"prepend "${1:#selector}" "${2:transition}">>\n$3\n<</prepend"#,
        sort_priority: 1,
    },
];

static REPLACE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<replace #selector>>…<</replace>>",
        detail: "Replace content of element(s) matching selector",
        snippet: r#"replace "${1:#selector}">>\n$2\n<</replace"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<replace #selector>>…<</replace>> (transition)",
        detail: "Replace content with transition",
        snippet: r#"replace "${1:#selector}" "${2:transition}">>\n$3\n<</replace"#,
        sort_priority: 1,
    },
];

// ── Navigation ────────────────────────────────────────────────────────

static GOTO_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<goto "passage">>"#,
        detail: "Navigate to named passage",
        snippet: r#"goto "${1:passage}""#,
        sort_priority: 0,
    },
];

static INCLUDE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<include "passage">>"#,
        detail: "Include content of named passage",
        snippet: r#"include "${1:passage}""#,
        sort_priority: 0,
    },
];

// ── Forms ─────────────────────────────────────────────────────────────

static CHECKBOX_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<checkbox "label" $var "checked" "unchecked">>"#,
        detail: "Checkbox bound to variable (checked/unchecked values)",
        snippet: r#"checkbox "${1:label}" ${2:\$var} "${3:checked}" "${4:unchecked}""#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<checkbox "label" $var "value">>"#,
        detail: "Checkbox with single value (unchecked = undefined)",
        snippet: r#"checkbox "${1:label}" ${2:\$var} "${3:value}""#,
        sort_priority: 1,
    },
];

// ── Timing ────────────────────────────────────────────────────────────

static TIMED_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<timed 2s>>…<</timed>>",
        detail: "Execute content after delay",
        snippet: r#"timed ${1:2s}>>\n$2\n<</timed"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<timed 2s>>…<<next>>…<</timed>>",
        detail: "Timed with chained next delays",
        snippet: r#"timed ${1:2s}>>\n$2\n<<next ${3:2s}>>\n$4\n<</timed"#,
        sort_priority: 1,
    },
];

static REPEAT_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<repeat 2s>>…<</repeat>>",
        detail: "Repeat content at interval",
        snippet: r#"repeat ${1:2s}>>\n$2\n<</repeat"#,
        sort_priority: 0,
    },
];

// ── Widgets ───────────────────────────────────────────────────────────

static WIDGET_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<widget "name">>…<</widget>>"#,
        detail: "Define a reusable widget macro",
        snippet: r#"widget "${1:name}">>\n$2\n<</widget"#,
        sort_priority: 0,
    },
];
