//! SugarCube workspace-level utilities.
//!
//! This module houses SugarCube-specific workspace analyses that operate across
//! all passages in the workspace rather than within a single passage:
//!
//! - **`helpers`**: Low-level text utilities (`strip_comments`, `line_from_offset`)
//!   shared by startup alias extraction, user callable extraction, and other
//!   workspace-wide analyses.
//!
//! - **`startup_aliases`**: Extraction of `State.variables` aliases from
//!   SugarCube script passages (e.g., `var g = gs()`, `var v = State.variables`).
//!
//! - **`user_callables`**: Detection of user-defined callables (custom macros
//!   via `Macro.add()` and widgets via `<<widget>>`) across the workspace.

mod helpers;
mod startup_aliases;
mod user_callables;

pub use helpers::{strip_comments, line_from_offset};
pub use startup_aliases::{extract_startup_aliases, StartupAlias, AliasResolution};
pub use user_callables::{
    extract_user_callables, UserCallable, UserCallableKind, PassageInfo,
};
