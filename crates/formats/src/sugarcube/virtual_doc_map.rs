//! Per-passage virtual document map.
//!
//! Each passage is stored as an independent entry containing its translated JS
//! function and exact line mapping. The monolithic virtual document is assembled
//! on demand from individual entries — never stored as a monolith.
//!
//! ## Incremental updates
//!
//! - **Body edit** (name unchanged): Overwrite `passages[name]` with new entry.
//! - **Rename**: Remove old key, insert new key, regenerate function with new name.
//! - **Delete**: Remove entry by name from `passages` and `file_passages`.
//! - **Add**: Insert new entry.
//! - **File-level invalidation**: Remove all passages for that URI, reprocess.

use std::collections::{HashMap, HashSet};

use url::Url;

use super::passage_tree::ExactLineMapping;

// ---------------------------------------------------------------------------
// PassageDocEntry
// ---------------------------------------------------------------------------

/// A single passage's virtual doc entry.
///
/// Stores the translated JS function (including the `function passage_Name() {`
/// wrapper) and the per-line mapping back to source positions.
#[derive(Debug, Clone)]
pub(crate) struct PassageDocEntry {
    /// The source file URI where this passage originates.
    pub source_file: Url,
    /// Whether this passage is a widget definition (tagged [widget]).
    /// Widget entries are placed before passage functions in assembly.
    pub is_widget: bool,
    /// Whether this passage is a script passage (tagged [script] or named
    /// "Story JavaScript"). Script passages contain raw JavaScript and are
    /// emitted as-is in the virtual doc (wrapped in a function for scoping).
    /// Script entries are assembled before widget functions in the monolithic
    /// virtual doc, since they define custom macros that other passages call.
    pub is_script: bool,
    /// The complete JS function string, including the wrapper.
    /// E.g., `function passage_Start() {\n  State.variables.gold = 100;\n}\n`
    /// or for widgets: `function myWidget() {\n  State.variables.gold -= 10;\n}\n`
    /// or for scripts: `function script_MyScript() {\n  ...raw JS...\n}\n`
    pub js_function: String,
    /// Per-line mapping from JS output lines back to source positions.
    /// Each entry maps one line of `js_function` to the original source line
    /// within the passage body (offset from passage header, NOT global).
    pub line_map: Vec<ExactLineMapping>,
}

// ---------------------------------------------------------------------------
// VirtualDocMap
// ---------------------------------------------------------------------------

/// Per-passage virtual document map.
///
/// Stores one `PassageDocEntry` per passage, keyed by passage name.
/// Provides surgical update methods for incremental processing and
/// on-demand assembly of the monolithic virtual doc.
#[derive(Debug, Clone, Default)]
pub(crate) struct VirtualDocMap {
    /// Passage name → its virtual doc entry (JS function + line mapping).
    passages: HashMap<String, PassageDocEntry>,
    /// Source file URI → passage names in that file.
    /// Used for file-level invalidation (remove all passages for a URI).
    file_passages: HashMap<Url, Vec<String>>,
    /// Set of passage names that are widget definitions.
    /// Widget entries are assembled before passage functions in the
    /// monolithic virtual doc.
    widget_passages: HashSet<String>,
    /// Set of passage names that are script passages.
    /// Script entries are assembled before widget functions in the
    /// monolithic virtual doc, since they define custom macros that
    /// other passages call.
    script_passages: HashSet<String>,
}

#[allow(dead_code)] // API surface for Phase C (incremental updates, VSCode wiring)
impl VirtualDocMap {
    /// Create a new, empty VirtualDocMap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a passage's virtual doc entry.
    ///
    /// If the passage name already exists, the old entry is overwritten
    /// (body edit scenario). The `file_passages` index is updated if the
    /// source file has changed.
    pub fn update_passage(&mut self, name: String, entry: PassageDocEntry) {
        // If this passage already existed under a different file, remove
        // it from the old file's passage list first.
        // Clone the old source_file before the mutable borrow to satisfy
        // the borrow checker (cannot hold an immutable reference from .get()
        // while calling &mut self methods).
        let old_file = self.passages.get(&name).map(|e| e.source_file.clone());
        if let Some(old_uri) = old_file {
            if old_uri != entry.source_file {
                self.remove_from_file_index(&name, &old_uri);
            }
        }

        // Update the file_passages index
        self.file_passages
            .entry(entry.source_file.clone())
            .or_default()
            .push(name.clone());

        // Update the widget_passages set
        if entry.is_widget {
            self.widget_passages.insert(name.clone());
        } else {
            self.widget_passages.remove(&name);
        }

        // Update the script_passages set
        if entry.is_script {
            self.script_passages.insert(name.clone());
        } else {
            self.script_passages.remove(&name);
        }

        // Insert/replace the entry
        self.passages.insert(name, entry);
    }

