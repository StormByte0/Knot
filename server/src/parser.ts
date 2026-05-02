import {
  ArrayLiteralNode,
  BinaryOpNode,
  CallNode,
  CommentNode,
  DocumentNode,
  ExpressionNode,
  IdentifierNode,
  IndexAccessNode,
  LinkNode,
  LiteralNode,
  MacroNode,
  MarkupNode,
  ObjectLiteralNode,
  ObjectProperty,
  ParseDiagnostic,
  ParseOutput,
  PassageKind,
  PassageNode,
  PropertyAccessNode,
  ScriptBodyNode,
  StoryVarNode,
  StyleBodyNode,
  TempVarNode,
  TextNode,
  UnaryOpNode,
} from './ast';
import { lex } from './lexer';
import { MacroPairTable, preScan } from './preScan';
import { Token, TokenType, SourceRange } from './tokenTypes';

// ---------------------------------------------------------------------------
// Passage span extraction (fast regex — no full parse needed)
// ---------------------------------------------------------------------------

export interface PassageSpan {
  name: string;
  tags: string[];
  nameStart: number;  // start of the name text
  headerEnd: number;  // end of the :: line (exclusive)
  bodyStart: number;
  bodyEnd: number;
}

const HEADER_RE = /^::[ \t]*([^\n\[{]+?)[ \t]*(?:\[([^\]]*)\])?[ \t]*(?:\{[^}]*\})?[ \t]*$/gm;

const SPECIAL_PASSAGES = new Set([
  'StoryInit', 'StoryCaption', 'StoryBanner', 'StorySubtitle',
  'StoryAuthor', 'StoryMenu', 'StoryDisplayTitle', 'StoryShare',
  'PassageDone', 'PassageHeader', 'PassageFooter',
]);

export function extractPassageSpans(text: string): PassageSpan[] {
  const headers: Array<{
    name: string; tags: string[];
    nameStart: number; headerEnd: number; headerStart: number;
  }> = [];

  HEADER_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = HEADER_RE.exec(text)) !== null) {
    const headerStart = m.index;
    const rawName = m[1]!.trim();
    const rawTags = m[2] ?? '';
    const tags = rawTags.split(/\s+/).map(t => t.trim()).filter(Boolean);

    // find name offset inside the full header line
    const headerLine = m[0];
    const nameInLine = headerLine.indexOf(rawName);
    const nameStart = headerStart + nameInLine;

    const lineEnd = text.indexOf('\n', headerStart);
    const headerEnd = lineEnd === -1 ? text.length : lineEnd + 1;

    headers.push({ name: rawName, tags, nameStart, headerEnd, headerStart });
  }

  return headers.map((h, idx) => ({
    name: h.name,
    tags: h.tags,
    nameStart: h.nameStart,
    headerEnd: h.headerEnd,
    bodyStart: h.headerEnd,
    bodyEnd: idx + 1 < headers.length
      ? headers[idx + 1]!.headerStart
      : text.length,
  }));
}

