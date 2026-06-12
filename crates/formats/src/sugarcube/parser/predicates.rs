//! Helper predicates for the SugarCube parser.

use crate::sugarcube::macros;

/// Check if a character can start an identifier.
pub(super) fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

/// Check if a character can continue an identifier.
pub(super) fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Check if a character can continue a **variable** identifier.
///
/// Unlike `is_ident_char`, this does NOT include `-` because variable
/// names in SugarCube do not contain hyphens. Macro names do (e.g.,
/// `<<link-replace>>`), which is why `is_ident_char` includes it, but
/// `$my-var` is NOT a valid SugarCube variable — it would be `$my`
/// followed by `-var`.
pub(super) fn is_var_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Check if a macro name assigns/writes variables.
pub(crate) fn is_assignment_macro(name: &str) -> bool {
    macros::variable_assignment_macros().contains(name)
}