    /// Remove a passage by name.
    ///
    /// Removes from `passages`, `file_passages`, and `widget_passages`.
    /// Returns true if the passage existed and was removed.
    pub fn remove_passage(&mut self, name: &str) -> bool {
        if let Some(entry) = self.passages.remove(name) {
            self.remove_from_file_index(name, &entry.source_file);
            self.widget_passages.remove(name);
            self.script_passages.remove(name);
            true
        } else {
            false
        }
    }

    /// Remove all passages originating from a given file URI.
    ///
    /// Used for file-level invalidation when an entire .tw file is
    /// reprocessed. Returns the number of passages removed.
    pub fn remove_file(&mut self, uri: &Url) -> usize {
        if let Some(names) = self.file_passages.remove(uri) {
            let count = names.len();
            for name in &names {
                self.passages.remove(name);
                self.widget_passages.remove(name);
                self.script_passages.remove(name);
            }
            count
        } else {
            0
        }
    }

    /// Look up a passage's virtual doc entry by name.
    pub fn get_passage(&self, name: &str) -> Option<&PassageDocEntry> {
        self.passages.get(name)
    }

    /// Get the number of passages in the map.
    pub fn len(&self) -> usize {
        self.passages.len()
    }

    /// Check if the map is empty.
    pub fn is_empty(&self) -> bool {
        self.passages.is_empty()
    }

    /// Get all passage names in the map.
    pub fn passage_names(&self) -> impl Iterator<Item = &String> {
        self.passages.keys()
    }

