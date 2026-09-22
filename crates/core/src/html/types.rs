//! HTML types — the span-carrying CST produced by [`crate::html::parser`].
//!
//! This module is the HTML equivalent of [`crate::oxc::types`] for JS. The
//! CST is built by `parse_html_fragment()` on top of the html5gum tokenizer
//! and is **tolerant by design**: malformed input never fails, it degrades
//! to `Text` nodes plus recorded (recoverable) tokenizer errors.
//!
//! ## Coordinate system
//!
//! All ranges are **byte offsets into the source string passed to
//! [`crate::html::parse_html_fragment`]** — NOT passage-relative, NOT
//! document-absolute. Callers that parse a sub-region of a passage must
//! shift ranges by the region offset themselves (same contract as the oxc
//! integration, where JS snippets are parsed with shifted spans).
//!
//! ## Source fidelity
//!
//! Ranges always point at the **source bytes**. Where the HTML spec requires
//! the tokenizer to decode character references (`&amp;` → `&`, in text,
//! attribute values, and RCDATA contents such as `<textarea>`), the decoded
//! bytes differ from the source slice — ranges still cover the *source*
//! extent (`&amp;` is 5 source bytes). Consumers that need rendered text
//! decode from the source slice themselves; consumers that need
//! highlighting/zoning use the ranges as-is. `<script>` bodies are raw text
//! per spec — no decoding happens there.
//!
//! Tag/attribute names are lowercased by the tokenizer (ASCII case folding
//! is 1:1, so a name's range covers the same byte extent in the source even
//! when the source spells it `<DIV>`).

use std::ops::Range;

/// Which raw-text flavor an element's content is tokenized in.
///
/// Mirrors html5gum's `naive_next_state` mapping (which follows the WHATWG
/// tokenizer's switching rules) — see
/// <https://html.spec.whatwg.org/multipage/#tokenization> and the
/// "optional" raw-text element list in the tree-construction spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlRawKind {
    /// `<script>` — SCRIPT_DATA state. Not decoded, `<` inside JS is text.
    Script,
    /// `<style>` — RAWTEXT state.
    Style,
    /// `<textarea>` — RCDATA state (character references ARE decoded here).
    TextArea,
    /// `<title>` — RCDATA state.
    Title,
    /// `<xmp>` — RAWTEXT state (obsolete but still tokenized raw).
    Xmp,
    /// `<iframe>` — RAWTEXT state.
    Iframe,
    /// `<noembed>` — RAWTEXT state.
    NoEmbed,
    /// `<noframes>` — RAWTEXT state.
    NoFrames,
    /// `<noscript>` — RAWTEXT state when scripting is enabled (Knot's
    /// default, matching browsers).
    NoScript,
    /// `<plaintext>` — PLAINTEXT state: everything to EOF is text and the
    /// element can never be closed.
    PlainText,
}

impl HtmlRawKind {
    /// The lowercase tag name this raw kind corresponds to.
    pub fn tag_name(self) -> &'static str {
        match self {
            HtmlRawKind::Script => "script",
            HtmlRawKind::Style => "style",
            HtmlRawKind::TextArea => "textarea",
            HtmlRawKind::Title => "title",
            HtmlRawKind::Xmp => "xmp",
            HtmlRawKind::Iframe => "iframe",
            HtmlRawKind::NoEmbed => "noembed",
            HtmlRawKind::NoFrames => "noframes",
            HtmlRawKind::NoScript => "noscript",
            HtmlRawKind::PlainText => "plaintext",
        }
    }

    /// Look up the raw kind for a (lowercase) tag name, if it is a raw-text
    /// element. `plaintext` is included: its body runs to EOF.
    pub fn from_tag_name(name: &[u8]) -> Option<HtmlRawKind> {
        match name {
            b"script" => Some(HtmlRawKind::Script),
            b"style" => Some(HtmlRawKind::Style),
            b"textarea" => Some(HtmlRawKind::TextArea),
            b"title" => Some(HtmlRawKind::Title),
            b"xmp" => Some(HtmlRawKind::Xmp),
            b"iframe" => Some(HtmlRawKind::Iframe),
            b"noembed" => Some(HtmlRawKind::NoEmbed),
            b"noframes" => Some(HtmlRawKind::NoFrames),
            b"noscript" => Some(HtmlRawKind::NoScript),
            b"plaintext" => Some(HtmlRawKind::PlainText),
            _ => None,
        }
    }
}

