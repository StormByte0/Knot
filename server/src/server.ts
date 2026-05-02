// Public API barrel — everything downstream imports from here

export { TokenType, type Token, type SourceRange }       from './tokenTypes';
export { lex }                                            from './lexer';
export { preScan, type MacroPairTable }                  from './preScan';
export { parseDocument, parsePassage, extractPassageSpans, type PassageSpan } from './parser';
export { IncrementalParser }                             from './incrementalParser';
export { SymbolKind, SymbolTable, buildSymbolTable,
         type BuiltinSymbol, type UserSymbol, type ReferenceSite } from './symbols';
export { WorkspaceIndex }    from './workspaceIndex';
export { SyntaxAnalyzer, type AnalysisResult, type SemanticToken } from './analyzer';
export { TypeInference, type InferredType, type InferenceResult }  from './typeInference';
export { VirtualDocGenerator, type VirtualDoc, type MappingEntry } from './virtualDoc';
export { runVirtualDiagnostics, type VirtualDiagnosticResult }     from './virtualDiagnostics';
export type {
  DocumentNode, PassageNode, MarkupNode, MacroNode, LinkNode,
  TextNode, CommentNode, ExpressionNode,
  ParseDiagnostic, ParseOutput,
} from './ast';