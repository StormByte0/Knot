//! CSS parsing service (plan.md Phase 6) — the `knot-core::css` equivalent
//! of [`crate::oxc`] for JS: a real parser in core, accessed by formats on
//! demand.
//!
//! [`parse_css()`] parses with `oxc-css-parser` (the raffia fork maintained
//! by the oxc-project org — the same ecosystem as our JS parser) and maps
//! the span-carrying AST into the stable [`CssToken`] stream, maps
//! `parser.comments()` into `Comment` tokens, and relays
//! `recoverable_errors()` as [`CssDiagnostic`]s (severity
//! [`CssDiagnosticSeverity::Error`] — a parse error is an error; the
//! user-facing policy is "show breaking code when written", never hide).
//!
//! ## Guarantees
//!
//! - **Total**: any input produces a well-formed [`CssParseOutcome`]. The
//!   underlying parser recovers at statement level (css-syntax-3 recovery:
//!   EOF closes blocks, bad strings are kept); only a hard `Err` falls back
//!   to the tolerant scanner in [`super::fallback`] — highlighting never
//!   goes dark.
//! - **No panics**: the crate parser allocates from its own arena and
//!   returns `Result`; the walk is total.
//! - **Sorted, non-overlapping tokens**: AST-derived tokens and collected
//!   comments are merged by start offset; overlapping spans keep the
//!   earlier-starting (longer) token, so the downstream semantic-token
//!   pipeline never sees disorder.
//!
//! ## Mapping policy
//!
//! Token kinds follow the *role* a node plays, matching the Phase 4.2
//! scanner's behavior so downstream consumers and tests keep working:
//! declaration names → Property (custom-property names included — the
//! upstream helper `InterpolableIdent::is_custom_property()` confirms the
//! shape), value literals → Keyword, numbers/dimensions/percentages/hex →
//! Number, strings → String, at-rule names (with the `@`) → AtRule,
//! selector parts → Selector, functions → Function, postcss `$var` /
//! dashed idents in value position → Variable, punctuation → Punctuation.
//!
//! The AST itself is deliberately NOT exposed across the `knot-core` API
//! boundary — these stable types are the isolation layer (the crate is
//! pinned `=0.0.15`; 0.0.x churn is real). Future handlers (e.g. go-to-
//! definition on custom properties) extend [`CssParseOutcome`] rather than
//! leaking crate types.

use super::fallback;
use super::types::{CssDiagnostic, CssDiagnosticSeverity, CssParseOutcome, CssToken, CssTokenKind};
use oxc_css_parser::{Allocator, ParserBuilder, Syntax, ast, token::TokenData};
use std::ops::Range;

/// Parse CSS source text (a stylesheet-shaped region: `[stylesheet]`
/// passages, `<style>` bodies, `<<style>>`/`<<css>>` blocks) and return
/// classified tokens + diagnostics. Spans are source-relative.
///
/// For declaration-list regions (`style="…"` attribute values) use
/// [`super::fallback::parse_css_declarations`] — that microsyntax is a
/// declaration list, not a stylesheet.
pub fn parse_css(source: &str) -> CssParseOutcome {
    let allocator = Allocator::default();
    let mut parser = ParserBuilder::new(&allocator, source)
        .syntax(Syntax::Css)
        .comments()
        .build();
    match parser.parse::<ast::Stylesheet>() {
        Ok(stylesheet) => {
            let mut tokens: Vec<CssToken> = Vec::new();
            walk_stylesheet(&stylesheet, source, &mut tokens);
            // Comments are collected separately by the parser (not AST
            // nodes) — they carry spans and become Comment tokens so themes
            // can color them.
            for comment in parser.comments() {
                tokens.push(CssToken {
                    kind: CssTokenKind::Comment,
                    span: comment.span.start..comment.span.end,
                });
            }
            // Relay recoverable errors — user policy: show breaking code
            // when written, never hide it.
            let diagnostics = parser
                .recoverable_errors()
                .iter()
                .map(|e| CssDiagnostic {
                    message: e.kind.to_string(),
                    range: e.span.start..e.span.end.min(source.len()),
                    severity: CssDiagnosticSeverity::Error,
                })
                .collect();
            CssParseOutcome {
                tokens: sort_and_dedupe(tokens),
                diagnostics,
            }
        }
        // Hard failure (no stylesheet at all): fall back to the tolerant
        // scanner so highlighting survives. Its recovery makes this path
        // rare; the scanner's unclosed-block summary is the only diagnostic.
        Err(_) => fallback::tokenize_css_fallback(source),
    }
}

// ---------------------------------------------------------------------------
// Token stream post-processing
// ---------------------------------------------------------------------------

/// Sort tokens by start offset and drop overlaps (earlier-start wins; on
/// equal starts the longer span wins). Comments and AST tokens never
/// overlap by construction, but the merge is defensive so the output is
/// always well-formed for the semantic-token pipeline.
fn sort_and_dedupe(mut tokens: Vec<CssToken>) -> Vec<CssToken> {
    tokens.sort_by(|a, b| {
        a.span
            .start
            .cmp(&b.span.start)
            .then(b.span.end.cmp(&a.span.end))
    });
    let mut kept: Vec<CssToken> = Vec::with_capacity(tokens.len());
    for tok in tokens {
        let overlaps = kept
            .last()
            .is_some_and(|prev| tok.span.start < prev.span.end);
        if !overlaps {
            kept.push(tok);
        }
    }
    kept
}

// ---------------------------------------------------------------------------
// Statement walk
// ---------------------------------------------------------------------------

