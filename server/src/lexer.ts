import { Token, TokenType } from './tokenTypes';
import type { StoryFormatAdapter } from './formats/types';

const MACRO_NAME_RE = /^[A-Za-z_=][A-Za-z0-9_-]*/;
const IDENTIFIER_RE = /^[A-Za-z_][A-Za-z0-9_]*/;
const NUMBER_RE = /^(?:\d+\.\d+|\d+)/;

/** Default sugar operators — SugarCube conventions used when no adapter is provided. */
const DEFAULT_SUGAR_OPS = new Set([
  'to', 'eq', 'neq', 'gt', 'gte', 'lt', 'lte',
  'and', 'or', 'not', 'is', 'isnot',
]);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Lex a passage body (everything after the :: header line).
 *
 * Comment handling:
 *   <!-- ... -->  opaque in ALL contexts (markup + macro args)
 *   /* ... * /    opaque in BOTH contexts:
 *                   - in markup context: consumed before << is seen, so any
 *                     <<macroName>> inside the comment is never tokenized
 *                   - in macro args: consumed by scanExprToken before the
 *                     >> end-check, so >> inside the comment is never seen
 *   // ...        line comment, only inside macro args (// is plain text in markup)
 */
export function lex(input: string, adapter?: StoryFormatAdapter): Token[] {
  // Build the operator set from the adapter if provided, otherwise use defaults
  const sugarOps = adapter
    ? new Set(Object.keys(adapter.getOperatorPrecedence()))
    : DEFAULT_SUGAR_OPS;
  return new Lexer(input, sugarOps, adapter).tokenize();
}

