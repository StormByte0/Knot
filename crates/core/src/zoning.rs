//! Unified zoning engine — type definitions and query methods.
//!
//! Classifies every byte of a passage body into one of five leaf kinds:
//! - [`LeafKind::Prose`] — bare narrative text that renders to the player.
//! - [`LeafKind::Markup`] — SugarCube formatting constructs (headings, lists,
//!   bold/italic, links, comments, code blocks, etc.). Conceptually also
//!   prose (renders to the player), kept separate for formatter dispatch.
//! - [`LeafKind::MacroTag`] — the `<<name args>>` / `<</name>>` / `<<=>>expr>>`
//!   token itself.
//! - [`LeafKind::Raw`] — foreign-language content (JS inside `<<script>>`,
//!   CSS inside a future `<<style>>`). Processed by external parsers (oxc
//!   for JS); the zone engine does NOT recurse into these.
//! - [`LeafKind::Error`] — parse errors. Carries the message so diagnostics
//!   can be emitted directly from the zone map.
//!
//! Additionally, a parallel array of [`MacroBody`] records carries the
//! enclosing-macro context for body regions, with full parent-chain
//! tracking for nested macros.
//!
//! ## Coordinate system
//!
//! All spans are **passage-relative** (0 = passage head `::`), matching the
//! canonical internal coordinate system. The zone builder (in `knot-formats`)
//! shifts body-relative AST spans → passage-relative at build time via
//! `body_offset_in_passage`. Translation to document-absolute LSP positions
//! happens at the LSP boundary only.
//!
//! ## Invariants
//!
//! - `leaves` is sorted by `span.start`.
//! - `leaves` covers the body region with no gaps and no overlaps (every byte
//!   is in exactly one leaf).
//! - `bodies` is sorted by `span.start` but MAY overlap (nested bodies).
//! - Every `LeafZone.body_idx` is either `None` (top-level) or a valid index
//!   into `bodies` whose `span` contains `leaf.span`.
//!
//! ## Why this lives in `knot-core`
//!
//! [`ZoneMap`] is stored on [`crate::passage::Passage`], which is the shared
//! document model. The *builder* logic (which walks the SugarCube AST) lives
//! in `knot-formats/src/zoning.rs`, but the *types* and *query methods* live
//! here so that `knot-core` has no dependency on `knot-formats`. Other
//! formats (Harlowe, Chapbook, Snowman) can adopt these types and provide
//! their own builders later.

use std::ops::Range;

use crate::types::{BodyRequirement, MacroKind};

// ---------------------------------------------------------------------------
// Leaf classification types
// ---------------------------------------------------------------------------

/// The language of a raw body region. Raw bodies are processed by external
/// parsers (oxc for JS, the `knot-core` html module for HTML, a future CSS
/// parser for CSS), not by the SugarCube parser. The zone engine does not
/// recurse into them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RawLanguage {
    /// JavaScript — processed by oxc. Currently `<<script>>` bodies.
    Js,
    /// CSS — processed by the `knot_core::css` tokenizer (plan.md Phase
    /// 4.2): `<style>` bodies, `<<style>>`/`<<css>>` blocks, stylesheet
    /// passages and `style="…"` attribute values.
    Css,
    /// HTML — parsed by `knot_core::html` (html5gum CST). Used by the
    /// Phase 2.2 htmlTag stratum (atomic tag interiors) and 2.3 raw-text
    /// zones; see plan.md Phase 2.
    Html,
}

/// The per-byte language authority answer (plan.md Phase 4.1): a coarse
/// classification of which language owns a byte, derived from the leaf kind
/// via [`LeafKind::language`]. Position-based LSP features consult this
/// instead of re-deriving context from raw text (the F6 bug class).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// SugarCube/Twine markup: prose, links, macro tags, format delimiters,
    /// errors — anything the story-format parser itself owns.
    Markup,
    /// Raw HTML bytes (tag interiors, raw-text tags). Inert for prose
    /// analysis.
    Html,
    /// Raw JavaScript bytes (script bodies / script passages).
    Js,
    /// Raw CSS bytes (style bodies).
    Css,
}

