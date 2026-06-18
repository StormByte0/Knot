//! Multi-form completion definitions for SugarCube macros.
//!
//! Many SugarCube macros can be invoked with different argument counts.
//! Each form produces a separate completion item so the user can choose
//! the right variant. Container macros always include closing tags.
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
//! already typed. Each snippet is **self-contained**: it includes `>>`
//! (and for block macros, `>>` on the closing tag) so the output is valid
//! SugarCube syntax whether or not VS Code auto-close is active.

use crate::types::MacroCompletionForm;

/// Return the multi-form completion definitions for a builtin macro.
///
/// Returns `None` for macros that have only a single form (their single-form
/// snippet comes from `snippets::macro_snippet()`). Returns `Some(&[...])`
/// for macros that need multiple completion items.
pub fn macro_completion_forms(name: &str) -> Option<&'static [MacroCompletionForm]> {
    match name {
        // ── Links / interaction (1-arg vs 2-arg vs 3-arg) ──
        "link" => Some(&LINK_FORMS),
        "button" => Some(&BUTTON_FORMS),
        "click" => Some(&CLICK_FORMS), // deprecated but still offer forms

        // ── Link modifiers (always block: 1-arg vs 2-arg) ──
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

        // ── Output (block) ──
        "do" => Some(&DO_FORMS),

        // ── DOM ──
        "append" => Some(&APPEND_FORMS),
        "prepend" => Some(&PREPEND_FORMS),
        "replace" => Some(&REPLACE_FORMS),

        // ── Navigation ──
        "goto" => Some(&GOTO_FORMS),
        "include" => Some(&INCLUDE_FORMS),
        "back" => Some(&BACK_FORMS),
        "return" => Some(&RETURN_FORMS),

        // ── Form inputs ──
        "checkbox" => Some(&CHECKBOX_FORMS),
        "textbox" => Some(&TEXTBOX_FORMS),
        "textarea" => Some(&TEXTAREA_FORMS),
        "radiobutton" => Some(&RADIOBUTTON_FORMS),
        "numberbox" => Some(&NUMBERBOX_FORMS),
        "listbox" => Some(&LISTBOX_FORMS),
        "cycle" => Some(&CYCLE_FORMS),

        // ── Timing ──
        "timed" => Some(&TIMED_FORMS),
        "repeat" => Some(&REPEAT_FORMS),

        // ── Audio ──
        "audio" => Some(&AUDIO_FORMS),
        "cacheaudio" => Some(&CACHEAUDIO_FORMS),
        "masteraudio" => Some(&MASTERAUDIO_FORMS),
        "playlist" => Some(&PLAYLIST_FORMS),
        "createplaylist" => Some(&CREATEPLAYLIST_FORMS),
        "createaudiogroup" => Some(&CREATEAUDIOGROUP_FORMS),

        // ── Output (inline) ──
        "print" => Some(&PRINT_FORMS),
        "=" => Some(&PRINT_ALIAS_FORMS),
        "-" => Some(&PRINT_TRIM_FORMS),
        "type" => Some(&TYPE_FORMS),
        "redo" => Some(&REDO_FORMS),
        "silent" => Some(&SILENT_FORMS),
        "css" => Some(&CSS_FORMS),

        // ── Sub-macros ──
        "option" => Some(&OPTION_FORMS),
        "optionsfrom" => Some(&OPTIONSFROM_FORMS),
        "track" => Some(&TRACK_FORMS),
        "next" => Some(&NEXT_FORMS),

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
// SugarCube `<<link>>` ALWAYS requires a closing tag. There is no inline form.
//   1. <<link "label" "passage">>…<</link>> — navigate to passage + body
//   2. <<link "label">>…<</link>>             — click handler with body
//   3. <<link "label" "passage" "tooltip">>…<</link>> — navigate with tooltip

static LINK_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<link "label" "passage">>…<</link>>"#,
        detail: "Navigate to passage on click",
        snippet: r#"link "${1:label}" "${2:passage}">>\n$3\n<</link>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<link "label">>…<</link>>"#,
        detail: "Click handler with body — execute content on click",
        snippet: r#"link "${1:label}">>\n$2\n<</link>>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<link "label" "passage" "tooltip">>…<</link>>"#,
        detail: "Navigate to passage on click with tooltip",
        snippet: r#"link "${1:label}" "${2:passage}" "${3:tooltip}">>\n$4\n<</link>>"#,
        sort_priority: 2,
    },
];