/// How an element terminates (or doesn't).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlElementKind {
    /// Has a matching end tag (or was auto-closed at EOF — check
    /// [`HtmlElement::end_tag_range`]).
    Normal,
    /// HTML5 void element (`br`, `img`, `input`, ...) — can have no content
    /// and no end tag.
    Void,
    /// Explicitly self-closed in source (`<circle/>`).
    ///
    /// Note: browsers only honor the solidus for void elements and foreign
    /// content (SVG/MathML); for ordinary HTML elements `<div/>` opens a
    /// div. Knot honors the solidus for ALL elements — predictable, and
    /// more correct for the SVG-heavy SugarCube use case. Documented
    /// divergence, deliberate.
    SelfClosing,
    /// Raw-text element (`script`, `style`, ...) — its body is tokenized in
    /// a special state. See [`HtmlRawKind`].
    RawText(HtmlRawKind),
}

/// One attribute of an element. Ranges are source-faithful (see module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlAttr {
    /// Lowercased attribute name (e.g. `class`, `sc-eval:class`).
    pub name: String,
    /// Source range of the name (same byte extent as the source spelling).
    pub name_range: Range<usize>,
    /// Source range of the value **without the surrounding quotes**, if the
    /// attribute has one. `None` for valueless attributes, and — a
    /// documented html5gum limitation — for **empty quoted** values
    /// (`a=""`): the tokenizer collapses their span to the name extent, so
    /// they are indistinguishable from valueless here (an empty range
    /// carries no highlightable bytes either way). Whitespace the author
    /// wrote between `=` and the value is not part of the range (WHATWG
    /// "before attribute value state" skips it). The bytes in this range
    /// are the *raw* source — character references are NOT decoded here
    /// (module docs).
    pub value_range: Option<Range<usize>>,
    /// Source range of the whole attribute as spelled: from the first byte
    /// of the name through the last byte of the value — **including** the
    /// `=`, any whitespace written around it, and the closing quote when
    /// quoted (excludes the one terminator byte html5gum over-extends for
    /// unquoted values, which the builder strips). Equal to `name_range`
    /// for valueless attributes.
    pub range: Range<usize>,
}

/// An element (`<div class="x">…</div>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlElement {
    /// Lowercased tag name.
    pub name: String,
    /// Source range of the tag name inside the start tag (after `<`).
    pub name_range: Range<usize>,
    /// Attributes in **source order** (html5gum yields them alphabetically
    /// via its BTreeMap; the builder re-sorts by position). Duplicate
    /// attributes: html5gum keeps the FIRST occurrence per the WHATWG spec
    /// and records an error — later duplicates are not in this list.
    pub attrs: Vec<HtmlAttr>,
    pub kind: HtmlElementKind,
    /// Source range of the entire start tag, e.g. `<div class="x">` —
    /// exactly as spelled, including attributes and the closing `>`.
    pub start_tag_range: Range<usize>,
    /// Source range of the end tag `</div>`. `None` when the element was
    /// auto-closed at EOF (tolerated, never an error) or has no end tag by
    /// construction (void / self-closing / raw kinds carry their own).
    pub end_tag_range: Option<Range<usize>>,
}

/// The kind of a CST node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlNodeKind {
    /// An element. See [`HtmlElement`].
    Element(HtmlElement),
    /// Character data between tags (or the whole source when nothing
    /// parses). Raw source bytes — character references are NOT decoded.
    Text,
    /// The body of a raw-text element ([`HtmlRawKind`]) — the content
    /// between the start and end tags as one node. For `<script>` this is
    /// the JS source; for `<style>` the CSS source (consumed by Phase 4.2).
    RawText(HtmlRawKind),
    /// `<!-- ... -->`. Range includes the delimiters.
    Comment,
    /// `<!DOCTYPE ...>`. Range includes the delimiters.
    Doctype,
}

/// A node of the HTML CST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlNode {
    pub kind: HtmlNodeKind,
    /// Source range covered by this node. For elements: start tag through
    /// end tag (or through the end of the last child when auto-closed).
    pub range: Range<usize>,
    /// Child nodes (elements, text, comments — nested structure). Only
    /// `Element` nodes ever have children; the raw body of a raw-text
    /// element is a single `RawText` child, so **no prose-style analysis
    /// recurses into raw-text bodies by accident** (the D2 no-black-holes
    /// rule: raw bodies are genuinely raw per upstream semantics).
    pub children: Vec<HtmlNode>,
}