/// Broad classification of a parse error for diagnostic routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ErrorKind {
    /// Orphan close tag (no matching open on the stack).
    OrphanClose,
    /// Generic parse error.
    Parse,
    /// Unknown macro (not in builtin catalog or custom registry).
    /// Used by Phase 8 diagnostics; the leaf itself is still a `MacroTag`.
    UnknownMacro,
}

/// The specific kind of SugarCube markup construct. Used by the formatter
/// and future highlighting to dispatch on the construct type.
///
/// Conceptually, markup IS prose — it renders to the player. We keep it as
/// a separate leaf kind so the formatter can dispatch on the specific kind
/// (e.g., indent headings, wrap list items).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MarkupKind {
    /// `!` through `!!!!!!` (1-6 levels).
    Heading,
    /// `*`/`**`/`#`/`##` at column 0.
    ListItem,
    /// `>`/`>>` (line) or `<<<\n...\n<<<` (block).
    Blockquote,
    /// `----` (4+ dashes alone on a line).
    HorizontalRule,
    /// `|...|` TiddlyWiki-style table.
    Table,
    /// `@@class;text@@` inline styling.
    InlineStyle,
    /// `''bold''`, `//italic//`, `__underline__`, `==strike==`, `~~sub~~`, `^^super^^`.
    TextFormat,
    /// `[[...]]` or `[img[...]]`.
    Link,
    /// `/%...%/`, `/*...*/`, `//...//` (JS line), `<!--...-->`.
    Comment,
    /// `{{{\n...\n}}}` block code (raw, not parsed).
    CodeBlock,
    /// `{{{...}}}` inline code (raw, not parsed).
    InlineCode,
    /// `"""..."""` or `<nowiki>...</nowiki>` (raw, not parsed).
    Verbatim,
}

/// Which part of a macro tag this leaf represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TagPart {
    /// `<<name args>>` — the full opening tag.
    Open,
    /// `<</name>>` — the full closing tag.
    Close,
    /// `<<=>>expr>>` or `<<->>expr>>` — the full expression tag (no body).
    Expression,
}

/// The classification of a single byte-covering leaf zone.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum LeafKind {
    /// Bare narrative text that renders to the player.
    ///
    /// `is_prose: false` for text inside non-rendering macros (`<<silently>>`,
    /// `<<script>>`) — equivalent to today's `AstNode::Text.is_prose` flag.
    ///
    /// Note: `Markup` leaves also render to the player — they're prose that
    /// happens to be formatted. `is_prose: false` here means "this text is
    /// code/non-rendering" (e.g., inside `<<silently>>`), not "this is the
    /// only kind of prose". See [`ZoneMap::renders_to_player`] for the union
    /// query.
    Prose { is_prose: bool },

    /// SugarCube formatting construct. See [`MarkupKind`] for subtypes.
    Markup(MarkupKind),

    /// The `<<name args>>` or `<</name>>` or `<<=>>expr>>` token itself.
    MacroTag {
        macro_name: String,
        part: TagPart,
        /// Catalog snapshot at parse time. `None` for unknown macros.
        /// Custom widgets are registered early (via `[widget]` tags processed
        /// before normal passages), so if this is `None`, the macro is
        /// genuinely unknown — Phase 8 emits an "unknown macro" diagnostic.
        macro_kind: Option<MacroKind>,
        body_requirement: Option<BodyRequirement>,
        /// True if this is an orphan close tag (no matching open on the stack).
        orphan: bool,
    },

    /// Raw foreign-language content — JS inside `<<script>>`, CSS inside a
    /// future `<<css>>`/`<<style>>`. Processed by external parsers; the zone
    /// engine does NOT recurse into these. The formatter defers these to
    /// language-specific sub-formatters.
    Raw { language: RawLanguage },

    /// A parse error. Carries the message so diagnostics can be emitted
    /// directly from the zone map without re-walking the AST.
    Error { message: String, kind: ErrorKind },
}

