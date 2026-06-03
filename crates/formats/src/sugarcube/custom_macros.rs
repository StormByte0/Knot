//! Workspace-wide custom macro registry.
//!
//! Custom macros defined via `Macro.add('name', { handler: function() { ... } })`
//! in `[script]` passages may be invoked from any passage in any file. This
//! registry accumulates all known custom macro definitions across the workspace
//! so that `walk_translate()` can translate invocations as function calls rather
//! than `/* unknown */` comments.
//!
//! ## Lifecycle
//!
//! - **Population**: During `parse()`, script passages are scanned for
//!   `Macro.add` definitions. The registry is updated per-file: on each
//!   `parse()` call, all custom macros from that file are refreshed (old
//!   entries for that file are removed, new ones are inserted).
//! - **Query**: Before `walk_translate()`, the registry's callable names are
//!   merged with the per-file `callables` list to build the full
//!   `TranslationContext`.
//!
//! ## Virtual doc integration
//!
//! When building the virtual doc for a script passage, `build_script_passage_js()`
//! emits TWO things:
//!
//! 1. **Raw JS wrapper**: `function script_MyScript() { ... raw JS ... }` —
//!    the script passage body wrapped in a function declaration.
//! 2. **Standalone function declarations**: For each `Macro.add('name', ...)`
//!    definition found in the script passage, a standalone function like
//!    `function name(...args) { ... translated handler ... }` is emitted
//!    after the raw JS wrapper. This makes custom macros visible to VSCode's
//!    JS language service as real functions, so invocations like `addTime(30);`
//!    validate correctly instead of showing "Cannot find name" errors.
//!
//! ## Thread safety
//!
//! Uses `RwLock` for interior mutability since `FormatPlugin` requires
//! `Send + Sync` and its methods take `&self` (not `&mut self`).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use crate::types::{UserCallable, UserCallableKind};

// ---------------------------------------------------------------------------
// Regexes for Macro.add() handler body transformation
// ---------------------------------------------------------------------------

/// `this.args` in handler bodies — replace with `args` rest parameter.
/// Matches `this.args` followed by a word boundary (handles `this.args[0]`,
/// `this.args.length`, bare `this.args`, etc.).
static RE_THIS_ARGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"this\.args\b").unwrap()
});

/// `this.name` in handler bodies — replace with a string literal of the
/// macro name. Matches `this.name` followed by a word boundary.
static RE_THIS_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"this\.name\b").unwrap()
});

// ---------------------------------------------------------------------------
// CustomMacroRegistry
// ---------------------------------------------------------------------------

/// Workspace-wide registry of custom macros defined via `Macro.add()`.
///
/// Accumulates `UserCallable` entries from all files. On each file reparse,
/// the old entries for that file are removed and replaced with the new ones.
/// This ensures that:
///
/// - Custom macros from file A are available when translating passages in
///   file B
/// - If a custom macro is renamed or removed in file A, the registry
///   reflects the change on the next reparse of file A
#[derive(Debug, Clone, Default)]
pub(crate) struct CustomMacroRegistry {
    /// All known custom macro callables (from Macro.add in script passages).
    /// Keyed by callable name for deduplication.
    macros: HashMap<String, UserCallable>,

    /// Source file URI → macro names defined in that file.
    /// Used for file-level invalidation (remove all macros for a URI).
    file_macros: HashMap<Url, Vec<String>>,
}

