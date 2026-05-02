import { Token, TokenType } from './tokenTypes';

const MACRO_NAME_RE = /^[A-Za-z_=][A-Za-z0-9_-]*/;
const IDENTIFIER_RE = /^[A-Za-z_][A-Za-z0-9_]*/;
const NUMBER_RE = /^(?:\d+\.\d+|\d+)/;
const SUGAR_OPS = new Set([
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
export function lex(input: string): Token[] {
  return new Lexer(input).tokenize();
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

  constructor(private src: string) {}

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
    const ch = this.src.charCodeAt(this.pos);

    // ---- Comments are checked FIRST in ALL contexts -----------------------
    // Both <!-- --> and /* */ are consumed atomically before any macro or link
    // scanning. This ensures that <<macroName>> and >> inside comment spans
    // are never tokenized as structural tokens.
    if (ch === 60 /* < */ && this.src.startsWith('<!--', this.pos)) { this.scanHtmlComment();   return; }
    if (ch === 47 /* / */ && this.src.charCodeAt(this.pos + 1) === 42 /* * */) { this.scanBlockComment();  return; }

    // ---- Context-specific scanning ----------------------------------------
    if (this.inMacro()) {
      // Line comments are only meaningful inside macro args
      if (ch === 47 /* / */ && this.src.charCodeAt(this.pos + 1) === 47 /* / */) { this.scanLineComment(); return; }
      this.scanExprToken();
      return;
    }

    // ---- Markup context (not inside any macro) ----------------------------
    if (ch === 60 /* < */) {
      if (this.src.charCodeAt(this.pos + 1) === 60 /* < */) {
        if (this.src.charCodeAt(this.pos + 2) === 47 /* / */) { this.scanMacroCloseOpen(); return; }
        this.scanMacroOpen(); return;
      }
    }
    if (ch === 91 /* [ */ && this.src.charCodeAt(this.pos + 1) === 91 /* [ */) { this.scanLink(); return; }
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

    // Inline regex execution — slice only the small prefix we need
    const nameMatch = MACRO_NAME_RE.exec(this.src.slice(this.pos, this.pos + 64));
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

    const nameMatch = MACRO_NAME_RE.exec(this.src.slice(this.pos, this.pos + 64));
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

    const ch = this.src.charCodeAt(this.pos);

    // Comments inside macro args — checked before the '/' operator case
    if (ch === 47 /* / */) {
      const next = this.src.charCodeAt(this.pos + 1);
      if (next === 47) { this.scanLineComment();  return; }
      if (next === 42) { this.scanBlockComment(); return; }
    }

    if (ch === 34 /* " */ || ch === 39 /* ' */ || ch === 96 /* ` */) { this.scanString(String.fromCharCode(ch)); return; }

    if (ch === 36 /* $ */) { this.scanVar(TokenType.StoryVar); return; }
    if (ch === 95 /* _ */) {
      const next = this.src.charCodeAt(this.pos + 1);
      if (next >= 65 && next <= 90 || next >= 97 && next <= 122 || next === 95) { this.scanVar(TokenType.TempVar); return; }
    }

    const numMatch = NUMBER_RE.exec(this.src.slice(this.pos, this.pos + 32));
    if (numMatch) {
      const v = numMatch[0]!;
      this.push(TokenType.Number, v, this.pos, this.pos + v.length);
      this.pos += v.length;
      return;
    }

    const idMatch = IDENTIFIER_RE.exec(this.src.slice(this.pos, this.pos + 64));
    if (idMatch) {
      const v = idMatch[0]!;
      const type = SUGAR_OPS.has(v) ? TokenType.SugarOperator : TokenType.Identifier;
      this.push(type, v, this.pos, this.pos + v.length);
      this.pos += v.length;
      return;
    }

    if (ch === 44 /* , */) { this.push(TokenType.Comma,   String.fromCharCode(ch), this.pos, this.pos + 1); this.pos++; return; }
    if (ch === 58 /* : */) { this.push(TokenType.Colon,   String.fromCharCode(ch), this.pos, this.pos + 1); this.pos++; return; }
    if (ch === 46 /* . */) {
      const next = this.src.charCodeAt(this.pos + 1);
      if (next >= 65 && next <= 90 || next >= 97 && next <= 122 || next === 95) {
        this.push(TokenType.PropertyAccess, '.', this.pos, this.pos + 1); this.pos++; return;
      }
    }
    if (ch === 40) { this.push(TokenType.ParenOpen,    '(', this.pos, this.pos + 1); this.stack.push(Enc.Paren);   this.pos++; return; }
    if (ch === 41) { this.push(TokenType.ParenClose,   ')', this.pos, this.pos + 1); this.popEncIf(Enc.Paren);     this.pos++; return; }
    if (ch === 91) { this.push(TokenType.BracketOpen,  '[', this.pos, this.pos + 1); this.stack.push(Enc.Bracket); this.pos++; return; }
    if (ch === 93) { this.push(TokenType.BracketClose, ']', this.pos, this.pos + 1); this.popEncIf(Enc.Bracket);   this.pos++; return; }
    if (ch === 123) { this.push(TokenType.BraceOpen,    '{', this.pos, this.pos + 1); this.stack.push(Enc.Brace);   this.pos++; return; }
    if (ch === 125) { this.push(TokenType.BraceClose,   '}', this.pos, this.pos + 1); this.popEncIf(Enc.Brace);     this.pos++; return; }

    // Three-char operators
    if (ch === 61 /* = */ || ch === 33 /* ! */) {
      const c1 = this.src.charCodeAt(this.pos + 1);
      const c2 = this.src.charCodeAt(this.pos + 2);
      if (c1 === 61 && c2 === 61) {
        const op = ch === 61 ? '===' : '!=='  ;
        this.push(TokenType.Operator, op, this.pos, this.pos + 3); this.pos += 3; return;
      }
    }
    // Two-char operators
    const ch1 = this.src.charCodeAt(this.pos + 1);
    if (ch === 61 && ch1 === 61) { this.push(TokenType.Operator, '==', this.pos, this.pos + 2); this.pos += 2; return; }
    if (ch === 33 && ch1 === 61) { this.push(TokenType.Operator, '!=', this.pos, this.pos + 2); this.pos += 2; return; }
    if (ch === 60 && ch1 === 61) { this.push(TokenType.Operator, '<=', this.pos, this.pos + 2); this.pos += 2; return; }
    if (ch === 62 && ch1 === 61) { this.push(TokenType.Operator, '>=', this.pos, this.pos + 2); this.pos += 2; return; }
    if (ch === 38 && ch1 === 38) { this.push(TokenType.Operator, '&&', this.pos, this.pos + 2); this.pos += 2; return; }
    if (ch === 124 && ch1 === 124) { this.push(TokenType.Operator, '||', this.pos, this.pos + 2); this.pos += 2; return; }
    if (ch === 45 && ch1 === 62) { this.push(TokenType.Operator, '->', this.pos, this.pos + 2); this.pos += 2; return; }
    if (ch === 60 && ch1 === 45) { this.push(TokenType.Operator, '<-', this.pos, this.pos + 2); this.pos += 2; return; }
    // Single-char operators
    if (ch === 43 || ch === 45 || ch === 42 || ch === 47 || ch === 37 || ch === 61 || ch === 33 || ch === 60 || ch === 62 || ch === 63) {
      this.push(TokenType.Operator, String.fromCharCode(ch), this.pos, this.pos + 1); this.pos++; return;
    }

    if (ch === 10 /* \n */) { this.push(TokenType.Newline, '\n', this.pos, this.pos + 1); this.pos++; return; }
    if (ch === 9 || ch === 11 || ch === 12 || ch === 13 || ch === 32 || ch === 160) {
      const start = this.pos;
      while (this.pos < this.src.length) {
        const c = this.src.charCodeAt(this.pos);
        if (c !== 9 && c !== 11 && c !== 12 && c !== 13 && c !== 32 && c !== 160) break;
        this.pos++;
      }
      this.push(TokenType.Whitespace, this.src.slice(start, this.pos), start, this.pos);
      return;
    }

    this.push(TokenType.Error, String.fromCharCode(ch), this.pos, this.pos + 1);
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
      const ch = this.src.charCodeAt(this.pos);
      // Check for structural tokens using indexOf for faster bulk skipping
      if (ch === 60 /* < */) {
        // Check <<, <</, <!--
        const next = this.src.charCodeAt(this.pos + 1);
        if (next === 60 /* < */) break; // << or <</
        if (next === 33 /* ! */ && this.src.startsWith('<!--', this.pos)) break;
      }
      if (ch === 47 /* / */ && this.src.charCodeAt(this.pos + 1) === 42 /* * */) break; // block comment
      if (ch === 91 /* [ */ && this.src.charCodeAt(this.pos + 1) === 91 /* [ */) break; // [[
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
      // Unterminated HTML comment — consume rest of file and emit error
      this.pos = this.src.length;
      this.push(TokenType.Error, 'unterminated HTML comment', start, this.pos);
      return;
    }
    this.pos = end + 3;
    this.push(TokenType.HtmlComment, this.src.slice(start, this.pos), start, this.pos);
  }

  private scanLineComment(): void {
    const start = this.pos;
    while (this.pos < this.src.length && this.src.charCodeAt(this.pos) !== 10 /* \n */) this.pos++;
    this.push(TokenType.LineComment, this.src.slice(start, this.pos), start, this.pos);
  }

  /**
   * Scan a block comment /* ... * / atomically.
   * Called from BOTH markup context (scanNext) and macro-args context (scanExprToken).
   * Emits a single BlockComment token — callers must not attempt to re-lex the content.
   * Any << >> [[ inside are consumed silently inside this scan.
   *
   * If the comment is unterminated, emits an Error token and the diagnostic
   * will be generated by the caller that collects lexer errors.
   */
  private scanBlockComment(): void {
    const start = this.pos;
    const end = this.src.indexOf('*/', this.pos + 2);
    if (end === -1) {
      // Unterminated block comment — consume rest of file and emit error
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
    const quoteCode = quote.charCodeAt(0);
    this.pos++;
    while (this.pos < this.src.length) {
      const ch = this.src.charCodeAt(this.pos);
      if (ch === 92 /* \ */) { this.pos += 2; continue; }
      if (ch === quoteCode) { this.pos++; break; }
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
    const idMatch = IDENTIFIER_RE.exec(this.src.slice(this.pos, this.pos + 64));
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
    while (this.pos < this.src.length) {
      const c = this.src.charCodeAt(this.pos);
      if (c !== 9 && c !== 10 && c !== 11 && c !== 12 && c !== 13 && c !== 32 && c !== 160) break;
      this.pos++;
    }
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