impl LeafKind {
    /// The [`Language`] this leaf's bytes belong to (plan.md Phase 4.1):
    /// raw leaves report their foreign language, everything else is the
    /// story format's own markup.
    pub fn language(&self) -> Language {
        match self {
            LeafKind::Raw {
                language: RawLanguage::Html,
            } => Language::Html,
            LeafKind::Raw {
                language: RawLanguage::Js,
            } => Language::Js,
            LeafKind::Raw {
                language: RawLanguage::Css,
            } => Language::Css,
            _ => Language::Markup,
        }
    }
}

/// A single byte-covering, non-overlapping leaf zone.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LeafZone {
    /// Passage-relative byte range.
    pub span: Range<usize>,
    /// What kind of zone this is.
    pub kind: LeafKind,
    /// Index into [`ZoneMap::bodies`]. `None` means this leaf is at the top
    /// level of the passage (not inside any macro body).
    pub body_idx: Option<usize>,
}

/// A macro body region. One per open/close pair (or unclosed block macro).
///
/// Bodies MAY overlap (nested bodies). Each body carries a `parent_body`
/// index so the full ancestor chain is recoverable in O(depth).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MacroBody {
    /// `[open_span.end, close_span.start)` — body-only, excludes tags.
    /// If unclosed, `span.end` is the end of the body region the parser
    /// assigned (typically end of passage, or end of the parent body).
    pub span: Range<usize>,

    /// Passage-relative span of just the macro name in the open tag
    /// (e.g., `link` in `<<link "X">>`). Carried so consumers don't need
    /// to re-scan the open tag to find the name position.
    pub name_span: Range<usize>,

    /// Passage-relative span of the full `<</name>>` close tag, if present.
    /// `None` if unclosed.
    pub close_span: Option<Range<usize>>,

    /// Passage-relative span of just the name in the close tag.
    /// `None` if unclosed or if the close tag had no name.
    pub close_name_span: Option<Range<usize>>,

    /// The macro name (e.g., "link", "if", "set").
    pub macro_name: String,

    /// Catalog snapshot at parse time. `None` for unknown macros.
    pub macro_kind: Option<MacroKind>,

    /// Body requirement from the catalog. `None` for unknown macros.
    pub body_requirement: Option<BodyRequirement>,

    /// `true` if the open tag had no matching close (unclosed block macro).
    pub unclosed: bool,

    /// Index into [`ZoneMap::bodies`] for the parent body, or `None` at top level.
    pub parent_body: Option<usize>,

    /// 0 at top level, increments each time we enter a nested body.
    pub depth: u32,

    /// `Some(Js)` for `<<script>>` bodies (raw JS, processed by oxc).
    /// `Some(Css)` for future `<<style>>`/`<<css>>` bodies.
    /// `Some(Html)` for HTML zones (Phase 2.2 tag interiors / 2.3 raw-text
    /// elements).
    /// `None` for normal SugarCube bodies (recursively parsed).
    pub raw_language: Option<RawLanguage>,
}

// ---------------------------------------------------------------------------
// ZoneMap — the top-level structure
// ---------------------------------------------------------------------------

/// A byte-covering, non-overlapping array of leaf zones for one passage,
/// plus a parallel array of overlapping macro-body context records.
///
/// Built once from `PassageAst` after `tree_builder::build_tree` (by the
/// builder in `knot-formats/src/zoning.rs`). Stored on
/// [`crate::passage::Passage`] and accessible to all LSP handlers.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ZoneMap {
    leaves: Vec<LeafZone>,
    bodies: Vec<MacroBody>,
}

impl ZoneMap {
    /// Create an empty `ZoneMap` (no leaves, no bodies).
    pub fn new() -> Self {
        Self::default()
    }

    /// O(log n). Returns the innermost leaf zone containing `offset`.
    /// `offset` is **passage-relative** (0 = passage head `::`).
    pub fn leaf_at(&self, offset: usize) -> Option<&LeafZone> {
        // Binary search: find the leaf whose [start, end) contains offset.
        let idx = self
            .leaves
            .partition_point(|z| z.span.start <= offset)
            .checked_sub(1)?;
        let leaf = self.leaves.get(idx)?;
        if offset < leaf.span.end {
            Some(leaf)
        } else {
            None
        }
    }