fn walk_stylesheet(sheet: &ast::Stylesheet, source: &str, out: &mut Vec<CssToken>) {
    for statement in &sheet.statements {
        walk_statement(statement, source, out);
    }
}

fn walk_statement(statement: &ast::Statement, source: &str, out: &mut Vec<CssToken>) {
    match statement {
        ast::Statement::QualifiedRule(rule) => {
            walk_selector_list(&rule.selector, out);
            walk_simple_block(&rule.block, source, out);
        }
        ast::Statement::AtRule(at_rule) => walk_at_rule(at_rule, source, out),
        ast::Statement::Declaration(decl) => walk_declaration(decl, out),
        ast::Statement::KeyframeBlock(block) => {
            for selector in &block.selectors {
                match selector {
                    // `from` / `to` — a rule-level ident (Selector, like the
                    // old scanner's rule-context classification).
                    ast::KeyframeSelector::Ident(ident) => {
                        push(out, CssTokenKind::Selector, interpolable_ident_span(ident));
                    }
                    ast::KeyframeSelector::Percentage(pct) => {
                        push(out, CssTokenKind::Number, span_of(&pct.span));
                    }
                    // Scroll-driven timeline range (`entry 0%`): name +
                    // percentage, conservatively split.
                    ast::KeyframeSelector::TimelineRange(range) => {
                        push(
                            out,
                            CssTokenKind::Selector,
                            interpolable_ident_span(&range.name),
                        );
                        push(out, CssTokenKind::Number, span_of(&range.percentage.span));
                    }
                }
            }
            for comma in &block.comma_spans {
                push(out, CssTokenKind::Punctuation, span_of(comma));
            }
            walk_simple_block(&block.block, source, out);
        }
        // postcss-simple-vars shape: `$var: value;` at root (CSS-mode
        // acceptance per the crate's README — real projects run through
        // postcss plugins).
        ast::Statement::PostcssSimpleVarDeclaration(decl) => {
            push(out, CssTokenKind::Variable, span_of(&decl.name.span));
            push(out, CssTokenKind::Punctuation, span_of(&decl.colon_span));
            walk_values(&decl.value, RawCtx::Value, out);
        }
        // Raw-prelude rule (`x: { ... }`, `50% { }` outside @keyframes):
        // prelude stays raw tokens — classify as selectors.
        ast::Statement::UnknownQualifiedRule(rule) => {
            walk_token_seq(&rule.prelude, RawCtx::Selector, out);
            walk_simple_block(&rule.block, source, out);
        }
        // Sass/Less-only statement shapes cannot occur under `Syntax::Css`.
        _ => {}
    }
}

fn walk_at_rule(at_rule: &ast::AtRule, source: &str, out: &mut Vec<CssToken>) {
    // The at-rule token covers `@` + name (the Phase 4.2 scanner emitted
    // "@media" as one AtRule token — tests pin that text).
    let mut name_range = span_of(&at_rule.name.span);
    if name_range.start > 0 && source.as_bytes()[name_range.start - 1] == b'@' {
        name_range.start -= 1;
    }
    push(out, CssTokenKind::AtRule, name_range);
    if let Some(prelude) = &at_rule.prelude {
        walk_prelude(prelude, out);
    }
    if let Some(block) = &at_rule.block {
        walk_simple_block(block, source, out);
    }
}

fn walk_declaration(decl: &ast::Declaration, out: &mut Vec<CssToken>) {
    // Property name — custom properties included (`--x` was a Property in
    // the Phase 4.2 stream; `is_custom_property()` exists but the kind is
    // the same).
    push(
        out,
        CssTokenKind::Property,
        interpolable_ident_span(&decl.name),
    );
    push(out, CssTokenKind::Punctuation, span_of(&decl.colon_span));
    walk_values(&decl.value, RawCtx::Value, out);
    if let Some(important) = &decl.important {
        // `!important` — one Keyword token (the span includes the `!`).
        push(out, CssTokenKind::Keyword, span_of(&important.span));
    }
}

fn walk_simple_block(block: &ast::SimpleBlock, source: &str, out: &mut Vec<CssToken>) {
    // A simple block's span covers `{` through `}`; emit both braces as
    // Punctuation (the Phase 4.2 stream did) by reading them out of the
    // source — the crate stores only the outer span. Sass indented blocks
    // have no braces; the byte checks keep those brace-free.
    let bytes = source.as_bytes();
    let opens_with_brace = block.span.start < bytes.len() && bytes[block.span.start] == b'{';
    if opens_with_brace {
        push(
            out,
            CssTokenKind::Punctuation,
            block.span.start..block.span.start + 1,
        );
    }
    for statement in &block.statements {
        walk_statement(statement, source, out);
    }
    if opens_with_brace && block.span.end > block.span.start {
        let close_start = block.span.end - 1;
        if close_start < bytes.len() && bytes[close_start] == b'}' {
            push(out, CssTokenKind::Punctuation, close_start..close_start + 1);
        }
    }
}

// ---------------------------------------------------------------------------
// Selectors
// ---------------------------------------------------------------------------

fn walk_selector_list(list: &ast::SelectorList, out: &mut Vec<CssToken>) {
    for complex in &list.selectors {
        for child in &complex.children {
            match child {
                ast::ComplexSelectorChild::CompoundSelector(compound) => {
                    for simple in &compound.children {
                        walk_simple_selector(simple, out);
                    }
                }
                ast::ComplexSelectorChild::Combinator(combinator) => {
                    // Descendant combinators are whitespace — span may be
                    // zero-width or cover the gap; both are safe to skip.
                    if combinator.span.end > combinator.span.start {
                        push(out, CssTokenKind::Punctuation, span_of(&combinator.span));
                    }
                }
            }
        }
    }
    for comma in &list.comma_spans {
        push(out, CssTokenKind::Punctuation, span_of(comma));
    }
}

