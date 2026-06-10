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
// JsAnalysis — oxc-derived analysis attached to AST nodes
// ---------------------------------------------------------------------------

/// JS analysis result, attached to AST nodes that contain JS.
///
/// Produced by Phase 2 (JS annotation pass) and consumed by Phase 3
/// (registry population). This replaces the old dual-path variable
/// extraction where both `scan_inline_vars` and `js_walk` would
/// independently produce variable entries.
///
/// A single `JsAnalysis` on a node is the **single source of truth** for
/// all JS-derived information in that node — variable operations, macro
/// definitions, template registrations, and function declarations.
#[derive(Debug, Clone, Default)]
pub struct JsAnalysis {
    /// Variable operations found by oxc.
    pub var_ops: Vec<AnalyzedVarOp>,
    /// `Macro.add()` calls found.
    pub macro_adds: Vec<MacroAddInfo>,
    /// `Template.add()` calls found.
    pub template_adds: Vec<TemplateAddInfo>,
    /// Function declarations/expressions found.
    pub function_defs: Vec<FunctionDefInfo>,
}

/// A variable operation extracted by the oxc AST walker.
///
/// Unlike `VarRef` (which is produced by the simple `scan_inline_vars`
/// scanner), `AnalyzedVarOp` comes from full JS AST analysis and
/// correctly classifies reads vs writes from assignment position.
/// SugarCube semantic overrides (CompoundWrite, Capture, Unset) are
/// applied during Phase 3 registry population, not here.
#[derive(Debug, Clone)]
pub struct AnalyzedVarOp {
    /// Variable name with sigil: "$hp", "_items", "$ITEMS"
    pub name: String,
    /// Whether this is a temporary variable (_ sigil).
    pub is_temporary: bool,
    /// The access kind (Read, Write, CompoundWrite, etc.).
    ///
    /// oxc determines Read vs Write from assignment position.
    /// SugarCube semantic overrides (CompoundWrite, Capture, Unset)
    /// are applied during Phase 3.
    pub access_kind: super::registries::variable_tree::VarAccessKind,
    /// Byte range in the passage body (passage-body-relative).
    pub span: Range<usize>,
    /// Dot-notation property path (e.g., "name" for $player.name).
    pub property_path: String,
    /// Per-segment spans for each path component, enabling "Go to Definition"
    /// to navigate to the exact property token rather than the whole expression.
    pub segment_spans: Vec<Range<usize>>,
    /// The span of the full construct at the root variable's focus level.
    ///
    /// For object literal property writes like `<<set $foo = {bar: 1, baz: 2}`>>,
    /// `span` covers just the property key (e.g., `bar`), but `construct_span`
    /// covers the entire `{...}` expression.
    pub construct_span: Option<Range<usize>>,
}

/// Information about a `Macro.add()` call found in JS.
#[derive(Debug, Clone)]
pub struct MacroAddInfo {
    /// The macro name being registered.
    pub name: String,
    /// Byte offset of the name argument in the passage body.
    pub name_offset: usize,
}

/// Information about a `Template.add()` call found in JS.
#[derive(Debug, Clone)]
pub struct TemplateAddInfo {
    /// The template name being registered.
    pub name: String,
    /// Byte offset of the name argument in the passage body.
    pub name_offset: usize,
    /// Whether this template is a string template or function template.
    pub is_string: bool,
}

/// Information about a function declaration/expression found in JS.
#[derive(Debug, Clone)]
pub struct FunctionDefInfo {
    /// The function name.
    pub name: String,
    /// Byte offset of the function name in the passage body.
    pub name_offset: usize,
    /// Number of parameters, if known.
    pub param_count: Option<usize>,
}

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
///
/// SugarCube/Twine projects can contain almost every variety of comment
/// from HTML, CSS, and JS — all of which must be excluded from analysis
/// (variable extraction, link extraction, macro parsing) to avoid false
/// positives. The parser recognizes all of these in passage body text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// Twine block comment: /% ... %/
    Twine,
    /// SugarCube block comment: /%% ... %%/
    SugarCube,
    /// HTML comment: <!-- ... -->
    Html,
    /// C-style block comment: /* ... */ (CSS and JS)
    CStyle,
    /// JavaScript single-line comment: // ... (to end of line)
    JsLine,
    /// HTML conditional comment: <!--[if ...]>...<![endif]-->
    HtmlConditional,
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
// Link source — where a link came from (for edge type classification)
// ---------------------------------------------------------------------------

