//! Workspace-wide custom macro registry.
//!
//! Custom macros defined via `Macro.add('name', { handler: function() { ... } })`
//! in `[script]` passages may be invoked from any passage in any file. This
//! registry accumulates all known custom macro definitions across the workspace
//! so that `walk_encounters()` can classify them as Stateful for variable
//! encounter collection.
//!
//! ## Lifecycle
//!
//! - **Population**: During `parse()`, script passages are scanned for
//!   `Macro.add` definitions. The registry is updated per-file: on each
//!   `parse()` call, all custom macros from that file are refreshed (old
//!   entries for that file are removed, new ones are inserted).
//! - **Query**: Before `walk_encounters()`, the registry's callable names are
//!   merged with the per-file `callables` list to build the full set of
//!   callable names.
//!
//! ## Thread safety
//!
//! Uses `RwLock` for interior mutability since `FormatPlugin` requires
//! `Send + Sync` and its methods take `&self` (not `&mut self`).

use std::collections::{HashMap, HashSet};

use url::Url;

use super::workspace::{UserCallable, UserCallableKind};

// ---------------------------------------------------------------------------
// CustomMacroRegistry
// ---------------------------------------------------------------------------

/// Workspace-wide registry of custom macros defined via `Macro.add()`.
///
/// Accumulates `UserCallable` entries from all files. On each file reparse,
/// the old entries for that file are removed and replaced with the new ones.
/// This ensures that:
///
/// - Custom macros from file A are available when walking passages in
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
    /// This ensures that `walk_encounters()` sees a complete set of callables
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
}