fn walk_simple_selector(selector: &ast::SimpleSelector, out: &mut Vec<CssToken>) {
    match selector {
        ast::SimpleSelector::Class(class) => {
            // Span covers `.name` (matches the Phase 4.2 one-token shape).
            push(out, CssTokenKind::Selector, span_of(&class.span));
        }
        ast::SimpleSelector::Id(id) => {
            push(out, CssTokenKind::Selector, span_of(&id.span));
        }
        ast::SimpleSelector::Type(type_sel) => match type_sel {
            ast::TypeSelector::TagName(tag) => {
                push(out, CssTokenKind::Selector, span_of(&tag.span))
            }
            ast::TypeSelector::Universal(universal) => {
                push(out, CssTokenKind::Selector, span_of(&universal.span));
            }
        },
        ast::SimpleSelector::Attribute(attr) => {
            push(out, CssTokenKind::Selector, span_of(&attr.name.span));
            if let Some(matcher) = &attr.matcher {
                push(out, CssTokenKind::Punctuation, span_of(&matcher.span));
            }
            if let Some(value) = &attr.value {
                match value {
                    ast::AttributeSelectorValue::Str(s) => {
                        push(out, CssTokenKind::String, interpolable_str_span(s));
                    }
                    ast::AttributeSelectorValue::Ident(i) => {
                        push(out, CssTokenKind::Selector, interpolable_ident_span(i));
                    }
                    ast::AttributeSelectorValue::Number(n) => {
                        push(out, CssTokenKind::Number, span_of(&n.span));
                    }
                    ast::AttributeSelectorValue::Dimension(d) => {
                        push(out, CssTokenKind::Number, span_of(&d.span));
                    }
                    ast::AttributeSelectorValue::Percentage(p) => {
                        push(out, CssTokenKind::Number, span_of(&p.span));
                    }
                    // Less escape / token-seq shapes cannot occur in CSS mode.
                    _ => {}
                }
            }
            if let Some(modifier) = &attr.modifier {
                push(out, CssTokenKind::Keyword, span_of(&modifier.span));
            }
        }
        ast::SimpleSelector::PseudoClass(pseudo) => {
            push(
                out,
                CssTokenKind::Selector,
                interpolable_ident_span(&pseudo.name),
            );
            if let Some(arg) = &pseudo.arg {
                push(out, CssTokenKind::Punctuation, span_of(&arg.l_paren));
                match &arg.kind {
                    ast::PseudoClassSelectorArgKind::CompoundSelectorList(list) => {
                        for compound in &list.selectors {
                            for simple in &compound.children {
                                walk_simple_selector(simple, out);
                            }
                        }
                        for comma in &list.comma_spans {
                            push(out, CssTokenKind::Punctuation, span_of(comma));
                        }
                    }
                    ast::PseudoClassSelectorArgKind::Ident(ident) => {
                        push(out, CssTokenKind::Selector, interpolable_ident_span(ident));
                    }
                    ast::PseudoClassSelectorArgKind::SelectorList(list) => {
                        walk_selector_list(list, out);
                    }
                    ast::PseudoClassSelectorArgKind::Number(n) => {
                        push(out, CssTokenKind::Number, span_of(&n.span));
                    }
                    ast::PseudoClassSelectorArgKind::Nth(_) => {
                        // `:nth-child(2n + 1)` internals — conservative:
                        // leave untokened rather than misclassify.
                    }
                    ast::PseudoClassSelectorArgKind::LanguageRangeList(list) => {
                        for range in &list.ranges {
                            match range {
                                ast::LanguageRange::Str(s) => {
                                    push(out, CssTokenKind::String, interpolable_str_span(s));
                                }
                                ast::LanguageRange::Ident(i) => {
                                    push(out, CssTokenKind::Keyword, interpolable_ident_span(i));
                                }
                            }
                        }
                        for comma in &list.comma_spans {
                            push(out, CssTokenKind::Punctuation, span_of(comma));
                        }
                    }
                    _ => {}
                }
                push(out, CssTokenKind::Punctuation, span_of(&arg.r_paren));
            }
        }
        ast::SimpleSelector::PseudoElement(pseudo) => {
            push(
                out,
                CssTokenKind::Selector,
                interpolable_ident_span(&pseudo.name),
            );
            if let Some(arg) = &pseudo.arg {
                push(out, CssTokenKind::Punctuation, span_of(&arg.l_paren));
                match &arg.kind {
                    ast::PseudoElementSelectorArgKind::CompoundSelector(compound) => {
                        for simple in &compound.children {
                            walk_simple_selector(simple, out);
                        }
                    }
                    ast::PseudoElementSelectorArgKind::CompoundSelectorList(list) => {
                        for compound in &list.selectors {
                            for simple in &compound.children {
                                walk_simple_selector(simple, out);
                            }
                        }
                        for comma in &list.comma_spans {
                            push(out, CssTokenKind::Punctuation, span_of(comma));
                        }
                    }
                    ast::PseudoElementSelectorArgKind::Ident(ident) => {
                        push(out, CssTokenKind::Selector, interpolable_ident_span(ident));
                    }
                    _ => {}
                }
                push(out, CssTokenKind::Punctuation, span_of(&arg.r_paren));
            }
        }
        ast::SimpleSelector::Nesting(nesting) => {
            push(out, CssTokenKind::Selector, span_of(&nesting.span));
        }
        ast::SimpleSelector::SassPlaceholder(placeholder) => {
            push(out, CssTokenKind::Selector, span_of(&placeholder.span));
        }
    }
}

