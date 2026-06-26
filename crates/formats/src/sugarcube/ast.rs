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
    /// Function call sites found (identifiers used as call targets).
    pub function_calls: Vec<FunctionCallInfo>,
    /// Literal tokens (strings, numbers, booleans, null) found by oxc.
    pub literal_spans: Vec<LiteralSpan>,
    /// Operator tokens found by oxc (including SugarCube keyword operators
    /// mapped back to their original positions via the preprocessor).
    pub operator_spans: Vec<OperatorSpan>,
    /// Global object/namespace references found by oxc (e.g., `Engine`,
    /// `Story`, `Config`, `Save`, `UI`, etc.).
    pub namespace_spans: Vec<NamespaceSpan>,
    /// Comments (`/* ... */` block comments and `// ...` line comments)
    /// found inside the JS expression. These are NOT collected by oxc's
    /// AST walker (comments aren't AST nodes), so the walker scans the
    /// raw JS source separately to find them. The token builder emits
    /// `Comment` tokens for these spans so themes can color them.
    pub comment_spans: Vec<CommentSpan>,
    /// JS keyword tokens found by oxc (`if`, `for`, `while`, `return`,
    /// `var`, `let`, `const`, `function`, `try`, `catch`, `finally`,
    /// `new`, `typeof`, `instanceof`, `delete`, `void`, `in`, `of`,
    /// `this`, `throw`). The token builder emits these as `Keyword`
    /// semantic tokens.
    pub keyword_spans: Vec<KeywordSpan>,
    /// Regular expression literal spans found by oxc. Used internally by
    /// `extract_comments` to skip over regex patterns (which may contain
    /// `/*` or `//` sequences that would be misidentified as comments).
    /// Not emitted as semantic tokens.
    pub regex_spans: Vec<std::ops::Range<usize>>,
    /// JS local variable references — identifiers that are NOT SugarCube
    /// variables (`$var`/`_var`), NOT properties, and NOT function calls.
    /// Covers plain JS locals like `el`, `g`, `profile`, `vm`, `html`.
    /// The token builder emits these as `Variable` semantic tokens.
    pub js_var_spans: Vec<std::ops::Range<usize>>,
    /// JS local variable declarations — the binding name in
    /// `var x = ...`, `let x = ...`, `const x = ...`, and function
    /// parameters. The token builder emits these as `Variable` tokens
    /// with the `Definition` modifier.
    pub js_var_def_spans: Vec<std::ops::Range<usize>>,
    /// JS method call names — the property name in `expr.method(...)`.
    /// e.g. `.forEach`, `.getElementById`, `.isArray`, `.filter`.
    /// The token builder emits these as `Function` semantic tokens.
    pub js_method_spans: Vec<std::ops::Range<usize>>,
    /// JS property access names — the property name in `expr.prop` (not
    /// followed by `(`). e.g. `.left`, `.length`, `.innerHTML`, `.showIf`.
    /// The token builder emits these as `Property` semantic tokens.
    pub js_property_spans: Vec<std::ops::Range<usize>>,
    /// JS global object references — identifiers that match known JS
    /// globals (`document`, `window`, `console`, `Array`, `Object`,
    /// `Math`, `JSON`, `Number`, `String`, `Boolean`, `Date`, `RegExp`,
    /// `Error`, `Promise`, `Set`, `Map`, `Symbol`, `WeakMap`, `WeakSet`).
    /// The token builder emits these as `Namespace` semantic tokens.
    pub js_global_spans: Vec<std::ops::Range<usize>>,
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
    /// Per-segment construct spans — the source range that groups each node
    /// and its written descendants at that depth.
    ///
    /// `segment_construct_spans[i]` is the construct span for the node at
    /// depth `i` in the path. For `$a.n1.name`:
    /// - `[0]` = `$a = {...}` (root assignment span)
    /// - `[1]` = `n1:{...}` (the n1 property with its full value)
    /// - `[2]` = `name:"apple"` (the leaf key-value pair)
    ///
    /// Used by `record_var` propagation: when a leaf write propagates up to
    /// an ancestor at depth `d`, the propagated write's span = the immediate
    /// child's construct span = `segment_construct_spans[d+1]`.
    ///
    /// For non-block writes (e.g., `<<set $foo.bar to 1>>`), all entries
    /// are the full assignment expression span — nothing to aggregate.
    pub segment_construct_spans: Vec<Range<usize>>,
}

