import * as acorn from 'acorn';
import * as walk from 'acorn-walk';
import { DocumentNode, ExpressionNode, MacroNode, MarkupNode } from './ast';
import { SourceRange } from './tokenTypes'; 
import { BUILTINS as knot_BUILTINS, BUILTIN_GLOBALS as knot_GLOBALS } from './formats/sugarcube/macros';


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

// Macros whose arguments include a passage name target
const PASSAGE_ARG_MACROS = new Set([
  'link', 'button', 'linkappend', 'linkprepend', 'linkreplace',
  'include', 'display', 'goto', 'actions', 'click',
]);

// For these macros: 1 arg => arg[0] is passage; 2 args => arg[0] label, arg[1] passage
const LABEL_THEN_PASSAGE = new Set([
  'link', 'button', 'click', 'linkappend', 'linkprepend', 'linkreplace',
]);

function passageArgIndex(macroName: string, argCount: number): number {
  if (LABEL_THEN_PASSAGE.has(macroName)) return argCount >= 2 ? 1 : 0;
  return 0;
}

function extractPassageNameArg(expr: ExpressionNode): string | null {
  if (expr.type === 'literal' && expr.kind === 'string') return String(expr.value);
  return null;
}

export class SymbolTable {
  private builtins   = new Map<string, BuiltinSymbol>();
  private userByKey  = new Map<string, UserSymbol>();
  private userByName = new Map<string, UserSymbol[]>();

  constructor() { this.seedBuiltins(); }

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

  private seedBuiltins(): void {
    // Single canonical source: formats/sugarcube/macros.ts
    for (const m of knot_BUILTINS) {
      this.builtins.set(m.name, {
        tier: 'builtin', kind: SymbolKind.Macro,
        name: m.name, description: m.description, hasBody: m.hasBody ?? false,
      });
    }
    for (const g of knot_GLOBALS) {
      this.builtins.set(g.name, {
        tier: 'builtin', kind: SymbolKind.RuntimeGlobal,
        name: g.name, description: g.description,
      });
    }
  }
}

export function buildSymbolTable(ast: DocumentNode, uri: string): SymbolBuildResult {
  const table = new SymbolTable();
  const unresolvedPassageLinks: Array<{ target: string; uri: string; range: SourceRange }> = [];

  for (const passage of ast.passages) {
    table.addUserSymbol(SymbolKind.Passage, passage.name, uri, passage.nameRange);
  }

  for (const passage of ast.passages) {
    if (Array.isArray(passage.body)) {
      collectMarkupSymbols(passage.body, uri, table, unresolvedPassageLinks);
      collectInlineScriptSymbols(passage.body, uri, table);
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
): void {
  for (const node of nodes) {
    if (node.type === 'link') {
      const resolved = table.resolveByKind(SymbolKind.Passage, node.target);
      if (resolved?.tier === 'user') {
        table.addReference(SymbolKind.Passage, node.target, { uri, range: node.range });
      } else {
        unresolved.push({ target: node.target, uri, range: node.range });
      }
      continue;
    }

    if (node.type === 'macro') {
      collectMacroSymbols(node, uri, table, unresolved);
      if (node.body) collectMarkupSymbols(node.body, uri, table, unresolved);
    }
  }
}

function collectMacroSymbols(
  node: MacroNode,
  uri: string,
  table: SymbolTable,
  unresolved: Array<{ target: string; uri: string; range: SourceRange }>,
): void {
  table.addReference(SymbolKind.Macro, node.name, { uri, range: node.nameRange });

  if (node.name === 'set') {
    const arg = node.args[0];
    if (arg?.type === 'binaryOp' && (arg.operator === 'to' || arg.operator === '=')) {
      const varName = extractStoryVarName(arg.left);
      if (varName) table.addUserSymbol(SymbolKind.StoryVar, varName, uri, arg.left.range);
    }
  }

  if (node.name === 'widget') {
    const arg = node.args[0];
    if (arg?.type === 'literal' && arg.kind === 'string') {
      table.addUserSymbol(SymbolKind.Widget, String(arg.value), uri, arg.range);
    } else if (arg?.type === 'identifier') {
      table.addUserSymbol(SymbolKind.Widget, arg.name, uri, arg.range);
    }
  }

  // Track passage references from link/include/goto/etc. macro args
  if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
    const idx = passageArgIndex(node.name, node.args.length);
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

function collectInlineScriptSymbols(nodes: MarkupNode[], uri: string, table: SymbolTable): void {
  for (const node of nodes) {
    if (node.type === 'macro' && node.name === 'script' && node.body) {
      const src = node.body.filter(n => n.type === 'text').map(n => (n as any).value).join('');
      if (src.trim()) {
        const baseOffset = node.range.start;
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
    if (node.type === 'macro' && node.body) collectInlineScriptSymbols(node.body, uri, table);
  }
}

function collectExprSymbols(expr: ExpressionNode, uri: string, table: SymbolTable): void {
  switch (expr.type) {
    case 'storyVar':
      table.addReference(SymbolKind.StoryVar, expr.name, { uri, range: expr.range }); return;
    case 'binaryOp':
      collectExprSymbols(expr.left, uri, table); collectExprSymbols(expr.right, uri, table); return;
    case 'unaryOp': collectExprSymbols(expr.operand, uri, table); return;
    case 'propertyAccess': collectExprSymbols(expr.object, uri, table); return;
    case 'indexAccess':
      collectExprSymbols(expr.object, uri, table); collectExprSymbols(expr.index, uri, table); return;
    case 'call':
      collectExprSymbols(expr.callee, uri, table);
      for (const a of expr.args) collectExprSymbols(a, uri, table); return;
    case 'arrayLiteral':
      for (const el of expr.elements) collectExprSymbols(el, uri, table); return;
    case 'objectLiteral':
      for (const p of expr.properties) collectExprSymbols(p.value, uri, table); return;
    default: return;
  }
}

function extractStoryVarName(expr: ExpressionNode): string | null {
  if (expr.type === 'storyVar') return expr.name;
  if (expr.type === 'propertyAccess') return extractStoryVarName(expr.object);
  if (expr.type === 'indexAccess') return extractStoryVarName(expr.object);
  return null;
}