//! Story format metadata extraction.
//!
//! Each Twine story format ships a `format.js` file whose first statement
//! is a parenthesized object literal containing format metadata:
//!
//! ```js
//! ({
//!     "name": "SugarCube",
//!     "version": "2.37.0",
//!     "description": "...",
//!     "author": "Thomas Michael Edwards",
//!     "image": "icon.svg",
//!     "url": "https://www.motoslave.net/sugarcube/2/",
//!     "license": "BSD-3-Clause",
//!     "source": "https://github.com/tmedwards/sugarcube-2"
//! })
//! ```
//!
//! Some formats also include `setup`, `register`, or other function-typed
//! fields — we ignore those and only extract the string-typed fields that
//! matter for toolchain discovery.
//!
//! ## Architecture
//!
//! This module is the **single source of truth** for "what story formats are
//! installed" inside the Knot toolchain. The LSP server uses it to:
//!
//! 1. Build an in-memory catalog of installed formats by scanning a directory
//!    on disk (no central URL index — the user's local copy is authoritative).
//! 2. Resolve the `--head=<dir>` flag to pass to tweego at build time.
//! 3. Report actionable diagnostics when a project's StoryData references a
//!    format that isn't installed.
//!
//! ## Why parse with oxc instead of regex?
//!
//! Real format.js files include function bodies, comments, and template
//! literals. A naive regex extraction fails on edge cases (escaped quotes
//! in descriptions, multi-line strings, function values that look like
//! strings until you hit the closing brace). Oxc gives us a proper AST
//! walk that handles all of these correctly.
//!
//! Knot already depends on oxc for JS analysis elsewhere (SugarCube's
//! `Macro.add()` extraction, etc.), so we reuse `knot_core::oxc::parse_js()`
//! — no new dependencies.

use knot_core::oxc::ParseMode;
use knot_core::oxc::parse_js;
use oxc_ast::ast::{Expression, ObjectPropertyKind, PropertyKey, Statement};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// FormatMeta — the parsed metadata header
// ---------------------------------------------------------------------------

/// Metadata parsed from a story format's `format.js` file.
///
/// Only the string-typed fields that matter for toolchain discovery are
/// extracted. Function-typed fields (`setup`, `register`, etc.) are
/// ignored — they're runtime code, not metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct FormatMeta {
    /// Format name (e.g. "SugarCube", "Harlowe", "Chapbook", "Snowman").
    pub name: String,
    /// Format version (e.g. "2.37.0").
    pub version: String,
    /// Short description.
    pub description: String,
    /// Author name(s).
    pub author: String,
    /// Image/icon filename (relative to the format directory).
    pub image: String,
    /// Homepage URL.
    pub url: String,
    /// License identifier (e.g. "BSD-3-Clause").
    pub license: String,
    /// Source code URL.
    pub source: String,
}

impl FormatMeta {
    /// Returns true if at least the name and version were extracted.
    ///
    /// Other fields may be empty if the format.js omitted them, but a
    /// useful entry must identify what format and what version it is.
    pub fn is_useful(&self) -> bool {
        !self.name.is_empty() && !self.version.is_empty()
    }
}

// ---------------------------------------------------------------------------
// InstalledFormat — disk-backed format entry
// ---------------------------------------------------------------------------

/// A format directory entry discovered on disk.
///
/// Combines the parsed [`FormatMeta`] with the absolute path to the format
/// directory, so callers can construct `--head=<dir>` arguments for tweego
/// and display the format in the "Configure Story Formats" UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledFormat {
    /// Parsed metadata from `format.js`.
    pub meta: FormatMeta,
    /// Absolute path to the format directory (contains format.js, format.html, etc.).
    pub dir: String,
    /// Name of the format directory (e.g. "sugarcube-2", "harlowe-3").
    pub dir_name: String,
}

// ---------------------------------------------------------------------------
// parse_format_js — extract metadata from a format.js file's content
// ---------------------------------------------------------------------------

