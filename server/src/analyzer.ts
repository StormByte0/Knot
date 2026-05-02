import { DocumentNode, ExpressionNode, MarkupNode, ParseDiagnostic } from './ast';
import { SymbolKind, SymbolTable, buildSymbolTable } from './symbols';
import { SourceRange } from './tokenTypes';
import { WorkspaceIndex } from './workspaceIndex';
import { PASSAGE_ARG_MACROS, passageArgIndex, passageNameFromExpr } from './passageArgs';

export interface SemanticToken {
  range: SourceRange;
  tokenType: 'macro' | 'passage' | 'variable' | 'operator' | 'string' | 'number' | 'comment';
}

export interface AnalysisResult {
  symbols: SymbolTable;
  diagnostics: ParseDiagnostic[];
  semanticTokens: SemanticToken[];
  resolvedLinks: Array<{ target: string; range: SourceRange; resolved: boolean }>;
}

export class SyntaxAnalyzer {
  analyze(ast: DocumentNode, uri: string, workspace?: WorkspaceIndex): AnalysisResult {
    const { table: symbols } = buildSymbolTable(ast, uri);
    const resolvedLinks = this.resolveLinks(ast, symbols, workspace);
    const diagnostics: ParseDiagnostic[] = [
      ...this.validateLinks(resolvedLinks),
      ...this.validateMacros(ast, symbols, workspace),
    ];
    const semanticTokens = this.generateTokens(ast);
    return { symbols, diagnostics, semanticTokens, resolvedLinks };
  }

  private isPassageKnown(name: string, symbols: SymbolTable, workspace?: WorkspaceIndex): boolean {
    return Boolean(
      symbols.resolveByKind(SymbolKind.Passage, name) ||
      workspace?.getPassageDefinition(name),
    );
  }

  private resolveLinks(
    ast: DocumentNode, symbols: SymbolTable, workspace?: WorkspaceIndex,
  ): Array<{ target: string; range: SourceRange; resolved: boolean }> {
    const links: Array<{ target: string; range: SourceRange; resolved: boolean }> = [];

    const walkNodes = (nodes: MarkupNode[]): void => {
      for (const node of nodes) {
        // [[Target]] syntax
        if (node.type === 'link') {
          links.push({
            target:   node.target,
            range:    node.range,
            resolved: this.isPassageKnown(node.target, symbols, workspace),
          });
        }

        if (node.type === 'macro') {
          // <<goto "Target">>, <<link "label" "Target">>, <<include "Target">>, etc.
          if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
            const idx  = passageArgIndex(node.name, node.args.length);
            const arg  = node.args[idx];
            const name = arg ? passageNameFromExpr(arg) : null;
            if (name && arg) {
              links.push({
                target:   name,
                range:    arg.range,
                resolved: this.isPassageKnown(name, symbols, workspace),
              });
            }
          }
          if (node.body) walkNodes(node.body);
        }
      }
    };

