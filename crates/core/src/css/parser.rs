//! CSS parser wrapper using cssparser.

use super::types::{CssToken, CssTokenKind, CssParseOutcome};
use cssparser::{Parser, ParserInput, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    SelectorOrRule,
    PropertyName,
    PropertyValue,
}

/// Parse CSS source text and return classified tokens + diagnostics.
pub fn parse_css(source: &str) -> CssParseOutcome {
    let mut tokens = Vec::new();
    let diagnostics = Vec::new();

    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);

    let mut context = Context::SelectorOrRule;
    let mut brace_depth: i32 = 0;

    loop {
        let start_byte = parser.position().byte_index();

        let token_result = parser.next_including_whitespace_and_comments();

        match token_result {
            Ok(token) => {
                let (kind, new_context) = classify_token(token, context, &mut brace_depth);
                let end_byte = parser.position().byte_index();
                context = new_context;

                if kind != CssTokenKind::Whitespace {
                    tokens.push(CssToken { kind, span: start_byte..end_byte });
                }
            }
            Err(_) => break,
        }
    }

    CssParseOutcome { tokens, diagnostics }
}

fn classify_token(
    token: &Token,
    context: Context,
    brace_depth: &mut i32,
) -> (CssTokenKind, Context) {
    match token {
        Token::Comment(_) => (CssTokenKind::Comment, context),
        Token::WhiteSpace(_) => (CssTokenKind::Whitespace, context),
        Token::AtKeyword(_) => (CssTokenKind::AtRule, Context::SelectorOrRule),
        Token::Ident(ident) => {
            if ident.starts_with("--") {
                return (CssTokenKind::Variable, context);
            }
            match context {
                Context::PropertyName => (CssTokenKind::Property, Context::PropertyName),
                Context::PropertyValue => (CssTokenKind::Keyword, Context::PropertyName),
                Context::SelectorOrRule => (CssTokenKind::Selector, Context::SelectorOrRule),
            }
        }
        Token::Function(_) => (CssTokenKind::Function, context),
        Token::Number { .. } | Token::Percentage { .. } | Token::Dimension { .. } => {
            (CssTokenKind::Number, Context::PropertyName)
        }
        Token::Hash(_) | Token::IDHash(_) => {
            match context {
                Context::PropertyValue => (CssTokenKind::Number, Context::PropertyName),
                _ => (CssTokenKind::Selector, context),
            }
        }
        Token::QuotedString(_) => (CssTokenKind::String, Context::PropertyName),
        Token::Colon => (CssTokenKind::Punctuation, Context::PropertyValue),
        Token::Semicolon => (CssTokenKind::Punctuation, Context::PropertyName),
        Token::CurlyBracketBlock => {
            *brace_depth += 1;
            (CssTokenKind::Punctuation, Context::PropertyName)
        }
        Token::CloseCurlyBracket => {
            *brace_depth -= 1;
            let next = if *brace_depth <= 0 { Context::SelectorOrRule } else { Context::PropertyName };
            (CssTokenKind::Punctuation, next)
        }
        Token::Comma => (CssTokenKind::Punctuation, context),
        Token::ParenthesisBlock | Token::CloseParenthesis => (CssTokenKind::Punctuation, context),
        Token::SquareBracketBlock | Token::CloseSquareBracket => (CssTokenKind::Punctuation, context),
        Token::Delim(_) => {
            match context {
                Context::SelectorOrRule => (CssTokenKind::Selector, context),
                _ => (CssTokenKind::Punctuation, context),
            }
        }
        _ => (CssTokenKind::Punctuation, context),
    }
}