/// The source context of a link, used to determine its graph edge type.
///
/// SugarCube has multiple ways to reference passages: `[[ ]]` links, navigation
/// macros (`<<goto>>`, `<<include>>`, `<<link>>`, `<<button>>`), the `<<actions>>`
/// macro, and `data-passage` HTML attributes in StoryInterface. Each source
/// maps to a different semantic edge type in the passage graph.
///
/// The graph engine uses `edge_type_hint` from `Passage.links` directly when
/// set, and only falls back to `classify_edge()` when the hint is `None`.
/// By setting the hint at extraction time, we avoid the post-hoc substring
/// matching approach that can produce false positives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSource {
    /// Standard `[[ ]]` passage link — Navigation edge type.
    PassageLink,
    /// `<<goto>>` macro — Jump edge type (unconditional redirect).
    Goto,
    /// `<<include>>` macro — Include edge type (passage inclusion).
    Include,
    /// `<<link>>` or `<<button>>` macro — Navigation edge type (player choice).
    NavigationMacro,
    /// `<<actions>>` macro — Navigation edge type (player choice list).
    Actions,
    /// `<<return>>` macro — Navigation edge type.
    Return,
    /// `<<back>>` macro — Navigation edge type.
    Back,
    /// `data-passage` HTML attribute — Navigation edge type.
    DataPassage,
    /// Widget invocation (`<<myWidget>>`) — Call edge type.
    /// Detected post-parse by checking the custom macro registry.
    WidgetCall,
}

impl LinkSource {
    /// Convert this link source to the corresponding graph edge type.
    ///
    /// This is the canonical mapping from SugarCube link sources to graph
    /// edge types. The `link_source_to_edge_type()` function in `mod.rs`
    /// delegates to this method.
    pub fn to_edge_type(self) -> knot_core::graph::EdgeType {
        match self {
            LinkSource::PassageLink => knot_core::graph::EdgeType::Navigation,
            LinkSource::Goto => knot_core::graph::EdgeType::Jump,
            LinkSource::Include => knot_core::graph::EdgeType::Include,
            LinkSource::NavigationMacro => knot_core::graph::EdgeType::Navigation,
            LinkSource::Actions => knot_core::graph::EdgeType::Navigation,
            LinkSource::Return => knot_core::graph::EdgeType::Navigation,
            LinkSource::Back => knot_core::graph::EdgeType::Navigation,
            LinkSource::DataPassage => knot_core::graph::EdgeType::Navigation,
            LinkSource::WidgetCall => knot_core::graph::EdgeType::Call,
        }
    }
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
// Set assignment — structured <<set>> macro parsing
// ---------------------------------------------------------------------------

/// Assignment operator for `<<set>>` macros.
///
/// SugarCube's `<<set>>` macro supports multiple assignment operators.
/// The `to` keyword is SugarCube-specific; the rest are standard JS
/// compound assignment operators. The SugarCube parser owns the
/// target + operator; only the RHS expression goes to oxc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOperator {
    /// `to` keyword (SugarCube-specific assignment)
    To,
    /// `=` operator
    Eq,
    /// `+=` operator
    PlusEq,
    /// `-=` operator
    MinusEq,
    /// `*=` operator
    StarEq,
    /// `/=` operator
    SlashEq,
    /// `%=` operator
    PercentEq,
    /// `++` postfix increment
    PostfixPlus,
    /// `--` postfix decrement
    PostfixMinus,
}

