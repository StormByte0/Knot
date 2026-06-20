//! CSS parser wrapper using cssparser.
//!
//! Uses `Parser::parse_nested_block()` to enter `{ }` blocks — this is
//! the cssparser-intended way to read block contents. Without it, the
//! Parser skips everything between `{` and `}`.

use super::types::{CssToken, CssTokenKind, CssParseOutcome};
use cssparser::{Parser, ParserInput, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    SelectorOrRule,
    PropertyName,
    PropertyValue,
}

pub fn parse_css(source: &str) -> CssParseOutcome {
    let mut tokens = Vec::new();
    let diagnostics = Vec::new();

    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);

    let mut context = Context::SelectorOrRule;

    loop {
        let start_byte = parser.position().byte_index();

        // Read token and extract info as owned data (releases borrow)
        let step = match parser.next() {
            Ok(token) => {
                let (kind, new_context) = classify_token(token, context);
                let is_block = matches!(token, Token::CurlyBracketBlock);
                Some((kind, new_context, is_block))
            }
            Err(_) => None,
        };

        if let Some((kind, new_context, is_block)) = step {
            let end_byte = parser.position().byte_index();
            context = new_context;

            if is_block {
                // Emit `{` then enter the block via parse_nested_block
                tokens.push(CssToken { kind: CssTokenKind::Punctuation, span: start_byte..end_byte });
                let block_tokens = parse_block(&mut parser);
                tokens.extend(block_tokens);
                context = Context::SelectorOrRule;
            } else if kind != CssTokenKind::Whitespace {
                tokens.push(CssToken { kind, span: start_byte..end_byte });
            }
        } else {
            break;
        }
    }

    CssParseOutcome { tokens, diagnostics }
}

/// Parse the contents of a `{ }` block using `parse_nested_block()`.
fn parse_block(parser: &mut Parser) -> Vec<CssToken> {
    let mut tokens = Vec::new();
    let mut context = Context::PropertyName;

    let _ = parser.parse_nested_block(|p| {
        loop {
            let start_byte = p.position().byte_index();

            let step = match p.next() {
                Ok(token) => {
                    let (kind, new_context) = classify_token(token, context);
                    let is_block = matches!(token, Token::CurlyBracketBlock);
                    Some((kind, new_context, is_block))
                }
                Err(_) => None,
            };

            if let Some((kind, new_context, is_block)) = step {
                let end_byte = p.position().byte_index();
                context = new_context;

                if is_block {
                    tokens.push(CssToken { kind: CssTokenKind::Punctuation, span: start_byte..end_byte });
                    // Recursively enter nested block
                    let nested = parse_block(p);
                    tokens.extend(nested);
                    context = Context::PropertyName;
                } else if kind != CssTokenKind::Whitespace {
                    tokens.push(CssToken { kind, span: start_byte..end_byte });
                }
            } else {
                break;
            }
        }
        Ok::<(), cssparser::ParseError<'_, ()>>(())
    });

    tokens
}

fn classify_token(token: &Token, context: Context) -> (CssTokenKind, Context) {
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
        Token::Comma => (CssTokenKind::Punctuation, context),
        Token::ParenthesisBlock | Token::CloseParenthesis => (CssTokenKind::Punctuation, context),
        Token::SquareBracketBlock | Token::CloseSquareBracket => (CssTokenKind::Punctuation, context),
        Token::CloseCurlyBracket => (CssTokenKind::Punctuation, Context::SelectorOrRule),
        Token::Delim(_) => {
            match context {
                Context::SelectorOrRule => (CssTokenKind::Selector, context),
                _ => (CssTokenKind::Punctuation, context),
            }
        }
        _ => (CssTokenKind::Punctuation, context),
    }
}