    /// The per-byte language authority query (plan.md Phase 4.1): which
    /// language owns `offset`? Bytes not covered by any leaf (structural
    /// gaps between leaves) are [`Language::Markup`] by definition — in a
    /// passage body, uncovered bytes are prose/structure the format parser
    /// owns.
    ///
    /// O(log n). This is the query position-based features (completions,
    /// hovers, decorations) should consult instead of re-deriving context by
    /// re-scanning raw text.
    pub fn language_at(&self, offset: usize) -> Language {
        self.leaf_at(offset)
            .map(|l| l.kind.language())
            .unwrap_or(Language::Markup)
    }

    /// Zone invariants (plan.md Phase 4.1/5.1): leaves must be non-empty,
    /// sorted, and non-overlapping — the contract [`ZoneMap::leaf_at`]'s
    /// binary search depends on. Returns `Err` describing the first violated
    /// pair when an invariant breaks.
    pub fn validate(&self) -> Result<(), String> {
        let mut prev_end: Option<usize> = None;
        for leaf in &self.leaves {
            if leaf.span.start >= leaf.span.end {
                return Err(format!(
                    "leaf span must be non-empty, got {:?} ({:?})",
                    leaf.span, leaf.kind
                ));
            }
            if let Some(pe) = prev_end
                && leaf.span.start < pe
            {
                return Err(format!(
                    "leaves must be sorted and non-overlapping: leaf starting at {} \
                     begins before the previous leaf's end {}",
                    leaf.span.start, pe
                ));
            }
            prev_end = Some(leaf.span.end);
        }
        Ok(())
    }

    /// O(log n + depth). Returns the innermost macro body containing `offset`,
    /// or `None` if the offset is at the top level of the passage.
    pub fn enclosing_body_at(&self, offset: usize) -> Option<&MacroBody> {
        let leaf = self.leaf_at(offset)?;
        let body_idx = leaf.body_idx?;
        // Walk up the parent chain to find the innermost body that actually
        // contains the offset. (The leaf's body_idx is the immediate parent,
        // which by construction contains the leaf's span, so it contains offset.)
        self.bodies.get(body_idx)
    }

    /// O(depth). Returns the full ancestor stack, outermost-first.
    /// E.g., for `<<link>><<capture>><<if $x>>body<</if>><</capture>><</link>>`
    /// with the cursor inside `<<if>>`'s body, returns `[link, capture, if]`.
    pub fn body_stack_at(&self, offset: usize) -> Vec<&MacroBody> {
        let mut stack = Vec::new();
        let mut current = self.enclosing_body_at(offset);
        while let Some(body) = current {
            stack.push(body);
            current = body.parent_body.and_then(|idx| self.bodies.get(idx));
        }
        stack.reverse(); // outermost-first
        stack
    }

    /// O(n) walk. Used by folding range, semantic token builders, and the
    /// formatter.
    pub fn iter_leaves(&self) -> impl Iterator<Item = &LeafZone> {
        self.leaves.iter()
    }

    /// O(n) walk. Used by folding range and the formatter.
    pub fn iter_bodies(&self) -> impl Iterator<Item = &MacroBody> {
        self.bodies.iter()
    }

    /// For diagnostics: all bodies where `unclosed == true`.
    pub fn unclosed_bodies(&self) -> impl Iterator<Item = &MacroBody> {
        self.bodies.iter().filter(|b| b.unclosed)
    }

    /// Does this leaf render to the player? Returns `true` for
    /// `Prose { is_prose: true }` and all `Markup`. Returns `false` for
    /// `Prose { is_prose: false }`, `MacroTag`, `Raw`, and `Error`.
    pub fn renders_to_player(leaf: &LeafZone) -> bool {
        match &leaf.kind {
            LeafKind::Prose { is_prose } => *is_prose,
            LeafKind::Markup(_) => true,
            LeafKind::MacroTag { .. } | LeafKind::Raw { .. } | LeafKind::Error { .. } => false,
        }
    }