/// A literal token found by oxc within a JS snippet.
///
/// Produced by Phase 2 (JS annotation pass) alongside `OperatorSpan` entries.
/// The token builder emits these as `String`, `Number`, `Boolean`, or `Keyword`
/// semantic tokens, giving fine-grained highlighting for literal values inside
/// macro arguments and inline expressions.
#[derive(Debug, Clone)]
pub struct LiteralSpan {
    /// The kind of literal.
    pub kind: LiteralKind,
    /// Byte range in the passage body (passage-body-relative).
    pub span: Range<usize>,
}

/// The kind of literal found by oxc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    /// A string literal: `"hello"` or `'world'`.
    String,
    /// A numeric literal: `42`, `3.14`, `0xFF`.
    Number,
    /// A boolean literal: `true`, `false`.
    Boolean,
    /// The `null` keyword.
    Null,
}

/// An operator token found by oxc within a JS snippet.
///
/// SugarCube keyword operators (`to`, `eq`, `and`, etc.) are normalized to JS
/// equivalents by the preprocessor before oxc sees them. The preprocessor's
/// substitution table maps each normalized token back to the original SugarCube
/// keyword position, so the `span` field references the *original* SugarCube
/// source position.
#[derive(Debug, Clone)]
pub struct OperatorSpan {
    /// The kind of operator.
    pub kind: OperatorKind,
    /// Byte range in the passage body (passage-body-relative).
    pub span: Range<usize>,
}

/// The kind of operator found by oxc.
///
/// Categorizes operators for semantic token emission. SugarCube keyword
/// operators like `to`, `eq`, `and` are normalized to JS operators (`=`, `===`,
/// `&&`) by the preprocessor, but this enum captures the semantic category
/// of the *original* SugarCube operator, not the JS equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorKind {
    /// Assignment: `=`, `to`.
    Assignment,
    /// Compound assignment: `+=`, `-=`, `*=`, `/=`, `%=`.
    CompoundAssign,
    /// Comparison: `===`, `!==`, `>`, `<`, `>=`, `<=`, `eq`, `neq`, `is`,
    /// `isnot`, `gt`, `gte`, `lt`, `lte`.
    Comparison,
    /// Logical: `&&`, `||`, `!`, `and`, `or`, `not`.
    Logical,
    /// Arithmetic: `+`, `-`, `*`, `/`, `%`.
    Arithmetic,
    /// Other operators: `?:`, `??`, `,`, etc.
    Other,
}

/// A reference to a SugarCube global object found by oxc.
///
/// When JS code references a known global like `Engine`, `Story`, `Config`,
/// etc., this records the span of the global name and any properties accessed
/// on it. Used by the token builder to emit `Namespace` + `Property` tokens.
///
/// **Deduplication**: `State.variables.x` patterns are already covered by
/// `AnalyzedVarOp` (which emits `Variable` + `Property` tokens). This type
/// is only used for non-`State` globals, or for `State` accesses that are NOT
/// variable reads/writes (e.g., `State.turns`, `State.passage`).
#[derive(Debug, Clone)]
pub struct NamespaceSpan {
    /// The global object name (e.g., "Engine", "Story", "Config").
    pub name: String,
    /// Byte range of the global name in the passage body.
    pub span: Range<usize>,
    /// Properties accessed on this global object.
    pub property_spans: Vec<PropertySpan>,
}

/// A property access on a SugarCube global object.
///
/// Records the name and span of a property accessed via dot notation on a
/// known global. Used for `Property` token emission.
#[derive(Debug, Clone)]
pub struct PropertySpan {
    /// The property name (e.g., "play", "has", "debug").
    pub name: String,
    /// Byte range of the property name in the passage body.
    pub span: Range<usize>,
}

/// A comment (`/* ... */` or `// ...`) found inside a JS expression.
///
/// Produced by the JS walker scanning the raw (preprocessed) JS source
/// for comments, since oxc's AST doesn't include comments as nodes. The
/// spans are mapped back to the ORIGINAL SugarCube source positions via
/// the preprocessor's substitution table (same as `OperatorSpan`).
#[derive(Debug, Clone)]
pub struct CommentSpan {
    /// The kind of comment.
    pub kind: CommentKind,
    /// Byte range in the passage body (passage-body-relative).
    pub span: Range<usize>,
}