#[allow(dead_code)] // API surface for Phase C (completions, hover, cross-file diagnostics)
impl CustomMacroRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Refresh all custom macros from a given file.
    ///
    /// Removes any existing entries for the file, then inserts new ones
    /// from the provided callables list. Only `UserCallableKind::CustomMacro`
    /// entries are stored; widgets are handled separately.
    pub fn update_file(&mut self, file_uri: &Url, callables: &[UserCallable]) {
        // Remove old entries for this file
        self.remove_file(file_uri);

        // Insert new custom macros from this file
        let mut file_names = Vec::new();
        for callable in callables {
            if callable.kind != UserCallableKind::CustomMacro {
                continue;
            }
            // Only track callables from this file
            if callable.file_uri != file_uri.to_string() {
                continue;
            }
            self.macros.insert(callable.name.clone(), callable.clone());
            file_names.push(callable.name.clone());
        }

        if !file_names.is_empty() {
            self.file_macros.insert(file_uri.clone(), file_names);
        }
    }

    /// Remove all custom macros originating from a given file URI.
    ///
    /// Used for file-level invalidation when an entire .tw file is
    /// reprocessed. Returns the number of macros removed.
    pub fn remove_file(&mut self, uri: &Url) -> usize {
        if let Some(names) = self.file_macros.remove(uri) {
            let count = names.len();
            for name in &names {
                self.macros.remove(name);
            }
            count
        } else {
            0
        }
    }

    /// Get all custom macro names known to the registry.
    pub fn macro_names(&self) -> HashSet<&str> {
        self.macros.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a custom macro with the given name is known.
    pub fn contains(&self, name: &str) -> bool {
        self.macros.contains_key(name)
    }

    /// Get a custom macro callable by name.
    pub fn get(&self, name: &str) -> Option<&UserCallable> {
        self.macros.get(name)
    }

    /// Get the number of registered custom macros.
    pub fn len(&self) -> usize {
        self.macros.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.macros.is_empty()
    }

    /// Merge workspace-wide custom macros with per-file callables.
    ///
    /// Returns a combined `Vec<UserCallable>` containing:
    /// 1. All callables from `per_file_callables` (widgets + same-file custom macros)
    /// 2. Any workspace-wide custom macros NOT already present in the per-file list
    ///
    /// This ensures that `walk_translate()` sees a complete set of callables
    /// including cross-file custom macros.
    pub fn merge_with_callables(&self, per_file_callables: &[UserCallable]) -> Vec<UserCallable> {
        let per_file_names: HashSet<&str> = per_file_callables
            .iter()
            .map(|c| c.name.as_str())
            .collect();

        let mut merged: Vec<UserCallable> = per_file_callables.to_vec();

        // Add workspace-wide custom macros not already in the per-file list
        for (name, callable) in &self.macros {
            if !per_file_names.contains(name.as_str()) {
                merged.push(callable.clone());
            }
        }

        merged
    }
}

// ---------------------------------------------------------------------------
// Script passage JS → virtual doc function builder
// ---------------------------------------------------------------------------

