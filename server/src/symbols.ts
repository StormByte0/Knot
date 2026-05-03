import * as acorn from 'acorn';
import * as walk from 'acorn-walk';
import { DocumentNode, ExpressionNode, MacroNode, MarkupNode } from './ast';
import { SourceRange } from './tokenTypes'; 
import type { StoryFormatAdapter } from './formats/types';
import { walkMarkup, walkExpression } from './visitors';

export enum SymbolKind {
  Passage = 'Passage',
  Macro = 'Macro',
  Widget = 'Widget',
  StoryVar = 'StoryVar',
  TempVar = 'TempVar',
  Function = 'Function',
  RuntimeGlobal = 'RuntimeGlobal',
}

export interface BuiltinSymbol {
  tier: 'builtin';
  kind: SymbolKind;
  name: string;
  description: string;
  hasBody?: boolean;
}

export interface ReferenceSite {
  uri: string;
  range: SourceRange;
}

export interface UserSymbol {
  tier: 'user';
  kind: SymbolKind;
  name: string;
  uri: string;
  range: SourceRange;
  references: ReferenceSite[];
}

export type AnySymbol = BuiltinSymbol | UserSymbol;

export interface SymbolBuildResult {
  table: SymbolTable;
  unresolvedPassageLinks: Array<{ target: string; uri: string; range: SourceRange }>;
}

function extractPassageNameArg(expr: ExpressionNode): string | null {
  if (expr.type === 'literal' && expr.kind === 'string') return String(expr.value);
  return null;
}

export class SymbolTable {
  private builtins   = new Map<string, BuiltinSymbol>();
  private userByKey  = new Map<string, UserSymbol>();
  private userByName = new Map<string, UserSymbol[]>();

  constructor(adapter?: StoryFormatAdapter) { this.seedBuiltins(adapter); }

  resolve(name: string): AnySymbol | undefined {
    return this.userByName.get(name)?.[0] ?? this.builtins.get(name);
  }

  resolveByKind(kind: SymbolKind, name: string): AnySymbol | undefined {
    const user = this.userByKey.get(`${kind}:${name}`);
    if (user) return user;
    const builtin = this.builtins.get(name);
    if (builtin?.kind === kind) return builtin;
    return undefined;
  }

  isBuiltin(name: string): boolean {
    return this.builtins.has(name) && !this.userByName.has(name);
  }

  getDefinition(name: string): UserSymbol | null {
    return this.userByName.get(name)?.[0] ?? null;
  }

  addUserSymbol(kind: SymbolKind, name: string, uri: string, range: SourceRange): UserSymbol {
    const key = `${kind}:${name}`;
    const existing = this.userByKey.get(key);
    if (existing) return existing;
    const sym: UserSymbol = { tier: 'user', kind, name, uri, range, references: [] };
    this.userByKey.set(key, sym);
    const arr = this.userByName.get(name) ?? [];
    arr.push(sym);
    this.userByName.set(name, arr);
    return sym;
  }

  addReference(kind: SymbolKind, name: string, ref: ReferenceSite): void {
    const sym = this.userByKey.get(`${kind}:${name}`);
    if (sym) sym.references.push(ref);
  }

  getBuiltins(): BuiltinSymbol[] { return [...this.builtins.values()]; }
  getUserSymbols(): UserSymbol[] { return [...this.userByKey.values()]; }

  private seedBuiltins(adapter?: StoryFormatAdapter): void {
    if (!adapter) return;
    // Seed from adapter — no direct import from formats/sugarcube/macros
    for (const m of adapter.getBuiltinMacros()) {
      this.builtins.set(m.name, {
        tier: 'builtin', kind: SymbolKind.Macro,
        name: m.name, description: m.description, hasBody: m.hasBody ?? false,
      });
    }
    for (const g of adapter.getBuiltinGlobals()) {
      this.builtins.set(g.name, {
        tier: 'builtin', kind: SymbolKind.RuntimeGlobal,
        name: g.name, description: g.description,
      });
    }
  }
}

export function buildSymbolTable(ast: DocumentNode, uri: string, adapter?: StoryFormatAdapter): SymbolBuildResult {
  const table = new SymbolTable(adapter);
  const unresolvedPassageLinks: Array<{ target: string; uri: string; range: SourceRange }> = [];

  for (const passage of ast.passages) {
    table.addUserSymbol(SymbolKind.Passage, passage.name, uri, passage.nameRange);
  }

  for (const passage of ast.passages) {
    if (Array.isArray(passage.body)) {
      collectMarkupSymbols(passage.body, uri, table, unresolvedPassageLinks, adapter);
      collectInlineScriptSymbols(passage.body, uri, table, adapter);
    } else if (passage.body.type === 'scriptBody') {
      collectScriptSymbols(passage.body.source, passage.body.range.start, uri, table);
    }
  }

  return { table, unresolvedPassageLinks };
}

function collectMarkupSymbols(
  nodes: MarkupNode[],
  uri: string,
  table: SymbolTable,
  unresolved: Array<{ target: string; uri: string; range: SourceRange }>,
  adapter?: StoryFormatAdapter,
): void {
  walkMarkup(nodes, {
    onLink(node) {
      const resolved = table.resolveByKind(SymbolKind.Passage, node.target);
      if (resolved?.tier === 'user') {
        table.addReference(SymbolKind.Passage, node.target, { uri, range: node.range });
      } else {
        unresolved.push({ target: node.target, uri, range: node.range });
      }
    },
    onMacro(node) {
      collectMacroSymbols(node, uri, table, unresolved, adapter);
    },
  });
}

