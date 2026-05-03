import { DocumentNode, ExpressionNode, MacroNode, MarkupNode, ParseDiagnostic, ScriptBodyNode } from './ast';
import { SymbolKind, SymbolTable, buildSymbolTable } from './symbols';
import { SourceRange } from './tokenTypes';
import { WorkspaceIndex } from './workspaceIndex';
import { passageNameFromExpr } from './passageArgs';
import type { StoryFormatAdapter } from './formats/types';
import { walkDocument, walkMarkup, walkExpression } from './visitors';
import type { DiagnosticEngine } from './diagnosticEngine';
import { DiagnosticRule, type RuleDiagnostic } from './diagnosticEngine';

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
    const engine  = workspace?.getDiagnosticEngine();
    const { table: symbols } = buildSymbolTable(ast, uri, adapter);
    const resolvedLinks = this.resolveLinks(ast, symbols, workspace);

    // Collect rule-tagged diagnostics internally, then convert at the end
    const ruleDiags: RuleDiagnostic[] = [
      ...this.validateLinks(resolvedLinks, engine),
      ...this.validateMacros(ast, symbols, workspace, engine),
    ];

    // Add structural validation if adapter is available
    if (adapter) {
      ruleDiags.push(...this.validateStructure(ast, adapter, engine));
    }

    // Convert all RuleDiagnostics to ParseDiagnostics via the engine
    const diagnostics = engine
      ? engine.toParseDiagnostics(ruleDiags)
      : ruleDiags.map(d => ({
          message:  d.message,
          range:    d.range,
          severity: defaultSeverityForRule(d.rule),
        }));

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
    const implicitPatterns = adapter?.getImplicitPassagePatterns() ?? [];
    const passageRefApis = adapter?.getPassageRefApiCalls() ?? [];
    const isKnown = (name: string) => this.isPassageKnown(name, symbols, workspace);

    // Build a quick lookup for API calls: "ObjectName.method" → true
    const apiCallSet = new Set<string>();
    for (const api of passageRefApis) {
      for (const method of api.methods) {
        apiCallSet.add(`${api.objectName}.${method}`);
      }
    }

    for (const passage of ast.passages) {
      const src = passage.name;

      // ── Standard links: [[Target]] and passage-arg macros ────────────────
      if (Array.isArray(passage.body)) {
        walkMarkup(passage.body, {
          onLink(node) {
            links.push({
              target:   node.target,
              range:    node.range,
              resolved: isKnown(node.target),
            });
          },
          onMacro(node) {
            // <<goto "Target">>, <<link "label" "Target">>, <<include "Target">>, etc.
            if (passageArgMacros?.has(node.name) && node.args.length > 0) {
              const idx  = adapter!.getPassageArgIndex(node.name, node.args.length);
              const arg  = node.args[idx];
              const name = arg ? passageNameFromExpr(arg) : null;
              if (name && arg) {
                links.push({
                  target:   name,
                  range:    arg.range,
                  resolved: isKnown(name),
                });
              }
            }

            // ── Implicit passage refs in macro expression args ───────────
            if (apiCallSet.size > 0) {
              for (const arg of node.args) {
                walkExpression(arg, expr => {
                  if (expr.type !== 'call') return;
                  const callee = expr.callee;
                  if (callee.type !== 'propertyAccess') return;
                  if (callee.object.type !== 'identifier') return;
                  const key = `${callee.object.name}.${callee.property}`;
                  if (!apiCallSet.has(key)) return;
                  if (expr.args.length === 0) return;
                  const firstArg = expr.args[0]!;
                  if (firstArg.type === 'literal' && firstArg.kind === 'string' && typeof firstArg.value === 'string') {
                    links.push({
                      target:   firstArg.value,
                      range:    firstArg.range,
                      resolved: isKnown(firstArg.value),
                    });
                  }
                });
              }
            }
          },
          // ── Implicit passage refs in text nodes ──────────────────────────
          onText(node) {
            if (implicitPatterns.length === 0) return;
            for (const ip of implicitPatterns) {
              ip.pattern.lastIndex = 0;
              let m: RegExpExecArray | null;
              while ((m = ip.pattern.exec(node.value)) !== null) {
                const name = m[1];
                if (!name) continue;
                const matchOffset = node.range.start + m.index;
                links.push({
                  target:   name,
                  range:    { start: matchOffset, end: matchOffset + m[0].length },
                  resolved: isKnown(name),
                });
              }
            }
          },
        });
      }

      // ── Script passages: scan for implicit passage refs ──────────────────
      if (passage.body && typeof passage.body === 'object' && (passage.body as ScriptBodyNode).type === 'scriptBody') {
        const scriptBody = passage.body as ScriptBodyNode;
        if (implicitPatterns.length > 0) {
          for (const ip of implicitPatterns) {
            ip.pattern.lastIndex = 0;
            let m: RegExpExecArray | null;
            while ((m = ip.pattern.exec(scriptBody.source)) !== null) {
              const name = m[1];
              if (!name) continue;
              const matchOffset = scriptBody.range.start + m.index;
              links.push({
                target:   name,
                range:    { start: matchOffset, end: matchOffset + m[0].length },
                resolved: isKnown(name),
              });
            }
          }
        }
      }
    }

    return links;
  }

  private validateLinks(
    links: Array<{ target: string; range: SourceRange; resolved: boolean }>,
    engine?: DiagnosticEngine,
  ): RuleDiagnostic[] {
    const diags: RuleDiagnostic[] = [];
    for (const l of links) {
      if (l.resolved) continue;
      if (engine && !engine.isEnabled(DiagnosticRule.UnknownPassage)) continue;
      const diag: RuleDiagnostic = {
        rule:    DiagnosticRule.UnknownPassage,
        message: `Unknown passage target: ${l.target}`,
        range:   l.range,
      };
      diags.push(diag);
    }
    return diags;
  }

  private validateMacros(
    ast: DocumentNode, symbols: SymbolTable, workspace?: WorkspaceIndex,
    engine?: DiagnosticEngine,
  ): RuleDiagnostic[] {
    const diags: RuleDiagnostic[] = [];
    const adapter = workspace?.getActiveAdapter();
    const varAssignmentMacros = adapter?.getVariableAssignmentMacros();
    const assignmentOps = adapter?.getAssignmentOperators() ?? ['to'];
    const comparisonOps = adapter?.getComparisonOperators() ?? ['gt', 'gte', 'lt', 'lte'];

    // Cache builtin names in a Set for O(1) lookup instead of repeated
    // calls to symbols.isBuiltin() which does Map lookups each time.
    const builtinNames = new Set(symbols.getBuiltins().map(b => b.name));

    const isMacroKnown = (name: string): boolean =>
      builtinNames.has(name) || Boolean(symbols.resolve(name) || workspace?.getMacroDefinition(name));

    // Build a map of builtin macro defs for deprecation & arg validation lookups
    const builtinDefs = new Map<string, { deprecated?: boolean; deprecationMessage?: string; args?: ReadonlyArray<{ position: number; label: string; isRequired?: boolean }> }>();
    if (adapter) {
      for (const m of adapter.getBuiltinMacros()) {
        builtinDefs.set(m.name, m);
      }
    }

    walkDocument(ast, {
      onMacro(node) {
        if (!isMacroKnown(node.name)) {
          if (!engine || engine.isEnabled(DiagnosticRule.UnknownMacro)) {
            diags.push({
              rule:    DiagnosticRule.UnknownMacro,
              message: `Unknown macro: <<${node.name}>>`,
              range:   node.nameRange,
            });
          }
        }

        // Check for deprecated macros
        const builtinDef = builtinDefs.get(node.name);
        if (builtinDef && builtinDef.deprecated) {
          if (!engine || engine.isEnabled(DiagnosticRule.DeprecatedMacro)) {
            diags.push({
              rule:    DiagnosticRule.DeprecatedMacro,
              message: `<<${node.name}>> is deprecated${builtinDef.deprecationMessage ? ': ' + builtinDef.deprecationMessage : ''}`,
              range:   node.nameRange,
            });
          }
        }

        // Validate required arguments
        if (builtinDef?.args) {
          for (const argDef of builtinDef.args) {
            if (argDef.isRequired && node.args.length <= argDef.position) {
              if (!engine || engine.isEnabled(DiagnosticRule.MissingRequiredArg)) {
                diags.push({
                  rule:    DiagnosticRule.MissingRequiredArg,
                  message: `<<${node.name}>> requires argument '${argDef.label}' at position ${argDef.position}`,
                  range:   node.nameRange,
                });
              }
            }
          }
        }

        for (const arg of node.args) {
          walkExpression(arg, expr => {
            if (expr.type !== 'binaryOp') return;
            // Use adapter for variable assignment check, but fall back to 'set' for compatibility
            const isAssignmentMacro = varAssignmentMacros?.has(node.name) ?? node.name === 'set';
            if (isAssignmentMacro && assignmentOps.includes(expr.operator)) {
              if (expr.left.type !== 'storyVar' && expr.left.type !== 'tempVar' &&
                  expr.left.type !== 'propertyAccess' && expr.left.type !== 'indexAccess') {
                if (!engine || engine.isEnabled(DiagnosticRule.AssignmentTarget)) {
                  diags.push({
                    rule:    DiagnosticRule.AssignmentTarget,
                    message: `Operator 'to' requires a variable on the left-hand side`,
                    range:   expr.range,
                  });
                }
              }
            }
            if (comparisonOps.includes(expr.operator)) {
              const lk = inferSimpleKind(expr.left);
              const rk = inferSimpleKind(expr.right);
              if ((lk === 'string' && rk === 'number') || (lk === 'number' && rk === 'string')) {
                if (!engine || engine.isEnabled(DiagnosticRule.TypeMismatch)) {
                  diags.push({
                    rule:    DiagnosticRule.TypeMismatch,
                    message: `Type mismatch for operator '${expr.operator}': ${lk} vs ${rk}`,
                    range:   expr.range,
                  });
                }
              }
            }
          });
        }
      },
    });

    return diags;
  }

  /**
   * Validate structural constraints on macros — e.g. <<elseif>> must be
   * inside an <<if>> chain, <<break>> must be inside <<for>>, etc.
   *
   * Structural validation requires a name stack that tracks the current
   * macro nesting. Since walkMarkup's onMacro callback fires pre-order and
   * there's no post-visit hook to pop the stack, we use walkMarkup directly
   * with a manually managed name stack via the parentStack parameter.
   */
  validateStructure(ast: DocumentNode, adapter: StoryFormatAdapter, engine?: DiagnosticEngine): RuleDiagnostic[] {
    const diags: RuleDiagnostic[] = [];
    const constraints = adapter.getMacroParentConstraints();
    if (constraints.size === 0) return diags;

    for (const passage of ast.passages) {
      if (Array.isArray(passage.body)) {
        walkMarkupStructure(passage.body, [], constraints, diags, engine);
      }
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

    walkDocument(ast, {
      onMacro(node) {
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
      },
      onLink(node) {
        tokens.push({ range: node.targetRange, tokenType: 'passage' });
      },
      onComment(node) {
        tokens.push({ range: node.range, tokenType: 'comment' });
      },
    });

    // Emit passage name tokens
    for (const p of ast.passages) {
      tokens.push({ range: p.nameRange, tokenType: 'passage' });
    }

    return tokens;
  }
}