/// A JS keyword token found by oxc within a JS snippet.
///
/// Produced by Phase 2 (JS annotation pass) alongside `OperatorSpan` entries.
/// Covers statement-level keywords (`if`, `for`, `while`, `return`, `try`,
/// `catch`, `finally`, `function`) and declaration keywords (`var`, `let`,
/// `const`), plus expression-level keywords (`new`, `typeof`, `instanceof`,
/// `delete`, `void`, `in`, `of`, `this`, `throw`).
///
/// The token builder emits these as `Keyword` semantic tokens so themes can
/// color JS keywords distinctly from SugarCube macro names.
#[derive(Debug, Clone)]
pub struct KeywordSpan {
    /// The keyword text (e.g. "if", "var", "function").
    pub text: &'static str,
    /// Byte range in the passage body (passage-body-relative).
    pub span: Range<usize>,
}

/// Information about a `Macro.add()` call found in JS.
#[derive(Debug, Clone)]
pub struct MacroAddInfo {
    /// The macro name being registered.
    pub name: String,
    /// Byte offset of the name argument in the passage body.
    pub name_offset: usize,
    /// Whether this macro has a body (container) or is inline.
    ///
    /// Derived from the `tags` field of the `Macro.add()` config object:
    /// - `tags` omitted → `BodyRequirement::Never` (inline, no close tag)
    /// - `tags: null` → `BodyRequirement::Required` (container, close tag
    ///   required, no named sub-tags)
    /// - `tags: ["a", "b"]` → `BodyRequirement::Required` (container with
    ///   named sub-tags)
    ///
    /// This is the **same semantics** as SugarCube's own runtime check.
    pub body: crate::types::BodyRequirement,
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
    /// The function name (with SugarCube sigil restored, e.g., "_myHelper").
    pub name: String,
    /// Byte offset of the function name in the passage body.
    pub name_offset: usize,
    /// Number of parameters, if known.
    pub param_count: Option<usize>,
}

/// Information about a function call site found in JS.
///
/// When an identifier that was preprocessed from a SugarCube `$var` or `_var`
/// is used as a function call target (e.g., `_myHelper()`), it should be
/// classified as a function call, not a variable reference. This struct
/// records those call sites so the token builder can emit `Function` tokens.
#[derive(Debug, Clone)]
pub struct FunctionCallInfo {
    /// The function name (with SugarCube sigil restored, e.g., "_myHelper").
    pub name: String,
    /// Byte range of the function name in the passage body (passage-body-relative).
    pub span: Range<usize>,
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
    /// Image link: [[img[URL][Passage]] — clickable image linking to a passage
    Image,
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
    /// `<<goto>>` macro — Navigation edge type (unconditional redirect).
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
    /// Widget invocation (`<<myWidget>>`) — Navigation edge type.
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
            LinkSource::Goto => knot_core::graph::EdgeType::Navigation,
            LinkSource::Include => knot_core::graph::EdgeType::Include,
            LinkSource::NavigationMacro => knot_core::graph::EdgeType::Navigation,
            LinkSource::Actions => knot_core::graph::EdgeType::Navigation,
            LinkSource::Return => knot_core::graph::EdgeType::Navigation,
            LinkSource::Back => knot_core::graph::EdgeType::Navigation,
            LinkSource::DataPassage => knot_core::graph::EdgeType::Include,
            LinkSource::WidgetCall => knot_core::graph::EdgeType::Navigation,
        }
    }
}

// ---------------------------------------------------------------------------
// Text formatting kind
// ---------------------------------------------------------------------------

/// The kind of SugarCube text formatting markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFormatKind {
    /// `''bold''` → `<strong>`
    Bold,
    /// `//italic//` → `<em>`
    Italic,
    /// `__underline__` → `<u>`
    Underline,
    /// `==strike==` → `<s>`
    Strike,
    /// `~~sub~~` → `<sub>`
    Sub,
    /// `^^super^^` → `<sup>`
    Super,
}

// ---------------------------------------------------------------------------
// TableRow / TableCell / TableRowType — TiddlyWiki table support
// ---------------------------------------------------------------------------
//
// These types support the `AstNode::Table` variant (see plan.md §3.9, §AD-3).
// They are NOT variants of `AstNode` themselves — a `Table` node owns its
// rows and cells directly, and the token builder recurses into them inside
// the `Table` match arm.

/// Row type for a TiddlyWiki table row, determined by the one-letter suffix
/// after the closing `|` (see plan.md §3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableRowType {
    /// No suffix — body row (`<tbody>`).
    Body,
    /// `h` suffix — header row (`<thead>`).
    Header,
    /// `f` suffix — footer row (`<tfoot>`).
    Footer,
    /// `c` suffix — caption (`<caption>`). The row's cell content is the
    /// caption text (usually a single cell).
    Caption,
    /// `k` suffix — CSS class assignment. The row's cell content is the
    /// class name applied to the `<table>` element.
    Class,
}