// ---------------------------------------------------------------------------
// At-rule preludes
// ---------------------------------------------------------------------------

fn walk_prelude(prelude: &ast::AtRulePrelude, out: &mut Vec<CssToken>) {
    use ast::AtRulePrelude as P;
    match prelude {
        P::Charset(s) => push(out, CssTokenKind::String, span_of(&s.span)),
        P::ColorProfile(profile) => match profile {
            ast::ColorProfilePrelude::DashedIdent(ident) => {
                push(out, CssTokenKind::Keyword, interpolable_ident_span(ident));
            }
            ast::ColorProfilePrelude::DeviceCmyk(ident) => {
                push(out, CssTokenKind::Keyword, span_of(&ident.span));
            }
        },
        P::Container(container) => {
            if let Some(name) = &container.name {
                push(out, CssTokenKind::Keyword, interpolable_ident_span(name));
            }
            if let Some(condition) = &container.condition {
                walk_container_condition(condition, out);
            }
        }
        P::CounterStyle(ident)
        | P::FontPaletteValues(ident)
        | P::PositionTry(ident)
        | P::Property(ident)
        | P::ScrollTimeline(ident) => {
            push(out, CssTokenKind::Keyword, interpolable_ident_span(ident));
        }
        P::Document(doc) => {
            for matcher in &doc.matchers {
                match matcher {
                    ast::DocumentPreludeMatcher::Url(url) => walk_url(url, out),
                    ast::DocumentPreludeMatcher::Function(function) => walk_function(function, out),
                }
            }
            for comma in &doc.comma_spans {
                push(out, CssTokenKind::Punctuation, span_of(comma));
            }
        }
        P::FontFeatureValues(family) => match family {
            ast::FontFamilyName::Str(s) => {
                push(out, CssTokenKind::String, interpolable_str_span(s));
            }
            ast::FontFamilyName::Unquoted(unquoted) => {
                for ident in &unquoted.idents {
                    push(out, CssTokenKind::Keyword, interpolable_ident_span(ident));
                }
            }
        },
        P::Import(import) => walk_import_prelude(import, out),
        P::Keyframes(name) => match name {
            ast::KeyframesName::Ident(ident) => {
                push(out, CssTokenKind::Keyword, interpolable_ident_span(ident));
            }
            ast::KeyframesName::Str(s) => {
                push(out, CssTokenKind::String, interpolable_str_span(s));
            }
            // Less-only shapes cannot occur in CSS mode.
            _ => {}
        },
        P::Layer(names) => {
            for name in &names.names {
                for ident in &name.idents {
                    push(out, CssTokenKind::Keyword, interpolable_ident_span(ident));
                }
            }
            for comma in &names.comma_spans {
                push(out, CssTokenKind::Punctuation, span_of(comma));
            }
        }
        P::Media(list) => walk_media_query_list(list, out),
        P::Namespace(ns) => {
            if let Some(prefix) = &ns.prefix {
                push(out, CssTokenKind::Keyword, interpolable_ident_span(prefix));
            }
            match &ns.uri {
                ast::NamespacePreludeUri::Str(s) => {
                    push(out, CssTokenKind::String, interpolable_str_span(s));
                }
                ast::NamespacePreludeUri::Url(url) => walk_url(url, out),
            }
        }
        P::Nest(list) => walk_selector_list(list, out),
        P::Page(list) => {
            for selector in &list.selectors {
                if let Some(name) = &selector.name {
                    push(out, CssTokenKind::Selector, interpolable_ident_span(name));
                }
                for pseudo in &selector.pseudo {
                    push(out, CssTokenKind::Selector, span_of(&pseudo.span));
                }
            }
            for comma in &list.comma_spans {
                push(out, CssTokenKind::Punctuation, span_of(comma));
            }
        }
        P::SassExpr(value) => walk_value(value, RawCtx::Value, out),
        P::Supports(condition) => walk_supports_condition(condition, out),
        P::Unknown(unknown) => match &**unknown {
            ast::UnknownAtRulePrelude::ComponentValue(value) => {
                walk_value(value, RawCtx::Selector, out)
            }
            ast::UnknownAtRulePrelude::TokenSeq(seq) => walk_token_seq(seq, RawCtx::Selector, out),
        },
        // Sass/Less-only preludes cannot occur under `Syntax::Css`; exotic
        // CSS preludes (@scope, custom-media variants) degrade to untokened
        // bytes inside the zone — the ZoneMap still covers them.
        _ => {}
    }
}

