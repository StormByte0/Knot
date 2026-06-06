//! Custom macro registry for the SugarCube format plugin.
//!
//! Tracks user-defined macros (widgets and `Macro.add()` calls) that are
//! discovered during the ordered parse pipeline. Like `VariableTree`, this
//! is a maintained side table that persists across parse calls.
//!
//! ## Population order
//!
//! 1. `[script]` passages → oxc walk → `Macro.add()` definitions
//! 2. `[widget]` passages → SugarCube parser → `<<widget name>>` definitions
//!
//! Both populate this registry so that later passages can query it for
//! completions, hover, and go-to-definition.

use std::collections::HashMap;
use std::ops::Range;

// ---------------------------------------------------------------------------
// CustomMacro — a user-defined macro entry
// ---------------------------------------------------------------------------

/// A user-defined macro discovered from `<<widget>>` or `Macro.add()`.
#[derive(Debug, Clone)]
pub struct CustomMacro {
    /// The macro name (e.g., "myWidget", "showStats").
    pub name: String,
    /// The passage where this macro is defined.
    pub defined_in: String,
    /// The file URI where this macro is defined.
    pub file_uri: String,
    /// The byte offset of the definition within the file.
    pub defined_at_offset: usize,
    /// The 0-based line number of the definition (0 until computed).
    pub defined_at_line: u32,
    /// The number of arguments this macro accepts (if known).
    pub arg_count: Option<usize>,
    /// Whether this was defined via `<<widget>>` (vs `Macro.add()`).
    pub is_widget: bool,
    /// Description/documentation (from comments above the definition).
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// CustomMacroRegistry — the side table
// ---------------------------------------------------------------------------

/// Registry of custom macros (widgets and `Macro.add()` definitions).
///
/// Updated incrementally during the parse pipeline. Used by completion,
/// hover, and go-to-definition handlers.
#[derive(Debug, Clone, Default)]
pub struct CustomMacroRegistry {
    /// Map of macro name → definition.
    macros: HashMap<String, CustomMacro>,
}

impl CustomMacroRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a widget definition from a `[widget]` passage.
    pub fn register_widget(
        &mut self,
        name: &str,
        defined_in: &str,
        file_uri: &str,
        defined_at_offset: usize,
        arg_count: Option<usize>,
    ) {
        self.macros.insert(
            name.to_string(),
            CustomMacro {
                name: name.to_string(),
                defined_in: defined_in.to_string(),
                file_uri: file_uri.to_string(),
                defined_at_offset,
                defined_at_line: 0,
                arg_count,
                is_widget: true,
                description: None,
            },
        );
    }

    /// Register a macro from a `Macro.add()` call in a script passage.
    pub fn register_macro_add(
        &mut self,
        name: &str,
        defined_in: &str,
        file_uri: &str,
        defined_at_offset: usize,
        arg_count: Option<usize>,
    ) {
        self.macros.insert(
            name.to_string(),
            CustomMacro {
                name: name.to_string(),
                defined_in: defined_in.to_string(),
                file_uri: file_uri.to_string(),
                defined_at_offset,
                defined_at_line: 0,
                arg_count,
                is_widget: false,
                description: None,
            },
        );
    }

    /// Look up a custom macro by name.
    pub fn get(&self, name: &str) -> Option<&CustomMacro> {
        self.macros.get(name)
    }

    /// Get all custom macro names.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.macros.keys()
    }

    /// Get all registered macros.
    pub fn all_macros(&self) -> impl Iterator<Item = &CustomMacro> {
        self.macros.values()
    }

    /// Get the number of registered macros.
    pub fn len(&self) -> usize {
        self.macros.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.macros.is_empty()
    }

    /// Clear all macro definitions (for full re-parse).
    pub fn clear(&mut self) {
        self.macros.clear();
    }

    /// Remove all entries for a specific file (for incremental re-parse).
    pub fn remove_file(&mut self, file_uri: &str) {
        self.macros.retain(|_, m| m.file_uri != file_uri);
    }

    /// Check if a macro name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.macros.contains_key(name)
    }

    /// Get macro names for completion (filtered by prefix).
    pub fn completion_names(&self, prefix: &str) -> Vec<String> {
        self.macros
            .keys()
            .filter(|n| n.starts_with(prefix))
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_widget() {
        let mut registry = CustomMacroRegistry::new();
        registry.register_widget("myWidget", "Widgets", "file:///test.tw", 100, None);

        let m = registry.get("myWidget").unwrap();
        assert_eq!(m.name, "myWidget");
        assert!(m.is_widget);
        assert_eq!(m.defined_in, "Widgets");
    }

    #[test]
    fn register_macro_add() {
        let mut registry = CustomMacroRegistry::new();
        registry.register_macro_add("showStats", "Scripts", "file:///test.tw", 200, Some(2));

        let m = registry.get("showStats").unwrap();
        assert_eq!(m.name, "showStats");
        assert!(!m.is_widget);
        assert_eq!(m.arg_count, Some(2));
    }

    #[test]
    fn completion_names() {
        let mut registry = CustomMacroRegistry::new();
        registry.register_widget("myWidget", "W", "f", 0, None);
        registry.register_widget("myMacro", "W", "f", 0, None);
        registry.register_widget("otherThing", "W", "f", 0, None);

        let names = registry.completion_names("my");
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn remove_file() {
        let mut registry = CustomMacroRegistry::new();
        registry.register_widget("a", "W", "file:///a.tw", 0, None);
        registry.register_widget("b", "W", "file:///b.tw", 0, None);

        registry.remove_file("file:///a.tw");
        assert!(!registry.contains("a"));
        assert!(registry.contains("b"));
    }
}