// ── <<button>> ────────────────────────────────────────────────────────────
//
// SugarCube `<<button>>` ALWAYS requires a closing tag. There is no inline form.
//   1. <<button "label" "passage">>…<</button>> — navigate to passage + body
//   2. <<button "label">>…<</button>>             — click handler with body
//   3. <<button "label" "passage" "tooltip">>…<</button>> — navigate with tooltip

static BUTTON_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<button "label" "passage">>…<</button>>"#,
        detail: "Button — navigate to passage on click",
        snippet: r#"button "${1:label}" "${2:passage}">>\n$3\n<</button>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<button "label">>…<</button>>"#,
        detail: "Button — click handler with body",
        snippet: r#"button "${1:label}">>\n$2\n<</button>>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<button "label" "passage" "tooltip">>…<</button>>"#,
        detail: "Button — navigate to passage with tooltip",
        snippet: r#"button "${1:label}" "${2:passage}" "${3:tooltip}">>\n$4\n<</button>>"#,
        sort_priority: 2,
    },
];

// ── <<click>> (deprecated) ─────────────────────────────────────────────
//
// SugarCube `<<click>>` ALWAYS requires a closing tag. Deprecated — use <<link>>.

static CLICK_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<click "label" "passage">>…<</click>>"#,
        detail: "[Deprecated] Use <<link>> — navigate to passage on click",
        snippet: r#"click "${1:label}" "${2:passage}">>\n$3\n<</click>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<click "label">>…<</click>>"#,
        detail: "[Deprecated] Use <<link>> — click handler with body",
        snippet: r#"click "${1:label}">>\n$2\n<</click>>"#,
        sort_priority: 1,
    },
];

// ── Link modifiers (always block, but 1-arg vs 2-arg) ────────────────

static LINKAPPEND_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<linkappend "label">>…<</linkappend>>"#,
        detail: "Append content after link text on click",
        snippet: r#"linkappend "${1:label}">>\n$2\n<</linkappend>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<linkappend "label" "transition">>…<</linkappend>>"#,
        detail: "Append content with transition on click",
        snippet: r#"linkappend "${1:label}" "${2:transition}">>\n$3\n<</linkappend>>"#,
        sort_priority: 1,
    },
];

static LINKPREPEND_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<linkprepend "label">>…<</linkprepend>>"#,
        detail: "Prepend content before link text on click",
        snippet: r#"linkprepend "${1:label}">>\n$2\n<</linkprepend>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<linkprepend "label" "transition">>…<</linkprepend>>"#,
        detail: "Prepend content with transition on click",
        snippet: r#"linkprepend "${1:label}" "${2:transition}">>\n$3\n<</linkprepend>>"#,
        sort_priority: 1,
    },
];

static LINKREPLACE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<linkreplace "label">>…<</linkreplace>>"#,
        detail: "Replace link text with content on click",
        snippet: r#"linkreplace "${1:label}">>\n$2\n<</linkreplace>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<linkreplace "label" "transition">>…<</linkreplace>>"#,
        detail: "Replace link text with transition on click",
        snippet: r#"linkreplace "${1:label}" "${2:transition}">>\n$3\n<</linkreplace>>"#,
        sort_priority: 1,
    },
];

// ── Control flow ──────────────────────────────────────────────────────

static FOR_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<for _i range $array>>…<</for>>",
        detail: "Iterate over array/object elements",
        snippet: r#"for ${1:_i} range ${2:\$array}>>\n$3\n<</for>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<for _key, _val range $obj>>…<</for>>",
        detail: "Iterate with key and value over object",
        snippet: r#"for ${1:_key}, ${2:_val} range ${3:\$obj}>>\n$4\n<</for>>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: "<<for _i range 0..10>>…<</for>>",
        detail: "Iterate over integer range",
        snippet: r#"for ${1:_i} range ${2:0}..${3:10}>>\n$4\n<</for>>"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: "<<for init; cond; post>>…<</for>>",
        detail: "C-style 3-part loop (init; condition; post)",
        snippet: r#"for ${1:_i = 0}; ${2:_i < 10}; ${3:_i++}>>\n$4\n<</for>>"#,
        sort_priority: 3,
    },
    MacroCompletionForm {
        label: "<<for $condition>>…<</for>>",
        detail: "Conditional loop (while-style)",
        snippet: r#"for ${1:condition}>>\n$2\n<</for>>"#,
        sort_priority: 4,
    },
];

