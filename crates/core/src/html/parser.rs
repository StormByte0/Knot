//! The HTML fragment parser: html5gum token stream → span-carrying CST.
//!
//! This is the "like oxc" integration (plan.md D3): `knot-core` owns the
//! parser; format crates access it on demand; the result is a CST with byte
//! spans (see [`crate::html::types`]).
//!
//! ## Why a tree builder at all
//!
//! html5gum is a tokenizer only — WHATWG tree construction (mis-nesting
//! fixes, implied end tags, foster parenting) is out of its scope. A real
//! browser-grade tree builder is neither needed nor wanted here: Knot's
//! consumers (zoning, semantic tokens, the Phase 2.2 htmlTag stratum) need
//! **faithful source spans** and a sane parent/child structure, not a
//! browser's DOM. The builder below therefore implements a small, tolerant
//! subset of tree construction:
//!
//! - open elements are tracked on a stack; a start tag pushes, a matching
//!   end tag pops (closing any unclosed inner elements first — the classic
//!   `<div><span></div>` tolerance);
//! - a stray end tag (no matching open element) is ignored, as the spec's
//!   tree construction does for most cases;
//! - elements still open at EOF are auto-closed (never an error — this is
//!   what makes malformed fragments total);
//! - raw-text elements ([`HtmlRawKind`]) capture their body as a single
//!   `RawText` child so nothing ever parses inside `<script>`/`<style>`.
//!
//! ## html5gum span semantics (measured empirically — see the tests)
//!
//! These facts are pinned by the test suite and must be re-verified on any
//! html5gum upgrade:
//!
//! - `StartTag.span` covers exactly `<div a="1">` (through the closing `>`),
//!   `EndTag.span` covers exactly `</div>`; quoted `>` inside attribute
//!   values does not terminate the tag.
//! - Tag/attribute names are lowercased; ranges are unaffected (ASCII).
//! - An unterminated tag at EOF (`<div class="x`) emits only an
//!   `EofInTag` error — the tag itself is dropped (spec behavior).
//! - Attribute spans: first byte of the name through the last byte of the
//!   value, **including** the `=` and any whitespace the author wrote
//!   around it. For quoted values this ends exactly past the closing
//!   quote; for unquoted values the raw span over-extends by one byte (the
//!   terminator: whitespace or `>`), which [`build_attr`] corrects;
//!   for valueless attributes the span is the name. An empty quoted value
//!   (`a=""`) collapses to the name-only extent — indistinguishable from
//!   valueless, see [`build_attr`]. `a=` directly before `>` is the spec's
//!   missing-attribute-value error: html5gum records it and still emits the
//!   tag, with the attribute collapsed to its name.
//! - Error tokens can precede the tag they belong to (missing-attribute-
//!   value, duplicate-attribute): the CST builder records them as
//!   diagnostics and then handles the tag; [`scan_leading_tag`] skips them
//!   when looking for the leading tag.
//! - Attribute values are entity-decoded in the token *value* but spans
//!   stay source-faithful; the builder takes values from the source slice,
//!   so no decoding ever leaks into the CST.
//! - Duplicate attributes keep the first occurrence (spec) + an error.

use html5gum::{DefaultEmitter, Span, Token, Tokenizer};

use super::types::{
    HtmlAttr, HtmlCst, HtmlDiagnostic, HtmlElement, HtmlElementKind, HtmlNode, HtmlNodeKind,
    HtmlRawKind, LeadingTagScan, ScannedTag,
};

/// HTML5 void elements — no content, no end tag.
/// <https://html.spec.whatwg.org/multipage/#void-elements>
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

fn is_void(name: &[u8]) -> bool {
    VOID_ELEMENTS.iter().any(|v| name == v.as_bytes())
}

/// Parse an HTML fragment into a span-carrying CST.
///
/// Total: never panics on input, never returns `Err`. Malformed markup
/// degrades to text nodes plus recoverable errors in
/// [`HtmlCst::errors`] (see the type's docs for why these are not
/// diagnostics by default).
pub fn parse_html_fragment(src: &str) -> HtmlCst {
    let mut builder = CstBuilder::new(src);
    builder.run();
    builder.finish()
}