/// A single row in a TiddlyWiki table.
///
/// A row is a line of the form `|cell|cell|...|[fhck]?` at column 0.
/// The `row_type` is determined by the optional one-letter suffix after
/// the closing `|`.
#[derive(Debug, Clone)]
pub struct TableRow {
    /// The cells in this row, in left-to-right order.
    pub cells: Vec<TableCell>,
    /// The row type (body / header / footer / caption / class).
    pub row_type: TableRowType,
    /// Byte range of the entire row line, body-relative.
    pub span: Range<usize>,
}

/// A single cell in a TiddlyWiki table row.
///
/// Cells are separated by `|`. A cell whose content begins with `!` is a
/// header cell (`<th>`). A cell containing only `>` triggers colspan; only
/// `~` triggers rowspan (see plan.md §3.9).
#[derive(Debug, Clone)]
pub struct TableCell {
    /// Recursively-parsed cell content (macros execute inside cells).
    pub children: Vec<AstNode>,
    /// `true` if the cell content begins with `!` (renders as `<th>`).
    pub is_header: bool,
    /// `true` if the cell content is just `>` (triggers colspan merge).
    pub colspan: bool,
    /// `true` if the cell content is just `~` (triggers rowspan extension).
    pub rowspan: bool,
    /// Byte range of the cell content (between `|` delimiters), body-relative.
    pub span: Range<usize>,
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
    /// `into` keyword (SugarCube-specific reverse assignment: value into variable)
    Into,
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
// StructuredMacroArg — catalog-driven structured arg extraction
// ---------------------------------------------------------------------------

/// The semantic kind of a parsed macro argument, derived from the catalog.
///
/// Unlike `MacroArgKind` in `types.rs` (which declares what a macro *expects*
/// in its signature), this enum records what a parsed argument *actually is*
/// based on the catalog's `MacroArgDef` declarations. The catalog is the
/// single source of truth — the parser just aligns extracted string/bare
/// tokens to the catalog's declared positions and kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedArgKind {
    /// A passage name reference (quoted or bare).
    /// e.g., `"Cave"` in `<<goto "Cave">>`, `Forest` in `<<goto Forest>>`
    PassageRef,
    /// A display label (first string arg of link/button macros).
    /// e.g., `"Talk"` in `<<link "Talk" "Shop">>`
    Label,
    /// A CSS selector argument.
    /// e.g., `"#hp-bar"` in `<<remove "#hp-bar">>`
    Selector,
    /// A generic string argument (not a passage ref, label, or selector).
    /// e.g., `"2s"` in `<<timed "2s">>`
    String,
    /// A variable reference argument ($var or _var).
    /// e.g., `$dest` in `<<goto $dest>>`
    VariableRef,
    /// A JS expression argument (the common case for most macros).
    /// e.g., `$hp gte 50` in `<<if $hp gte 50>>`
    Expression,
    /// A bareword keyword flag (e.g., `autofocus`, `selected`, `keep`,
    /// `container`, `autocheck`, `checked`, `once`, `autoselect`).
    /// These appear without quotes as positional args.
    Keyword,
    /// A link markup argument (`[[...]]` syntax).
    /// e.g., `[[Forest]]` in `<<goto [[Forest]]>>`
    LinkMarkup,
    /// An image markup argument (`[img[...]]` syntax).
    /// e.g., `[img[forest.png][Forest]]` in `<<link [img[forest.png][Forest]]>>`
    ImageMarkup,
    /// A numeric literal (e.g., `100`, `0.5`).
    /// e.g., `100` in `<<numberbox "$x" 100>>`
    Number,
}

/// A structured macro argument extracted from the raw args string.
///
/// Phase 6 populates this by scanning the args string for quoted/bare tokens
/// and aligning them with the `MacroArgDef` catalog entries. This gives
/// downstream consumers (token builder, link extraction, completion) direct
/// access to what each argument position means without re-parsing the raw
/// `args` string.
///
/// **Conservative approach**: Only quoted string arguments and bare passage
/// names are extracted. Complex JS expressions are left to oxc (Phase 2).
/// This covers the most impactful use cases: passage name references for
/// link extraction, graph edges, and go-to-definition.
#[derive(Debug, Clone)]
pub struct StructuredMacroArg {
    /// The semantic kind of this argument, derived from the catalog.
    pub kind: ParsedArgKind,
    /// The argument value (string content without quotes for quoted args,
    /// or the bare token for unquoted args).
    pub value: String,
    /// Byte range of the argument in the passage body (passage-body-relative).
    /// For quoted strings, this covers the content inside the quotes
    /// (not including the quote characters themselves).
    pub span: Range<usize>,
}