    /// Get all passage names for a given file URI.
    pub fn passages_for_file(&self, uri: &Url) -> &[String] {
        self.file_passages.get(uri).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Assemble the monolithic virtual document on demand.
    ///
    /// The assembly order is:
    /// 1. Static preamble (`State.variables` ambient declaration)
    /// 2. Widget functions (standalone, no `passage_` wrapper)
    /// 3. Passage functions (wrapped in `function passage_Name() { ... }`)
    ///
    /// This is called only when VSCode requests the virtual doc content.
    /// The result is never stored — it's rebuilt from individual entries.
    pub fn assemble_virtual_doc(&self) -> String {
        let mut doc = String::with_capacity(self.passages.len() * 256);

        // 1. Static preamble
        doc.push_str(
            "/** @type {{ variables: Record<string, any> }} */\n\
             const State = { variables: {} };\n\n",
        );

        // 2. Script passages first (define custom macros, aliases, setup)
        //    These must come before everything else because other passages
        //    reference the functions/macros defined here.
        let mut script_names: Vec<&String> = self.script_passages.iter().collect();
        script_names.sort();
        for name in script_names {
            if let Some(entry) = self.passages.get(name) {
                doc.push_str(&entry.js_function);
                doc.push_str("\n\n");
            }
        }

        // 3. Widget functions (workspace-global scope, may use macros from scripts)
        for name in &self.widget_passages {
            if let Some(entry) = self.passages.get(name) {
                doc.push_str(&entry.js_function);
                doc.push_str("\n\n");
            }
        }

        // 4. Passage functions (sorted for deterministic output)
        let mut passage_names: Vec<&String> = self
            .passages
            .keys()
            .filter(|n| !self.widget_passages.contains(*n) && !self.script_passages.contains(*n))
            .collect();
        passage_names.sort();

        for name in passage_names {
            if let Some(entry) = self.passages.get(name) {
                doc.push_str(&entry.js_function);
                doc.push_str("\n\n");
            }
        }

        doc
    }

    /// Assemble a mapping from virtual doc line numbers to source positions.
    ///
    /// Returns a flat `Vec<ExactLineMapping>` that covers the entire
    /// assembled virtual doc. Preamble lines map to line 0 (sentinel).
    /// Widget function lines come first, then passage function lines.
    ///
    /// This is used to route JS diagnostics from VSCode back to the
    /// correct source position in .tw files.
    pub fn assemble_line_map(&self) -> Vec<ExactLineMapping> {
        let mut map = Vec::new();

        // Preamble lines (2 lines: JSDoc + const declaration)
        // These map to a sentinel (line 0, byte 0) since they have no
        // source correspondence.
        map.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: 0,
        });
        map.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: 0,
        });
        // Blank line after preamble
        map.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: 0,
        });

        // Script passage line maps (sorted, before widgets)
        let mut script_names: Vec<&String> = self.script_passages.iter().collect();
        script_names.sort();
        for name in &script_names {
            if let Some(entry) = self.passages.get(*name) {
                map.extend(entry.line_map.iter().cloned());
            }
            // Two blank lines after each function
            map.push(ExactLineMapping {
                original_line: 0,
                original_start_byte: 0,
            });
            map.push(ExactLineMapping {
                original_line: 0,
                original_start_byte: 0,
            });
        }

        // Widget function line maps
        for name in &self.widget_passages {
            if let Some(entry) = self.passages.get(name) {
                map.extend(entry.line_map.iter().cloned());
            }
            // Two blank lines after each function
            map.push(ExactLineMapping {
                original_line: 0,
                original_start_byte: 0,
            });
            map.push(ExactLineMapping {
                original_line: 0,
                original_start_byte: 0,
            });
        }

        // Passage function line maps (sorted for deterministic output)
        let mut passage_names: Vec<&String> = self
            .passages
            .keys()
            .filter(|n| !self.widget_passages.contains(*n) && !self.script_passages.contains(*n))
            .collect();
        passage_names.sort();

        for name in passage_names {
            if let Some(entry) = self.passages.get(name) {
                map.extend(entry.line_map.iter().cloned());
            }
            // Two blank lines after each function
            map.push(ExactLineMapping {
                original_line: 0,
                original_start_byte: 0,
            });
            map.push(ExactLineMapping {
                original_line: 0,
                original_start_byte: 0,
            });
        }

        map
    }

    /// Verify that the assembled virtual doc's line count matches the
    /// assembled line map's entry count.
    ///
    /// This is a critical invariant: the downstream consumer indexes the
    /// line map by virtual doc line number, so every line in the assembled
    /// doc must have exactly one corresponding entry in the line map.
    /// Returns `Ok(())` if the invariant holds, or `Err(message)` with
    /// diagnostic details.
    pub fn verify_line_map_invariant(&self) -> Result<(), String> {
        let doc = self.assemble_virtual_doc();
        let map = self.assemble_line_map();
        let doc_lines = doc.lines().count();
        if doc_lines != map.len() {
            return Err(format!(
                "Virtual doc line count ({}) != line_map length ({}). \
                 Every virtual doc line must have a corresponding line_map entry.",
                doc_lines, map.len(),
            ));
        }
        Ok(())
    }

    // ── Internal helpers ──────────────────────────────────────────────

    /// Remove a passage name from the file_passages index.
    fn remove_from_file_index(&mut self, name: &str, uri: &Url) {
        if let Some(names) = self.file_passages.get_mut(uri) {
            names.retain(|n| n != name);
            if names.is_empty() {
                self.file_passages.remove(uri);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_url(path: &str) -> Url {
        Url::parse(&format!("file:///{}", path)).unwrap()
    }

    fn make_entry(name: &str, file: &str, is_widget: bool) -> PassageDocEntry {
        PassageDocEntry {
            source_file: make_url(file),
            is_widget,
            is_script: false,
            js_function: format!(
                "function {}() {{\n  /* body */;\n}}\n",
                if is_widget { name.to_string() } else { format!("passage_{}", name) }
            ),
            line_map: vec![
                ExactLineMapping { original_line: 0, original_start_byte: 0 },
                ExactLineMapping { original_line: 1, original_start_byte: 10 },
                ExactLineMapping { original_line: 0, original_start_byte: 0 },
            ],
        }
    }

    #[test]
    fn test_update_and_get() {
        let mut map = VirtualDocMap::new();
        let entry = make_entry("Start", "story.tw", false);
        map.update_passage("Start".to_string(), entry);

        assert!(map.get_passage("Start").is_some());
        assert!(map.get_passage("Nonexistent").is_none());
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_remove_passage() {
        let mut map = VirtualDocMap::new();
        map.update_passage("Start".to_string(), make_entry("Start", "story.tw", false));
        assert!(map.remove_passage("Start"));
        assert!(map.get_passage("Start").is_none());
        assert!(map.is_empty());
        assert!(!map.remove_passage("Start")); // already removed
    }

    #[test]
    fn test_remove_file() {
        let mut map = VirtualDocMap::new();
        let url = make_url("story.tw");
        map.update_passage("Start".to_string(), make_entry("Start", "story.tw", false));
        map.update_passage("Shop".to_string(), make_entry("Shop", "story.tw", false));
        map.update_passage("Other".to_string(), make_entry("Other", "other.tw", false));

        assert_eq!(map.remove_file(&url), 2);
        assert!(map.get_passage("Start").is_none());
        assert!(map.get_passage("Shop").is_none());
        assert!(map.get_passage("Other").is_some());
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_widget_tracking() {
        let mut map = VirtualDocMap::new();
        map.update_passage("myWidget".to_string(), make_entry("myWidget", "widgets.tw", true));
        map.update_passage("Start".to_string(), make_entry("Start", "story.tw", false));

        assert!(map.widget_passages.contains("myWidget"));
        assert!(!map.widget_passages.contains("Start"));
    }

    #[test]
    fn test_assemble_virtual_doc() {
        let mut map = VirtualDocMap::new();
        map.update_passage("Start".to_string(), make_entry("Start", "story.tw", false));
        map.update_passage("myWidget".to_string(), make_entry("myWidget", "widgets.tw", true));
        map.update_passage("Shop".to_string(), make_entry("Shop", "story.tw", false));

        let doc = map.assemble_virtual_doc();

        // Preamble should be present
        assert!(doc.contains("const State = { variables: {} };"));

        // Widget should come before passage functions
        let widget_pos = doc.find("function myWidget()").unwrap();
        let start_pos = doc.find("function passage_Start()").unwrap();
        let shop_pos = doc.find("function passage_Shop()").unwrap();

        assert!(widget_pos < shop_pos, "Widget should appear before passage functions");
        // Alphabetically: "Shop" < "Start", so passage_Shop comes first
        assert!(shop_pos < start_pos, "Passages should be sorted alphabetically (Shop < Start)");
    }

    #[test]
    fn test_overwrite_passage() {
        let mut map = VirtualDocMap::new();
        map.update_passage("Start".to_string(), make_entry("Start", "story.tw", false));
        assert_eq!(map.get_passage("Start").unwrap().js_function, "function passage_Start() {\n  /* body */;\n}\n");

        // Overwrite with new content
        let mut new_entry = make_entry("Start", "story.tw", false);
        new_entry.js_function = "function passage_Start() {\n  State.variables.x = 42;\n}\n".to_string();
        map.update_passage("Start".to_string(), new_entry);

        assert_eq!(map.get_passage("Start").unwrap().js_function, "function passage_Start() {\n  State.variables.x = 42;\n}\n");
        assert_eq!(map.len(), 1); // No duplicate
    }

    #[test]
    fn test_passage_rename_scenario() {
        let mut map = VirtualDocMap::new();
        map.update_passage("OldName".to_string(), make_entry("OldName", "story.tw", false));

        // Rename: remove old, insert new
        map.remove_passage("OldName");
        map.update_passage("NewName".to_string(), make_entry("NewName", "story.tw", false));

        assert!(map.get_passage("OldName").is_none());
        assert!(map.get_passage("NewName").is_some());
    }

    #[test]
    fn test_widget_to_non_widget_transition() {
        let mut map = VirtualDocMap::new();
        map.update_passage("myWidget".to_string(), make_entry("myWidget", "widgets.tw", true));
        assert!(map.widget_passages.contains("myWidget"));

        // Same passage name, now not a widget
        map.update_passage("myWidget".to_string(), make_entry("myWidget", "story.tw", false));
        assert!(!map.widget_passages.contains("myWidget"));
    }

    // ── Pipeline invariant tests ──────────────────────────────────────

    /// Test the critical invariant: js_function line count == line_map length.
    ///
    /// This is the #1 source of pipeline bugs. If these counts don't match,
    /// the downstream consumer (assemble_line_map / assemble_annotated_line_map)
    /// produces incorrect indices, and VSCode diagnostics get mapped to the
    /// wrong lines in .tw files.
    #[test]
    fn test_per_passage_line_count_invariant() {
        // Every entry produced by make_entry has 3 lines in js_function
        // and 3 entries in line_map — this should hold for all entries.
        let entry = make_entry("Test", "test.tw", false);
        let js_lines = entry.js_function.lines().count();
        assert_eq!(
            js_lines, entry.line_map.len(),
            "js_function has {} lines but line_map has {} entries",
            js_lines, entry.line_map.len(),
        );
    }

    /// Test the assembled virtual doc line count == assembled line_map length.
    ///
    /// When multiple passages are assembled into the monolithic virtual doc,
    /// the preamble + passage functions + separator lines must produce exactly
    /// the same number of entries in the assembled line map.
    #[test]
    fn test_assembled_line_map_invariant() {
        let mut map = VirtualDocMap::new();
        map.update_passage("Start".to_string(), make_entry("Start", "story.tw", false));
        map.update_passage("myWidget".to_string(), make_entry("myWidget", "widgets.tw", true));
        map.update_passage("Shop".to_string(), make_entry("Shop", "story.tw", false));

        // Verify the invariant
        map.verify_line_map_invariant()
            .expect("Virtual doc line count must match line_map length");
    }
}