/// Scan for a tag starting at position 0 of `src` (plan.md Phase 2.2).
///
/// The format-layer entry point behind the SugarCube htmlTag stratum: the
/// wikifier dispatches on `<` and needs to know whether a *tag* starts here
/// and where it ends. Returns `None` when `src` does not begin with a
/// well-formed tag:
///
/// - prose with a stray `<` (`5 < 6`, `<3`) — the tokenizer degrades these
///   to text (spec: invalid-first-character-of-tag-name), exactly like
///   upstream's htmlTag regex failing to match;
/// - `< div` (whitespace after `<` — not a tag-open character);
/// - comments/doctype/bogus markup (`<!--`, `<!DOCTYPE`, `<?xml`) — the
///   first real token is not a tag (the SugarCube `<!--` arm handles
///   comments before this is ever consulted);
/// - an unterminated tag at EOF (`<div class="x`) — the tokenizer drops
///   the tag and emits only an `eof-in-tag` error (spec), matching
///   upstream where the htmlTag regex requires the closing `>`.
///
/// Tokenizer **error tokens do not reject the scan**: html5gum may emit an
/// error before the tag itself (`<div a= >` records missing-attribute-value
/// and `<p a="1" a="2">` records duplicate-attribute, then the tag token) —
/// those tags are still tags, so leading error tokens are skipped. A tag
/// that only *fails to exist* (stray `<` → error then text) stays `None`
/// because the first structural token is text, not a tag.
///
/// Total: never panics, never returns `Err`.
pub fn scan_leading_tag(src: &str) -> Option<ScannedTag> {
    scan_leading_tag_detailed(src).tag
}

/// The reporting variant of [`scan_leading_tag`]: same question, but the
/// tokenizer errors skipped on the way to the first structural token are
/// returned alongside the tag (see [`LeadingTagScan`] for the contract and
/// the no-tag error shapes).
///
/// The errors are the ones the plain scan silently discards —
/// `duplicate-attribute`, `missing-attribute-value` before an emitted tag,
/// or `eof-in-tag` when no tag ever forms. Consumers relaying them as
/// diagnostics whitelist the codes they consider authoring mistakes; the
/// rest (prose-`<` shapes like `invalid-first-character-of-tag-name`) are
/// legal Twee source and must stay silent.
///
/// Total: never panics, never returns `Err`.
pub fn scan_leading_tag_detailed(src: &str) -> LeadingTagScan {
    // Same tokenizer configuration as the CST builder (spans + raw-text
    // state switching; StringReader is Infallible). Only the FIRST emitted
    // *structural* token matters: a leading tag starts at slice position 0,
    // so anything else (text, comment, doctype, error-then-text) means "not
    // a leading tag". Error tokens are collected for the caller (the CST
    // builder records them as diagnostics; the plain scan drops them).
    let mut emitter = DefaultEmitter::<usize>::new_with_span();
    emitter.naively_switch_states(true);
    let mut tokenizer = Tokenizer::new_with_emitter(src, emitter);
    let mut errors = Vec::new();
    let token: Option<Token<usize>> = loop {
        let Some(tok) = tokenizer.next() else {
            break None; // EOF before any structural token
        };
        let tok: Token<usize> = tok.expect("StringReader cannot fail");
        if let Token::Error(e) = tok {
            errors.push(HtmlDiagnostic {
                code: e.value.as_str().to_string(),
                span: e.span.start..e.span.end,
            });
            continue;
        }
        break Some(tok);
    };
    let tag = match token {
        Some(Token::StartTag(tag)) => {
            let Span { start, end } = tag.span;
            if start != 0 {
                // Defensive: the first token always covers position 0
                // (leading text would have been a String token), so a
                // non-zero start cannot be a *leading* tag.
                None
            } else {
                let name_bytes: &[u8] = &tag.name;
                let name = String::from_utf8_lossy(name_bytes).into_owned();
                let name_range = start + 1..start + 1 + name.len();
                let mut attrs: Vec<HtmlAttr> = tag
                    .attributes
                    .iter()
                    .map(|(aname, avalue)| build_attr(src, aname, avalue.span))
                    .collect();
                // html5gum yields attributes alphabetically (BTreeMap);
                // restore source order (same correction as the CST builder).
                attrs.sort_by_key(|a| a.range.start);
                Some(ScannedTag {
                    name,
                    name_range,
                    is_end: false,
                    self_closing: tag.self_closing,
                    attrs,
                    range: start..end,
                })
            }
        }
        Some(Token::EndTag(tag)) => {
            let Span { start, end } = tag.span;
            if start != 0 {
                return LeadingTagScan { tag: None, errors };
            }
            let name = String::from_utf8_lossy(&tag.name).into_owned();
            let name_range = start + 2..start + 2 + name.len();
            Some(ScannedTag {
                name,
                name_range,
                is_end: true,
                self_closing: false,
                // End-tag attributes are ignored per the spec; upstream's
                // terminator regex (`<\/name\s*>`) allows none either.
                attrs: Vec::new(),
                range: start..end,
            })
        }
        // Anything else (text, comment, doctype) — the slice does not
        // begin with a tag.
        _ => None,
    };
    LeadingTagScan { tag, errors }
}

