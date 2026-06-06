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

/// Check if a macro name is a block macro (has a close tag).
pub(super) fn is_block_macro(name: &str) -> bool {
    // Use the macros module's block_macro_names set
    macros::block_macro_names().contains(name)
        || name.eq_ignore_ascii_case("widget")
        || name.eq_ignore_ascii_case("script")
        || name.eq_ignore_ascii_case("style")
        || name.eq_ignore_ascii_case("css")
        || name.eq_ignore_ascii_case("nobr")
        || name.eq_ignore_ascii_case("silently")
        || name.eq_ignore_ascii_case("done")
        || name.eq_ignore_ascii_case("capture")
}

/// Check if a macro name is a "block modifier" — a clause marker that
/// belongs inside a parent block macro rather than being a standalone block.
///
/// In SugarCube, `<<else>>` and `<<elseif>>` are clause markers within
/// `<<if>>` blocks. They do NOT have their own close tags — they're
/// siblings of the content sections inside `<<if>>...<</if>>`.
/// Similarly, `<<case>>` and `<<default>>` are clause markers within
/// `<<switch>>` blocks.
///
/// The parser treats these as **inline macros** (no `children`, no close
/// tag search) so they don't consume the rest of the passage body looking
/// for a nonexistent `<</else>>`.
pub(super) fn is_block_modifier(name: &str) -> bool {
    macros::folding_modifier_names().contains(name)
}

/// Check if a macro name assigns/writes variables.
pub(super) fn is_assignment_macro(name: &str) -> bool {
    macros::variable_assignment_macros().contains(name)
}
