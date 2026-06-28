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
//! ## Synchronization
//!
//! All sub-registries are stored as plain fields (no interior mutability).
//! The caller must hold exclusive access (`&mut self`) to mutate them.
//! In the LSP server, this is guaranteed by the `tokio::RwLock` on
//! `ServerStateInner` — the server's write lock is the SOLE synchronization
//! mechanism.
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

pub mod custom_macros;
pub mod function_registry;
pub mod registry_populate;
pub mod template_registry;
pub mod var_extract;
pub mod variable_tree;

use std::collections::HashSet;

use crate::types::{FormatRegistry, VariableTreeNode};
use variable_tree::VariableTree;

pub use custom_macros::{CustomMacro, CustomMacroRegistry};
pub use function_registry::{FunctionEntry, FunctionKind, FunctionRegistry};
pub use template_registry::{TemplateEntry, TemplateKind, TemplateRegistry};

// ---------------------------------------------------------------------------
// SugarCubeRegistry — the unified hub
// ---------------------------------------------------------------------------

/// The unified registry hub for the SugarCube format.
///
/// Owns all sub-registries and provides both fine-grained access (individual
/// read/write methods) and bulk operations (clear/remove for re-parse).
///
/// All sub-registries are stored as plain fields — no interior mutability.
/// The caller must hold `&mut self` to mutate, which in the server means
/// holding the write lock on `ServerStateInner`.
pub struct SugarCubeRegistry {
    /// Side table tracking all `$var` / `_var` references across the workspace.
    variables: VariableTree,
    /// Registry of user-defined macros (widgets and `Macro.add()` calls).
    custom_macros: CustomMacroRegistry,
    /// Registry of JS function definitions found in script passages.
    functions: FunctionRegistry,
    /// Registry of `Template.add()` definitions found in script passages.
    templates: TemplateRegistry,
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
            variables: VariableTree::new(),
            custom_macros: CustomMacroRegistry::new(),
            functions: FunctionRegistry::new(),
            templates: TemplateRegistry::new(),
        }
    }

    // ── Individual sub-registry access (read) ──────────────────────────

    /// Get read access to the variable tree.
    pub fn variables(&self) -> &VariableTree {
        &self.variables
    }

    /// Get read access to the custom macro registry.
    pub fn custom_macros(&self) -> &CustomMacroRegistry {
        &self.custom_macros
    }

    /// Get read access to the function registry.
    pub fn functions(&self) -> &FunctionRegistry {
        &self.functions
    }

    /// Get read access to the template registry.
    pub fn templates(&self) -> &TemplateRegistry {
        &self.templates
    }

    // ── Individual sub-registry access (write) ─────────────────────────

    /// Get write access to the variable tree.
    pub fn variables_mut(&mut self) -> &mut VariableTree {
        &mut self.variables
    }

    /// Get write access to the custom macro registry.
    pub fn custom_macros_mut(&mut self) -> &mut CustomMacroRegistry {
        &mut self.custom_macros
    }

    /// Get write access to the function registry.
    pub fn functions_mut(&mut self) -> &mut FunctionRegistry {
        &mut self.functions
    }

    /// Get write access to the template registry.
    pub fn templates_mut(&mut self) -> &mut TemplateRegistry {
        &mut self.templates
    }

    /// Get mutable access to the definition registries (custom macros,
    /// functions, templates) for populate functions that need to write
    /// to multiple registries at once.
    pub fn definition_registries_mut(
        &mut self,
    ) -> (
        &mut CustomMacroRegistry,
        &mut FunctionRegistry,
        &mut TemplateRegistry,
    ) {
        (
            &mut self.custom_macros,
            &mut self.functions,
            &mut self.templates,
        )
    }

    // ── Bulk operations (coordinated across all sub-registries) ────────

    /// Remove all entries for a specific file from ALL sub-registries.
    ///
    /// Called during full re-parse to clear stale data before re-populating.
    pub fn remove_file(&mut self, file_uri: &str) {
        self.variables.remove_file(file_uri);
        self.custom_macros.remove_file(file_uri);
        self.functions.remove_file(file_uri);
        self.templates.remove_file(file_uri);
    }

    /// Remove all entries for a specific passage from ALL sub-registries.
    ///
    /// Called during incremental single-passage re-parse to clear the old
    /// entries before adding new ones from the re-parsed AST.
    pub fn remove_passage(&mut self, passage_name: &str) {
        self.variables.remove_passage(passage_name);
        self.custom_macros.remove_passage(passage_name);
        self.functions.remove_passage(passage_name);
        self.templates.remove_passage(passage_name);
    }

    /// Clear ALL sub-registries (for full workspace re-parse).
    pub fn clear(&mut self) {
        self.variables.clear();
        self.custom_macros.clear();
        self.functions.clear();
        self.templates.clear();
    }

    // ── Convenience accessors (delegate to sub-registries) ─────────────

    /// Get all workspace variable names for completion.
    ///
    /// **Warning:** leaks temporary variables from all passages. Use
    /// [`variable_names_for_passage`] for completion contexts where the
    /// enclosing passage is known.
    ///
    /// [`variable_names_for_passage`]: Self::variable_names_for_passage
    pub fn variable_names(&self) -> std::collections::HashSet<String> {
        self.variables.completion_names()
    }

    /// Get variable names for completion, scoped to a passage.
    ///
    /// Returns all persistent (`$`) vars plus only the temporary (`_`)
    /// vars declared in `passage_name`. When `passage_name` is `None`,
    /// only persistent vars are returned (safe degradation — never
    /// leaks another passage's temps).
    ///
    /// SugarCube `_` variables are passage-scoped at runtime; the
    /// global [`variable_names`] is kept for non-completion callers
    /// (e.g., workspace symbol enumeration) that legitimately want
    /// the full set.
    ///
    /// [`variable_names`]: Self::variable_names
    pub fn variable_names_for_passage(
        &self,
        passage_name: Option<&str>,
    ) -> std::collections::HashSet<String> {
        self.variables.completion_names_for_passage(passage_name)
    }

    /// Get known property paths for a variable (for dot-notation completion).
    pub fn variable_properties(&self, var_name: &str) -> std::collections::HashSet<String> {
        self.variables.known_properties(var_name)
    }

    /// Get all custom macro names for completion.
    pub fn custom_macro_names(&self) -> Vec<String> {
        self.custom_macros.names().cloned().collect()
    }

    /// Get all function names for completion.
    pub fn function_names(&self) -> Vec<String> {
        self.functions.names().cloned().collect()
    }

    /// Get all template names for completion (with `?` prefix).
    pub fn template_completion_names(&self) -> Vec<String> {
        self.templates.completion_names()
    }

    /// Build the variable tree for the workspace.
    ///
    /// Before building, computes passage body start positions from the
    /// document cache so that passage-relative line numbers can be
    /// converted to document-absolute line numbers at the output boundary.
    /// Line numbers are computed at record time from passage body text,
    /// so no post-hoc `resolve_line_numbers()` call is needed.
    pub fn build_variable_tree(
        &self,
        source_text: &dyn crate::plugin::SourceTextProvider,
    ) -> Vec<VariableTreeNode> {
        // Compute passage positions from the source text for relative→absolute conversion.
        let passage_positions = self.compute_passage_positions(source_text);

        self.variables.build_tree(&passage_positions)
    }

    /// Compute passage positions for all files referenced in the variable tree.
    ///
    /// Scans each file's source text for `:: PassageName` headers and builds
    /// a map from `(file_uri, passage_name)` to `PassagePosition`. This map
    /// is used at the output boundary to convert passage-relative line numbers
    /// and byte offsets to document-absolute values.
    pub fn compute_passage_positions(
        &self,
        source_text: &dyn crate::plugin::SourceTextProvider,
    ) -> variable_tree::PassagePositionMap {
        use variable_tree::compute_passage_positions;

        // Collect all unique file URIs from the variable tree
        let file_uris: HashSet<String> = self.variables.collect_file_uris();

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
    fn remove_file(&mut self, file_uri: &str) {
        self.variables.remove_file(file_uri);
        self.custom_macros.remove_file(file_uri);
        self.functions.remove_file(file_uri);
        self.templates.remove_file(file_uri);
    }

    fn remove_passage(&mut self, passage_name: &str) {
        self.variables.remove_passage(passage_name);
        self.custom_macros.remove_passage(passage_name);
        self.functions.remove_passage(passage_name);
        self.templates.remove_passage(passage_name);
    }

    fn clear(&mut self) {
        self.variables.clear();
        self.custom_macros.clear();
        self.functions.clear();
        self.templates.clear();
    }

    fn variable_names(&self) -> std::collections::HashSet<String> {
        self.variables.completion_names()
    }

    fn variable_properties(&self, var_name: &str) -> std::collections::HashSet<String> {
        self.variables.known_properties(var_name)
    }

    fn custom_definition_names(&self) -> Vec<String> {
        self.custom_macros.names().cloned().collect()
    }

    fn function_names(&self) -> Vec<String> {
        self.functions.names().cloned().collect()
    }

    fn template_names(&self) -> Vec<String> {
        self.templates.completion_names()
    }
}
