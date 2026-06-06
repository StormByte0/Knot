//! Unified registry hub for the SugarCube format plugin.
//!
//! This module provides [`SugarCubeRegistry`], the central hub that owns and
//! coordinates all runtime-populated registries for the SugarCube format.
//! It serves as both the implementation detail (format-owned state) and as
//! the **functional template** for how other formats (Harlowe, Snowman,
//! Chapbook) should organize their own registries when they are implemented.
//!
//! ## Registry Categories
//!
//! The SugarCube format tracks five categories of runtime-populated data,
//! each with its own sub-registry:
//!
//! | Category | Registry | Populated by |
//! |----------|----------|-------------|
//! | **Variables** | [`VariableTree`] | `<<set>>`, `$var`, `State.variables.*` |
//! | **Custom Macros** | [`CustomMacroRegistry`] | `<<widget>>`, `Macro.add()` |
//! | **Functions** | [`FunctionRegistry`] | `function`, `const fn = () =>` in `[script]` |
//! | **Templates** | [`TemplateRegistry`] | `Template.add()` in `[script]` |
//!
//! ## Thread Safety
//!
//! Each sub-registry is wrapped in its own [`RwLock`], allowing fine-grained
//! concurrent access. Multiple readers (completion, hover, references) can
//! access different registries simultaneously. Only the parse pipeline needs
//! write locks, and it only locks the registries it needs to update.
//!
//! ## Template for Other Formats
//!
//! When implementing Harlowe, Snowman, or Chapbook, follow this pattern:
//!
//! 1. Identify the format's runtime-populated categories (e.g., Harlowe has
//!    variables, macros, and datatypes; Snowman has `window.*` globals)
//! 2. Create a sub-registry for each category with the standard interface:
//!    - `register_*()` — add entries during parsing
//!    - `remove_file()` / `remove_passage()` — incremental updates
//!    - `clear()` — full re-parse
//!    - `completion_names()` — for IDE completion
//!    - `get()` — for hover/go-to-definition
//! 3. Compose them into a unified `FormatNameRegistry` hub
//! 4. Expose through the `FormatPlugin` trait via registry accessor methods
//!
//! ## Population Order
//!
//! The ordered parse pipeline ensures registries are warm for later passages:
//!
//! ```text
//! [script] passages   → oxc walk   → variables, custom_macros, functions, templates
//! [widget] passages   → SC parser  → custom_macros (widgets), variables
//! Named specials       → SC parser  → variables
//! Normal passages      → SC parser  → variables (can query all registries)
//! ```

pub mod function_registry;
pub mod template_registry;
pub mod variable_tree;
pub mod custom_macros;
pub mod var_extract;
pub mod registry_populate;

use parking_lot::RwLock;
use std::collections::HashSet;

use custom_macros::CustomMacroRegistry;
use variable_tree::VariableTree;
use crate::types::{FormatRegistry, VariableTreeNode};

pub use function_registry::{FunctionEntry, FunctionKind, FunctionRegistry};
pub use template_registry::{TemplateEntry, TemplateKind, TemplateRegistry};

// ---------------------------------------------------------------------------
// SugarCubeRegistry — the unified hub
// ---------------------------------------------------------------------------

/// The unified registry hub for the SugarCube format.
///
/// Owns all sub-registries and provides both fine-grained access (individual
/// read/write guards) and bulk operations (clear/remove for re-parse).
///
/// This struct replaces the previous design where `SugarCubePlugin` held
/// separate `RwLock<VariableTree>` and `RwLock<CustomMacroRegistry>` fields.
/// Consolidating into a hub makes the registry structure explicit, enables
/// coordinated bulk operations, and serves as a template for other formats.
pub struct SugarCubeRegistry {
    /// Side table tracking all `$var` / `_var` references across the workspace.
    variables: RwLock<VariableTree>,
    /// Registry of user-defined macros (widgets and `Macro.add()` calls).
    custom_macros: RwLock<CustomMacroRegistry>,
    /// Registry of JS function definitions found in script passages.
    functions: RwLock<FunctionRegistry>,
    /// Registry of `Template.add()` definitions found in script passages.
    templates: RwLock<TemplateRegistry>,
}