    /// Human-readable multi-line dump for insta snapshot tests.
    ///
    /// `body` is the full passage body text (used for snippet display).
    /// The `body` offsets in the zone map are passage-relative, so `body`
    /// should be the text from the passage head `::` onward (or the caller
    /// can pass a slice whose start aligns with offset 0).
    pub fn debug_dump(&self, body: &str) -> String {
        let mut out = String::new();
        out.push_str("=== Leaves ===\n");
        for leaf in &self.leaves {
            let snippet = &body[leaf.span.start.min(body.len())..leaf.span.end.min(body.len())];
            let snippet_display: String = snippet.chars().take(40).collect();
            let kind_str = match &leaf.kind {
                LeafKind::Prose { is_prose } => {
                    if *is_prose {
                        "Prose".to_string()
                    } else {
                        "Prose(non-rendering)".to_string()
                    }
                }
                LeafKind::Markup(k) => format!("Markup({:?})", k),
                LeafKind::MacroTag {
                    macro_name,
                    part,
                    macro_kind,
                    orphan,
                    ..
                } => {
                    let kind_str = macro_kind
                        .map(|k| format!("{:?}", k))
                        .unwrap_or_else(|| "Unknown".to_string());
                    let orphan_str = if *orphan { " [orphan]" } else { "" };
                    format!(
                        "MacroTag({:?} name={} kind={}{})",
                        part, macro_name, kind_str, orphan_str
                    )
                }
                LeafKind::Raw { language } => format!("Raw({:?})", language),
                LeafKind::Error { message, kind } => {
                    format!("Error({:?}: \"{}\")", kind, message)
                }
            };
            let body_str = leaf
                .body_idx
                .map(|i| format!("body={}", i))
                .unwrap_or_else(|| "body=None".to_string());
            out.push_str(&format!(
                "  [{:4}..{:4}) {:<45} {} | {:?}\n",
                leaf.span.start, leaf.span.end, kind_str, body_str, snippet_display
            ));
        }
        out.push_str("\n=== Bodies ===\n");
        if self.bodies.is_empty() {
            out.push_str("  (none)\n");
        } else {
            for (i, body) in self.bodies.iter().enumerate() {
                let parent_str = body
                    .parent_body
                    .map(|p| format!("parent={}", p))
                    .unwrap_or_else(|| "parent=None".to_string());
                let close_str = body
                    .close_span
                    .as_ref()
                    .map(|c| format!("[{}..{})", c.start, c.end))
                    .unwrap_or_else(|| "None".to_string());
                let raw_str = body
                    .raw_language
                    .map(|l| format!(", raw={:?}", l))
                    .unwrap_or_default();
                let unclosed_str = if body.unclosed { " [unclosed]" } else { "" };
                out.push_str(&format!(
                    "  [{}] [{:4}..{:4}) name={:<12} depth={} {} close={}{}{}\n",
                    i,
                    body.span.start,
                    body.span.end,
                    body.macro_name,
                    body.depth,
                    parent_str,
                    close_str,
                    raw_str,
                    unclosed_str
                ));
            }
        }
        out
    }

    // -----------------------------------------------------------------------
    // Builder access — the builder lives in `knot-formats` (a different
    // crate), so these constructors must be `pub`, not `pub(crate)`.
    // -----------------------------------------------------------------------

    /// Construct a `ZoneMap` from its parts. Used by the builder in
    /// `knot-formats`. Not intended for general use — prefer
    /// `ZoneMap::build_from_ast` (in `knot-formats`).
    pub fn from_parts(leaves: Vec<LeafZone>, bodies: Vec<MacroBody>) -> Self {
        Self { leaves, bodies }
    }

    /// Mutable access to the leaves vector. Used by the builder for
    /// sort/validate after construction.
    pub fn leaves_mut(&mut self) -> &mut Vec<LeafZone> {
        &mut self.leaves
    }

    /// Number of leaf zones.
    pub fn leaf_count(&self) -> usize {
        self.leaves.len()
    }

    /// Number of macro bodies.
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    /// Get a body by index.
    pub fn body(&self, idx: usize) -> Option<&MacroBody> {
        self.bodies.get(idx)
    }
}