static IF_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<if condition>>…<</if>>",
        detail: "Conditional block with if/else",
        snippet: r#"if ${1:condition}>>\n$2\n<</if>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<if>>…<<else>>…<</if>>",
        detail: "Conditional block with else branch",
        snippet: r#"if ${1:condition}>>\n$2\n<<else>>\n$3\n<</if>>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: "<<if>>…<<elseif>>…<</if>>",
        detail: "Conditional block with else-if branches",
        snippet: r#"if ${1:condition}>>\n$2\n<<elseif ${3:condition}>>\n$4\n<</if>>"#,
        sort_priority: 2,
    },
];

static SWITCH_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<switch $var>>…<<case>>…<</switch>>",
        detail: "Switch statement with case branches",
        snippet: r#"switch ${1:\$var}>>\n<<case ${2:value}>>\n$3\n<</switch>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<switch $var>>…<<case>>…<<default>>…<</switch>>",
        detail: "Switch statement with case and default",
        snippet: r#"switch ${1:\$var}>>\n<<case ${2:value}>>\n$3\n<<default>>\n$4\n<</switch>>"#,
        sort_priority: 1,
    },
];

// ── Variables ─────────────────────────────────────────────────────────

static SET_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<set $var to value>>",
        detail: "Set variable to value (TwineScript syntax)",
        snippet: r#"set ${1:\$var} to ${2:value}>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<set $var = value>>",
        detail: "Set variable to value (JavaScript syntax)",
        snippet: r#"set ${1:\$var} = ${2:value}>>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: "<<set $var++>>",
        detail: "Increment variable by 1",
        snippet: r#"set ${1:\$var}++>>"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: "<<set $var += value>>",
        detail: "Add value to variable",
        snippet: r#"set ${1:\$var} += ${2:value}>>"#,
        sort_priority: 3,
    },
    MacroCompletionForm {
        label: "<<set $var-->>",
        detail: "Decrement variable by 1",
        snippet: r#"set ${1:\$var}-->>"#,
        sort_priority: 4,
    },
    MacroCompletionForm {
        label: "<<set $var -= value>>",
        detail: "Subtract value from variable",
        snippet: r#"set ${1:\$var} -= ${2:value}>>"#,
        sort_priority: 5,
    },
];

static CAPTURE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<capture $var>>…<</capture>>",
        detail: "Capture link interaction into variable",
        snippet: r#"capture ${1:\$var}>>\n$2\n<</capture>>"#,
        sort_priority: 0,
    },
];

// ── DOM ───────────────────────────────────────────────────────────────

static APPEND_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<append #selector>>…<</append>>",
        detail: "Append content to element(s) matching selector",
        snippet: r#"append "${1:#selector}">>\n$2\n<</append>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<append #selector>>…<</append>> (transition)",
        detail: "Append content with transition",
        snippet: r#"append "${1:#selector}" "${2:transition}">>\n$3\n<</append>>"#,
        sort_priority: 1,
    },
];

static PREPEND_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<prepend #selector>>…<</prepend>>",
        detail: "Prepend content to element(s) matching selector",
        snippet: r#"prepend "${1:#selector}">>\n$2\n<</prepend>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<prepend #selector>>…<</prepend>> (transition)",
        detail: "Prepend content with transition",
        snippet: r#"prepend "${1:#selector}" "${2:transition}">>\n$3\n<</prepend>>"#,
        sort_priority: 1,
    },
];

static REPLACE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<replace #selector>>…<</replace>>",
        detail: "Replace content of element(s) matching selector",
        snippet: r#"replace "${1:#selector}">>\n$2\n<</replace>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<replace #selector>>…<</replace>> (transition)",
        detail: "Replace content with transition",
        snippet: r#"replace "${1:#selector}" "${2:transition}">>\n$3\n<</replace>>"#,
        sort_priority: 1,
    },
];