/// Structured representation of a `<<set>>` macro's assignment.
///
/// For `<<set $hp to 100>>`, this captures:
/// - target: `$hp` (the LHS variable, which SugarCube owns)
/// - operator: `SetOperator::To` (SugarCube-specific `to` keyword)
/// - expression: `Some("100")` (the RHS, which is the ONLY part oxc parses)
///
/// For `<<set $hp++>>`, there is no RHS expression:
/// - target: `$hp`
/// - operator: `SetOperator::PostfixPlus`
/// - expression: `None`
///
/// This separation ensures that the SugarCube parser owns the assignment
/// structure (target + operator), and oxc only sees the value expression.
#[derive(Debug, Clone)]
pub struct SetAssignment {
    /// The target variable being assigned to.
    pub target: VarRef,
    /// The assignment operator.
    pub operator: SetOperator,
    /// The RHS expression (None for postfix `++` and `--`).
    /// This is the ONLY part of a `<<set>>` that goes to oxc.
    pub expression: Option<String>,
    /// Byte range of the expression in the passage body.
    pub expression_span: Option<Range<usize>>,
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
        /// JS analysis results from oxc, attached during Phase 2 (annotation pass).
        ///
        /// When `Some`, this is the **single source of truth** for JS-derived
        /// variable operations. When `None`, the annotation pass hasn't run yet
        /// or this macro doesn't contain JS content.
        js_analysis: Option<JsAnalysis>,
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
        /// For `<<set>>` macros: structured assignment info.
        ///
        /// When present, the SugarCube parser has split the `<<set>>` args
        /// into target + operator + expression. The `target` and `operator`
        /// are SugarCube-owned; only `expression` goes to oxc.
        ///
        /// When `None`, the macro is not a `<<set>>` (or the args couldn't
        /// be parsed as a simple assignment, e.g. `<<set $arr.push(1)>>`).
        set_assignment: Option<SetAssignment>,
    },

    /// An inline expression: `<<=>>expr>>` or `<<->>expr>>`.
    Expression {
        /// Whether this is a print (`=`) or silent (`-`) expression.
        kind: ExprKind,
        /// The expression content between the delimiters.
        content: String,
        /// Variable references found in the expression.
        var_refs: Vec<VarRef>,
        /// JS analysis results from oxc, attached during Phase 2 (annotation pass).
        ///
        /// When `Some`, this is the **single source of truth** for JS-derived
        /// variable operations. When `None`, the annotation pass hasn't run yet.
        js_analysis: Option<JsAnalysis>,
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
    /// For script passages: the JS analysis for the entire passage body.
    ///
    /// Script passages contain pure JS (no SugarCube syntax), so the parser
    /// produces a single Text node. Since Text nodes don't have `js_analysis`,
    /// we store the analysis here at the PassageAst level.
    ///
    /// Populated during Phase 2 (JS annotation pass) for script passages.
    /// `None` for all other passage modes.
    pub script_js_analysis: Option<JsAnalysis>,
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
    /// The source context of this link, used for edge type classification.
    ///
    /// When set, this allows `build_passage()` to set `edge_type_hint`
    /// directly on the resulting `Passage.links` entry, avoiding the
    /// post-hoc `classify_edge()` substring matching that can produce
    /// false positives.
    pub source: LinkSource,
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
            script_js_analysis: None,
        }
    }

    /// Extract graph connections from this passage's links.
    ///
    /// Converts the `LinkInfo` entries into `PassageConnection` instances
    /// with concrete `EdgeType` values. This is the primary method for
    /// the graph handler to get edge data from a parsed passage.
    pub fn graph_connections(&self) -> Vec<PassageConnection> {
        self.links.iter().map(|link| {
            PassageConnection {
                target: link.target.clone(),
                display: link.display.clone(),
                edge_type: link.source.to_edge_type(),
                is_dynamic: link.is_dynamic,
                span: link.span.clone(),
            }
        }).collect()
    }
}

