use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssTokenKind {
    Property,
    Keyword,
    Number,
    String,
    Selector,
    AtRule,
    Variable,
    Function,
    Comment,
    Punctuation,
    Whitespace,
}

#[derive(Debug, Clone)]
pub struct CssToken {
    pub kind: CssTokenKind,
    pub span: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct CssDiagnostic {
    pub message: String,
    pub range: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct CssParseOutcome {
    pub tokens: Vec<CssToken>,
    pub diagnostics: Vec<CssDiagnostic>,
}
