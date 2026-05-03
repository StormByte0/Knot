import { DocumentNode, ExpressionNode, MacroNode, MarkupNode, ParseDiagnostic } from './ast';
import { SymbolKind, SymbolTable, buildSymbolTable } from './symbols';
import { SourceRange } from './tokenTypes';
import { WorkspaceIndex } from './workspaceIndex';
import { passageNameFromExpr } from './passageArgs';
import type { StoryFormatAdapter } from './formats/types';

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
    const adapter = workspace?.getActiveAdapter();
    const { table: symbols } = buildSymbolTable(ast, uri, adapter);
    const resolvedLinks = this.resolveLinks(ast, symbols, workspace);
    const diagnostics: ParseDiagnostic[] = [
      ...this.validateLinks(resolvedLinks),
      ...this.validateMacros(ast, symbols, workspace),
    ];

    // Add structural validation if adapter is available
    if (adapter) {
      diagnostics.push(...this.validateStructure(ast, adapter));
    }

    const semanticTokens = this.generateTokens(ast, adapter);
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
    const adapter = workspace?.getActiveAdapter();
    const passageArgMacros = adapter?.getPassageArgMacros();

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
          if (passageArgMacros?.has(node.name) && node.args.length > 0) {
            const idx  = adapter!.getPassageArgIndex(node.name, node.args.length);
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
    const adapter = workspace?.getActiveAdapter();
    const varAssignmentMacros = adapter?.getVariableAssignmentMacros();

    // Cache builtin names in a Set for O(1) lookup instead of repeated
    // calls to symbols.isBuiltin() which does Map lookups each time.
    const builtinNames = new Set(symbols.getBuiltins().map(b => b.name));

    const isMacroKnown = (name: string): boolean =>
      builtinNames.has(name) || Boolean(symbols.resolve(name) || workspace?.getMacroDefinition(name));

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
            // Use adapter for variable assignment check, but fall back to 'set' for compatibility
            const isAssignmentMacro = varAssignmentMacros?.has(node.name) ?? node.name === 'set';
            if (isAssignmentMacro && expr.operator === 'to') {
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

  /**
   * Validate structural constraints on macros — e.g. <<elseif>> must be
   * inside an <<if>> chain, <<break>> must be inside <<for>>, etc.
   */
  validateStructure(ast: DocumentNode, adapter: StoryFormatAdapter): ParseDiagnostic[] {
    const diags: ParseDiagnostic[] = [];
    const constraints = adapter.getMacroParentConstraints();
    if (constraints.size === 0) return diags;

    // Stack of open macro names as we walk the tree
    const parentStack: string[] = [];

    const walkNodes = (nodes: MarkupNode[]): void => {
      for (const node of nodes) {
        if (node.type !== 'macro') continue;

        // Check if this macro has parent constraints
        const validParents = constraints.get(node.name);
        if (validParents) {
          // Find the nearest constrained parent in the stack
          let foundValidParent = false;
          let foundElse = false;

          for (let i = parentStack.length - 1; i >= 0; i--) {
            const parent = parentStack[i]!;
            if (validParents.has(parent)) {
              foundValidParent = true;

              // Special case: <<else>> followed by <<elseif>> is an error
              if (node.name === 'elseif' && parent === 'else') {
                foundElse = true;
              }
              break;
            }
            // If we hit 'else' while looking for if/elseif parents, mark it
            if (node.name === 'elseif' && parent === 'else') {
              foundElse = true;
            }
          }

          if (!foundValidParent) {
            const parentNames = [...validParents].map(n => `<<${n}>>`).join(' or ');
            diags.push({
              message:  `<<${node.name}>> outside of ${parentNames} chain`,
              range:    node.nameRange,
              severity: 'error',
            });
          } else if (foundElse && node.name === 'elseif') {
            diags.push({
              message:  `<<elseif>> after <<else>> in the same <<if>> chain`,
              range:    node.nameRange,
              severity: 'error',
            });
          }
        }

        // Push this macro onto the parent stack and recurse
        parentStack.push(node.name);
        if (node.body) walkNodes(node.body);
        parentStack.pop();
      }
    };

    for (const p of ast.passages) {
      if (Array.isArray(p.body)) walkNodes(p.body);
    }
    return diags;
  }

  private generateTokens(ast: DocumentNode, adapter?: StoryFormatAdapter): SemanticToken[] {
    const tokens: SemanticToken[] = [];
    const passageArgMacros = adapter?.getPassageArgMacros();

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
          if (passageArgMacros?.has(node.name) && node.args.length > 0) {
            const idx = adapter!.getPassageArgIndex(node.name, node.args.length);
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