/**
 * Structural validation requires pushing/popping a name stack as we enter/exit
 * macro bodies. Since walkMarkup's onMacro callback fires pre-order without a
 * post-visit hook, we use this dedicated walker for the structural case.
 */
function walkMarkupStructure(
  nodes: MarkupNode[],
  nameStack: string[],
  constraints: ReadonlyMap<string, ReadonlySet<string>>,
  diags: RuleDiagnostic[],
  engine?: DiagnosticEngine,
): void {
  for (const node of nodes) {
    if (node.type !== 'macro') continue;

    // Check if this macro has parent constraints
    const validParents = constraints.get(node.name);
    if (validParents) {
      // Skip if the rule is suppressed
      if (!engine || engine.isEnabled(DiagnosticRule.ContainerStructure)) {
        let foundValidParent = false;
        let foundElse = false;

        for (let i = nameStack.length - 1; i >= 0; i--) {
          const parent = nameStack[i]!;
          if (validParents.has(parent)) {
            foundValidParent = true;
            if (node.name === 'elseif' && parent === 'else') {
              foundElse = true;
            }
            break;
          }
          if (node.name === 'elseif' && parent === 'else') {
            foundElse = true;
          }
        }

        if (!foundValidParent) {
          const parentNames = [...validParents].map(n => `<<${n}>>`).join(' or ');
          diags.push({
            rule:    DiagnosticRule.ContainerStructure,
            message: `<<${node.name}>> outside of ${parentNames} chain`,
            range:   node.nameRange,
          });
        } else if (foundElse && node.name === 'elseif') {
          diags.push({
            rule:    DiagnosticRule.ContainerStructure,
            message: `<<elseif>> after <<else>> in the same <<if>> chain`,
            range:   node.nameRange,
          });
        }
      }
    }

    // Push this macro onto the name stack and recurse into body
    nameStack.push(node.name);
    if (node.body) walkMarkupStructure(node.body, nameStack, constraints, diags, engine);
    nameStack.pop();
  }
}

function inferSimpleKind(expr: ExpressionNode): 'string' | 'number' | 'unknown' {
  if (expr.type === 'literal') {
    if (expr.kind === 'string') return 'string';
    if (expr.kind === 'number') return 'number';
  }
  return 'unknown';
}

/** Fallback severity when no DiagnosticEngine is available. */
function defaultSeverityForRule(rule: DiagnosticRule): 'error' | 'warning' {
  switch (rule) {
    case DiagnosticRule.DuplicatePassage:
    case DiagnosticRule.TypeMismatch:
    case DiagnosticRule.ContainerStructure:
    case DiagnosticRule.MissingRequiredArg:
    case DiagnosticRule.AssignmentTarget:
      return 'error';
    default:
      return 'warning';
  }
}