/// One open element on the builder's stack.
struct OpenElement {
    node: HtmlNode,
    /// Set when this element is a raw-text element: the next `String`
    /// token(s) become its `RawText` body and the matching end tag closes
    /// it. Raw-text elements don't nest (per tokenizer states).
    raw: Option<HtmlRawKind>,
}

struct CstBuilder<'a> {
    src: &'a str,
    roots: Vec<HtmlNode>,
    stack: Vec<OpenElement>,
    errors: Vec<HtmlDiagnostic>,
}

impl<'a> CstBuilder<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            roots: Vec::new(),
            stack: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn run(&mut self) {
        // Spanned tokens (S = usize) + naive state switching (raw-text
        // bodies arrive as String tokens instead of being re-tokenized as
        // markup). StringReader's error type is Infallible, so unwrap is
        // total.
        let mut emitter = DefaultEmitter::<usize>::new_with_span();
        emitter.naively_switch_states(true);
        let tokenizer = Tokenizer::new_with_emitter(self.src, emitter);
        for token in tokenizer {
            let token: Token<usize> = token.expect("StringReader cannot fail");
            self.handle_token(token);
        }
        // Auto-close anything left open — tolerated, never an error.
        while let Some(open) = self.stack.pop() {
            self.attach(open.node);
        }
    }

    fn handle_token(&mut self, token: Token<usize>) {
        match token {
            Token::StartTag(tag) => {
                let Span { start, end } = tag.span;
                let name_bytes: &[u8] = &tag.name;
                let name = String::from_utf8_lossy(name_bytes).into_owned();
                // Tag names are ASCII (lowercased by the tokenizer), so the
                // name extent is start+1 .. start+1+len regardless of the
                // source's casing.
                let name_range = start + 1..start + 1 + name.len();
                let raw = HtmlRawKind::from_tag_name(name_bytes);
                let kind = if let Some(raw) = raw {
                    HtmlElementKind::RawText(raw)
                } else if is_void(name_bytes) {
                    HtmlElementKind::Void
                } else if tag.self_closing {
                    HtmlElementKind::SelfClosing
                } else {
                    HtmlElementKind::Normal
                };

                let mut attrs: Vec<HtmlAttr> = tag
                    .attributes
                    .iter()
                    .map(|(aname, avalue)| build_attr(self.src, aname, avalue.span))
                    .collect();
                // html5gum yields attributes alphabetically (BTreeMap);
                // restore source order for the CST.
                attrs.sort_by_key(|a| a.range.start);

                let element = HtmlElement {
                    name,
                    name_range,
                    attrs,
                    kind,
                    start_tag_range: start..end,
                    end_tag_range: None,
                };
                let node = HtmlNode {
                    kind: HtmlNodeKind::Element(element),
                    range: start..end,
                    children: Vec::new(),
                };

                // Which elements open a scope (get pushed on the stack)?
                // - void: never (no content by definition)
                // - self-closing: never (Knot honors the solidus — see
                //   HtmlElementKind::SelfClosing for the divergence note)
                // - everything else: pushed; raw-text elements carry their
                //   kind so the following String token(s) become the body.
                let opens_scope = !matches!(
                    node.kind,
                    HtmlNodeKind::Element(HtmlElement {
                        kind: HtmlElementKind::Void | HtmlElementKind::SelfClosing,
                        ..
                    })
                );

                if opens_scope {
                    let raw = match &node.kind {
                        HtmlNodeKind::Element(HtmlElement {
                            kind: HtmlElementKind::RawText(r),
                            ..
                        }) => Some(*r),
                        _ => None,
                    };
                    self.stack.push(OpenElement { node, raw });
                } else {
                    self.attach(node);
                }
            }
            Token::EndTag(tag) => {
                let Span { start, end } = tag.span;
                let name: &[u8] = &tag.name;
                // Find the nearest matching open element. Closing it also
                // auto-closes anything opened after it (mis-nesting
                // tolerance). No match ⇒ stray end tag ⇒ ignored (spec).
                // `plaintext` can never appear here: the tokenizer never
                // leaves PLAINTEXT state, so no end tag token follows it.
                if let Some(pos) = self
                    .stack
                    .iter()
                    .rposition(|open| open.node.element_name().as_bytes() == name)
                {
                    while self.stack.len() > pos + 1 {
                        let inner = self.stack.pop().expect("len checked");
                        self.attach(inner.node);
                    }
                    let mut closed = self.stack.pop().expect("len checked");
                    if let HtmlNodeKind::Element(el) = &mut closed.node.kind {
                        el.end_tag_range = Some(start..end);
                    }
                    closed.node.range.end = end;
                    self.attach(closed.node);
                }
            }
            Token::String(s) => {
                let range = s.span.start..s.span.end;
                // If the innermost open element is a raw-text element, its
                // body arrives here (normally one folded String token;
                // html5gum may split it, in which case adjacent pieces are
                // merged back into one node).
                let is_raw_body = matches!(self.stack.last().map(|top| top.raw), Some(Some(_)));
                if is_raw_body {
                    let top = self.stack.last_mut().expect("checked above");
                    merge_rawtext_child(top, range);
                } else {
                    self.attach(HtmlNode {
                        kind: HtmlNodeKind::Text,
                        range,
                        children: Vec::new(),
                    });
                }
            }
            Token::Comment(c) => {
                self.attach(HtmlNode {
                    kind: HtmlNodeKind::Comment,
                    range: c.span.start..c.span.end,
                    children: Vec::new(),
                });
            }
            Token::Doctype(d) => {
                self.attach(HtmlNode {
                    kind: HtmlNodeKind::Doctype,
                    range: d.span.start..d.span.end,
                    children: Vec::new(),
                });
            }
            Token::Error(e) => {
                self.errors.push(HtmlDiagnostic {
                    code: e.value.as_str().to_string(),
                    span: e.span.start..e.span.end,
                });
            }
        }
    }

    /// Attach a completed node to the innermost open element, or to the
    /// roots when the stack is empty. Extends the parent element's range to
    /// cover the child (an element's range grows with its content until its
    /// end tag arrives, or to EOF when auto-closed).
    fn attach(&mut self, node: HtmlNode) {
        if let Some(top) = self.stack.last_mut() {
            top.node.range.end = top.node.range.end.max(node.range.end);
            top.node.children.push(node);
        } else {
            self.roots.push(node);
        }
    }

    fn finish(self) -> HtmlCst {
        HtmlCst {
            nodes: self.roots,
            errors: self.errors,
        }
    }
}