/// A connection from this passage to another passage, for graph building.
#[derive(Debug, Clone)]
pub struct PassageConnection {
    /// The target passage name.
    pub target: String,
    /// Display text for the link (if any).
    pub display: Option<String>,
    /// The graph edge type (Navigation, Jump, Include, Call).
    pub edge_type: knot_core::graph::EdgeType,
    /// Whether this connection uses a dynamic variable target.
    pub is_dynamic: bool,
    /// Byte range of the link in the passage body.
    pub span: Range<usize>,
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
            AstNode::Macro { children: Some(ch), .. } => {
                result.extend(collect_links(ch));
            }
            AstNode::Macro { children: None, .. } => {}
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
            AstNode::Macro { children: Some(ch), .. } => {
                result.extend(collect_errors(ch));
            }
            AstNode::Macro { children: None, .. } => {}
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
    /// Macro names that contain inline JS expressions (excluding "set",
    /// which is handled specially via set_assignment).
    const INLINE_JS_MACROS: &[&str] = &["run", "if", "elseif", "else", "print", "nobr"];
    /// Macro names that contain full JS blocks.
    const BLOCK_JS_MACROS: &[&str] = &["script"];

    for node in nodes {
        if let AstNode::Macro {
            name,
            args,
            open_span,
            children,
            set_assignment,
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
            } else if name == "set" {
                // <<set>> is special: SugarCube owns the target + operator,
                // oxc only parses the RHS expression.
                if let Some(sa) = set_assignment {
                    // Structured assignment: only the expression goes to oxc.
                    //
                    // The expression_span is relative to the passage body start.
                    // We need the offset to the START of the expression within
                    // the passage body, so that JS diagnostics can be mapped
                    // back to the correct position.
                    //
                    // IMPORTANT: When `expression` is trimmed (leading/trailing
                    // whitespace removed), the body_offset must account for the
                    // trimmed portion. Otherwise, diagnostics at the start of
                    // the expression will be mapped to the wrong position.
                    if let Some(expr) = &sa.expression {
                        let trimmed = expr.trim();
                        if !trimmed.is_empty() {
                            // Calculate how many bytes of leading whitespace
                            // were trimmed from the expression, so the offset
                            // accounts for the trim.
                            let leading_ws = expr.len() - expr.trim_start().len();
                            let expr_body_offset = sa.expression_span.as_ref()
                                .map(|s| s.start + leading_ws)
                                .unwrap_or(open_span.start);
                            result.push(JsSnippet {
                                source: trimmed.to_string(),
                                body_offset: expr_body_offset,
                                macro_name: "set".to_string(),
                                is_block: false,
                            });
                        }
                    }
                    // Postfix ++ / --: no JS snippet needed
                } else {
                    // No structured assignment (e.g., <<set $arr.push("item")>>).
                    // The entire args are a JS expression — same as <<run>>.
                    //
                    // For correct span mapping, the body_offset must point to
                    // where the args actually start in the passage body, which
                    // is after `<<set ` (the `<<` + macro name + space).
                    // Using `open_span.start` would point to `<<`, making
                    // diagnostics appear at the wrong position.
                    let trimmed = args.trim();
                    if !trimmed.is_empty() {
                        // Calculate the byte offset of the args start in the
                        // passage body.
                        //
                        // open_span.end = span_start + position_past_>>>
                        // args_end (before >>) = open_span.end - span_start - 2
                        // args_start = args_end - args.len()
                        // args_start in passage body = span_start + args_start
                        //   = span_start + (open_span.end - span_start - 2 - args.len())
                        //   = open_span.end - 2 - args.len()
                        //
                        // We also add leading_ws to account for trimmed whitespace.
                        let leading_ws = args.len() - args.trim_start().len();
                        let args_body_start = open_span.end - 2 - args.len() + leading_ws;
                        result.push(JsSnippet {
                            source: trimmed.to_string(),
                            body_offset: args_body_start,
                            macro_name: "set".to_string(),
                            is_block: false,
                        });
                    }
                }
            } else if INLINE_JS_MACROS.contains(&name.as_str()) {
                // Inline JS: the args are a JS expression.
                // Calculate the args start offset similarly to the <<set>> case.
                // open_span.end - 2 accounts for the >> closing tag.
                let trimmed = args.trim();
                if !trimmed.is_empty() {
                    let leading_ws = args.len() - args.trim_start().len();
                    let args_body_start = open_span.end - 2 - args.len() + leading_ws;
                    result.push(JsSnippet {
                        source: trimmed.to_string(),
                        body_offset: args_body_start,
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
