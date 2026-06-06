//! SugarCube AST node types.
//!
//! These are the output types of the recursive descent parser. Every
//! downstream consumer (tokens, diagnostics, links, vars, registries)
//! walks the same AST — no separate regex scanning.
//!
//! ## Design
//!
//! The AST is a **flat node list with optional nesting**. Block macros
//! (`<<if>>...<</if>>`) carry their children inline. This gives the
//! best of both worlds: simple flat iteration for tokens/diagnostics,
//! tree structure for block content extraction.
//!
//! All byte offsets are **relative to the passage body start** (i.e.,
//! byte 0 is the first character after the header line newline). The
//! caller adds `body_offset` to get document-absolute positions.

use std::ops::Range;

// ---------------------------------------------------------------------------
// VarRef — extracted from macro args and text gaps
// ---------------------------------------------------------------------------

/// A variable reference extracted from the AST.
///
/// `$hp` is a story variable, `_i` is a temporary variable.
/// The `is_write` flag is set when the variable appears in a write
/// context (left side of `to`/`=` in `<<set>>`, `<<capture>>`, etc.).
#[derive(Debug, Clone)]
pub struct VarRef {
    /// The variable name including sigil (e.g., `$hp`, `_i`).
    pub name: String,
    /// Dot-notation property path after the base name (e.g., "name" for `$player.name`).
    /// Empty string if no property access.
    pub property_path: String,
    /// Whether this is a temporary variable (`_` sigil).
    pub is_temporary: bool,
    /// Whether this reference is a write/assignment (vs a read).
    pub is_write: bool,
    /// Byte range of the variable reference in the passage body.
    pub span: Range<usize>,
}

// ---------------------------------------------------------------------------
// Comment kind
// ---------------------------------------------------------------------------

/// The kind of a comment block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// Twine block comment: /% ... %/
    Twine,
    /// SugarCube block comment: /%% ... %%/
    SugarCube,
    /// HTML comment: <!-- ... -->
    Html,
    /// C-style block comment: /* ... */
    CStyle,
}

// ---------------------------------------------------------------------------
// Link kind
// ---------------------------------------------------------------------------

/// The kind of a link, determining how its target is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// Simple link: [[Target]]
    Simple,
    /// Pipe link: [[Display|Target]]
    Pipe,
    /// Arrow link: [[Display->Target]]
    ArrowRight,
    /// Left-arrow link: [[Target<-Display]]
    ArrowLeft,
    /// Setter link: [[Target][$var to value]]
    Setter,
}

// ---------------------------------------------------------------------------
// Expression kind
// ---------------------------------------------------------------------------

/// The kind of an inline expression macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprKind {
    /// Print expression: <<=>>
    Print,
    /// Silent expression: <<->>
    Silent,
}

// ---------------------------------------------------------------------------
// AstNode — the core AST node enum
// ---------------------------------------------------------------------------

/// A node in the SugarCube AST.
///
/// Each variant represents a syntactic construct found by the parser.
/// The parser produces a `Vec<AstNode>` for each passage body.
#[derive(Debug, Clone)]
pub enum AstNode {
    /// Plain text content between delimiters.
    ///
    /// May contain `$var` and `_var` references that were extracted
    /// from the text gap — these are inline variable reads.
    Text {
        /// The text content.
        content: String,
        /// Variable references found in this text gap.
        var_refs: Vec<VarRef>,
        /// Byte range in the passage body.
        span: Range<usize>,
    },

    /// A macro invocation: `<<name args>>` or `<<name args>>...<</name>>`.
    ///
    /// Block macros carry their children in `children`. Inline macros
    /// have `children = None`.
    Macro {
        /// The macro name (e.g., "set", "if", "link").
        name: String,
        /// The raw argument string (everything between the name and `>>`).
        args: String,
        /// Variable references found in the args string.
        var_refs: Vec<VarRef>,
        /// For block macros: the child nodes between `<<name>>` and `<</name>>`.
        /// For inline macros: `None`.
        children: Option<Vec<AstNode>>,
        /// Byte range of the opening tag: `<<name args>>`.
        name_span: Range<usize>,
        /// Byte range of the full opening tag including `<<` and `>>`.
        open_span: Range<usize>,
        /// For block macros: byte range of `<</name>>`.
        close_span: Option<Range<usize>>,
        /// Byte range of the entire macro construct (open + body + close).
        full_span: Range<usize>,
    },