    for (const p of ast.passages) {
      if (Array.isArray(p.body)) walkNodes(p.body);
    }
    return links;
  }

  private validateLinks(
    links: Array<{ target: string; range: SourceRange; resolved: boolean }>,
  ): ParseDiagnostic[] {
    return links
      .filter(l => !l.resolved)
      .map(l => ({
        message:  `Unknown passage target: ${l.target}`,
        range:    l.range,
        severity: 'warning' as const,
      }));
  }

  private validateMacros(
    ast: DocumentNode, symbols: SymbolTable, workspace?: WorkspaceIndex,
  ): ParseDiagnostic[] {
    const diags: ParseDiagnostic[] = [];

    const isMacroKnown = (name: string): boolean =>
      Boolean(symbols.resolve(name) || workspace?.getMacroDefinition(name) || symbols.isBuiltin(name));

    const walkNodes = (nodes: MarkupNode[]): void => {
      for (const node of nodes) {
        if (node.type !== 'macro') continue;

        if (!isMacroKnown(node.name)) {
          diags.push({
            message:  `Unknown macro: <<${node.name}>>`,
            range:    node.nameRange,
            severity: 'warning',
          });
        }

        for (const arg of node.args) {
          this.walkExpr(arg, expr => {
            if (expr.type !== 'binaryOp') return;
            if (node.name === 'set' && expr.operator === 'to') {
              if (expr.left.type !== 'storyVar' && expr.left.type !== 'tempVar' &&
                  expr.left.type !== 'propertyAccess' && expr.left.type !== 'indexAccess') {
                diags.push({
                  message:  `Operator 'to' requires a variable on the left-hand side`,
                  range:    expr.range,
                  severity: 'error',
                });
              }
            }
            if (['gt', 'gte', 'lt', 'lte'].includes(expr.operator)) {
              const lk = inferSimpleKind(expr.left);
              const rk = inferSimpleKind(expr.right);
              if ((lk === 'string' && rk === 'number') || (lk === 'number' && rk === 'string')) {
                diags.push({
                  message:  `Type mismatch for operator '${expr.operator}': ${lk} vs ${rk}`,
                  range:    expr.range,
                  severity: 'error',
                });
              }
            }
          });
        }

        if (node.body) walkNodes(node.body);
      }
    };

    for (const p of ast.passages) {
      if (Array.isArray(p.body)) walkNodes(p.body);
    }
    return diags;
  }

  private generateTokens(ast: DocumentNode): SemanticToken[] {
    const tokens: SemanticToken[] = [];

    const emitExpr = (expr: ExpressionNode): void => {
      switch (expr.type) {
        case 'storyVar':
          tokens.push({ range: expr.range, tokenType: 'variable' }); return;
        case 'tempVar':
          tokens.push({ range: expr.range, tokenType: 'variable' }); return;
        case 'literal':
          tokens.push({ range: expr.range, tokenType: expr.kind === 'string' ? 'string' : 'number' }); return;
        case 'binaryOp':
          emitExpr(expr.left); emitExpr(expr.right); return;
        case 'unaryOp':
          emitExpr(expr.operand); return;
        case 'propertyAccess':
          emitExpr(expr.object);
          tokens.push({ range: expr.propertyRange, tokenType: 'variable' }); return;
        case 'indexAccess':
          emitExpr(expr.object); emitExpr(expr.index); return;
        case 'call':
          emitExpr(expr.callee); expr.args.forEach(emitExpr); return;
        case 'arrayLiteral':
          expr.elements.forEach(emitExpr); return;
        case 'objectLiteral':
          expr.properties.forEach(p => emitExpr(p.value)); return;
        default: return;
      }
    };

    const walkNodes = (nodes: MarkupNode[]): void => {
      for (const node of nodes) {
        if (node.type === 'macro') {
          tokens.push({ range: node.nameRange, tokenType: 'macro' });
          if (node.closeNameRange) {
            tokens.push({ range: node.closeNameRange, tokenType: 'macro' });
          }

          // Emit passage token for passage-arg macros so hover/highlight works
          if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
            const idx = passageArgIndex(node.name, node.args.length);
            const arg = node.args[idx];
            if (arg && arg.type === 'literal' && arg.kind === 'string') {
              // Emit the inside of the string (without quotes) as a passage token
              tokens.push({
                range:     { start: arg.range.start + 1, end: arg.range.end - 1 },
                tokenType: 'passage',
              });
              // Don't also emit it as a string token — skip remaining emitExpr for this arg
              // Emit remaining args normally
              node.args.forEach((a, i) => { if (i !== idx) emitExpr(a); });
            } else {
              node.args.forEach(emitExpr);
            }
          } else {
            node.args.forEach(emitExpr);
          }

          if (node.body) walkNodes(node.body);
        }
        if (node.type === 'link') {
          tokens.push({ range: node.targetRange, tokenType: 'passage' });
        }
        if (node.type === 'comment') {
          tokens.push({ range: node.range, tokenType: 'comment' });
        }
      }
    };

    for (const p of ast.passages) {
      tokens.push({ range: p.nameRange, tokenType: 'passage' });
      if (Array.isArray(p.body)) walkNodes(p.body);
    }

    return tokens;
  }

  private walkExpr(expr: ExpressionNode, visitor: (e: ExpressionNode) => void): void {
    visitor(expr);
    switch (expr.type) {
      case 'binaryOp':
        this.walkExpr(expr.left, visitor); this.walkExpr(expr.right, visitor); return;
      case 'unaryOp':
        this.walkExpr(expr.operand, visitor); return;
      case 'propertyAccess':
        this.walkExpr(expr.object, visitor); return;
      case 'indexAccess':
        this.walkExpr(expr.object, visitor); this.walkExpr(expr.index, visitor); return;
      case 'call':
        this.walkExpr(expr.callee, visitor);
        expr.args.forEach(a => this.walkExpr(a, visitor)); return;
      case 'arrayLiteral':
        expr.elements.forEach(el => this.walkExpr(el, visitor)); return;
      case 'objectLiteral':
        expr.properties.forEach(p => this.walkExpr(p.value, visitor)); return;
      default: return;
    }
  }
}

function inferSimpleKind(expr: ExpressionNode): 'string' | 'number' | 'unknown' {
  if (expr.type === 'literal') {
    if (expr.kind === 'string') return 'string';
    if (expr.kind === 'number') return 'number';
  }
  return 'unknown';
}