impl Default for SugarCubeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SugarCubeRegistry {
    /// Create a new registry hub with all sub-registries empty.
    pub fn new() -> Self {
        Self {
            variables: RwLock::new(VariableTree::new()),
            custom_macros: RwLock::new(CustomMacroRegistry::new()),
            functions: RwLock::new(FunctionRegistry::new()),
            templates: RwLock::new(TemplateRegistry::new()),
        }
    }

    // ── Individual sub-registry access (read guards) ──────────────────

    /// Get read access to the variable tree.
    pub fn variables(&self) -> parking_lot::RwLockReadGuard<'_, VariableTree> {
        self.variables.read()
    }

    /// Get read access to the custom macro registry.
    pub fn custom_macros(&self) -> parking_lot::RwLockReadGuard<'_, CustomMacroRegistry> {
        self.custom_macros.read()
    }

    /// Get read access to the function registry.
    pub fn functions(&self) -> parking_lot::RwLockReadGuard<'_, FunctionRegistry> {
        self.functions.read()
    }

    /// Get read access to the template registry.
    pub fn templates(&self) -> parking_lot::RwLockReadGuard<'_, TemplateRegistry> {
        self.templates.read()
    }

    // ── Individual sub-registry access (write guards) ─────────────────

    /// Get write access to the variable tree.
    pub fn variables_mut(&self) -> parking_lot::RwLockWriteGuard<'_, VariableTree> {
        self.variables.write()
    }

    /// Get write access to the custom macro registry.
    pub fn custom_macros_mut(&self) -> parking_lot::RwLockWriteGuard<'_, CustomMacroRegistry> {
        self.custom_macros.write()
    }

    /// Get write access to the function registry.
    pub fn functions_mut(&self) -> parking_lot::RwLockWriteGuard<'_, FunctionRegistry> {
        self.functions.write()
    }

    /// Get write access to the template registry.
    pub fn templates_mut(&self) -> parking_lot::RwLockWriteGuard<'_, TemplateRegistry> {
        self.templates.write()
    }

    // ── Bulk operations (coordinated across all sub-registries) ────────

    /// Remove all entries for a specific file from ALL sub-registries.
    ///
    /// Called during full re-parse to clear stale data before re-populating.
    pub fn remove_file(&self, file_uri: &str) {
        self.variables.write().remove_file(file_uri);
        self.custom_macros.write().remove_file(file_uri);
        self.functions.write().remove_file(file_uri);
        self.templates.write().remove_file(file_uri);
    }

    /// Remove all entries for a specific passage from ALL sub-registries.
    ///
    /// Called during incremental single-passage re-parse to clear the old
    /// entries before adding new ones from the re-parsed AST.
    pub fn remove_passage(&self, passage_name: &str) {
        self.variables.write().remove_passage(passage_name);
        self.custom_macros.write().remove_passage(passage_name);
        self.functions.write().remove_passage(passage_name);
        self.templates.write().remove_passage(passage_name);
    }

    /// Clear ALL sub-registries (for full workspace re-parse).
    pub fn clear(&self) {
        self.variables.write().clear();
        self.custom_macros.write().clear();
        self.functions.write().clear();
        self.templates.write().clear();
    }

    // ── Convenience accessors (delegate to sub-registries) ─────────────

    /// Get all workspace variable names for completion.
    pub fn variable_names(&self) -> std::collections::HashSet<String> {
        self.variables.read().completion_names()
    }

    /// Get known property paths for a variable (for dot-notation completion).
    pub fn variable_properties(&self, var_name: &str) -> std::collections::HashSet<String> {
        self.variables
            .read()
            .get_variable(var_name)
            .map(|e| e.known_properties())
            .unwrap_or_default()
    }

    /// Get all custom macro names for completion.
    pub fn custom_macro_names(&self) -> Vec<String> {
        self.custom_macros.read().names().cloned().collect()
    }

    /// Get all function names for completion.
    pub fn function_names(&self) -> Vec<String> {
        self.functions.read().names().cloned().collect()
    }

    /// Get all template names for completion (with `?` prefix).
    pub fn template_completion_names(&self) -> Vec<String> {
        self.templates.read().completion_names()
    }

    /// Build the variable tree for the workspace.
    ///
    /// Before building, computes passage body start positions from the
    /// document cache so that passage-relative line numbers can be
    /// converted to document-absolute line numbers at the output boundary.
    /// Line numbers are computed at record time from passage body text,
    /// so no post-hoc `resolve_line_numbers()` call is needed.
    pub fn build_variable_tree(&self, source_text: &dyn crate::plugin::SourceTextProvider) -> Vec<VariableTreeNode> {
        // Compute passage positions from the source text for relative→absolute conversion.
        let passage_positions = self.compute_passage_positions(source_text);

        self.variables.read().build_tree(&passage_positions)
    }

    /// Compute passage positions for all files referenced in the variable tree.
    ///
    /// Scans each file's source text for `:: PassageName` headers and builds
    /// a map from `(file_uri, passage_name)` to `PassagePosition`. This map
    /// is used at the output boundary to convert passage-relative line numbers
    /// and byte offsets to document-absolute values.
    pub fn compute_passage_positions(&self, source_text: &dyn crate::plugin::SourceTextProvider) -> variable_tree::PassagePositionMap {
        use variable_tree::compute_passage_positions;

        // Collect all unique file URIs from the variable tree
        let file_uris: HashSet<String> = {
            let vtree = self.variables.read();
            let mut uris = HashSet::new();
            for (_, entry) in vtree.iter() {
                collect_file_uris_from_node(&entry.node, &mut uris);
            }
            uris
        };

        // For each file, compute passage positions from the source text
        let mut all_positions = variable_tree::PassagePositionMap::new();
        for file_uri in file_uris {
            if let Some(text) = source_text.get_source_text(&file_uri) {
                let positions = compute_passage_positions(text, &file_uri);
                all_positions.extend(positions);
            }
        }

        all_positions
    }
}