/// A recoverable HTML tokenizer error (WHATWG parse error code + position).
///
/// These are spec parse errors, not authoring errors: prose like `5 < 6`
/// emits `invalid-first-character-of-tag-name`. Format consumers decide
/// whether to surface them — the SugarCube stratum (Phase 2.2) does NOT
/// (upstream SugarCube is silent on malformed HTML), they exist for
/// debugging and zone invariants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlDiagnostic {
    /// WHATWG parse error code, e.g. `eof-in-tag`.
    pub code: String,
    /// Source position the error was detected at (empty range).
    pub span: Range<usize>,
}

/// A single tag scanned from the start of a larger source string — the
/// format-layer service behind the SugarCube htmlTag stratum (plan.md
/// Phase 2.2).
///
/// This is deliberately NOT a tree node. The SugarCube wikifier needs exactly
/// one thing from the HTML side: *"does a tag start at position 0 of this
/// slice, and what are its source extents?"* Element pairing and content
/// handling stay in the format layer — upstream `htmlTag` (parserlib.js
/// L1704-1829) searches for its own case-insensitive terminator and
/// subWikifies the content, so Knot's format layer does the same (the tree
/// builder's pairing is a different consumer's concern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedTag {
    /// Lowercased tag name.
    pub name: String,
    /// Source range of the name (`div` in `<div …>`; after `</` for end
    /// tags). ASCII case folding is 1:1, so the range covers the source
    /// spelling even when the source is `<DIV>`.
    pub name_range: Range<usize>,
    /// `true` for `</name>` end tags.
    pub is_end: bool,
    /// `true` when a start tag ends with `/>`.
    pub self_closing: bool,
    /// Attributes in source order (html5gum yields them alphabetically; the
    /// scan re-sorts by position, same correction as the CST builder).
    /// Spans are source-faithful — same shape and over-extension fixes as
    /// [`HtmlAttr`]. Empty for end tags (end-tag attributes are ignored per
    /// the spec; upstream's terminator regex allows none).
    pub attrs: Vec<HtmlAttr>,
    /// Source range of the whole tag, exactly as spelled (`<div a="1">`,
    /// `</div>`).
    pub range: Range<usize>,
}

/// The full result of a leading-tag scan: the tag (when one starts the
/// slice) **plus every tokenizer error recorded before it**.
///
/// [`scan_leading_tag`](super::parser::scan_leading_tag) answers one
/// question — *"is there a tag here?"* — and throws the errors away
/// (a tag preceded by recoverable errors is still a tag). Consumers that
/// also want to *report* those errors (the diagnostics pipeline) use
/// [`scan_leading_tag_detailed`](super::parser::scan_leading_tag_detailed)
/// instead: the errors arrive with the tag so one scan serves both jobs.
///
/// ## When there is no tag
///
/// A `tag: None` outcome means the slice does not *begin* with a
/// well-formed tag — the errors then explain why:
///
/// - `eof-in-tag` — the tag never terminated (`<div class="x` at EOF).
///   The author almost certainly wrote a broken tag.
/// - `invalid-first-character-of-tag-name` (and friends) — prose `<`
///   (`5 < 6`, `a <3`). Legal in Twee source, **not** a reportable
///   condition; consumers whitelist the codes they relay.
///
/// Spans are slice-relative, same coordinate system as [`ScannedTag`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeadingTagScan {
    /// The tag, when a well-formed one starts the slice.
    pub tag: Option<ScannedTag>,
    /// Tokenizer errors recorded before the first structural token (or
    /// before EOF, when no structural token ever arrives).
    pub errors: Vec<HtmlDiagnostic>,
}

/// The parsed HTML fragment: a forest of top-level nodes plus the
/// recoverable tokenizer errors.
///
/// A fragment may legitimately have multiple roots (`<b>hi</b> there`) and
/// stray text (`just prose`) — HTML fragments are not required to have a
/// single root element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlCst {
    /// Top-level nodes in source order.
    pub nodes: Vec<HtmlNode>,
    /// Recoverable tokenizer errors (see [`HtmlDiagnostic`]).
    pub errors: Vec<HtmlDiagnostic>,
}

impl HtmlNode {
    /// Convenience: the element name when this node is an element
    /// (empty string for non-element nodes — use [`HtmlNodeKind`] for
    /// dispatch instead when the distinction matters).
    pub fn element_name(&self) -> &str {
        match &self.kind {
            HtmlNodeKind::Element(el) => &el.name,
            _ => "",
        }
    }

    /// Depth-first pre-order walk over this node and all descendants.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a HtmlNode)) {
        f(self);
        for child in &self.children {
            child.walk(f);
        }
    }
}
