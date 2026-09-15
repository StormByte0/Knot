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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssDiagnosticSeverity {
    /// A definite syntax problem the parser recovered from (or could not
    /// recover from). Rendered as an error to the author.
    Error,
    /// Suspicious-but-parseable input. Reserved for future lint passes —
    /// the parser itself only reports errors today.
    Warning,
    /// Informational notes. Reserved for future lint passes.
    Info,
}

#[derive(Debug, Clone)]
pub struct CssDiagnostic {
    pub message: String,
    pub range: Range<usize>,
    pub severity: CssDiagnosticSeverity,
}

#[derive(Debug, Clone)]
pub struct CssParseOutcome {
    pub tokens: Vec<CssToken>,
    pub diagnostics: Vec<CssDiagnostic>,
}