// ── Navigation ────────────────────────────────────────────────────────

static GOTO_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<goto "passage">>"#,
        detail: "Navigate to named passage",
        snippet: r#"goto "${1:passage}">>"#,
        sort_priority: 0,
    },
];

static INCLUDE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<include "passage">>"#,
        detail: "Include content of named passage",
        snippet: r#"include "${1:passage}">>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<include "passage" "element">>"#,
        detail: "Include passage content into a specific DOM element",
        snippet: r#"include "${1:passage}" "${2:element}">>"#,
        sort_priority: 1,
    },
];

// ── Forms ─────────────────────────────────────────────────────────────

static CHECKBOX_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<checkbox "$var" "checked" "unchecked">>"#,
        detail: "Checkbox bound to variable (checked/unchecked values)",
        snippet: r#"checkbox "${1:\$var}" "${2:checked}" "${3:unchecked}">>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<checkbox "$var" "value">>"#,
        detail: "Checkbox with single value (unchecked = undefined)",
        snippet: r#"checkbox "${1:\$var}" "${2:value}">>"#,
        sort_priority: 1,
    },
];

// ── Timing ────────────────────────────────────────────────────────────

static TIMED_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<timed 2s>>…<</timed>>",
        detail: "Execute content after delay",
        snippet: r#"timed ${1:2s}>>\n$2\n<</timed>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: "<<timed 2s>>…<<next>>…<</timed>>",
        detail: "Timed with chained next delays",
        snippet: r#"timed ${1:2s}>>\n$2\n<<next ${3:2s}>>\n$4\n<</timed>>"#,
        sort_priority: 1,
    },
];

static REPEAT_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<repeat 2s>>…<</repeat>>",
        detail: "Repeat content at interval",
        snippet: r#"repeat ${1:2s}>>\n$2\n<</repeat>>"#,
        sort_priority: 0,
    },
];

// ── Widgets ───────────────────────────────────────────────────────────

static WIDGET_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<widget "name">>…<</widget>>"#,
        detail: "Define a reusable widget macro",
        snippet: r#"widget "${1:name}">>\n$2\n<</widget>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<widget "name" container>>…<</widget>>"#,
        detail: "Define a container widget that wraps content (access via _contents)",
        snippet: r#"widget "${1:name}" container>>\n$2\n<</widget>>"#,
        sort_priority: 1,
    },
];

// ── Do/Redo ──────────────────────────────────────────────────────────

static DO_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<do>>…<</do>>",
        detail: "Create a re-renderable output block (v2.37.0+)",
        snippet: r#"do>>\n$1\n<</do>>"#,
        sort_priority: 0,
    },
];

// ── Navigation: back/return ──────────────────────────────────────────

static BACK_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<back>>",
        detail: "Go back in history with default label",
        snippet: "back>>",
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<back "label">>"#,
        detail: "Go back with custom link label",
        snippet: r#"back "${1:label}">>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<back "label" "passage">>"#,
        detail: "Go back to specific passage with custom label",
        snippet: r#"back "${1:label}" "${2:passage}">>"#,
        sort_priority: 2,
    },
];

static RETURN_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<return>>",
        detail: "Return to a prior passage with default label",
        snippet: "return>>",
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<return "label">>"#,
        detail: "Return with custom link label",
        snippet: r#"return "${1:label}">>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<return "label" "passage">>"#,
        detail: "Return to specific passage with custom label",
        snippet: r#"return "${1:label}" "${2:passage}">>"#,
        sort_priority: 2,
    },
];

// ── Form input macros ────────────────────────────────────────────────

static TEXTBOX_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<textbox "$var" "default">>"#,
        detail: "Text input bound to variable",
        snippet: r#"textbox "${1:\$var}" "${2:default}">>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<textbox "$var" "default" "passage">>"#,
        detail: "Text input that navigates to passage on Enter",
        snippet: r#"textbox "${1:\$var}" "${2:default}" "${3:passage}">>"#,
        sort_priority: 1,
    },
];