// ---------------------------------------------------------------------------
// Internal enclosure types
// ---------------------------------------------------------------------------
const enum Enc {
  Macro,
  Link,
  String,
  Paren,
  Bracket,
  Brace,
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------
class Lexer {
  private pos = 0;
  private tokens: Token[] = [];
  private stack: Enc[] = [];

  constructor(
    private src: string,
    private sugarOps: Set<string>,
    private adapter?: StoryFormatAdapter,
  ) {}

  tokenize(): Token[] {
    while (this.pos < this.src.length) {
      this.scanNext();
    }
    this.push(TokenType.EOF, '', this.pos, this.pos);
    return this.tokens;
  }

  // -------------------------------------------------------------------------
  // Top-level dispatcher
  // -------------------------------------------------------------------------
  private scanNext(): void {
    // ---- Comments are checked FIRST in ALL contexts -----------------------
    // Both <!-- --> and /* */ are consumed atomically before any macro or link
    // scanning. This ensures that <<macroName>> and >> inside comment spans
    // are never tokenized as structural tokens.
    if (this.src.startsWith('<!--', this.pos)) { this.scanHtmlComment();   return; }
    if (this.src.startsWith('/*',   this.pos)) { this.scanBlockComment();  return; }

    // ---- Context-specific scanning ----------------------------------------
    if (this.inMacro()) {
      // Line comments are only meaningful inside macro args
      if (this.src.startsWith('//', this.pos)) { this.scanLineComment(); return; }
      this.scanExprToken();
      return;
    }

    // ---- Markup context (not inside any macro) ----------------------------
    if (this.src.startsWith('<</', this.pos)) { this.scanMacroCloseOpen(); return; }
    if (this.src.startsWith('<<',  this.pos)) { this.scanMacroOpen();      return; }
    if (this.src.startsWith('[[',  this.pos)) { this.scanLink();            return; }
    this.scanText();
  }

  // -------------------------------------------------------------------------
  // Macro open: << name args >>
  // -------------------------------------------------------------------------
  private scanMacroOpen(): void {
    const start = this.pos;
    this.pos += 2;
    this.push(TokenType.MacroOpen, '<<', start, this.pos);
    this.stack.push(Enc.Macro);

    this.skipWhitespaceInExpr();

    const nameMatch = this.src.slice(this.pos).match(MACRO_NAME_RE);
    if (nameMatch) {
      const name = nameMatch[0]!;
      this.push(TokenType.MacroName, name, this.pos, this.pos + name.length);
      this.pos += name.length;
    }

    this.scanExprUntilMacroClose();

    if (this.src.startsWith('>>', this.pos)) {
      this.push(TokenType.MacroClose, '>>', this.pos, this.pos + 2);
      this.pos += 2;
    } else {
      this.push(TokenType.Error, 'unclosed macro', this.pos, this.pos);
    }
    this.stack.pop();
  }

  // -------------------------------------------------------------------------
  // Macro close-open: <</name>>
  // -------------------------------------------------------------------------
  private scanMacroCloseOpen(): void {
    const start = this.pos;
    this.pos += 3;
    this.push(TokenType.MacroCloseOpen, '<</', start, this.pos);

    this.skipWhitespaceInExpr();

    const nameMatch = this.src.slice(this.pos).match(MACRO_NAME_RE);
    if (nameMatch) {
      const name = nameMatch[0]!;
      this.push(TokenType.MacroName, name, this.pos, this.pos + name.length);
      this.pos += name.length;
    }

    this.skipWhitespaceInExpr();
    if (this.src.startsWith('>>', this.pos)) {
      this.push(TokenType.MacroClose, '>>', this.pos, this.pos + 2);
      this.pos += 2;
    } else {
      this.push(TokenType.Error, 'unclosed macro close tag', this.pos, this.pos);
    }
  }

  // -------------------------------------------------------------------------
  // Expression scanning inside macro args
  // -------------------------------------------------------------------------
  private scanExprUntilMacroClose(): void {
    while (this.pos < this.src.length) {
      if (this.src.startsWith('>>', this.pos) && this.stackDepthAboveMacro() === 0) {
        break;
      }
      this.scanExprToken();
    }
  }

  private stackDepthAboveMacro(): number {
    let depth = 0;
    for (let i = this.stack.length - 1; i >= 0; i--) {
      if (this.stack[i] === Enc.Macro) break;
      depth++;
    }
    return depth;
  }

  private scanExprToken(): void {
    this.skipWhitespaceInExpr();
    if (this.pos >= this.src.length) return;

    const ch = this.src[this.pos]!;

    // Comments inside macro args — checked before the '/' operator case
    if (this.src.startsWith('//', this.pos)) { this.scanLineComment();  return; }
    if (this.src.startsWith('/*', this.pos)) { this.scanBlockComment(); return; }

    if (ch === '"' || ch === "'" || ch === '`') { this.scanString(ch); return; }

    if (this.adapter) {
      const sigilType = this.adapter.resolveVariableSigil(ch);
      if (sigilType === 'story') { this.scanVar(TokenType.StoryVar); return; }
      if (sigilType === 'temporary') {
        const next = this.src[this.pos + 1];
        if (next && /[A-Za-z_]/.test(next)) { this.scanVar(TokenType.TempVar); return; }
      }
    } else {
      // Default SugarCube behavior
      if (ch === '$') { this.scanVar(TokenType.StoryVar); return; }
      if (ch === '_') {
        const next = this.src[this.pos + 1];
        if (next && /[A-Za-z_]/.test(next)) { this.scanVar(TokenType.TempVar); return; }
      }
    }

    const numMatch = this.src.slice(this.pos).match(NUMBER_RE);
    if (numMatch) {
      const v = numMatch[0]!;
      this.push(TokenType.Number, v, this.pos, this.pos + v.length);
      this.pos += v.length;
      return;
    }

    const idMatch = this.src.slice(this.pos).match(IDENTIFIER_RE);
    if (idMatch) {
      const v = idMatch[0]!;
      const type = this.sugarOps.has(v) ? TokenType.SugarOperator : TokenType.Identifier;
      this.push(type, v, this.pos, this.pos + v.length);
      this.pos += v.length;
      return;
    }

    if (ch === ',') { this.push(TokenType.Comma,   ch, this.pos, this.pos + 1); this.pos++; return; }
    if (ch === ':') { this.push(TokenType.Colon,   ch, this.pos, this.pos + 1); this.pos++; return; }
    if (ch === '.') {
      const next = this.src[this.pos + 1];
      if (next && /[A-Za-z_]/.test(next)) {
        this.push(TokenType.PropertyAccess, ch, this.pos, this.pos + 1); this.pos++; return;
      }
    }
    if (ch === '(') { this.push(TokenType.ParenOpen,    ch, this.pos, this.pos + 1); this.stack.push(Enc.Paren);   this.pos++; return; }
    if (ch === ')') { this.push(TokenType.ParenClose,   ch, this.pos, this.pos + 1); this.popEncIf(Enc.Paren);     this.pos++; return; }
    if (ch === '[') { this.push(TokenType.BracketOpen,  ch, this.pos, this.pos + 1); this.stack.push(Enc.Bracket); this.pos++; return; }
    if (ch === ']') { this.push(TokenType.BracketClose, ch, this.pos, this.pos + 1); this.popEncIf(Enc.Bracket);   this.pos++; return; }
    if (ch === '{') { this.push(TokenType.BraceOpen,    ch, this.pos, this.pos + 1); this.stack.push(Enc.Brace);   this.pos++; return; }
    if (ch === '}') { this.push(TokenType.BraceClose,   ch, this.pos, this.pos + 1); this.popEncIf(Enc.Brace);     this.pos++; return; }

    const three = this.src.slice(this.pos, this.pos + 3);
    const two   = this.src.slice(this.pos, this.pos + 2);
    if (['===', '!=='].includes(three)) { this.push(TokenType.Operator, three, this.pos, this.pos + 3); this.pos += 3; return; }
    if (['==', '!=', '<=', '>=', '&&', '||', '->', '<-'].includes(two)) {
      this.push(TokenType.Operator, two, this.pos, this.pos + 2); this.pos += 2; return;
    }
    if ('+-*/%=!<>?'.includes(ch)) { this.push(TokenType.Operator, ch, this.pos, this.pos + 1); this.pos++; return; }

    if (ch === '\n') { this.push(TokenType.Newline, ch, this.pos, this.pos + 1); this.pos++; return; }
    if (/\s/.test(ch)) {
      const start = this.pos;
      while (this.pos < this.src.length && /[^\S\n]/.test(this.src[this.pos]!)) this.pos++;
      this.push(TokenType.Whitespace, this.src.slice(start, this.pos), start, this.pos);
      return;
    }

    this.push(TokenType.Error, ch, this.pos, this.pos + 1);
    this.pos++;
  }

  // -------------------------------------------------------------------------
  // Link: [[ display | target ]] or [[ target ]]
  // -------------------------------------------------------------------------
  private scanLink(): void {
    const start = this.pos;
    this.pos += 2;
    this.push(TokenType.LinkOpen, '[[', start, this.pos);

    const closeIdx = this.src.indexOf(']]', this.pos);
    if (closeIdx === -1) {
      this.push(TokenType.Error, 'unclosed link', this.pos, this.src.length);
      this.pos = this.src.length;
      return;
    }

    const inner = this.src.slice(this.pos, closeIdx);
    const innerStart = this.pos;

    let sepIdx = -1;
    let sepTok: '|' | '->' | '<-' | null = null;
    const pipeIdx = inner.indexOf('|');
    const fwdIdx  = inner.indexOf('->');
    const backIdx = inner.indexOf('<-');
    const candidates: [number, '|' | '->' | '<-'][] = [];
    if (pipeIdx  !== -1) candidates.push([pipeIdx,  '|']);
    if (fwdIdx   !== -1) candidates.push([fwdIdx,   '->']);
    if (backIdx  !== -1) candidates.push([backIdx,  '<-']);
    if (candidates.length > 0) {
      candidates.sort((a, b) => a[0] - b[0]);
      [sepIdx, sepTok] = candidates[0]!;
    }

    if (sepTok) {
      const sepLen = sepTok.length;
      const left  = inner.slice(0, sepIdx).trim();
      const right = inner.slice(sepIdx + sepLen).trim();
      if (left)  this.push(TokenType.Text, left,  innerStart, innerStart + sepIdx);
      this.push(TokenType.LinkSeparator, sepTok, innerStart + sepIdx, innerStart + sepIdx + sepLen);
      if (right) this.push(TokenType.Text, right, innerStart + sepIdx + sepLen, closeIdx);
    } else {
      if (inner.trim()) this.push(TokenType.Text, inner.trim(), innerStart, closeIdx);
    }

    this.pos = closeIdx;
    this.push(TokenType.LinkClose, ']]', this.pos, this.pos + 2);
    this.pos += 2;
  }

  // -------------------------------------------------------------------------
  // Markup text — scan until next structural token or comment opener
  // -------------------------------------------------------------------------
  private scanText(): void {
    const start = this.pos;
    while (this.pos < this.src.length) {
      if (this.src.startsWith('<!--', this.pos)) break;
      if (this.src.startsWith('/*',   this.pos)) break;  // block comment in markup
      if (this.src.startsWith('<</',  this.pos)) break;
      if (this.src.startsWith('<<',   this.pos)) break;
      if (this.src.startsWith('[[',   this.pos)) break;
      this.pos++;
    }
    if (this.pos > start) {
      this.push(TokenType.Text, this.src.slice(start, this.pos), start, this.pos);
    }
  }

  // -------------------------------------------------------------------------
  // Comments
  // -------------------------------------------------------------------------
  private scanHtmlComment(): void {
    const start = this.pos;
    const end = this.src.indexOf('-->', this.pos + 4);
    if (end === -1) {
      // Unterminated HTML comment — consume rest of file, emit error
      this.pos = this.src.length;
      this.push(TokenType.Error, 'unterminated HTML comment', start, this.pos);
      return;
    }
    this.pos = end + 3;
    this.push(TokenType.HtmlComment, this.src.slice(start, this.pos), start, this.pos);
  }

  private scanLineComment(): void {
    const start = this.pos;
    while (this.pos < this.src.length && this.src[this.pos] !== '\n') this.pos++;
    this.push(TokenType.LineComment, this.src.slice(start, this.pos), start, this.pos);
  }

  /**
   * Scan a block comment /* ... * / atomically.
   * Called from BOTH markup context (scanNext) and macro-args context (scanExprToken).
   * Emits a single BlockComment token — callers must not attempt to re-lex the content.
   * Any << >> [[ inside are consumed silently inside this scan.
   */
  private scanBlockComment(): void {
    const start = this.pos;
    const end = this.src.indexOf('*/', this.pos + 2);
    if (end === -1) {
      // Unterminated block comment — consume rest of file, emit error
      this.pos = this.src.length;
      this.push(TokenType.Error, 'unterminated block comment', start, this.pos);
      return;
    }
    this.pos = end + 2;
    this.push(TokenType.BlockComment, this.src.slice(start, this.pos), start, this.pos);
  }

  // -------------------------------------------------------------------------
  // Strings
  // -------------------------------------------------------------------------
  private scanString(quote: string): void {
    const start = this.pos;
    this.pos++;
    while (this.pos < this.src.length) {
      const ch = this.src[this.pos]!;
      if (ch === '\\') { this.pos += 2; continue; }
      if (ch === quote) { this.pos++; break; }
      this.pos++;
    }
    this.push(TokenType.String, this.src.slice(start, this.pos), start, this.pos);
  }

  // -------------------------------------------------------------------------
  // Variables
  // -------------------------------------------------------------------------
  private scanVar(type: TokenType.StoryVar | TokenType.TempVar): void {
    const start = this.pos;
    this.pos++;
    const idMatch = this.src.slice(this.pos).match(IDENTIFIER_RE);
    if (idMatch) this.pos += idMatch[0]!.length;
    this.push(type, this.src.slice(start, this.pos), start, this.pos);
  }

  // -------------------------------------------------------------------------
  // Helpers
  // -------------------------------------------------------------------------
  private inMacro(): boolean {
    for (let i = this.stack.length - 1; i >= 0; i--) {
      if (this.stack[i] === Enc.Macro) return true;
    }
    return false;
  }

  private skipWhitespaceInExpr(): void {
    while (this.pos < this.src.length && /\s/.test(this.src[this.pos]!)) this.pos++;
  }

  private popEncIf(enc: Enc): void {
    for (let i = this.stack.length - 1; i >= 0; i--) {
      if (this.stack[i] === enc) { this.stack.splice(i, 1); return; }
    }
  }

  private push(type: TokenType, value: string, start: number, end: number): void {
    this.tokens.push({ type, value, range: { start, end } });
  }
}