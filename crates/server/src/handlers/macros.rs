//! SugarCube macro signature table.
//!
//! Provides completion, hover, and signature-help data for built-in
//! SugarCube macros.

// ---------------------------------------------------------------------------
// MacroSignature
// ---------------------------------------------------------------------------

/// A SugarCube macro signature entry.
pub(crate) struct MacroSignature {
    pub name: &'static str,
    pub signature: &'static str,
    pub description: &'static str,
    pub has_params: bool,
    pub deprecated: bool,
}

impl MacroSignature {
    /// Return the snippet portion after the macro name (for insertion).
    pub fn insert_snippet(&self) -> &'static str {
        if self.has_params {
            " ${1:args}"
        } else {
            ""
        }
    }

    /// Return parameter names for signature help.
    pub fn param_names(&self) -> Vec<String> {
        if self.signature.is_empty() {
            return vec![];
        }
        self.signature
            .split(',')
            .map(|s| s.trim().to_string())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Built-in macro signatures
// ---------------------------------------------------------------------------

/// Built-in SugarCube macro signatures.
pub(crate) fn sugarcube_macro_signatures() -> Vec<MacroSignature> {
    vec![
        MacroSignature { name: "set", signature: "$var to expr", description: "Set a variable to a value.\n\nExample: `<<set $gold to 100>>`", has_params: true, deprecated: false },
        MacroSignature { name: "if", signature: "condition", description: "Conditional block — executes content if condition is true.\n\nExample: `<<if $gold > 50>>`", has_params: true, deprecated: false },
        MacroSignature { name: "elseif", signature: "condition", description: "Else-if clause for conditional blocks.", has_params: true, deprecated: false },
        MacroSignature { name: "else", signature: "", description: "Else clause for conditional blocks.", has_params: false, deprecated: false },
        MacroSignature { name: "for", signature: "$var, $var2, ... to expr", description: "Iterate over a collection or range.\n\nExample: `<<for _i to 0; _i < 5; _i++>>`", has_params: true, deprecated: false },
        MacroSignature { name: "switch", signature: "expr", description: "Switch statement for multi-way branching.", has_params: true, deprecated: false },
        MacroSignature { name: "case", signature: "value", description: "Case clause within a switch block.", has_params: true, deprecated: false },
        MacroSignature { name: "include", signature: "passageName", description: "Include the content of another passage inline.\n\nExample: `<<include \"Sidebar\">>`", has_params: true, deprecated: false },
        MacroSignature { name: "print", signature: "expr", description: "Print the result of an expression.\n\nExample: `<<print $gold>>`", has_params: true, deprecated: false },
        MacroSignature { name: "nobr", signature: "", description: "Suppress automatic line break handling within the block.", has_params: false, deprecated: false },
        MacroSignature { name: "script", signature: "", description: "Include raw JavaScript code.", has_params: false, deprecated: false },
        MacroSignature { name: "run", signature: "expr", description: "Run a JavaScript expression silently (no output).\n\nExample: `<<run state.active.passage = 'Start'>>`", has_params: true, deprecated: false },
        MacroSignature { name: "capture", signature: "$var", description: "Capture rendered content into a variable.", has_params: true, deprecated: false },
        MacroSignature { name: "append", signature: "selector", description: "Append content to a DOM element matching the selector.", has_params: true, deprecated: false },
        MacroSignature { name: "prepend", signature: "selector", description: "Prepend content to a DOM element matching the selector.", has_params: true, deprecated: false },
        MacroSignature { name: "replace", signature: "selector", description: "Replace the content of a DOM element matching the selector.", has_params: true, deprecated: false },
        MacroSignature { name: "remove", signature: "selector", description: "Remove a DOM element matching the selector.", has_params: true, deprecated: false },
        MacroSignature { name: "button", signature: "label, passageName", description: "Create a button that navigates to a passage.", has_params: true, deprecated: false },
        MacroSignature { name: "link", signature: "label, passageName", description: "Create a passage link with optional display text.", has_params: true, deprecated: false },
        MacroSignature { name: "actions", signature: "", description: "Container for `<<choice>>` macros.", has_params: false, deprecated: false },
        MacroSignature { name: "choice", signature: "label, passageName", description: "Create a one-time choice within an `<<actions>>` block.", has_params: true, deprecated: false },
        MacroSignature { name: "goto", signature: "passageName", description: "Immediately navigate to another passage.\n\nExample: `<<goto \"EndGame\">>`", has_params: true, deprecated: false },
        MacroSignature { name: "back", signature: "", description: "Navigate to the previous passage in history.", has_params: false, deprecated: false },
        MacroSignature { name: "return", signature: "", description: "Return from an `<<include>>` or `<<widget>>`.", has_params: false, deprecated: false },
        MacroSignature { name: "widget", signature: "widgetName", description: "Define a reusable widget macro.\n\nExample: `<<widget \"hello\">>Hello!<</widget>>`", has_params: true, deprecated: false },
        MacroSignature { name: "type", signature: "text, speed", description: "Typewriter effect for text content.", has_params: true, deprecated: false },
        MacroSignature { name: "timed", signature: "delay", description: "Execute content after a delay (in milliseconds or CSS time).\n\nExample: `<<timed 2s>>`", has_params: true, deprecated: false },
        MacroSignature { name: "next", signature: "delay", description: "Chain additional timed content after a `<<timed>>` block.", has_params: true, deprecated: false },
        MacroSignature { name: "visit", signature: "passageName", description: "Check if a passage has been visited and how many times.", has_params: true, deprecated: false },
        // Deprecated macros
        MacroSignature { name: "display", signature: "passageName", description: "Deprecated — use `<<include>>` or `<<transclude>>` instead.\n\nExample: `<<display \"Sidebar\">>`", has_params: true, deprecated: true },
        MacroSignature { name: "remember", signature: "$var to expr", description: "Deprecated in SugarCube 2 — use `<<set>>` with persistent storage instead.", has_params: true, deprecated: true },
        MacroSignature { name: "forget", signature: "$var", description: "Deprecated in SugarCube 2 — use `<<set>>` with persistent storage instead.", has_params: true, deprecated: true },
        MacroSignature { name: "setcss", signature: "css", description: "Deprecated — use `<<addclass>>` or `<<removeclass>>` instead.", has_params: true, deprecated: true },
        MacroSignature { name: "settitle", signature: "title", description: "Deprecated — set `document.title` directly via `<<run>>` instead.", has_params: true, deprecated: true },
    ]
}