// ---------------------------------------------------------------------------
// ForLoopVars — structured <<for>> macro parsing
// ---------------------------------------------------------------------------

/// Structured representation of a `<<for>>` macro's loop variables.
///
/// SugarCube's `<<for>>` macro has two syntax forms:
///
/// 1. **Simplified iteration**: `<<for _i, $array>>` — iterates over `$array`,
///    binding each element to `_i`. This form is detected by the comma separator
///    between the index variable and the iterated variable.
///
/// 2. **C-style for loop**: `<<for _i to 0; _i lt 10; _i++>>` — uses the
///    standard three-part loop header. This form falls through to the JS
///    annotation pass for full oxc analysis; `for_loop_vars` will be `None`.
///
/// This type is only populated for the simplified iteration form. The C-style
/// form relies entirely on `JsAnalysis` for variable extraction.
#[derive(Debug, Clone)]
pub struct ForLoopVars {
    /// The loop iteration variable (e.g., `_i` in `<<for _i, $array>>`).
    /// This is the variable that receives each element during iteration.
    pub index_var: VarRef,
    /// The collection being iterated (e.g., `$array` in `<<for _i, $array>>`).
    /// This is the variable being read for its contents.
    pub iterated_var: VarRef,
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
    ///
    /// The `is_prose` flag indicates whether this text is narrative/story
    /// content (rendered to the player) vs. non-prose text (inside
    /// `<<silently>>`, `<<script>>`, `<<style>>`, or other non-rendering
    /// contexts). Top-level text in a passage body is always prose.
    /// Text inside block macros like `<<if>>`, `<<for>>`, `<<nobr>>`,
    /// `<<capture>>`, `<<type>>` is also prose, because those macros
    /// render their body content. Text inside `<<silently>>`,
    /// `<<script>>`, or `<<style>>` is NOT prose.
    Text {
        /// The text content.
        content: String,
        /// Variable references found in this text gap.
        var_refs: Vec<VarRef>,
        /// Byte range in the passage body.
        span: Range<usize>,
        /// Whether this text is prose (narrative content rendered to the player).
        ///
        /// Prose text gets a `Prose` semantic token, enabling themes to
        /// style narrative content distinctly from code/comments. Non-prose
        /// text (inside `<<silently>>`, `<<script>>`, `<<style>>`) does not
        /// get a prose token.
        is_prose: bool,
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
        /// For definition macros (`<<widget>>`): the span of the name being
        /// defined (e.g., "myWidget" in `<<widget myWidget>>`).
        ///
        /// This enables the token builder to emit a `Function` token with
        /// `Definition` modifier for the defined name, distinct from the
        /// `Macro` token on the keyword itself.
        definition_name_span: Option<Range<usize>>,
        /// For block macros: byte range of the name portion within the close tag.
        /// Combined with `close_span`, this provides lossless round-trip capability
        /// for the close tag — the full `<</name>>` span and the name portion can
        /// both be reconstructed.
        close_name_span: Option<Range<usize>>,
        /// For `<<capture>>` macros: the target variable being captured.
        ///
        /// When present, the SugarCube parser has identified the variable reference
        /// in the args (e.g., `$target` in `<<capture $target>>`). This enables
        /// registry population to mark the variable as `VarAccessKind::Capture`
        /// without relying on the JS annotation pass's heuristic.
        capture_target: Option<VarRef>,
        /// For `<<for>>` macros: structured loop variable information.
        ///
        /// Only populated for the simplified iteration form (`<<for _i, $array>>`).
        /// The C-style form (`<<for _i to 0; _i lt 10; _i++>>`) falls through
        /// to the JS annotation pass, and `for_loop_vars` will be `None`.
        for_loop_vars: Option<ForLoopVars>,
        /// Structured argument information derived from the `MacroDef` catalog.
        ///
        /// When present, each entry corresponds to a parsed argument from the
        /// raw `args` string, classified by the catalog's `MacroArgDef` declarations.
        /// Only macros with declared args (`MacroDef.args.is_some()`) get structured
        /// extraction. Macros with no catalog entry or undeclared args remain `None`.
        ///
        /// This field is the **AST-level source of truth** for what each argument
        /// position means. Downstream consumers (token builder, link extraction,
        /// completion) should prefer this over re-parsing `args` when available.
        ///
        /// **Conservative approach**: Only quoted string arguments and bare passage
        /// names are extracted. Complex JS expressions are left to oxc (Phase 2).
        structured_args: Option<Vec<StructuredMacroArg>>,
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
        /// How the link was formatted (pipe, arrow, simple, setter, image).
        kind: LinkKind,
        /// For setter links: the setter variable name (e.g., `$var`).
        setter_var: Option<String>,
        /// For image links: the image URL (e.g., `http://example.com/pic.jpg`).
        image_url: Option<String>,
        /// Byte range of the entire link construct.
        span: Range<usize>,
        /// Byte range of the display text within the link (body-relative).
        ///
        /// For `[[Display|Target]]` this covers "Display".
        /// For `[[Target]]` (simple), this is `None` — there is no separate
        /// display text; the target IS the display.
        /// For `[[Target<-Display]]` (left arrow), this covers "Display".
        ///
        /// When present, the token builder emits a `String` token over this
        /// region so the editor can visually differentiate display vs
        /// target. The `Link` token is emitted over `target_span` instead
        /// (so completion/hover trigger on the target, not the display).
        ///
        /// All offsets are body-relative (same coordinate space as `span`).
        display_span: Option<Range<usize>>,
        /// Byte range of the target passage name within the link
        /// (body-relative).
        ///
        /// For all link forms, this covers just the target passage name
        /// (excluding `[[`, `]]`, separators, and setter). The token
        /// builder emits a `Link` token over this region.
        ///
        /// Offset is body-relative (same coordinate space as `span`).
        target_span: Range<usize>,
        /// Byte range of the setter expression within the link
        /// (body-relative), if any.
        ///
        /// For `[[Target][$var += 5]]` this covers `$var += 5` (the content
        /// between `][$` and the closing `]]`). The token builder does NOT
        /// emit a token over this region — the inline variable scanner
        /// already tokenizes `$var` as a Variable, and the rest is JS
        /// expression content. The span is exposed for future tooling
        /// (e.g., dedicated setter diagnostics, code actions).
        ///
        /// Offset is body-relative (same coordinate space as `span`).
        setter_span: Option<Range<usize>>,
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

    /// SugarCube inline styling: `@@class;text@@` or `@class;text@`.
    ///
    /// Produces `<span class="class">text</span>` in the rendered output.
    /// The `children` contain the parsed body content (Text nodes with prose,
    /// variable references, etc.). The `class` field holds the CSS class(es).
    InlineStyle {
        /// CSS class name(s) — e.g., ".highlight", ".red;.bold"
        class: String,
        /// Byte range of the class name within the passage body.
        class_span: Range<usize>,
        /// Parsed body content (Text nodes, variable refs, etc.)
        children: Vec<AstNode>,
        /// Byte range of the entire inline style construct.
        span: Range<usize>,
    },

    /// SugarCube text formatting markup: `''bold''`, `//italic//`, `__underline__`,
    /// `==strike==`, `~~sub~~`, `^^super^^`.
    TextFormat {
        /// What kind of formatting this is.
        kind: TextFormatKind,
        /// The formatted text content.
        content: String,
        /// Byte range of the entire formatting construct including delimiters.
        span: Range<usize>,
    },

    /// A macro close tag: `<</name>>`.
    ///
    /// Produced by the flat parser and consumed by the tree builder. Does not
    /// appear in the final nested AST — its span information is preserved in
    /// the parent `Macro`'s `close_span` and `close_name_span` fields.
    ///
    /// The tree builder pairs `Macro` with `MacroClose` to establish nesting,
    /// then removes `MacroClose` nodes from the final tree.
    MacroClose {
        /// The macro name being closed.
        name: String,
        /// Byte range of the name portion in the passage body.
        name_span: Range<usize>,
        /// Byte range of the full `<</name>>` tag in the passage body.
        span: Range<usize>,
    },

    /// A parse error — unclosed delimiter, invalid syntax, etc.
    Error {
        /// Human-readable description of the error.
        message: String,
        /// Byte range of the problematic construct.
        span: Range<usize>,
    },

    // ── Block-level markup (Phase 1+ of the block-markup overhaul) ───────
    //
    // These variants are introduced in Phase 1 as scaffolding. The parser
    // does not yet emit them; subsequent phases fill in the arms that
    // produce each variant. See `plan.md` §6 for the phase roadmap.
    //
    // Spans are body-relative (byte 0 = first character after the passage
    // header newline), matching the convention used by all other variants.

    /// A heading: `!` through `!!!!!!` (1-6 levels).
    ///
    /// SugarCube's `heading` parser calls `subWikify`, so macros, variables,
    /// and links INSIDE heading text are processed (not raw). The `children`
    /// field holds the recursively-parsed content (see plan.md §3.5).
    ///
    /// The `level` is `1..=6` (number of `!` characters). A 7th `!` becomes
    /// the first character of heading text.
    ///
    /// `span` covers the `!` run through end of line (exclusive of the `\n`).
    Heading {
        /// Heading level: 1 (`!`) through 6 (`!!!!!!`).
        level: u8,
        /// Recursively-parsed heading content (macros execute).
        children: Vec<AstNode>,
        /// Byte range of the heading, body-relative.
        span: Range<usize>,
    },

    /// A horizontal rule: `----` (4+ dashes alone on a line).
    ///
    /// SugarCube requires `^----+\s*$` — 4 or more dashes at column 0 with
    /// only trailing whitespace allowed (see plan.md §3.6). `---` (3 dashes)
    /// is NOT a horizontal rule.
    HorizontalRule {
        /// Byte range of the `----` run, body-relative.
        span: Range<usize>,
    },

    /// A list item: `*`/`**`/`#`/`##` etc. at column 0.
    ///
    /// SugarCube's list syntax is `*` (unordered) or `#` (ordered), with
    /// nesting determined by marker character count (NOT indentation). Mixed
    /// markers like `*#` are NOT supported — the regex matches all-`*` or
    /// all-`#` only (see plan.md §3.7).
    ///
    /// Flat model: there is no `List` wrapper variant. Consumers reconstruct
    /// nesting from `depth` (marker char count), the same way block macros
    /// are flat-emitted and paired later by the tree builder.
    ListItem {
        /// Nesting depth = marker character count (1 = top level, 2 = nested, …).
        depth: u8,
        /// `true` for `#` (ordered), `false` for `*` (unordered).
        ordered: bool,
        /// The raw marker string (e.g. `"*"`, `"**"`, `"#"`, `"###"`).
        marker: String,
        /// Recursively-parsed item content (macros execute).
        children: Vec<AstNode>,
        /// Byte range of the item, body-relative (marker through end of line).
        span: Range<usize>,
    },

    /// A line-style blockquote: `>`/`>>`/etc. at column 0.
    ///
    /// SugarCube's `quoteByLine` parser builds nested `<blockquote>` elements
    /// based on the number of `>`. Each line of a multi-line blockquote must
    /// begin with `>` (no lazy continuation — see plan.md §3.8.1).
    ///
    /// `depth` is the `>` count. `children` holds the recursively-parsed line
    /// content (macros execute).
    Blockquote {
        /// Nesting depth = `>` count (1 = `>`, 2 = `>>`, …).
        depth: u8,
        /// Recursively-parsed blockquote line content.
        children: Vec<AstNode>,
        /// Byte range of the blockquote line, body-relative.
        span: Range<usize>,
    },

    /// A block-style blockquote: `<<<\n...\n<<<`.
    ///
    /// This is the TiddlyWiki-derived "quoteByBlock" form. It is present in
    /// SugarCube's source but UNDOCUMENTED in the official v2 markup docs
    /// (see plan.md §3.8.2). A line of exactly `<<<` opens the blockquote;
    /// another `<<<` line closes it. Everything between is wrapped in a
    /// single `<blockquote>`.
    BlockquoteBlock {
        /// Recursively-parsed block content (macros execute).
        children: Vec<AstNode>,
        /// Byte range of the opening `<<<` line, body-relative.
        open_span: Range<usize>,
        /// Byte range of the closing `<<<` line, body-relative. `None` if
        /// the block was never closed (the parser emits a diagnostic).
        close_span: Option<Range<usize>>,
        /// Byte range of the entire blockquote block (open + body + close).
        span: Range<usize>,
    },

    /// A TiddlyWiki-style table.
    ///
    /// SugarCube's `table` parser is undocumented in the official v2 markup
    /// docs but fully implemented in the source (see plan.md §3.9). Rows are
    /// `|`-delimited lines with an optional one-letter row-type suffix
    /// (`h`/`f`/`c`/`k`). Cells beginning with `!` are header cells. Cells
    /// containing only `>` trigger colspan; only `~` triggers rowspan.
    Table {
        /// Header rows (row-type suffix `h`). Stored as a single `TableRow`
        /// because SugarCube emits one `<thead>` containing all `h` rows.
        /// `None` if there are no header rows.
        header: Option<TableRow>,
        /// Body rows (no row-type suffix).
        rows: Vec<TableRow>,
        /// Footer rows (row-type suffix `f`). `None` if there are no footer rows.
        footer: Option<TableRow>,
        /// Caption text from a `c`-suffix row, if any.
        caption: Option<String>,
        /// Byte range of the caption row, body-relative.
        caption_span: Option<Range<usize>>,
        /// CSS class name from a `k`-suffix row, if any.
        class: Option<String>,
        /// Byte range of the class row, body-relative.
        class_span: Option<Range<usize>>,
        /// Byte range of the entire table, body-relative.
        span: Range<usize>,
    },

    /// Block code: `{{{\n...\n}}}` (raw content, no macro processing).
    ///
    /// SugarCube's `monospacedByBlock` parser requires `{{{` immediately
    /// followed by a newline at column 0, and `}}}` alone on its own line
    /// (see plan.md §3.10.1). The content is HTML-escaped and rendered as
    /// `<pre><code>…</code></pre>`. Macros do NOT execute inside.
    CodeBlock {
        /// Raw content between `{{{\n` and `\n}}}`. Not recursively parsed.
        content: String,
        /// Byte range of the entire code block (`{{{` through `}}}`).
        span: Range<usize>,
    },

    /// Inline code: `{{{...}}}` appearing mid-line (raw content).
    ///
    /// Disambiguated from `CodeBlock` by position: `{{{` NOT at column 0 or
    /// NOT immediately followed by `\n` is inline code (see plan.md §3.10.2).
    /// The content is HTML-escaped and rendered as `<code>…</code>`. Macros
    /// do NOT execute inside. The closing `}}}` is the first one found
    /// (non-greedy).
    InlineCode {
        /// Raw content between `{{{` and `}}}`. Not recursively parsed.
        content: String,
        /// Byte range of the entire inline code (`{{{` through `}}}`).
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
///
/// `known_macro_names` is the set of all macro names the system knows about
/// (builtin catalog names + custom macro names from the registry). The
/// fallback that sends `$var`-containing args to oxc only fires for macros
/// NOT in this set — this prevents false JS validation errors on custom
/// widgets like `<<statblock "Strength" $stats.strength>>` whose args use
/// SugarCube discrete-argument syntax, not JS.
pub fn collect_js_snippets(
    nodes: &[AstNode],
    known_macro_names: &std::collections::HashSet<String>,
) -> Vec<JsSnippet> {
    let mut result = Vec::new();
    collect_js_snippets_recursive(nodes, &mut result, known_macro_names);
    result
}

fn collect_js_snippets_recursive(
    nodes: &[AstNode],
    result: &mut Vec<JsSnippet>,
    known_macro_names: &std::collections::HashSet<String>,
) {
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
            } else if super::macros::inline_js_macro_names().contains(name.as_str()) {
                // Inline JS: the args are a JS expression.
                // Uses the catalog-derived inline_js_macro_names() so the list
                // stays in sync with the macro catalog automatically.
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
            } else if name != "for"
                && (args.contains('$') || args.contains('_'))
                && !known_macro_names.contains(name.as_str())
            {
                // Fallback: any UNKNOWN macro whose args contain $var or _var
                // references likely contains a JS expression. This catches
                // macros not in the catalog at all (e.g., <<goto $target>>
                // when goto isn't yet in the catalog, or custom macros that
                // take a variable reference).
                //
                // CRITICAL: we ONLY apply this fallback when the macro is NOT
                // in the builtin catalog. Macros like <<textbox>>, <<checkbox>>,
                // <<radiobutton>>, <<listbox>>, <<cycle>>, etc. ARE in the
                // catalog with mixed arg kinds (Variable + String + Keyword +
                // Number) — their args are SugarCube discrete-argument syntax,
                // NOT valid JS. Sending `"$playerName" $playerName "Time"
                // autofocus` to oxc produces false "Expected `,` or `)`"
                // errors.
                //
                // `inline_js_macro_names()` already correctly excludes these
                // mixed-arg macros. But this fallback was catching them anyway
                // because their args contain `$var`. The `find_macro(name)
                // .is_none()` guard ensures we only fall back for macros the
                // catalog doesn't know about — for catalog macros, the
                // deliberate inclusion/exclusion in `inline_js_macro_names()`
                // is respected.
                //
                // NOTE: `for` is excluded because its args use SugarCube's own
                // syntax (C-style, range, simple iteration, for-in) — not JS.
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
                collect_js_snippets_recursive(ch, result, known_macro_names);
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