fn walk_import_prelude(import: &ast::ImportPrelude, out: &mut Vec<CssToken>) {
    match &import.href {
        ast::ImportPreludeHref::Str(s) => {
            push(out, CssTokenKind::String, interpolable_str_span(s));
        }
        ast::ImportPreludeHref::Url(url) => walk_url(url, out),
        ast::ImportPreludeHref::Function(function) => walk_function(function, out),
    }
    if let Some(layer) = &import.layer {
        match layer {
            ast::ImportPreludeLayer::Empty(ident) => {
                push(out, CssTokenKind::Function, span_of(&ident.span));
            }
            ast::ImportPreludeLayer::WithName(function) => walk_function(function, out),
        }
    }
    if let Some(supports) = &import.supports {
        match &supports.kind {
            ast::ImportPreludeSupportsKind::SupportsCondition(condition) => {
                push(out, CssTokenKind::Function, span_of(&supports.span));
                walk_supports_condition(condition, out);
            }
            ast::ImportPreludeSupportsKind::Declaration(decl) => {
                push(out, CssTokenKind::Function, span_of(&supports.span));
                walk_declaration(decl, out);
            }
        }
    }
    if let Some(media) = &import.media {
        walk_media_query_list(media, out);
    }
    // The raw post-URL tail (modifiers) rides TokenSeq when the typed
    // grammar didn't apply — handled by `import.modifiers` below if needed;
    // untokened bytes inside the zone are acceptable for this rare shape.
}

fn walk_media_query_list(list: &ast::MediaQueryList, out: &mut Vec<CssToken>) {
    for query in &list.queries {
        match query {
            ast::MediaQuery::ConditionOnly(condition) => {
                walk_media_condition(condition, out);
            }
            ast::MediaQuery::WithType(with_type) => {
                if let Some(modifier) = &with_type.modifier {
                    push(out, CssTokenKind::Keyword, span_of(&modifier.span));
                }
                push(
                    out,
                    CssTokenKind::Keyword,
                    interpolable_ident_span(&with_type.media_type),
                );
                if let Some(condition) = &with_type.condition {
                    push(out, CssTokenKind::Keyword, span_of(&condition.and.span));
                    walk_media_condition(&condition.condition, out);
                }
            }
            ast::MediaQuery::Function(function) => walk_function(function, out),
            // Less-only shapes cannot occur in CSS mode.
            _ => {}
        }
    }
    for comma in &list.comma_spans {
        push(out, CssTokenKind::Punctuation, span_of(comma));
    }
}

fn walk_media_condition(condition: &ast::MediaCondition, out: &mut Vec<CssToken>) {
    for kind in &condition.conditions {
        match kind {
            ast::MediaConditionKind::MediaInParens(in_parens) => match &in_parens.kind {
                ast::MediaInParensKind::MediaCondition(inner) => {
                    walk_media_condition(inner, out);
                }
                ast::MediaInParensKind::MediaFeature(feature) => {
                    walk_media_feature(feature, out);
                }
                ast::MediaInParensKind::GeneralEnclosed(seq) => {
                    walk_token_seq(seq, RawCtx::Value, out);
                }
                // Sass interpolation cannot occur in CSS mode.
                _ => {}
            },
            ast::MediaConditionKind::And(and) => {
                push(out, CssTokenKind::Keyword, span_of(&and.keyword.span));
                walk_media_in_parens(&and.media_in_parens, out);
            }
            ast::MediaConditionKind::Or(or) => {
                push(out, CssTokenKind::Keyword, span_of(&or.keyword.span));
                walk_media_in_parens(&or.media_in_parens, out);
            }
            ast::MediaConditionKind::Not(not) => {
                push(out, CssTokenKind::Keyword, span_of(&not.keyword.span));
                walk_media_in_parens(&not.media_in_parens, out);
            }
        }
    }
}

fn walk_media_in_parens(in_parens: &ast::MediaInParens, out: &mut Vec<CssToken>) {
    match &in_parens.kind {
        ast::MediaInParensKind::MediaCondition(inner) => walk_media_condition(inner, out),
        ast::MediaInParensKind::MediaFeature(feature) => walk_media_feature(feature, out),
        ast::MediaInParensKind::GeneralEnclosed(seq) => walk_token_seq(seq, RawCtx::Value, out),
        _ => {}
    }
}

fn walk_media_feature(feature: &ast::MediaFeature, out: &mut Vec<CssToken>) {
    match feature {
        ast::MediaFeature::Plain(plain) => {
            push(
                out,
                CssTokenKind::Keyword,
                media_feature_name_span(&plain.name),
            );
            push(out, CssTokenKind::Punctuation, span_of(&plain.colon_span));
            walk_value(&plain.value, RawCtx::Value, out);
        }
        ast::MediaFeature::Boolean(boolean) => {
            push(
                out,
                CssTokenKind::Keyword,
                media_feature_name_span(&boolean.name),
            );
        }
        ast::MediaFeature::Range(range) => {
            walk_value(&range.left, RawCtx::Value, out);
            push(
                out,
                CssTokenKind::Punctuation,
                span_of(&range.comparison.span),
            );
            walk_value(&range.right, RawCtx::Value, out);
        }
        ast::MediaFeature::RangeInterval(interval) => {
            walk_value(&interval.left, RawCtx::Value, out);
            push(
                out,
                CssTokenKind::Punctuation,
                span_of(&interval.left_comparison.span),
            );
            push(
                out,
                CssTokenKind::Keyword,
                media_feature_name_span(&interval.name),
            );
            push(
                out,
                CssTokenKind::Punctuation,
                span_of(&interval.right_comparison.span),
            );
            walk_value(&interval.right, RawCtx::Value, out);
        }
    }
}

fn media_feature_name_span(name: &ast::MediaFeatureName) -> Range<usize> {
    match name {
        ast::MediaFeatureName::Ident(ident) => interpolable_ident_span(ident),
        ast::MediaFeatureName::PostcssSimpleVar(var) => span_of(&var.span),
        // Sass-only shapes cannot occur in CSS mode; the variant list may
        // still grow — cover them via the Ident path is impossible here, so
        // return an empty range (no token) for safety.
        _ => 0..0,
    }
}