/// Build a virtual doc JS function for a script passage.
///
/// Script passages contain raw JavaScript. We wrap them in a function
/// declaration so they integrate cleanly with the virtual doc's
/// function-per-passage structure. Additionally, for each `Macro.add()`
/// definition found in the script passage, we emit a standalone function
/// declaration so that VSCode's JS language service recognizes the custom
/// macro as a callable function.
///
/// ## Output structure
///
/// ```js
/// // 1. Raw JS wrapper (the script passage body):
/// function script_MyScript() {
///   Macro.add('addTime', { handler: function() { ... } });
///   // ... other raw JS ...
/// }
///
/// // 2. Standalone function declarations (one per Macro.add definition):
/// function addTime(...args) {
///   var hours = args[0];   // this.args[0] → args[0]
///   State.variables.time += hours;
/// }
///
/// function removeItem(...args) {
///   State.variables.inventory.splice(args[0], 1);
/// }
/// ```
///
/// ## Handler body transformation
///
/// Inside `Macro.add()` handler functions, the `this` context provides
/// `this.args` (the arguments array) and `this.name` (the macro name).
/// In the emitted standalone function, these are translated:
///
/// - `this.args` → `args` (the rest parameter)
/// - `this.name` → `'macroName'` (a string literal)
/// - Other `this.xxx` references are left as-is (they reference the
///   SugarCube macro context which doesn't exist in the standalone function,
///   but VSCode doesn't validate runtime semantics)
///
/// Dollar references (`$var`) in the handler body are translated to
/// `State.variables.var` so VSCode's JS validation tracks variable usage.
pub(crate) fn build_script_passage_js(
    passage_name: &str,
    body: &str,
    body_offset: usize,
    custom_macros_in_passage: &[UserCallable],
) -> (String, Vec<super::passage_tree::ExactLineMapping>) {
    use super::passage_tree::ExactLineMapping;
    use super::virtual_doc::translate_dollar_refs_in_js;

    let safe_name = passage_name.replace(' ', "_");
    let func_name = format!("script_{}", safe_name);

    let translated_js = translate_dollar_refs_in_js(body);

    let mut js_output = format!("function {}() {{\n", func_name);
    let mut line_map = Vec::new();

    // Function header line
    line_map.push(ExactLineMapping {
        original_line: 0,
        original_start_byte: body_offset,
    });

    // Body lines — track which source line each JS line came from
    // by counting newlines in the original body text
    let mut source_line: u32 = 0;
    for line in translated_js.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            js_output.push_str(&format!("  {}\n", trimmed));
            line_map.push(ExactLineMapping {
                original_line: source_line,
                original_start_byte: body_offset,
            });
        }
        source_line += 1;
    }

    // Close function
    js_output.push_str("}\n");
    line_map.push(ExactLineMapping {
        original_line: 0,
        original_start_byte: body_offset,
    });

    // ── Emit standalone function declarations for Macro.add definitions ──
    //
    // For each custom macro defined in this script passage, emit a
    // standalone function declaration. This makes the custom macro visible
    // to VSCode's JS language service as a real function, so invocations
    // like `addTime(30);` validate correctly.
    for callable in custom_macros_in_passage {
        if callable.kind != UserCallableKind::CustomMacro {
            continue;
        }
        if callable.defined_in != passage_name {
            continue;
        }
        emit_custom_macro_standalone_function(
            callable, &mut js_output, &mut line_map, body_offset,
        );
    }

    (js_output, line_map)
}

