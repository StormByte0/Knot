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
    /// The byte offset of the definition within the passage, **passage-relative**
    /// (0 = the `::` prefix of the passage header). To convert to
    /// document-absolute, add the passage's `passage_offset`. This matches
    /// the convention used by the `Passage` struct, enabling cross-document
    /// passage moves and incremental re-parsing without offset recomputation.
    pub defined_at_offset: usize,
    /// The 0-based line number of the definition (0 until computed).
    pub defined_at_line: u32,
    /// The number of arguments this macro accepts (if known).
    pub arg_count: Option<usize>,
    /// Whether this was defined via `<<widget>>` (vs `Macro.add()`).
    pub is_widget: bool,
    /// Whether this macro has a body (container) or is inline.
    ///
    /// For `Macro.add()` macros: derived from the `tags` field of the config
    /// object — `Required` if `tags` is present (whether `null` or an array),
    /// `Never` if absent.
    ///
    /// For `<<widget>>` macros: `Required` if defined with the `container`
    /// keyword (`<<widget name container>>`), `Never` otherwise.
    ///
    /// This field drives both the tree builder (whether to push onto the
    /// stack waiting for a close tag) and the diagnostic builder (whether
    /// to emit "Unclosed block macro" when no close tag is found).
    pub body: crate::types::BodyRequirement,
    /// Description/documentation (from comments above the definition).
    pub description: Option<String>,
}

impl CustomMacro {
    /// Backward-compat: returns `true` if this macro is a container (has a
    /// body requirement of `Required` or `Optional`).
    ///
    /// Prefer using `body` directly in new code.
    pub fn is_container(&self) -> bool {
        matches!(self.body, crate::types::BodyRequirement::Required | crate::types::BodyRequirement::Optional)
    }
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
    ///
    /// `body` should be `BodyRequirement::Required` if the widget was defined
    /// with the `container` keyword (e.g., `<<widget "name" container>>`),
    /// meaning it requires a closing tag and has access to `_contents`.
    /// Otherwise `BodyRequirement::Never` (inline, self-closing).
    pub fn register_widget(
        &mut self,
        name: &str,
        defined_in: &str,
        file_uri: &str,
        defined_at_offset: usize,
        arg_count: Option<usize>,
        body: crate::types::BodyRequirement,
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
                body,
                description: None,
            },
        );
    }

    /// Register a macro from a `Macro.add()` call in a script passage.
    ///
    /// `body` is derived from the `tags` field of the `Macro.add()` config
    /// object — `Required` if `tags` is present (whether `null` or an array),
    /// `Never` if absent. See `extract_body_requirement_from_macro_add_config`
    /// in `js_walk.rs`.
    pub fn register_macro_add(
        &mut self,
        name: &str,
        defined_in: &str,
        file_uri: &str,
        defined_at_offset: usize,
        arg_count: Option<usize>,
        body: crate::types::BodyRequirement,
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
                body,
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

    /// Remove all entries defined in a specific passage (for incremental re-parse).
    ///
    /// Unlike `remove_file()`, this removes macros by their `defined_in`
    /// passage name, which is needed when a single passage is re-parsed
    /// without re-parsing the entire file.
    pub fn remove_passage(&mut self, passage_name: &str) {
        self.macros.retain(|_, m| m.defined_in != passage_name);
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
        registry.register_widget("myWidget", "Widgets", "file:///test.tw", 100, None, crate::types::BodyRequirement::Never);

        let m = registry.get("myWidget").unwrap();
        assert_eq!(m.name, "myWidget");
        assert!(m.is_widget);
        assert!(!m.is_container());
        assert_eq!(m.defined_in, "Widgets");
    }

    #[test]
    fn register_container_widget() {
        let mut registry = CustomMacroRegistry::new();
        registry.register_widget("wrapWidget", "Widgets", "file:///test.tw", 200, None, crate::types::BodyRequirement::Required);

        let m = registry.get("wrapWidget").unwrap();
        assert!(m.is_widget);
        assert!(m.is_container());
    }

    #[test]
    fn register_macro_add() {
        let mut registry = CustomMacroRegistry::new();
        registry.register_macro_add("showStats", "Scripts", "file:///test.tw", 200, Some(2), crate::types::BodyRequirement::Never);

        let m = registry.get("showStats").unwrap();
        assert_eq!(m.name, "showStats");
        assert!(!m.is_widget);
        assert_eq!(m.arg_count, Some(2));
    }

    #[test]
    fn completion_names() {
        let mut registry = CustomMacroRegistry::new();
        registry.register_widget("myWidget", "W", "f", 0, None, crate::types::BodyRequirement::Never);
        registry.register_widget("myMacro", "W", "f", 0, None, crate::types::BodyRequirement::Never);
        registry.register_widget("otherThing", "W", "f", 0, None, crate::types::BodyRequirement::Never);

        let names = registry.completion_names("my");
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn remove_file() {
        let mut registry = CustomMacroRegistry::new();
        registry.register_widget("a", "W", "file:///a.tw", 0, None, crate::types::BodyRequirement::Never);
        registry.register_widget("b", "W", "file:///b.tw", 0, None, crate::types::BodyRequirement::Never);

        registry.remove_file("file:///a.tw");
        assert!(!registry.contains("a"));
        assert!(registry.contains("b"));
    }
}