static TEXTAREA_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<textarea "$var" "default">>"#,
        detail: "Multi-line text input bound to variable",
        snippet: r#"textarea "${1:\$var}" "${2:default}">>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<textarea "$var" "default" autofocus>>"#,
        detail: "Multi-line text input with autofocus",
        snippet: r#"textarea "${1:\$var}" "${2:default}" autofocus>>"#,
        sort_priority: 1,
    },
];

static RADIOBUTTON_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<radiobutton "$var" "value">>"#,
        detail: "Radio button bound to variable with value",
        snippet: r#"radiobutton "${1:\$var}" "${2:value}">>"#,
        sort_priority: 0,
    },
];

static NUMBERBOX_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<numberbox "$var" 0>>"#,
        detail: "Numeric input bound to variable",
        snippet: r#"numberbox "${1:\$var}" ${2:0}>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<numberbox "$var" 0 "passage">>"#,
        detail: "Numeric input that navigates to passage on Enter",
        snippet: r#"numberbox "${1:\$var}" ${2:0} "${3:passage}">>"#,
        sort_priority: 1,
    },
];

static LISTBOX_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<listbox "$var">>…<</listbox>>"#,
        detail: "Select dropdown bound to variable",
        snippet: r#"listbox "${1:\$var}">>\n<<option "${2:display}" "${3:value}">>\n<</listbox>>"#,
        sort_priority: 0,
    },
];

static CYCLE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<cycle "$var">>…<</cycle>>"#,
        detail: "Cycling selector bound to variable",
        snippet: r#"cycle "${1:\$var}">>\n<<option "${2:display}" "${3:value}">>\n<</cycle>>"#,
        sort_priority: 0,
    },
];

// ── Audio ────────────────────────────────────────────────────────────

static AUDIO_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<audio "track" play>>"#,
        detail: "Play an audio track",
        snippet: r#"audio "${1:track}" play>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<audio "track" stop>>"#,
        detail: "Stop an audio track",
        snippet: r#"audio "${1:track}" stop>>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<audio "track" volume 0.5>>"#,
        detail: "Set volume of a track (0.0-1.0)",
        snippet: r#"audio "${1:track}" volume ${2:0.5}>>"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: r#"<<audio "track" fadein 2s>>"#,
        detail: "Fade in a track over duration",
        snippet: r#"audio "${1:track}" fadein ${2:2s}>>"#,
        sort_priority: 3,
    },
    MacroCompletionForm {
        label: r#"<<audio "track" fadeout 2s>>"#,
        detail: "Fade out a track over duration",
        snippet: r#"audio "${1:track}" fadeout ${2:2s}>>"#,
        sort_priority: 4,
    },
    MacroCompletionForm {
        label: r#"<<audio "track" loop>>"#,
        detail: "Set a track to loop",
        snippet: r#"audio "${1:track}" loop>>"#,
        sort_priority: 5,
    },
    MacroCompletionForm {
        label: r#"<<audio ":all" stop>>"#,
        detail: "Stop all audio tracks",
        snippet: r#"audio ":all" stop>>"#,
        sort_priority: 6,
    },
];

static CACHEAUDIO_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<cacheaudio "track" "source.mp3">>"#,
        detail: "Cache an audio track with source URL(s)",
        snippet: r#"cacheaudio "${1:track}" "${2:source}">>"#,
        sort_priority: 0,
    },
];

static MASTERAUDIO_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<masteraudio stop>>"#,
        detail: "Stop all audio tracks via master channel",
        snippet: r#"masteraudio stop>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<masteraudio volume 0.5>>"#,
        detail: "Set master volume (0.0–1.0)",
        snippet: r#"masteraudio volume ${1:0.5}>>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<masteraudio mute>>"#,
        detail: "Mute all audio via master channel",
        snippet: r#"masteraudio mute>>"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: r#"<<masteraudio unmute>>"#,
        detail: "Unmute all audio via master channel",
        snippet: r#"masteraudio unmute>>"#,
        sort_priority: 3,
    },
];

static PLAYLIST_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<playlist "list" play>>"#,
        detail: "Play a playlist",
        snippet: r#"playlist "${1:list}" play>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<playlist "list" stop>>"#,
        detail: "Stop a playlist",
        snippet: r#"playlist "${1:list}" stop>>"#,
        sort_priority: 1,
    },
];