/// Emit a standalone function declaration for a custom macro definition.
///
/// Transforms the `Macro.add()` handler into a standalone function that
/// VSCode can recognize. The handler body is adapted:
///
/// - `this.args` → `args` (the rest parameter)
/// - `this.name` → `'macroName'` (string literal)
/// - `$var` → `State.variables.var` (via translate_dollar_refs_in_js)
///
/// The function uses `...args` rest parameter syntax, matching SugarCube's
/// variadic macro invocation semantics. If `arg_count` is known, we also
/// emit destructured parameter aliases for better intellisense:
///
/// ```js
/// function addTime(...args) {
///   var arg0 = args[0]; // if arg_count is Some(1)
///   // ... handler body with this.args → args replacement ...
/// }
/// ```
fn emit_custom_macro_standalone_function(
    callable: &UserCallable,
    js_output: &mut String,
    line_map: &mut Vec<super::passage_tree::ExactLineMapping>,
    body_offset: usize,
) {
    use super::passage_tree::ExactLineMapping;
    use super::virtual_doc::translate_dollar_refs_in_js;

    // Blank line separator before the function
    js_output.push('\n');

    // Function signature: function macroName(...args) {
    js_output.push_str(&format!("function {}(...args) {{\n", callable.name));
    line_map.push(ExactLineMapping {
        original_line: callable.defined_at_line,
        original_start_byte: body_offset,
    });

    // Emit argument aliases if arg_count is known
    // e.g., var arg0 = args[0]; var arg1 = args[1];
    // This gives VSCode better intellisense for the handler body.
    if let Some(arg_count) = callable.arg_count {
        for i in 0..arg_count {
            js_output.push_str(&format!("  var arg{} = args[{}];\n", i, i));
            line_map.push(ExactLineMapping {
                original_line: callable.defined_at_line,
                original_start_byte: body_offset,
            });
        }
    }

    // Translate the handler body
    if let Some(ref handler_body) = callable.body {
        // Step 1: Translate $var → State.variables.var
        let translated = translate_dollar_refs_in_js(handler_body);

        // Step 2: Replace this.args → args and this.name → 'macroName'
        let translated = RE_THIS_ARGS
            .replace_all(&translated, "args")
            .to_string();
        let translated = RE_THIS_NAME
            .replace_all(&translated, format!("'{}'", callable.name).as_str())
            .to_string();

        // Emit each non-empty line with indentation
        for line in translated.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                js_output.push_str(&format!("  {}\n", trimmed));
                line_map.push(ExactLineMapping {
                    original_line: callable.defined_at_line,
                    original_start_byte: body_offset,
                });
            }
        }
    }

    // Close function
    js_output.push_str("}\n");
    line_map.push(ExactLineMapping {
        original_line: callable.defined_at_line,
        original_start_byte: body_offset,
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_update_and_query() {
        let mut registry = CustomMacroRegistry::new();
        let url = Url::parse("file:///test.tw").unwrap();

        let callables = vec![
            UserCallable {
                name: "addTime".to_string(),
                kind: UserCallableKind::CustomMacro,
                arg_count: Some(1),
                defined_in: "MyScript".to_string(),
                file_uri: url.to_string(),
                defined_at_line: 5,
                body: Some("State.variables.time += this.args[0];".to_string()),
            },
            UserCallable {
                name: "myWidget".to_string(),
                kind: UserCallableKind::Widget,
                arg_count: None,
                defined_in: "Widgets".to_string(),
                file_uri: url.to_string(),
                defined_at_line: 1,
                body: None,
            },
        ];

        registry.update_file(&url, &callables);

        // CustomMacro should be registered
        assert!(registry.contains("addTime"));
        // Widget should NOT be in the registry (handled separately)
        assert!(!registry.contains("myWidget"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_registry_file_invalidation() {
        let mut registry = CustomMacroRegistry::new();
        let url = Url::parse("file:///test.tw").unwrap();

        let callables = vec![
            UserCallable {
                name: "addTime".to_string(),
                kind: UserCallableKind::CustomMacro,
                arg_count: Some(1),
                defined_in: "MyScript".to_string(),
                file_uri: url.to_string(),
                defined_at_line: 5,
                body: None,
            },
        ];

        registry.update_file(&url, &callables);
        assert_eq!(registry.len(), 1);

        // Remove file
        let removed = registry.remove_file(&url);
        assert_eq!(removed, 1);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_merge_with_callables() {
        let mut registry = CustomMacroRegistry::new();
        let url_a = Url::parse("file:///a.tw").unwrap();
        let url_b = Url::parse("file:///b.tw").unwrap();

        // File A defines a custom macro
        let callables_a = vec![
            UserCallable {
                name: "fromFileA".to_string(),
                kind: UserCallableKind::CustomMacro,
                arg_count: None,
                defined_in: "ScriptA".to_string(),
                file_uri: url_a.to_string(),
                defined_at_line: 1,
                body: None,
            },
        ];
        registry.update_file(&url_a, &callables_a);

        // File B has a widget and its own custom macro
        let callables_b = vec![
            UserCallable {
                name: "fromFileB".to_string(),
                kind: UserCallableKind::CustomMacro,
                arg_count: None,
                defined_in: "ScriptB".to_string(),
                file_uri: url_b.to_string(),
                defined_at_line: 1,
                body: None,
            },
            UserCallable {
                name: "myWidget".to_string(),
                kind: UserCallableKind::Widget,
                arg_count: None,
                defined_in: "WidgetPassage".to_string(),
                file_uri: url_b.to_string(),
                defined_at_line: 1,
                body: None,
            },
        ];

        // Merge: per-file callables from B + workspace-wide custom macros
        let merged = registry.merge_with_callables(&callables_b);
        let merged_names: HashSet<&str> = merged.iter().map(|c| c.name.as_str()).collect();

        // Should include all three
        assert!(merged_names.contains("fromFileA"));
        assert!(merged_names.contains("fromFileB"));
        assert!(merged_names.contains("myWidget"));
    }

    #[test]
    fn test_build_script_passage_js_with_macros() {
        let body = r#"Macro.add('addTime', {
    handler: function() {
        var hours = this.args[0];
        State.variables.time += hours;
    }
});

Macro.add('removeItem', {
    handler: function() {
        State.variables.inventory.splice(this.args[0], 1);
    }
});

// Other setup code
State.variables.time = 0;"#;

        let custom_macros = vec![
            UserCallable {
                name: "addTime".to_string(),
                kind: UserCallableKind::CustomMacro,
                arg_count: Some(1),
                defined_in: "Macros".to_string(),
                file_uri: "file:///test.tw".to_string(),
                defined_at_line: 0,
                body: Some("var hours = this.args[0];\nState.variables.time += hours;".to_string()),
            },
            UserCallable {
                name: "removeItem".to_string(),
                kind: UserCallableKind::CustomMacro,
                arg_count: Some(1),
                defined_in: "Macros".to_string(),
                file_uri: "file:///test.tw".to_string(),
                defined_at_line: 6,
                body: Some("State.variables.inventory.splice(this.args[0], 1);".to_string()),
            },
        ];

        let (js, _line_map) = build_script_passage_js("Macros", body, 0, &custom_macros);

        // Should contain the raw JS wrapper
        assert!(js.contains("function script_Macros()"));
        assert!(js.contains("Macro.add('addTime'"));

        // Should contain standalone function declarations
        assert!(js.contains("function addTime(...args)"));
        assert!(js.contains("function removeItem(...args)"));

        // Handler body transformation: this.args → args in standalone functions
        // (the raw JS wrapper still has this.args from Macro.add() calls)
        assert!(js.contains("args[0]"));

        // The standalone function addTime should have args[0], not this.args[0]
        let addtime_func_start = js.find("function addTime").unwrap();
        let addtime_func_end = js[addtime_func_start..].find("}\n").unwrap() + addtime_func_start + 1;
        let addtime_func = &js[addtime_func_start..addtime_func_end];
        assert!(!addtime_func.contains("this.args"),
            "Standalone addTime function should not contain this.args: {}", addtime_func);

        // Argument aliases for known arg_count
        assert!(js.contains("var arg0 = args[0]"));
    }

    #[test]
    fn test_build_script_passage_js_with_dollar_refs() {
        let body = r#"var gold = $gold;
$gold = 100;"#;

        let (js, _line_map) = build_script_passage_js("Init", body, 0, &[]);

        // Dollar refs should be translated
        assert!(js.contains("State.variables.gold"));
        assert!(!js.contains("$gold"));
    }

    #[test]
    fn test_build_script_passage_js_widget_not_emitted() {
        // Widget callables should NOT produce standalone functions
        // from build_script_passage_js (widgets have their own passages)
        let custom_macros = vec![
            UserCallable {
                name: "myWidget".to_string(),
                kind: UserCallableKind::Widget,
                arg_count: None,
                defined_in: "WidgetPassage".to_string(),
                file_uri: "file:///test.tw".to_string(),
                defined_at_line: 0,
                body: None,
            },
        ];

        let (js, _) = build_script_passage_js("SomeScript", "// JS", 0, &custom_macros);

        // Widget should NOT appear as a standalone function
        assert!(!js.contains("function myWidget"));
    }

    #[test]
    fn test_this_name_replacement() {
        let body = r#"Macro.add('myMacro', {
    handler: function() {
        console.log(this.name);
    }
});"#;

        let custom_macros = vec![
            UserCallable {
                name: "myMacro".to_string(),
                kind: UserCallableKind::CustomMacro,
                arg_count: None,
                defined_in: "Script".to_string(),
                file_uri: "file:///test.tw".to_string(),
                defined_at_line: 0,
                body: Some("console.log(this.name);".to_string()),
            },
        ];

        let (js, _) = build_script_passage_js("Script", body, 0, &custom_macros);

        // this.name should be replaced with string literal in standalone function
        // (the raw JS wrapper still has this.name from Macro.add() calls)
        assert!(js.contains("'myMacro'"));

        // The standalone function myMacro should not contain this.name
        let func_start = js.find("function myMacro").unwrap();
        let func_end = js[func_start..].rfind("}\n").unwrap() + func_start + 1;
        let func_body = &js[func_start..func_end];
        assert!(!func_body.contains("this.name"),
            "Standalone myMacro function should not contain this.name: {}", func_body);
    }
}