function collectMacroSymbols(
  node: MacroNode,
  uri: string,
  table: SymbolTable,
  unresolved: Array<{ target: string; uri: string; range: SourceRange }>,
  adapter?: StoryFormatAdapter,
): void {
  table.addReference(SymbolKind.Macro, node.name, { uri, range: node.nameRange });

  // Use adapter for variable assignment macros
  const varAssignmentMacros = adapter?.getVariableAssignmentMacros();
  const assignmentOps = adapter?.getAssignmentOperators() ?? ['to', '='];
  if (varAssignmentMacros?.has(node.name)) {
    const arg = node.args[0];
    if (arg?.type === 'binaryOp' && assignmentOps.includes(arg.operator)) {
      const varName = extractStoryVarName(arg.left);
      if (varName) table.addUserSymbol(SymbolKind.StoryVar, varName, uri, arg.left.range);
    }
  }

  // Use adapter for macro definition macros
  const macroDefMacros = adapter?.getMacroDefinitionMacros();
  if (macroDefMacros?.has(node.name)) {
    const arg = node.args[0];
    if (arg?.type === 'literal' && arg.kind === 'string') {
      table.addUserSymbol(SymbolKind.Widget, String(arg.value), uri, arg.range);
    } else if (arg?.type === 'identifier') {
      table.addUserSymbol(SymbolKind.Widget, arg.name, uri, arg.range);
    }
  }

  // Track passage references from link/include/goto/etc. macro args — use adapter
  const passageArgMacros = adapter?.getPassageArgMacros();
  if (passageArgMacros?.has(node.name) && node.args.length > 0) {
    const idx = adapter?.getPassageArgIndex(node.name, node.args.length) ?? 0;
    const arg = node.args[idx];
    if (arg) {
      const passageName = extractPassageNameArg(arg);
      if (passageName) {
        const resolved = table.resolveByKind(SymbolKind.Passage, passageName);
        if (resolved?.tier === 'user') {
          table.addReference(SymbolKind.Passage, passageName, { uri, range: arg.range });
        } else {
          unresolved.push({ target: passageName, uri, range: arg.range });
        }
      }
    }
  }

  for (const arg of node.args) collectExprSymbols(arg, uri, table);
}

function collectScriptSymbols(source: string, baseOffset: number, uri: string, table: SymbolTable): void {
  try {
    const program = acorn.parse(source, { ecmaVersion: 'latest' });
    walk.simple(program as never, {
      CallExpression(node: any) {
        if (node?.callee?.type !== 'MemberExpression') return;
        if (node.callee.object?.name !== 'Macro') return;
        if (node.callee.property?.name !== 'add') return;
        const firstArg = node.arguments?.[0];
        if (!firstArg || firstArg.type !== 'Literal' || typeof firstArg.value !== 'string') return;
        const start = baseOffset + firstArg.start + 1;
        const end = start + firstArg.value.length;
        table.addUserSymbol(SymbolKind.Macro, firstArg.value, uri, { start, end });
      },
    });
  } catch {}
}

function collectInlineScriptSymbols(
  nodes: MarkupNode[],
  uri: string,
  table: SymbolTable,
  adapter?: StoryFormatAdapter,
): void {
  const inlineScriptMacros = adapter?.getInlineScriptMacros();
  walkMarkup(nodes, {
    onMacro(node) {
      const isInlineScript = inlineScriptMacros?.has(node.name) ?? node.name === 'script';
      if (isInlineScript && node.body) {
        const src = node.body.filter(n => n.type === 'text').map(n => (n as import('./ast').TextNode).value).join('');
        if (src.trim()) {
          const baseOffset = node.body.length > 0 ? node.body[0]!.range.start : 0;
          try {
            const program = acorn.parse(src, { ecmaVersion: 'latest' });
            walk.simple(program as never, {
              CallExpression(callNode: any) {
                if (callNode?.callee?.type !== 'MemberExpression') return;
                if (callNode.callee.object?.name !== 'Macro') return;
                if (callNode.callee.property?.name !== 'add') return;
                const firstArg = callNode.arguments?.[0];
                if (!firstArg || firstArg.type !== 'Literal' || typeof firstArg.value !== 'string') return;
                const start = baseOffset + (firstArg.start ?? 0) + 1;
                const end = start + firstArg.value.length;
                table.addUserSymbol(SymbolKind.Macro, firstArg.value, uri, { start, end });
              },
            });
          } catch {}
        }
      }
    },
  });
}

function collectExprSymbols(expr: ExpressionNode, uri: string, table: SymbolTable): void {
  walkExpression(expr, e => {
    if (e.type === 'storyVar') {
      table.addReference(SymbolKind.StoryVar, e.name, { uri, range: e.range });
    }
  });
}

function extractStoryVarName(expr: ExpressionNode): string | null {
  if (expr.type === 'storyVar') return expr.name;
  if (expr.type === 'propertyAccess') return extractStoryVarName(expr.object);
  if (expr.type === 'indexAccess') return extractStoryVarName(expr.object);
  return null;
}
