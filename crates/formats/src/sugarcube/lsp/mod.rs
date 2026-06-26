//! LSP feature support — syntax detection, semantic token building,
//! and structured pipeline logging.

pub mod pipeline_log;
pub mod syntax_detect;
pub mod token_builder;

#[cfg(test)]
mod debug_tokens_test;

#[cfg(test)]
mod h1_audit_test;