/// Convert html5gum's attribute span into our [`HtmlAttr`] shape,
/// correcting the measured over-extensions (see module docs).
///
/// Free function so both the CST builder and [`scan_leading_tag`] share the
/// exact same span semantics.
///
/// The tokenizer emits, per attribute, a span that runs from the first byte
/// of the **name** to the last byte of the **value** — with the separator
/// (`=`, and any whitespace the author wrote around it) in between, and one
/// terminator byte over-extended for unquoted values. This function re-derives
/// the value's true extent from the source bytes, following the WHATWG
/// tokenizer's own state machine ("before attribute value state" skips
/// whitespace before the value; the value may be single- or double-quoted, or
/// unquoted):
///
/// - **Quoted** (`a = "b"`): the raw span ends exactly past the closing
///   quote, so `value` covers the bytes between the quotes and `range` runs
///   from the name through the closing quote.
/// - **Unquoted** (`a = b`): the raw span includes one terminator byte
///   (whitespace or `>`) after the value, which is stripped; `value` starts
///   at the first byte after the separator whitespace.
/// - **Valueless** (`d`, and `a=` followed by `>` — the spec's
///   missing-attribute-value error, which html5gum records before emitting
///   the tag): the span collapses to the name extent; `value_range` is
///   `None`.
/// - **Empty quoted** (`a=""`, `a = ''`): html5gum's span collapses to the
///   name extent too, so these are indistinguishable from valueless
///   attributes — `value_range` is `None` (documented limitation; the empty
///   range would carry no highlightable bytes either way).
fn build_attr(src: &str, name: &[u8], span: Span<usize>) -> HtmlAttr {
    let name = String::from_utf8_lossy(name).into_owned();
    let name_len = name.len();
    let raw_slice = &src[span.start..span.end];
    let raw_bytes = raw_slice.as_bytes();

    // Valueless attribute: span == name extent.
    if raw_bytes.len() <= name_len {
        return HtmlAttr {
            name,
            name_range: span.start..span.end,
            value_range: None,
            range: span.start..span.end,
        };
    }

    // Locate the `=` between name and value. Names cannot contain `=` in
    // anything Knot treats as a tag (html5gum tokenizes the first `=` after
    // the name as the value separator).
    let Some(eq_rel) = raw_bytes[name_len..]
        .iter()
        .position(|b| *b == b'=')
        .map(|i| i + name_len)
    else {
        // Separator bytes without an `=` cannot occur in an html5gum span
        // (defensive): treat as valueless.
        return HtmlAttr {
            name,
            name_range: span.start..span.start + name_len,
            value_range: None,
            range: span.start..span.start + name_len,
        };
    };

    // Before-attribute-value state: whitespace between `=` and the value is
    // skipped (WHATWG) — `a= "b"`, `a = b` and `a = "b"` all have the value
    // starting after it.
    let mut v = eq_rel + 1;
    while matches!(
        raw_bytes.get(v),
        Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
    ) {
        v += 1;
    }
    let value_start = span.start + v;

    match raw_bytes.get(v) {
        Some(&quote @ (b'"' | b'\'')) => {
            // Quoted value. In an emitted tag the raw span ends exactly past
            // the closing quote (a quoted value always terminates before the
            // tag does — an unterminated one swallows the `>` and the whole
            // tag is dropped with an eof-in-tag error instead). The closing
            // check is defensive for hypothetical shapes.
            let closes = raw_bytes.last() == Some(&quote) && raw_bytes.len() - 1 > v;
            if closes {
                HtmlAttr {
                    name,
                    name_range: span.start..span.start + name_len,
                    value_range: Some(value_start + 1..span.start + raw_bytes.len() - 1),
                    range: span.start..span.end,
                }
            } else {
                // Pathological unterminated quote: keep the tokenizer's
                // extent as-is.
                HtmlAttr {
                    name,
                    name_range: span.start..span.start + name_len,
                    value_range: Some(value_start + 1..span.end),
                    range: span.start..span.end,
                }
            }
        }
        _ => {
            // Unquoted value: the raw span includes one terminator byte
            // (whitespace or `>`) — strip it. The missing-attribute-value
            // shape (`a= >`) never reaches here: html5gum collapses its span
            // to the name extent above. The `max` guards the hypothetical
            // all-whitespace tail (keep the range valid, possibly empty).
            let value_end = (span.end - 1).max(value_start);
            HtmlAttr {
                name,
                name_range: span.start..span.start + name_len,
                value_range: Some(value_start..value_end),
                range: span.start..value_end,
            }
        }
    }
}

/// Merge a raw-text body range into the top element's single `RawText`
/// child (html5gum may split the body across String tokens; the body is
/// one source range when the pieces are adjacent).
fn merge_rawtext_child(top: &mut OpenElement, range: std::ops::Range<usize>) {
    if let Some(last) = top.node.children.last_mut().filter(|last| {
        matches!(last.kind, HtmlNodeKind::RawText(_)) && last.range.end == range.start
    }) {
        last.range.end = range.end;
        top.node.range.end = top.node.range.end.max(range.end);
        return;
    }
    let raw_kind = top.raw.expect("raw-text caller");
    top.node.children.push(HtmlNode {
        kind: HtmlNodeKind::RawText(raw_kind),
        range: range.clone(),
        children: Vec::new(),
    });
    top.node.range.end = top.node.range.end.max(range.end);
}