fn walk_container_condition(condition: &ast::ContainerCondition, out: &mut Vec<CssToken>) {
    for kind in &condition.conditions {
        match kind {
            ast::ContainerConditionKind::QueryInParens(query) => {
                walk_query_in_parens(query, out);
            }
            ast::ContainerConditionKind::And(and) => {
                push(out, CssTokenKind::Keyword, span_of(&and.keyword.span));
                walk_query_in_parens(&and.query_in_parens, out);
            }
            ast::ContainerConditionKind::Or(or) => {
                push(out, CssTokenKind::Keyword, span_of(&or.keyword.span));
                walk_query_in_parens(&or.query_in_parens, out);
            }
            ast::ContainerConditionKind::Not(not) => {
                push(out, CssTokenKind::Keyword, span_of(&not.keyword.span));
                walk_query_in_parens(&not.query_in_parens, out);
            }
        }
    }
}

fn walk_query_in_parens(query: &ast::QueryInParens, out: &mut Vec<CssToken>) {
    match &query.kind {
        ast::QueryInParensKind::SizeFeature(feature) => walk_media_feature(feature, out),
        ast::QueryInParensKind::ScrollState(feature) => walk_media_feature(feature, out),
        ast::QueryInParensKind::ContainerCondition(condition) => {
            walk_container_condition(condition, out);
        }
        ast::QueryInParensKind::GeneralEnclosed(seq) => walk_token_seq(seq, RawCtx::Value, out),
        // Style queries — internals skipped conservatively (rare).
        _ => {}
    }
}

fn walk_supports_condition(condition: &ast::SupportsCondition, out: &mut Vec<CssToken>) {
    for kind in &condition.conditions {
        match kind {
            ast::SupportsConditionKind::Not(not) => {
                push(out, CssTokenKind::Keyword, span_of(&not.keyword.span));
                walk_supports_in_parens(&not.condition, out);
            }
            ast::SupportsConditionKind::And(and) => {
                push(out, CssTokenKind::Keyword, span_of(&and.keyword.span));
                walk_supports_in_parens(&and.condition, out);
            }
            ast::SupportsConditionKind::Or(or) => {
                push(out, CssTokenKind::Keyword, span_of(&or.keyword.span));
                walk_supports_in_parens(&or.condition, out);
            }
            ast::SupportsConditionKind::SupportsInParens(in_parens) => {
                walk_supports_in_parens(in_parens, out);
            }
        }
    }
}

fn walk_supports_in_parens(in_parens: &ast::SupportsInParens, out: &mut Vec<CssToken>) {
    match &in_parens.kind {
        ast::SupportsInParensKind::SupportsCondition(inner) => {
            walk_supports_condition(inner, out);
        }
        ast::SupportsInParensKind::Feature(decl) => walk_declaration(&decl.decl, out),
        ast::SupportsInParensKind::Selector(list) => walk_selector_list(list, out),
        ast::SupportsInParensKind::Function(function) => walk_function(function, out),
        ast::SupportsInParensKind::GeneralEnclosed(seq) => walk_token_seq(seq, RawCtx::Value, out),
        ast::SupportsInParensKind::Interpolation(ident) => {
            push(out, CssTokenKind::Keyword, interpolable_ident_span(ident));
        }
    }
}

// ---------------------------------------------------------------------------
// Component values
// ---------------------------------------------------------------------------

/// Context for raw `TokenWithSpan` runs — they appear in value position
/// (raw custom-property values, supports pre-contains) and selector-ish
/// positions (unknown at-rule preludes, raw rule preludes). Idents classify
/// as Keyword in value context and Selector in selector context, matching
/// the Phase 4.2 scanner's phase behavior.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RawCtx {
    Value,
    Selector,
}

fn walk_values(values: &[ast::ComponentValue], ctx: RawCtx, out: &mut Vec<CssToken>) {
    for value in values {
        walk_value(value, ctx, out);
    }
}

fn walk_value(value: &ast::ComponentValue, ctx: RawCtx, out: &mut Vec<CssToken>) {
    match value {
        ast::ComponentValue::BracketBlock(block) => {
            walk_values(&block.value, ctx, out);
        }
        ast::ComponentValue::Calc(calc) => {
            walk_value(&calc.left, ctx, out);
            push(out, CssTokenKind::Punctuation, span_of(&calc.op.span));
            walk_value(&calc.right, ctx, out);
        }
        ast::ComponentValue::CalcParenthesized(parenthesized) => {
            walk_value(&parenthesized.expr, ctx, out);
        }
        ast::ComponentValue::Delimiter(delimiter) => {
            push(out, CssTokenKind::Punctuation, span_of(&delimiter.span));
        }
        // Dimension spans cover number + unit (`1.5em` — one token, like
        // the Phase 4.2 stream).
        ast::ComponentValue::Dimension(dimension) => {
            push(out, CssTokenKind::Number, span_of(&dimension.span));
        }
        ast::ComponentValue::Function(function) => walk_function(function, out),
        ast::ComponentValue::HexColor(hex) => {
            push(out, CssTokenKind::Number, span_of(&hex.span));
        }
        ast::ComponentValue::ImportantAnnotation(important) => {
            push(out, CssTokenKind::Keyword, span_of(&important.span));
        }
        ast::ComponentValue::InterpolableIdent(ident) => {
            // A dashed ident in value position is a custom-property
            // reference or declaration name (`--x`): Variable, matching the
            // Phase 4.2 stream. Plain idents are value keywords.
            let range = interpolable_ident_span(ident);
            if ident_is_dashed(ident) {
                push(out, CssTokenKind::Variable, range);
            } else {
                push(out, CssTokenKind::Keyword, range);
            }
        }
        ast::ComponentValue::InterpolableStr(s) => {
            push(out, CssTokenKind::String, interpolable_str_span(s));
        }
        ast::ComponentValue::Number(number) => {
            push(out, CssTokenKind::Number, span_of(&number.span));
        }
        ast::ComponentValue::Percentage(percentage) => {
            push(out, CssTokenKind::Number, span_of(&percentage.span));
        }
        ast::ComponentValue::Ratio(ratio) => {
            push(out, CssTokenKind::Number, span_of(&ratio.numerator.span));
            push(out, CssTokenKind::Punctuation, span_of(&ratio.solidus_span));
            push(out, CssTokenKind::Number, span_of(&ratio.denominator.span));
        }
        ast::ComponentValue::TokenWithSpan(token) => {
            push(
                out,
                token_data_kind(&token.token, ctx),
                span_of(&token.span),
            );
        }
        ast::ComponentValue::UnicodeRange(range) => {
            push(out, CssTokenKind::Number, span_of(&range.span));
        }
        ast::ComponentValue::Url(url) => walk_url(url, out),
        // Sass/Less-only value shapes cannot occur under `Syntax::Css`.
        _ => {}
    }
}