// ---------------------------------------------------------------------------
// FormatRegistry implementation — the template for other formats
// ---------------------------------------------------------------------------

impl FormatRegistry for SugarCubeRegistry {
    fn remove_file(&self, file_uri: &str) {
        self.variables.write().remove_file(file_uri);
        self.custom_macros.write().remove_file(file_uri);
        self.functions.write().remove_file(file_uri);
        self.templates.write().remove_file(file_uri);
    }

    fn remove_passage(&self, passage_name: &str) {
        self.variables.write().remove_passage(passage_name);
        self.custom_macros.write().remove_passage(passage_name);
        self.functions.write().remove_passage(passage_name);
        self.templates.write().remove_passage(passage_name);
    }

    fn clear(&self) {
        self.variables.write().clear();
        self.custom_macros.write().clear();
        self.functions.write().clear();
        self.templates.write().clear();
    }

    fn variable_names(&self) -> std::collections::HashSet<String> {
        self.variables.read().completion_names()
    }

    fn variable_properties(&self, var_name: &str) -> std::collections::HashSet<String> {
        self.variables
            .read()
            .get_variable(var_name)
            .map(|e| e.known_properties())
            .unwrap_or_default()
    }

    fn custom_definition_names(&self) -> Vec<String> {
        self.custom_macros.read().names().cloned().collect()
    }

    fn function_names(&self) -> Vec<String> {
        self.functions.read().names().cloned().collect()
    }

    fn template_names(&self) -> Vec<String> {
        self.templates.read().completion_names()
    }
}

/// Recursively collect all unique file URIs from a node's accesses.
fn collect_file_uris_from_node(node: &variable_tree::VarNode, uris: &mut HashSet<String>) {
    for access in &node.accesses {
        uris.insert(access.file_uri.clone());
    }
    for child in node.children.values() {
        collect_file_uris_from_node(child, uris);
    }
}
