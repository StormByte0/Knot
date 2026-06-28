//! JavaScript function registry for the SugarCube format plugin.
//!
//! Tracks function declarations discovered in `[script]` passages and inline
//! JS blocks during the ordered parse pipeline. Like [`VariableTree`] and
//! [`CustomMacroRegistry`], this is a maintained side table that persists
//! across parse calls.
//!
//! ## Why track functions?
//!
//! SugarCube script passages can define JS functions that are callable from
//! other passages via `<<run>>` or `<<set>>`. Tracking these enables:
//!
//! - **Completion** for function names in JS contexts
//! - **Hover** documentation for user-defined functions
//! - **Go-to-definition** from function call sites
//! - **Diagnostics** for calls to undefined functions
//!
//! ## Population order
//!
//! 1. `[script]` passages → oxc walk → `function`/`var`/`let`/`const` declarations
//! 2. Inline JS in `<<run>>`/`<<set>>` → expression-level function expressions

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// FunctionEntry — a discovered function definition
// ---------------------------------------------------------------------------

/// The kind of function definition discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKind {
    /// A `function` declaration: `function myFunc() { ... }`
    Declaration,
    /// A named function expression: `var x = function myFunc() { ... }`
    NamedExpression,
    /// An arrow function assigned to a variable: `const myFunc = () => { ... }`
    ArrowFunction,
    /// A method definition in an object literal or class.
    Method,
}

/// A function definition discovered during JS analysis.
#[derive(Debug, Clone)]
pub struct FunctionEntry {
    /// The function name (e.g., "myFunc", "calculateScore").
    pub name: String,
    /// The kind of function definition.
    pub kind: FunctionKind,
    /// The passage where this function is defined.
    pub defined_in: String,
    /// The file URI where this function is defined.
    pub file_uri: String,
    /// The byte offset of the function definition within the passage,
    /// **passage-relative** (0 = the `::` prefix of the passage header).
    /// To convert to document-absolute, add the passage's `passage_offset`.
    /// This matches the convention used by the `Passage` struct.
    pub defined_at_offset: usize,
    /// The 0-based line number of the definition (0 until computed).
    pub defined_at_line: u32,
    /// The number of parameters this function accepts (if known).
    pub param_count: Option<usize>,
    /// Whether this function is exported (for module-style scripts).
    pub is_exported: bool,
}

// ---------------------------------------------------------------------------
// FunctionRegistry — the side table
// ---------------------------------------------------------------------------

/// Registry of JavaScript function definitions discovered during parsing.
///
/// Updated incrementally during the parse pipeline. Used by completion,
/// hover, and go-to-definition handlers.
#[derive(Debug, Clone, Default)]
pub struct FunctionRegistry {
    /// Map of function name → definition.
    functions: HashMap<String, FunctionEntry>,
}

impl FunctionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a function definition discovered during JS analysis.
    pub fn register_function(
        &mut self,
        name: &str,
        kind: FunctionKind,
        defined_in: &str,
        file_uri: &str,
        defined_at_offset: usize,
        param_count: Option<usize>,
    ) {
        self.functions.insert(
            name.to_string(),
            FunctionEntry {
                name: name.to_string(),
                kind,
                defined_in: defined_in.to_string(),
                file_uri: file_uri.to_string(),
                defined_at_offset,
                defined_at_line: 0,
                param_count,
                is_exported: false,
            },
        );
    }

    /// Look up a function by name.
    pub fn get(&self, name: &str) -> Option<&FunctionEntry> {
        self.functions.get(name)
    }

    /// Get all function names.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.functions.keys()
    }

    /// Get all registered functions.
    pub fn all_functions(&self) -> impl Iterator<Item = &FunctionEntry> {
        self.functions.values()
    }

    /// Get the number of registered functions.
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Clear all function definitions (for full re-parse).
    pub fn clear(&mut self) {
        self.functions.clear();
    }

    /// Remove all entries for a specific file (for incremental re-parse).
    pub fn remove_file(&mut self, file_uri: &str) {
        self.functions.retain(|_, f| f.file_uri != file_uri);
    }

    /// Remove all entries defined in a specific passage (for incremental re-parse).
    pub fn remove_passage(&mut self, passage_name: &str) {
        self.functions.retain(|_, f| f.defined_in != passage_name);
    }

    /// Check if a function name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_key(name)
    }

    /// Get function names for completion (filtered by prefix).
    pub fn completion_names(&self, prefix: &str) -> Vec<String> {
        self.functions
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
    fn register_function_declaration() {
        let mut registry = FunctionRegistry::new();
        registry.register_function(
            "myFunc",
            FunctionKind::Declaration,
            "Scripts",
            "file:///test.tw",
            100,
            Some(2),
        );

        let f = registry.get("myFunc").unwrap();
        assert_eq!(f.name, "myFunc");
        assert_eq!(f.kind, FunctionKind::Declaration);
        assert_eq!(f.param_count, Some(2));
    }

    #[test]
    fn register_arrow_function() {
        let mut registry = FunctionRegistry::new();
        registry.register_function(
            "calculateScore",
            FunctionKind::ArrowFunction,
            "Utils",
            "file:///test.tw",
            200,
            None,
        );

        let f = registry.get("calculateScore").unwrap();
        assert_eq!(f.kind, FunctionKind::ArrowFunction);
    }

    #[test]
    fn remove_file() {
        let mut registry = FunctionRegistry::new();
        registry.register_function(
            "a",
            FunctionKind::Declaration,
            "S1",
            "file:///a.tw",
            0,
            None,
        );
        registry.register_function(
            "b",
            FunctionKind::Declaration,
            "S2",
            "file:///b.tw",
            0,
            None,
        );

        registry.remove_file("file:///a.tw");
        assert!(!registry.contains("a"));
        assert!(registry.contains("b"));
    }

    #[test]
    fn completion_names() {
        let mut registry = FunctionRegistry::new();
        registry.register_function("myFunc", FunctionKind::Declaration, "S", "f", 0, None);
        registry.register_function("myHelper", FunctionKind::ArrowFunction, "S", "f", 0, None);
        registry.register_function("otherFunc", FunctionKind::Declaration, "S", "f", 0, None);

        let names = registry.completion_names("my");
        assert_eq!(names.len(), 2);
    }
}