/// A dashed ident (`--main`) is a custom-property reference/declaration —
/// Variable in the Phase 4.2 stream, not Keyword.
fn ident_is_dashed(ident: &ast::InterpolableIdent) -> bool {
    matches!(
        ident,
        ast::InterpolableIdent::Literal(literal) if literal.name.starts_with("--")
    )
}

fn walk_function(function: &ast::Function, out: &mut Vec<CssToken>) {
    if let ast::FunctionName::Ident(ident) = &function.name {
        push(out, CssTokenKind::Function, interpolable_ident_span(ident));
    }
    walk_values(&function.args, RawCtx::Value, out);
}

fn walk_url(url: &ast::Url, out: &mut Vec<CssToken>) {
    push(out, CssTokenKind::Function, span_of(&url.name.span));
    if let Some(value) = &url.value {
        match value {
            ast::UrlValue::Raw(raw) => push(out, CssTokenKind::String, span_of(&raw.span)),
            ast::UrlValue::Str(s) => push(out, CssTokenKind::String, interpolable_str_span(s)),
            // Sass/Less-only shapes cannot occur in CSS mode.
            _ => {}
        }
    }
}

fn walk_token_seq(seq: &ast::TokenSeq, ctx: RawCtx, out: &mut Vec<CssToken>) {
    for token in &seq.tokens {
        push(
            out,
            token_data_kind(&token.token, ctx),
            span_of(&token.span),
        );
    }
}

/// Map a raw token to its highlight kind. This is the same classification
/// the Phase 4.2 scanner applied to bare token runs.
fn token_data_kind(token: &TokenData, ctx: RawCtx) -> CssTokenKind {
    use CssTokenKind::*;
    match token {
        TokenData::AtKeyword(_) => AtRule,
        TokenData::Dimension(_) | TokenData::Number(_) | TokenData::Percentage(_) => Number,
        TokenData::Str(_)
        | TokenData::BadStr(_)
        | TokenData::UrlRaw(_)
        | TokenData::UrlTemplate(_) => String,
        // `$var` (postcss/Less) and `@{var}` (Less interpolation) — Variable
        // families regardless of context.
        TokenData::DollarVar(_) | TokenData::DollarLBraceVar(_) | TokenData::AtLBraceVar(_) => {
            Variable
        }
        TokenData::Ident(_) => match ctx {
            RawCtx::Value => Keyword,
            RawCtx::Selector => Selector,
        },
        // Everything punctuation-shaped: brackets, colons, semicolons,
        // commas, delimiters, comparison operators, CDO/CDC, whitespace-
        // adjacent sigils.
        _ => Punctuation,
    }
}

// ---------------------------------------------------------------------------
// Span helpers
// ---------------------------------------------------------------------------

fn span_of(span: &oxc_css_parser::Span) -> Range<usize> {
    span.start..span.end
}

fn push(out: &mut Vec<CssToken>, kind: CssTokenKind, range: Range<usize>) {
    // Zero-width spans carry no highlightable bytes — skip them here so
    // every caller can pass spans unguarded.
    if range.end > range.start {
        out.push(CssToken { kind, span: range });
    }
}

fn interpolable_ident_span(ident: &ast::InterpolableIdent) -> Range<usize> {
    match ident {
        ast::InterpolableIdent::Literal(literal) => span_of(&literal.span),
        ast::InterpolableIdent::SassInterpolated(interpolated) => span_of(&interpolated.span),
        ast::InterpolableIdent::LessInterpolated(interpolated) => span_of(&interpolated.span),
        ast::InterpolableIdent::Placeholder(placeholder) => span_of(&placeholder.span),
    }
}