    /// An inline expression: `<<=>>expr>>` or `<<->>expr>>`.
    Expression {
        /// Whether this is a print (`=`) or silent (`-`) expression.
        kind: ExprKind,
        /// The expression content between the delimiters.
        content: String,
        /// Variable references found in the expression.
        var_refs: Vec<VarRef>,
        /// Byte range of the entire expression construct.
        span: Range<usize>,
    },

    /// A link: `[[...]]`.
    Link {
        /// Display text (may differ from target).
        display: Option<String>,
        /// Target passage name.
        target: String,
        /// How the link was formatted (pipe, arrow, simple, setter).
        kind: LinkKind,
        /// For setter links: the setter variable name (e.g., `$var`).
        setter_var: Option<String>,
        /// Byte range of the entire link construct.
        span: Range<usize>,
    },

    /// A comment block.
    Comment {
        /// The comment content (without delimiters).
        content: String,
        /// What kind of comment this is.
        kind: CommentKind,
        /// Byte range of the entire comment including delimiters.
        span: Range<usize>,
    },

    /// A parse error — unclosed delimiter, invalid syntax, etc.
    Error {
        /// Human-readable description of the error.
        message: String,
        /// Byte range of the problematic construct.
        span: Range<usize>,
    },
}

// ---------------------------------------------------------------------------
// ParseMode — determines how the parser processes the body
// ---------------------------------------------------------------------------

/// How the parser should process a passage body.
///
/// Different passage categories require different parsing strategies.
/// Script passages contain pure JS (no SugarCube syntax). Stylesheet
/// passages contain CSS (no parsing needed). Normal passages get full
/// SugarCube parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMode {
    /// Full SugarCube parsing (macros, links, vars, comments).
    Normal,
    /// JS-only parsing — body is passed directly to oxc.
    Script,
    /// SugarCube parsing with widget definition extraction.
    Widget,
    /// HTML parsing — only `data-passage` attribute extraction.
    Interface,
    /// Skip parsing entirely (stylesheets).
    Stylesheet,
    /// Minimal parsing — metadata only, no body analysis (StoryData).
    Minimal,
}

// ---------------------------------------------------------------------------
// PassageAst — the output of parsing a passage body
// ---------------------------------------------------------------------------

/// The result of parsing a single passage body.
///
/// Contains the flat AST node list and convenience collections that
/// are extracted during parsing (links, variable operations) so that
/// downstream consumers don't need to re-walk the AST for the most
/// common queries.
#[derive(Debug, Clone)]
pub struct PassageAst {
    /// The AST nodes produced by the parser.
    pub nodes: Vec<AstNode>,
    /// All links found in the passage body.
    pub links: Vec<LinkInfo>,
    /// All variable operations (reads and writes) in source order.
    pub var_ops: Vec<VarOpInfo>,
    /// The parse mode that was used.
    pub mode: ParseMode,
}

/// A link extracted from the AST, in a format-agnostic representation.
#[derive(Debug, Clone)]
pub struct LinkInfo {
    /// Display text (may differ from target).
    pub display: Option<String>,
    /// Target passage name.
    pub target: String,
    /// Byte range of the link in the passage body.
    pub span: Range<usize>,
    /// Whether this link uses a variable target (dynamic navigation).
    pub is_dynamic: bool,
}

/// A variable operation extracted from the AST.
#[derive(Debug, Clone)]
pub struct VarOpInfo {
    /// The variable name including sigil (e.g., `$hp`).
    pub name: String,
    /// Dot-notation property path (e.g., "name" for `$player.name`).
    pub property_path: String,
    /// Whether this is a temporary variable.
    pub is_temporary: bool,
    /// Whether this is a write/assignment.
    pub is_write: bool,
    /// Byte range in the passage body.
    pub span: Range<usize>,
}

impl PassageAst {
    /// Create an empty AST for passages that don't need parsing.
    pub fn empty(mode: ParseMode) -> Self {
        Self {
            nodes: Vec::new(),
            links: Vec::new(),
            var_ops: Vec::new(),
            mode,
        }
    }
}