static CREATEPLAYLIST_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<createplaylist "list">>…<<track>>…<</createplaylist>>"#,
        detail: "Define a playlist with tracks",
        snippet: r#"createplaylist "${1:list}">>\n<<track "${2:track}">>\n<</createplaylist>>"#,
        sort_priority: 0,
    },
];

static CREATEAUDIOGROUP_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<createaudiogroup ":group">>…<<track>>…<</createaudiogroup>>"#,
        detail: "Define an audio group with tracks",
        snippet: r#"createaudiogroup "${1::group}">>\n<<track "${2:track}">>\n<</createaudiogroup>>"#,
        sort_priority: 0,
    },
];

// ── Output macros ───────────────────────────────────────────────────────

static PRINT_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<print expression>>"#,
        detail: "Output the result of an expression",
        snippet: r#"print ${1:expression}>>"#,
        sort_priority: 0,
    },
];

static PRINT_ALIAS_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<= expression>>"#,
        detail: "Shorthand for <<print>> — output expression result",
        snippet: r#"= ${1:expression}>>"#,
        sort_priority: 0,
    },
];

static PRINT_TRIM_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<- expression>>"#,
        detail: "Like <<print>> but with trimmed (leading/trailing whitespace removed) output",
        snippet: r#"- ${1:expression}>>"#,
        sort_priority: 0,
    },
];

static TYPE_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<type 40ms>>…<</type>>"#,
        detail: "Type out content at speed (ms per character)",
        snippet: r#"type ${1:40ms}>>\n$2\n<</type>>"#,
        sort_priority: 0,
    },
    MacroCompletionForm {
        label: r#"<<type 40ms 2s>>…<</type>>"#,
        detail: "Type out content with start delay",
        snippet: r#"type ${1:40ms} ${2:2s}>>\n$3\n<</type>>"#,
        sort_priority: 1,
    },
    MacroCompletionForm {
        label: r#"<<type 40ms keep>>…<</type>>"#,
        detail: "Type out content and keep after completion",
        snippet: r#"type ${1:40ms} keep>>\n$2\n<</type>>"#,
        sort_priority: 2,
    },
    MacroCompletionForm {
        label: r#"<<type 40ms class "fade">>…<</type>>"#,
        detail: "Type out content with CSS class",
        snippet: r#"type ${1:40ms} class "${2:fade}">>\n$3\n<</type>>"#,
        sort_priority: 3,
    },
];

static REDO_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<redo>>",
        detail: "Trigger re-render of the nearest <<do>> block (v2.37.0+)",
        snippet: "redo>>",
        sort_priority: 0,
    },
];

static SILENT_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<silent>>…<</silent>>",
        detail: "Execute content silently (no output rendered)",
        snippet: r#"silent>>\n$1\n<</silent>>"#,
        sort_priority: 0,
    },
];

static CSS_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: "<<css>>…<</css>>",
        detail: "Output CSS stylesheet content",
        snippet: r#"css>>\n$1\n<</css>>"#,
        sort_priority: 0,
    },
];

// ── Audio sub-macros ────────────────────────────────────────────────────

static OPTION_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<option "display" "value">>"#,
        detail: "Add an option to <<listbox>> or <<cycle>>",
        snippet: r#"option "${1:display}" "${2:value}">>"#,
        sort_priority: 0,
    },
];

static OPTIONSFROM_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<optionsfrom $collection>>"#,
        detail: "Generate options from a collection for <<listbox>> or <<cycle>>",
        snippet: r#"optionsfrom ${1:\$collection}>>"#,
        sort_priority: 0,
    },
];

static TRACK_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<track "track">>"#,
        detail: "Add a track to <<createaudiogroup>> or <<createplaylist>>",
        snippet: r#"track "${1:track}">>"#,
        sort_priority: 0,
    },
];

static NEXT_FORMS: &[MacroCompletionForm] = &[
    MacroCompletionForm {
        label: r#"<<next 2s>>"#,
        detail: "Chain a delayed section inside <<timed>>",
        snippet: r#"next ${1:2s}>>"#,
        sort_priority: 0,
    },
];