fn interpolable_str_span(s: &ast::InterpolableStr) -> Range<usize> {
    match s {
        ast::InterpolableStr::Literal(literal) => span_of(&literal.span),
        ast::InterpolableStr::SassInterpolated(interpolated) => span_of(&interpolated.span),
        ast::InterpolableStr::LessInterpolated(interpolated) => span_of(&interpolated.span),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(CssTokenKind, String)> {
        parse_css(src)
            .tokens
            .into_iter()
            .map(|t| (t.kind, src[t.span.start..t.span.end].to_string()))
            .collect()
    }

    #[test]
    fn classifies_rule_shape() {
        let out = kinds(".hud { color: red; }");
        assert!(out.contains(&(CssTokenKind::Selector, ".hud".into())));
        assert!(out.contains(&(CssTokenKind::Property, "color".into())));
        assert!(out.contains(&(CssTokenKind::Keyword, "red".into())));
    }

    #[test]
    fn classifies_numbers_units_and_functions() {
        let out = kinds(".a { margin: -1.5em; left: calc(100% - 2px); }");
        assert!(out.contains(&(CssTokenKind::Number, "-1.5em".into())));
        assert!(out.contains(&(CssTokenKind::Function, "calc".into())));
        assert!(out.contains(&(CssTokenKind::Number, "100%".into())));
        assert!(out.contains(&(CssTokenKind::Number, "2px".into())));
    }

    #[test]
    fn custom_properties_are_variables() {
        let out = kinds(".a { color: var(--main); --x: 1; }");
        assert!(out.contains(&(CssTokenKind::Function, "var".into())));
        assert!(out.contains(&(CssTokenKind::Variable, "--main".into())));
        assert!(out.contains(&(CssTokenKind::Property, "--x".into())));
    }

    #[test]
    fn at_rules_and_comments() {
        let out = kinds("/* hi */\n@media (min-width: 400px) { .a { color: blue; } }");
        assert!(out.contains(&(CssTokenKind::Comment, "/* hi */".into())));
        assert!(out.contains(&(CssTokenKind::AtRule, "@media".into())));
        // Inside the nested rule block, `.a` is still a selector and `color`
        // still a property (the @media block is a RULE block).
        assert!(out.contains(&(CssTokenKind::Selector, ".a".into())));
        assert!(out.contains(&(CssTokenKind::Property, "color".into())));
    }

    #[test]
    fn media_feature_names_are_keywords() {
        let out = kinds("@media (min-width: 400px) { .a { color: blue; } }");
        assert!(
            out.contains(&(CssTokenKind::Keyword, "min-width".into())),
            "media feature name should be a Keyword token, got: {out:?}"
        );
        assert!(out.contains(&(CssTokenKind::Number, "400px".into())));
    }

    #[test]
    fn hex_colors_are_numbers_in_values() {
        let out = kinds(".a { color: #ff00aa; }");
        assert!(out.contains(&(CssTokenKind::Number, "#ff00aa".into())));
    }

    #[test]
    fn important_is_keyword() {
        let out = kinds(".a { color: red !important; }");
        assert!(out.contains(&(CssTokenKind::Keyword, "!important".into())));
    }

    #[test]
    fn eof_recovered_block_is_reported_as_error() {
        // css-syntax-3 recovery: EOF closes the block. The parser reports a
        // recoverable error; policy relays it with Error severity.
        let outcome = parse_css(".a { color: red;");
        assert!(
            !outcome.diagnostics.is_empty(),
            "unclosed block must produce at least one diagnostic"
        );
        assert!(
            outcome
                .diagnostics
                .iter()
                .all(|d| d.severity == CssDiagnosticSeverity::Error)
        );
        // Highlighting still survives the broken input.
        assert!(
            outcome
                .tokens
                .iter()
                .any(|t| t.kind == CssTokenKind::Property)
        );
    }

    #[test]
    fn tokens_are_sorted_and_non_overlapping() {
        let outcome = parse_css(
            "/* c1 */ .hud, .wrap > a:hover { margin: 0 auto; /* c2 */ color: red; }\n@media (min-width: 400px) { .a { --x: 1; } }",
        );
        let tokens = &outcome.tokens;
        for pair in tokens.windows(2) {
            assert!(
                pair[0].span.start <= pair[1].span.start,
                "tokens must be sorted: {:?} then {:?}",
                pair[0],
                pair[1]
            );
            assert!(
                pair[0].span.end <= pair[1].span.start,
                "tokens must not overlap: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn selectors_and_pseudo_classes() {
        let out = kinds(".wrap > a:hover, #main[disabled] { color: red; }");
        assert!(out.contains(&(CssTokenKind::Selector, ".wrap".into())));
        assert!(out.contains(&(CssTokenKind::Selector, "a".into())));
        assert!(out.contains(&(CssTokenKind::Selector, "hover".into())));
        assert!(out.contains(&(CssTokenKind::Selector, "#main".into())));
        assert!(out.contains(&(CssTokenKind::Selector, "disabled".into())));
    }

    #[test]
    fn strings_and_urls() {
        let out =
            kinds(".a { background: url(\"bg.png\") no-repeat; font-family: \"Fira Code\"; }");
        assert!(out.contains(&(CssTokenKind::Function, "url".into())));
        assert!(out.contains(&(CssTokenKind::String, "\"bg.png\"".into())));
        assert!(out.contains(&(CssTokenKind::String, "\"Fira Code\"".into())));
    }

    #[test]
    fn never_panics_on_garbage() {
        for src in [
            "",
            "{",
            "}}}",
            "\"unterminated",
            "@media {",
            ".a { -- } }",
            "@media (min-width:)",
            "/* unterminated",
            ".a { color: var(--)",
            "@import \"x",
            "a:hover:nth-child(2n+1) { color: red }",
        ] {
            let outcome = parse_css(src);
            // Totality: tokens always sorted/non-overlapping.
            for pair in outcome.tokens.windows(2) {
                assert!(pair[0].span.start <= pair[1].span.start);
                assert!(pair[0].span.end <= pair[1].span.start);
            }
        }
    }
}
