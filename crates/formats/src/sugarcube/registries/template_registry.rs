//! Template registry for the SugarCube format plugin.
//!
//! Tracks `Template.add()` definitions discovered in `[script]` passages
//! during the ordered parse pipeline. SugarCube templates are named output
//! fragments that expand in passage text (e.g., `?templatename`).
//!
//! ## SugarCube Template API
//!
//! ```javascript
//! Template.add("heal", function () {
//!     return "<<link 'Heal' `passage()`>>\
//!         <<run $hp += 10>>\
//!     <</link>>";
//! });
//! ```
//!
//! Templates are invoked in passage text with the `?` prefix:
//! `?heal` expands to the template's output string.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// TemplateEntry — a discovered template definition
// ---------------------------------------------------------------------------

/// The kind of template definition discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    /// `Template.add("name", function)` — function template
    Function,
    /// `Template.add("name", "string")` — string template
    String,
}

/// A template definition discovered during JS analysis.
#[derive(Debug, Clone)]
pub struct TemplateEntry {
    /// The template name (without the `?` prefix).
    pub name: String,
    /// The kind of template (function or string).
    pub kind: TemplateKind,
    /// The passage where this template is defined.
    pub defined_in: String,
    /// The file URI where this template is defined.
    pub file_uri: String,
    /// The byte offset of the definition within the passage, **passage-relative**
    /// (0 = the `::` prefix of the passage header). To convert to
    /// document-absolute, add the passage's `passage_offset`. This matches
    /// the convention used by the `Passage` struct.
    pub defined_at_offset: usize,
    /// The 0-based line number of the definition (0 until computed).
    pub defined_at_line: u32,
}

// ---------------------------------------------------------------------------
// TemplateRegistry — the side table
// ---------------------------------------------------------------------------

/// Registry of template definitions discovered during parsing.
///
/// Updated incrementally during the parse pipeline. Used by completion,
/// hover, and go-to-definition handlers for the `?templatename` syntax.
#[derive(Debug, Clone, Default)]
pub struct TemplateRegistry {
    /// Map of template name → definition.
    templates: HashMap<String, TemplateEntry>,
}

impl TemplateRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a template definition discovered during JS analysis.
    pub fn register_template(
        &mut self,
        name: &str,
        kind: TemplateKind,
        defined_in: &str,
        file_uri: &str,
        defined_at_offset: usize,
    ) {
        self.templates.insert(
            name.to_string(),
            TemplateEntry {
                name: name.to_string(),
                kind,
                defined_in: defined_in.to_string(),
                file_uri: file_uri.to_string(),
                defined_at_offset,
                defined_at_line: 0,
            },
        );
    }

    /// Look up a template by name (without `?` prefix).
    pub fn get(&self, name: &str) -> Option<&TemplateEntry> {
        self.templates.get(name)
    }

    /// Get all template names.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.templates.keys()
    }

    /// Get all registered templates.
    pub fn all_templates(&self) -> impl Iterator<Item = &TemplateEntry> {
        self.templates.values()
    }

    /// Get the number of registered templates.
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Clear all template definitions (for full re-parse).
    pub fn clear(&mut self) {
        self.templates.clear();
    }

    /// Remove all entries for a specific file (for incremental re-parse).
    pub fn remove_file(&mut self, file_uri: &str) {
        self.templates.retain(|_, t| t.file_uri != file_uri);
    }

    /// Remove all entries defined in a specific passage (for incremental re-parse).
    pub fn remove_passage(&mut self, passage_name: &str) {
        self.templates.retain(|_, t| t.defined_in != passage_name);
    }

    /// Check if a template name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.templates.contains_key(name)
    }

    /// Get template names for completion (with `?` prefix added).
    pub fn completion_names(&self) -> Vec<String> {
        self.templates
            .keys()
            .map(|n| format!("?{}", n))
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
    fn register_function_template() {
        let mut registry = TemplateRegistry::new();
        registry.register_template(
            "heal",
            TemplateKind::Function,
            "Scripts",
            "file:///test.tw",
            100,
        );

        let t = registry.get("heal").unwrap();
        assert_eq!(t.name, "heal");
        assert_eq!(t.kind, TemplateKind::Function);
    }

    #[test]
    fn register_string_template() {
        let mut registry = TemplateRegistry::new();
        registry.register_template(
            "separator",
            TemplateKind::String,
            "Scripts",
            "file:///test.tw",
            200,
        );

        let t = registry.get("separator").unwrap();
        assert_eq!(t.kind, TemplateKind::String);
    }

    #[test]
    fn completion_names_with_prefix() {
        let mut registry = TemplateRegistry::new();
        registry.register_template("heal", TemplateKind::Function, "S", "f", 0);
        registry.register_template("attack", TemplateKind::Function, "S", "f", 0);

        let names = registry.completion_names();
        assert!(names.contains(&"?heal".to_string()));
        assert!(names.contains(&"?attack".to_string()));
    }

    #[test]
    fn remove_file() {
        let mut registry = TemplateRegistry::new();
        registry.register_template("a", TemplateKind::Function, "S1", "file:///a.tw", 0);
        registry.register_template("b", TemplateKind::Function, "S2", "file:///b.tw", 0);

        registry.remove_file("file:///a.tw");
        assert!(!registry.contains("a"));
        assert!(registry.contains("b"));
    }
}