// ---------------------------------------------------------------------------
// AstWalker — convenience methods for common AST queries
// ---------------------------------------------------------------------------

/// Walk an AST and collect all macros (including nested ones).
pub fn collect_macros(nodes: &[AstNode]) -> Vec<&AstNode> {
    let mut result = Vec::new();
    for node in nodes {
        if let AstNode::Macro { children, .. } = node {
            result.push(node);
            if let Some(ch) = children {
                result.extend(collect_macros(ch));
            }
        }
    }
    result
}

/// Walk an AST and collect all links (including inside macros).
pub fn collect_links(nodes: &[AstNode]) -> Vec<&AstNode> {
    let mut result = Vec::new();
    for node in nodes {
        match node {
            AstNode::Link { .. } => result.push(node),
            AstNode::Macro { children, .. } => {
                if let Some(ch) = children {
                    result.extend(collect_links(ch));
                }
            }
            _ => {}
        }
    }
    result
}

/// Walk an AST and collect all errors (including inside macros).
pub fn collect_errors(nodes: &[AstNode]) -> Vec<&AstNode> {
    let mut result = Vec::new();
    for node in nodes {
        match node {
            AstNode::Error { .. } => result.push(node),
            AstNode::Macro { children, .. } => {
                if let Some(ch) = children {
                    result.extend(collect_errors(ch));
                }
            }
            _ => {}
        }
    }
    result
}

/// Walk an AST and collect JS snippets from `<<script>>`, `<<run>>`, `<<set>>`,
/// `<<=>>` blocks for oxc validation.
pub struct JsSnippet {
    /// The JavaScript source text (after $var preprocessing).
    pub source: String,
    /// Byte offset where this snippet starts in the passage body.
    pub body_offset: usize,
    /// The macro name that contains this JS (e.g., "script", "run", "set").
    pub macro_name: String,
    /// Whether this is a full script block (vs an inline expression).
    pub is_block: bool,
}

/// Collect JS snippets from the AST for oxc validation.
pub fn collect_js_snippets(nodes: &[AstNode]) -> Vec<JsSnippet> {
    let mut result = Vec::new();
    collect_js_snippets_recursive(nodes, &mut result);
    result
}

fn collect_js_snippets_recursive(nodes: &[AstNode], result: &mut Vec<JsSnippet>) {
    /// Macro names that contain inline JS expressions.
    const INLINE_JS_MACROS: &[&str] = &["set", "run", "if", "elseif", "else", "print", "nobr"];
    /// Macro names that contain full JS blocks.
    const BLOCK_JS_MACROS: &[&str] = &["script"];

    for node in nodes {
        if let AstNode::Macro {
            name,
            args,
            open_span,
            children,
            ..
        } = node
        {
            if BLOCK_JS_MACROS.contains(&name.as_str()) {
                // <<script>> blocks: the body is JS
                if let Some(ch) = children {
                    // Collect text content from children as JS source
                    let mut js_source = String::new();
                    let mut body_start = open_span.end;
                    for child in ch {
                        if let AstNode::Text { content, span, .. } = child {
                            if js_source.is_empty() {
                                body_start = span.start;
                            }
                            js_source.push_str(content);
                        }
                    }
                    if !js_source.is_empty() {
                        result.push(JsSnippet {
                            source: js_source,
                            body_offset: body_start,
                            macro_name: name.clone(),
                            is_block: true,
                        });
                    }
                }
            } else if INLINE_JS_MACROS.contains(&name.as_str()) {
                // Inline JS: the args are a JS expression
                let trimmed = args.trim();
                if !trimmed.is_empty() {
                    result.push(JsSnippet {
                        source: trimmed.to_string(),
                        body_offset: open_span.start,
                        macro_name: name.clone(),
                        is_block: false,
                    });
                }
            }

            // Recurse into children
            if let Some(ch) = children {
                collect_js_snippets_recursive(ch, result);
            }
        }

        if let AstNode::Expression { content, kind: _, span, .. } = node {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                result.push(JsSnippet {
                    source: trimmed.to_string(),
                    body_offset: span.start,
                    macro_name: "=".to_string(), // <<=>>
                    is_block: false,
                });
            }
        }
    }
}