function detectPassageKind(name: string, tags: string[]): PassageKind {
  // NOTE: [widget] passages are intentionally kept as 'markup', NOT 'script'.
  // Widget passages contain <<widget "name">>...</widget>> Twee markup — treating
  // them as 'script' would parse their body as a raw ScriptBodyNode, skipping
  // collectMarkupSymbols and losing all <<widget>> declarations.
  if (name === 'Story JavaScript' || tags.includes('script'))               return 'script';
  if (name === 'Story Stylesheet' || tags.includes('stylesheet') || tags.includes('style')) return 'stylesheet';
  if (SPECIAL_PASSAGES.has(name) || name.startsWith('_'))                   return 'special';
  return 'markup';
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

export function parseDocument(text: string): ParseOutput {
  const diagnostics: ParseDiagnostic[] = [];
  const spans = extractPassageSpans(text);
  const passages = spans.map(s => parsePassage(text, s, diagnostics));
  return {
    ast: { type: 'document', range: { start: 0, end: text.length }, passages },
    diagnostics,
  };
}

export function parsePassage(
  text: string,
  span: PassageSpan,
  diagnostics: ParseDiagnostic[],
): PassageNode {
  const kind = detectPassageKind(span.name, span.tags);
  const nameRange = { start: span.nameStart, end: span.nameStart + span.name.length };
  const range = { start: span.nameStart, end: span.bodyEnd };
  const bodySource = text.slice(span.bodyStart, span.bodyEnd);

  if (kind === 'script') {
    const body: ScriptBodyNode = {
      type: 'scriptBody', source: bodySource,
      range: { start: span.bodyStart, end: span.bodyEnd },
    };
    return { type: 'passage', name: span.name, tags: span.tags, kind, nameRange, range, body };
  }

  if (kind === 'stylesheet') {
    const body: StyleBodyNode = {
      type: 'styleBody', source: bodySource,
      range: { start: span.bodyStart, end: span.bodyEnd },
    };
    return { type: 'passage', name: span.name, tags: span.tags, kind, nameRange, range, body };
  }

  const body = parseMarkupBody(bodySource, span.bodyStart, diagnostics);
  return { type: 'passage', name: span.name, tags: span.tags, kind, nameRange, range, body };
}

// ---------------------------------------------------------------------------
// Parser class
// ---------------------------------------------------------------------------

class Parser {
  private pos = 0;

  constructor(
    private tokens: Token[],
    private pairTable: MacroPairTable,
    private diagnostics: ParseDiagnostic[],
  ) {}

  // ---- Markup context -------------------------------------------------------

  parseMarkup(): MarkupNode[] {
    const nodes: MarkupNode[] = [];

    while (true) {
      const tok = this.peek();
      if (!tok || tok.type === TokenType.EOF) break;
      if (tok.type === TokenType.MacroCloseOpen) break; // parent macro's </n>

      switch (tok.type) {
        case TokenType.Text:
          nodes.push(this.parseText());
          break;

        case TokenType.Whitespace:
        case TokenType.Newline:
          // Collapse into text node
          nodes.push({ type: 'text', value: this.advance().value, range: this.prev().range });
          break;

        case TokenType.MacroOpen:
          nodes.push(this.parseMacro());
          break;

        case TokenType.LinkOpen:
          nodes.push(this.parseLink());
          break;

        case TokenType.HtmlComment:
        case TokenType.BlockComment:
        case TokenType.LineComment:
          nodes.push(this.parseComment());
          break;

        default:
          // Unexpected token — skip with error
          this.emitError(tok, `Unexpected token in markup: ${tok.value}`);
          this.advance();
      }
    }

    return nodes;
  }

  // ---- Individual markup nodes ---------------------------------------------

  private parseText(): TextNode {
    const tok = this.advance();
    // Coalesce adjacent text/whitespace/newline tokens
    let value = tok.value;
    const start = tok.range.start;
    while (
      this.peek()?.type === TokenType.Text ||
      this.peek()?.type === TokenType.Whitespace ||
      this.peek()?.type === TokenType.Newline
    ) {
      value += this.advance().value;
    }
    return { type: 'text', value, range: { start, end: this.prev().range.end } };
  }

  private parseComment(): CommentNode {
    const tok = this.advance();
    const style: CommentNode['style'] =
      tok.type === TokenType.HtmlComment ? 'html' :
      tok.type === TokenType.BlockComment ? 'block' : 'line';
    return { type: 'comment', style, value: tok.value, range: tok.range };
  }

  private parseLink(): LinkNode {
    const open = this.advance(); // [[
    const innerTokens: Token[] = [];

    while (this.peek() && this.peek()!.type !== TokenType.LinkClose && this.peek()!.type !== TokenType.EOF) {
      innerTokens.push(this.advance());
    }

    let closeRange = open.range;
    if (this.peek()?.type === TokenType.LinkClose) {
      closeRange = this.advance().range; // ]]
    } else {
      this.emitError(open, 'Unclosed link [[');
    }

    // Reconstruct inner text from tokens, find separator
    const innerText = innerTokens.map(t => t.value).join('');
    const sepIdx = innerText.indexOf('|') !== -1 ? innerText.indexOf('|')
      : innerText.indexOf('->') !== -1 ? innerText.indexOf('->')
      : innerText.indexOf('<-') !== -1 ? innerText.indexOf('<-')
      : -1;

    let target = innerText.trim();
    let display: string | null = null;

    if (sepIdx !== -1) {
      const sep = innerText[sepIdx] === '|' ? '|'
        : innerText.startsWith('->', sepIdx) ? '->'
        : '<-';
      const sepLen = sep.length;
      const left  = innerText.slice(0, sepIdx).trim();
      const right = innerText.slice(sepIdx + sepLen).trim();
      if (sep === '<-') { target = left; display = right; }
      else              { display = left; target = right; }
    }

    // target range — approximate from inner tokens
    const targetStart = innerTokens.length > 0 ? innerTokens[0]!.range.start : open.range.start;
    const targetEnd   = innerTokens.length > 0 ? innerTokens[innerTokens.length - 1]!.range.end : open.range.end;
    const targetRange = { start: targetStart + (innerText.length - innerText.trimStart().length),
                          end:   targetEnd   - (innerText.length - innerText.trimEnd().length) };

    return {
      type: 'link', target, targetRange, display,
      range: { start: open.range.start, end: closeRange.end },
    };
  }

  private parseMacro(): MacroNode {
    const open = this.advance();
    const nameTok = this.peek()?.type === TokenType.MacroName ? this.advance() : null;
    const name = nameTok?.value ?? '';
    const nameRange = nameTok?.range ?? open.range;

    const argTokens: Token[] = [];
    while (this.peek() && this.peek()!.type !== TokenType.MacroClose && this.peek()!.type !== TokenType.EOF) {
      const t = this.peek()!;
      if (t.type === TokenType.MacroOpen || t.type === TokenType.MacroCloseOpen) break;
      argTokens.push(this.advance());
    }
    if (this.peek()?.type === TokenType.MacroClose) {
      this.advance();
    } else {
      this.emitError(open, `Unclosed macro <<${name}>`);
    }

    const args = parseExprArgs(argTokens, this.diagnostics);
    const closeOffset = this.pairTable.pairs.get(open.range.start);
    const hasBody = closeOffset !== null && closeOffset !== undefined;

    let body: MarkupNode[] | null = null;
    let endRange = this.prev().range;
    let closeNameRange: SourceRange | undefined;

    if (hasBody) {
      body = this.parseMarkup();

      if (this.peek()?.type === TokenType.MacroCloseOpen) {
        this.advance();
        const closeNameTok = this.peek()?.type === TokenType.MacroName ? this.advance() : null;
        closeNameRange = closeNameTok?.range;
        if (this.peek()?.type === TokenType.MacroClose) {
          endRange = this.advance().range;
        }
      } else {
        this.emitError(open, `Unclosed body for <<${name}>>`);
      }
    }

    return {
      type: 'macro', name, nameRange, closeNameRange, args, hasBody, body,
      range: { start: open.range.start, end: endRange.end },
    };
  }

  // ---- Cursor helpers -------------------------------------------------------

  peek(): Token | undefined {
    return this.tokens[this.pos];
  }

  advance(): Token {
    const t = this.tokens[this.pos];
    if (t) this.pos++;
    return t ?? { type: TokenType.EOF, value: '', range: { start: 0, end: 0 } };
  }

  prev(): Token {
    return this.tokens[this.pos - 1] ?? { type: TokenType.EOF, value: '', range: { start: 0, end: 0 } };
  }

  private emitError(tok: Token, message: string): void {
    this.diagnostics.push({ message, range: tok.range, severity: 'error' });
  }
}

// ---------------------------------------------------------------------------
// parseMarkupBody — entry point for a passage body string
// ---------------------------------------------------------------------------

function parseMarkupBody(
  bodySource: string,
  baseOffset: number,
  diagnostics: ParseDiagnostic[],
): MarkupNode[] {
  // Lex relative to body, then shift ranges to absolute offsets
  const rawTokens = lex(bodySource);
  const tokens = rawTokens
    .filter(t => t.type !== TokenType.EOF)
    .map(t => ({
      ...t,
      range: { start: t.range.start + baseOffset, end: t.range.end + baseOffset },
    }));

  const pairTable = preScan(tokens);

  // Emit diagnostics for orphaned close tags
  for (const orphan of pairTable.orphans) {
    diagnostics.push({ message: `Unexpected closing macro tag`, range: orphan.range, severity: 'error' });
  }

  const parser = new Parser(tokens, pairTable, diagnostics);
  const nodes = parser.parseMarkup();

  // Emit diagnostics for genuinely unclosed macros.
  // pairTable.unclosed contains only offsets that were never matched by a close tag.
  // Self-closing macros (<<set>>, <<include>>, etc.) complete their token sequence
  // without ending up in unclosed, so they produce no spurious errors here.
  for (const openOffset of pairTable.unclosed) {
    const openTok = tokens.find(t => t.type === TokenType.MacroOpen && t.range.start === openOffset);
    if (openTok) {
      const nameTok = tokens.find(t => t.type === TokenType.MacroName && t.range.start > openTok.range.start && t.range.start <= openTok.range.start + 20);
      const name = nameTok?.value ?? '';
      diagnostics.push({
        message: `Unclosed macro <<${name}>> — missing </</${name}>>`,
        range: openTok.range,
        severity: 'error',
      });
    }
  }

  return nodes;
}

// ---------------------------------------------------------------------------
// Expression parsing — standalone, called with arg token arrays
// ---------------------------------------------------------------------------

function skipTrivia(tokens: Token[], state: { i: number }): void {
  while (state.i < tokens.length) {
    const t = tokens[state.i];
    if (!t) break;
    if (
      t.type === TokenType.Whitespace ||
      t.type === TokenType.Newline ||
      t.type === TokenType.LineComment ||
      t.type === TokenType.BlockComment
    ) { state.i++; continue; }
    break;
  }
}

function getPrecedence(tok: Token): number {
  if (tok.type === TokenType.SugarOperator) {
    const p: Record<string, number> = {
      or: 1, and: 2,
      eq: 3, neq: 3, is: 3, isnot: 3,
      gt: 4, gte: 4, lt: 4, lte: 4,
      to: 0, // assignment-like — lowest
    };
    return p[tok.value] ?? -1;
  }
  if (tok.type === TokenType.Operator) {
    const p: Record<string, number> = {
      '=': 0,
      '||': 1, '&&': 2,
      '==': 3, '===': 3, '!=': 3, '!==': 3,
      '>': 4, '>=': 4, '<': 4, '<=': 4,
      '+': 5, '-': 5,
      '*': 6, '/': 6, '%': 6,
    };
    return p[tok.value] ?? -1;
  }
  return -1;
}

export function parseExprArgs(tokens: Token[], diagnostics: ParseDiagnostic[]): ExpressionNode[] {
  const state = { i: 0 };
  const args: ExpressionNode[] = [];

  while (state.i < tokens.length) {
    skipTrivia(tokens, state);
    if (state.i >= tokens.length) break;

    const expr = parseExpr(tokens, state, diagnostics, 0);
    if (!expr) {
      const tok = tokens[state.i];
      if (tok) { diagnostics.push({ message: `Unexpected token: ${tok.value}`, range: tok.range }); state.i++; }
      continue;
    }
    args.push(expr);
    skipTrivia(tokens, state);
    if (tokens[state.i]?.type === TokenType.Comma) state.i++;
  }

  return args;
}

function parseExpr(
  tokens: Token[], state: { i: number },
  diagnostics: ParseDiagnostic[], minPrec: number,
): ExpressionNode | null {
  let left = parseUnary(tokens, state, diagnostics);
  if (!left) return null;

  while (state.i < tokens.length) {
    skipTrivia(tokens, state);
    const op = tokens[state.i];
    if (!op) break;
    const prec = getPrecedence(op);
    if (prec < minPrec) break;
    state.i++;
    const right = parseExpr(tokens, state, diagnostics, prec + 1);
    if (!right) {
      diagnostics.push({ message: `Missing right-hand side for '${op.value}'`, range: op.range });
      return left;
    }
    left = {
      type: 'binaryOp', operator: op.value, left, right,
      range: { start: left.range.start, end: right.range.end },
    } as BinaryOpNode;
  }

  return left;
}

function parseUnary(tokens: Token[], state: { i: number }, diagnostics: ParseDiagnostic[]): ExpressionNode | null {
  skipTrivia(tokens, state);
  const tok = tokens[state.i];
  if (!tok) return null;

  const isUnary =
    (tok.type === TokenType.Operator && ['!', '-', '+'].includes(tok.value)) ||
    (tok.type === TokenType.SugarOperator && tok.value === 'not');

  if (!isUnary) return parsePostfix(tokens, state, diagnostics);

  state.i++;
  const operand = parseUnary(tokens, state, diagnostics);
  if (!operand) { diagnostics.push({ message: `Missing operand for '${tok.value}'`, range: tok.range }); return null; }
  return { type: 'unaryOp', operator: tok.value, operand, range: { start: tok.range.start, end: operand.range.end } } as UnaryOpNode;
}

function parsePostfix(tokens: Token[], state: { i: number }, diagnostics: ParseDiagnostic[]): ExpressionNode | null {
  let expr = parsePrimary(tokens, state, diagnostics);
  if (!expr) return null;

  while (state.i < tokens.length) {
    skipTrivia(tokens, state);
    const tok = tokens[state.i];
    if (!tok) break;

    // .property
    if (tok.type === TokenType.PropertyAccess) {
      const propTok = tokens[state.i + 1];
      if (!propTok || propTok.type !== TokenType.Identifier) {
        diagnostics.push({ message: 'Expected property name after .', range: tok.range });
        state.i++;
        break;
      }
      expr = {
        type: 'propertyAccess', object: expr, property: propTok.value,
        propertyRange: propTok.range,
        range: { start: expr.range.start, end: propTok.range.end },
      } as PropertyAccessNode;
      state.i += 2;
      continue;
    }

    // [index]
    if (tok.type === TokenType.BracketOpen) {
      state.i++;
      const idx = parseExpr(tokens, state, diagnostics, 0);
      skipTrivia(tokens, state);
      const close = tokens[state.i];
      if (!idx || !close || close.type !== TokenType.BracketClose) {
        diagnostics.push({ message: 'Expected ] for index access', range: tok.range });
        if (close?.type === TokenType.BracketClose) state.i++;
        break;
      }
      expr = {
        type: 'indexAccess', object: expr, index: idx,
        range: { start: expr.range.start, end: close.range.end },
      } as IndexAccessNode;
      state.i++;
      continue;
    }

    // (call)
    if (tok.type === TokenType.ParenOpen) {
      state.i++;
      const callArgs: ExpressionNode[] = [];
      while (state.i < tokens.length && tokens[state.i]?.type !== TokenType.ParenClose) {
        skipTrivia(tokens, state);
        const arg = parseExpr(tokens, state, diagnostics, 0);
        if (!arg) break;
        callArgs.push(arg);
        skipTrivia(tokens, state);
        if (tokens[state.i]?.type === TokenType.Comma) state.i++;
      }
      const close = tokens[state.i];
      if (!close || close.type !== TokenType.ParenClose) {
        diagnostics.push({ message: 'Expected ) for call', range: tok.range });
        break;
      }
      expr = {
        type: 'call', callee: expr, args: callArgs,
        range: { start: expr.range.start, end: close.range.end },
      } as CallNode;
      state.i++;
      continue;
    }

    break;
  }

  return expr;
}

function parsePrimary(tokens: Token[], state: { i: number }, diagnostics: ParseDiagnostic[]): ExpressionNode | null {
  skipTrivia(tokens, state);
  const tok = tokens[state.i];
  if (!tok) return null;

  if (tok.type === TokenType.StoryVar) {
    state.i++;
    return { type: 'storyVar', name: tok.value.slice(1), range: tok.range } as StoryVarNode;
  }

  if (tok.type === TokenType.TempVar) {
    state.i++;
    return { type: 'tempVar', name: tok.value.slice(1), range: tok.range } as TempVarNode;
  }

  if (tok.type === TokenType.Identifier) {
    state.i++;
    // true / false / null / undefined as literals
    if (tok.value === 'true')      return { type: 'literal', kind: 'boolean', value: true,      range: tok.range } as LiteralNode;
    if (tok.value === 'false')     return { type: 'literal', kind: 'boolean', value: false,     range: tok.range } as LiteralNode;
    if (tok.value === 'null')      return { type: 'literal', kind: 'null',    value: null,      range: tok.range } as LiteralNode;
    if (tok.value === 'undefined') return { type: 'literal', kind: 'undefined', value: undefined, range: tok.range } as LiteralNode;
    return { type: 'identifier', name: tok.value, range: tok.range } as IdentifierNode;
  }

  if (tok.type === TokenType.Number) {
    state.i++;
    return { type: 'literal', kind: 'number', value: Number(tok.value), range: tok.range } as LiteralNode;
  }

  if (tok.type === TokenType.String) {
    state.i++;
    const raw = tok.value;
    const inner = raw.length >= 2 ? raw.slice(1, -1) : raw;
    return { type: 'literal', kind: 'string', value: inner, range: tok.range } as LiteralNode;
  }

  if (tok.type === TokenType.ParenOpen) {
    state.i++;
    const expr = parseExpr(tokens, state, diagnostics, 0);
    skipTrivia(tokens, state);
    if (tokens[state.i]?.type === TokenType.ParenClose) state.i++;
    else diagnostics.push({ message: 'Expected )', range: tok.range });
    return expr;
  }

  if (tok.type === TokenType.BracketOpen) return parseArrayLit(tokens, state, diagnostics);
  if (tok.type === TokenType.BraceOpen)   return parseObjectLit(tokens, state, diagnostics);

  return null;
}

function parseArrayLit(tokens: Token[], state: { i: number }, diagnostics: ParseDiagnostic[]): ExpressionNode | null {
  const open = tokens[state.i]!;
  state.i++;
  const elements: ExpressionNode[] = [];

  while (state.i < tokens.length && tokens[state.i]?.type !== TokenType.BracketClose) {
    skipTrivia(tokens, state);
    if (tokens[state.i]?.type === TokenType.BracketClose) break;
    const el = parseExpr(tokens, state, diagnostics, 0);
    if (!el) break;
    elements.push(el);
    skipTrivia(tokens, state);
    if (tokens[state.i]?.type === TokenType.Comma) state.i++;
  }

  const close = tokens[state.i];
  if (!close || close.type !== TokenType.BracketClose) {
    diagnostics.push({ message: 'Expected ] for array', range: open.range });
    return null;
  }
  state.i++;
  return { type: 'arrayLiteral', elements, range: { start: open.range.start, end: close.range.end } } as ArrayLiteralNode;
}

function parseObjectLit(tokens: Token[], state: { i: number }, diagnostics: ParseDiagnostic[]): ExpressionNode | null {
  const open = tokens[state.i]!;
  state.i++;
  const properties: ObjectProperty[] = [];

  while (state.i < tokens.length && tokens[state.i]?.type !== TokenType.BraceClose) {
    skipTrivia(tokens, state);
    const keyTok = tokens[state.i];
    if (!keyTok || keyTok.type === TokenType.BraceClose) break;

    const isStr  = keyTok.type === TokenType.String;
    const isId   = keyTok.type === TokenType.Identifier;
    if (!isStr && !isId) { diagnostics.push({ message: 'Expected object key', range: keyTok.range }); break; }

    const key = isStr && keyTok.value.length >= 2 ? keyTok.value.slice(1, -1) : keyTok.value;
    state.i++;
    skipTrivia(tokens, state);

    const colon = tokens[state.i];
    if (!colon || colon.type !== TokenType.Colon) { diagnostics.push({ message: 'Expected : after key', range: keyTok.range }); break; }
    state.i++;

    const val = parseExpr(tokens, state, diagnostics, 0);
    if (!val) { diagnostics.push({ message: `Expected value for key ${key}`, range: keyTok.range }); break; }

    properties.push({ key, value: val, range: { start: keyTok.range.start, end: val.range.end } });
    skipTrivia(tokens, state);
    if (tokens[state.i]?.type === TokenType.Comma) state.i++;
  }

  const close = tokens[state.i];
  if (!close || close.type !== TokenType.BraceClose) {
    diagnostics.push({ message: 'Expected } for object', range: open.range });
    return null;
  }
  state.i++;
  return { type: 'objectLiteral', properties, range: { start: open.range.start, end: close.range.end } } as ObjectLiteralNode;
}