/// Parse a `format.js` file's content and extract the metadata header.
///
/// Uses the oxc JS parser to walk the AST and pull out the string-typed
/// fields from the top-level object literal. Function-typed fields like
/// `setup` are ignored.
///
/// # Returns
///
/// - `Ok(FormatMeta)` if a parseable header was found. Individual fields
///   may be empty if the format.js omitted them, but `name` and `version`
///   are required for the result to be considered successful.
/// - `Err(String)` if the file could not be parsed as a format.js header
///   (no top-level object literal, or no name/version fields found).
///
/// # Examples
///
/// ```
/// use knot_formats::format_meta::parse_format_js;
///
/// let content = r#"({
///     "name": "SugarCube",
///     "version": "2.37.0",
///     "description": "The SugarCube story format."
/// })"#;
///
/// let meta = parse_format_js(content).expect("should parse");
/// assert_eq!(meta.name, "SugarCube");
/// assert_eq!(meta.version, "2.37.0");
/// ```
pub fn parse_format_js(content: &str) -> Result<FormatMeta, String> {
    let outcome = parse_js(content, ParseMode::Module);

    let mut meta = FormatMeta::default();

    let found = outcome.with_program(|program| {
        for stmt in &program.body {
            // format.js headers look like: ({ "name": "...", ... })
            // oxc parses this as ExpressionStatement → ParenthesizedExpression → ObjectExpression
            // Some format.js files omit the wrapping parens, in which case
            // oxc parses directly as ExpressionStatement → ObjectExpression.
            let Statement::ExpressionStatement(expr_stmt) = stmt else {
                continue;
            };

            // format.js headers look like: ({ "name": "...", ... })
            // oxc parses this as ExpressionStatement → ParenthesizedExpression → ObjectExpression
            // Some format.js files omit the wrapping parens, in which case
            // oxc parses directly as ExpressionStatement → ObjectExpression.
            let inner_expr: &Expression = match &expr_stmt.expression {
                Expression::ParenthesizedExpression(pe) => &pe.expression,
                _ => &expr_stmt.expression,
            };

            let object_expr = match inner_expr {
                Expression::ObjectExpression(oe) => oe.as_ref(),
                _ => continue,
            };

            // Walk object properties, extracting known string fields.
            for prop in &object_expr.properties {
                let ObjectPropertyKind::ObjectProperty(p) = prop else {
                    continue;
                };

                // Property key: Identifier or StringLiteral (or numeric, etc.)
                let key_name = match &p.key {
                    PropertyKey::Identifier(id) => id.name.as_str().to_string(),
                    PropertyKey::StringLiteral(s) => s.value.as_str().to_string(),
                    PropertyKey::PrivateIdentifier(id) => id.name.as_str().to_string(),
                    _ => continue,
                };

                // Property value: only extract strings.
                // Some formats use template literals for descriptions; we
                // only accept single-quasi templates (no interpolations).
                let value_str = match &p.value {
                    Expression::StringLiteral(s) => s.value.as_str().to_string(),
                    Expression::TemplateLiteral(t) => {
                        if t.quasis.len() == 1 {
                            t.quasis[0].value.raw.as_str().to_string()
                        } else {
                            continue;
                        }
                    }
                    _ => continue,
                };

                match key_name.as_str() {
                    "name" => meta.name = value_str,
                    "version" => meta.version = value_str,
                    "description" => meta.description = value_str,
                    "author" => meta.author = value_str,
                    "image" => meta.image = value_str,
                    "url" => meta.url = value_str,
                    "license" => meta.license = value_str,
                    "source" => meta.source = value_str,
                    _ => {}
                }
            }

            if meta.is_useful() {
                return true;
            }
        }
        false
    });

    if found.unwrap_or(false) {
        Ok(meta)
    } else {
        Err(
            "No format.js header found — file does not contain a top-level object literal with name and version fields"
                .to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// scan_storyformats_dir — discover installed formats on disk
// ---------------------------------------------------------------------------

/// Scan a directory for installed story formats.
///
/// Each immediate subdirectory of `dir` is checked for a `format.js` file.
/// If found, the file is parsed and added to the returned list.
/// Subdirectories without a `format.js` are silently skipped — this matches
/// tweego's own storyformats directory layout.
///
/// This function does NOT recurse — story format layouts are always
/// `<dir>/<format-name>/format.js`. (SugarCube's `sugarcube-2/`, Harlowe's
/// `harlowe-3/`, etc.)
///
/// # Errors
///
/// Returns an empty vec if the directory does not exist or cannot be read.
/// Individual format.js parse failures are logged via `tracing::warn!` and
/// skipped — one broken format file does not prevent the others from being
/// discovered.
pub fn scan_storyformats_dir(dir: &std::path::Path) -> Vec<InstalledFormat> {
    let mut formats = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::debug!(
                "storyformats dir {} could not be read: {}",
                dir.display(),
                err
            );
            return formats;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let format_js_path = path.join("format.js");
        if !format_js_path.exists() {
            continue;
        }

        let content = match std::fs::read_to_string(&format_js_path) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!(
                    "Failed to read format.js at {}: {}",
                    format_js_path.display(),
                    err
                );
                continue;
            }
        };

        match parse_format_js(&content) {
            Ok(meta) => {
                let dir_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                tracing::debug!(
                    "Discovered story format: {} v{} at {}",
                    meta.name,
                    meta.version,
                    path.display()
                );
                formats.push(InstalledFormat {
                    meta,
                    dir: path.to_string_lossy().to_string(),
                    dir_name,
                });
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to parse format.js at {}: {}",
                    format_js_path.display(),
                    e
                );
            }
        }
    }

    // Sort by name then version for stable display in the UI.
    formats.sort_by(|a, b| {
        a.meta
            .name
            .cmp(&b.meta.name)
            .then_with(|| a.meta.version.cmp(&b.meta.version))
    });

    formats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SUGARCUBE: &str = r#"({
    "name": "SugarCube",
    "version": "2.37.0",
    "description": "The SugarCube story format.",
    "author": "Thomas Michael Edwards",
    "image": "icon.svg",
    "url": "https://www.motoslave.net/sugarcube/2/",
    "license": "BSD-3-Clause",
    "source": "https://github.com/tmedwards/sugarcube-2",
    "setup": function() {
        // this should be ignored by the parser
        return { foo: "bar" };
    }
})"#;

    const SAMPLE_HARLOWE: &str = r#"({
    "name": "Harlowe",
    "version": "3.3.8",
    "description": "The default story format for Twine 2.",
    "author": "Chris Klimas",
    "image": "icon.svg",
    "url": "https://twinejs.neocities.org/",
    "license": "BSD-3-Clause",
    "source": "https://foss.hkdfngsd.com/gti/harlowe"
})"#;

    const SAMPLE_CHAPBOOK: &str = r#"({
    "name": "Chapbook",
    "version": "1.2.1",
    "description": "A story format for Twine 2.",
    "author": "Chris Klimas",
    "image": "icon.svg",
    "url": "https://klembot.github.io/chapbook/",
    "license": "MIT",
    "source": "https://github.com/klembot/chapbook"
})"#;

    const SAMPLE_SNOWMAN: &str = r#"({
    "name": "Snowman",
    "version": "2.0.2",
    "description": "A minimal story format that lets you write Twine stories using Markdown and Underscore template syntax.",
    "author": "Chris Klimas",
    "image": "icon.svg",
    "url": "https://twinelab.net/snowman/2",
    "license": "MIT",
    "source": "https://github.com/klembot/snowman"
})"#;

    #[test]
    fn test_parse_sugarcube_format() {
        let meta = parse_format_js(SAMPLE_SUGARCUBE).expect("should parse SugarCube");
        assert_eq!(meta.name, "SugarCube");
        assert_eq!(meta.version, "2.37.0");
        assert_eq!(meta.author, "Thomas Michael Edwards");
        assert_eq!(meta.license, "BSD-3-Clause");
        assert_eq!(meta.source, "https://github.com/tmedwards/sugarcube-2");
        assert_eq!(meta.url, "https://www.motoslave.net/sugarcube/2/");
        assert!(meta.is_useful());
    }

    #[test]
    fn test_parse_harlowe_format() {
        let meta = parse_format_js(SAMPLE_HARLOWE).expect("should parse Harlowe");
        assert_eq!(meta.name, "Harlowe");
        assert_eq!(meta.version, "3.3.8");
        assert!(meta.is_useful());
    }

    #[test]
    fn test_parse_chapbook_format() {
        let meta = parse_format_js(SAMPLE_CHAPBOOK).expect("should parse Chapbook");
        assert_eq!(meta.name, "Chapbook");
        assert_eq!(meta.version, "1.2.1");
        assert_eq!(meta.license, "MIT");
        assert!(meta.is_useful());
    }

    #[test]
    fn test_parse_snowman_format() {
        let meta = parse_format_js(SAMPLE_SNOWMAN).expect("should parse Snowman");
        assert_eq!(meta.name, "Snowman");
        assert_eq!(meta.version, "2.0.2");
        assert!(meta.is_useful());
    }

    #[test]
    fn test_parse_empty_file() {
        let result = parse_format_js("// no content here\n");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_no_object_literal() {
        let result = parse_format_js("var x = 1;");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_object_without_name() {
        // Object literal exists but missing name field — should fail.
        let result = parse_format_js(r#"({ "version": "1.0" })"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_object_without_version() {
        // Object literal exists but missing version field — should fail.
        let result = parse_format_js(r#"({ "name": "Test" })"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_minimal_header() {
        let result = parse_format_js(r#"({ "name": "Test", "version": "1.0" })"#);
        let meta = result.expect("should parse minimal header");
        assert_eq!(meta.name, "Test");
        assert_eq!(meta.version, "1.0");
        assert!(meta.is_useful());
    }

    #[test]
    fn test_parse_with_comments_before() {
        let result = parse_format_js(
            r#"// Copyright (c) 2024
// Some license text
({
    "name": "Test",
    "version": "1.0"
})"#,
        );
        let meta = result.expect("should parse with leading comments");
        assert_eq!(meta.name, "Test");
    }

    #[test]
    fn test_scan_empty_dir() {
        // Create a temp dir with no formats.
        let temp = tempfile::tempdir().expect("create temp dir");
        let formats = scan_storyformats_dir(temp.path());
        assert!(formats.is_empty());
    }

    #[test]
    fn test_scan_dir_with_one_format() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let format_dir = temp.path().join("sugarcube-2");
        std::fs::create_dir_all(&format_dir).unwrap();
        std::fs::write(format_dir.join("format.js"), SAMPLE_SUGARCUBE).unwrap();

        let formats = scan_storyformats_dir(temp.path());
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].meta.name, "SugarCube");
        assert_eq!(formats[0].meta.version, "2.37.0");
        assert_eq!(formats[0].dir_name, "sugarcube-2");
        assert!(formats[0].dir.ends_with("sugarcube-2"));
    }

    #[test]
    fn test_scan_dir_skips_dirs_without_format_js() {
        let temp = tempfile::tempdir().expect("create temp dir");

        // Subdir with format.js
        let with_format = temp.path().join("sugarcube-2");
        std::fs::create_dir_all(&with_format).unwrap();
        std::fs::write(with_format.join("format.js"), SAMPLE_SUGARCUBE).unwrap();

        // Subdir without format.js
        let without_format = temp.path().join("empty-dir");
        std::fs::create_dir_all(&without_format).unwrap();

        // Random non-directory file
        std::fs::write(temp.path().join("README.txt"), "hello").unwrap();

        let formats = scan_storyformats_dir(temp.path());
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].meta.name, "SugarCube");
    }

    #[test]
    fn test_scan_nonexistent_dir_returns_empty() {
        let formats = scan_storyformats_dir(std::path::Path::new(
            "/nonexistent/path/that/does/not/exist",
        ));
        assert!(formats.is_empty());
    }
}
