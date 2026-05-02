import type { CompletionItem, Diagnostic } from 'vscode-languageserver/node';

// ---------------------------------------------------------------------------
// Format adapter contract
//
// Every story format the server knows about provides one StoryFormatAdapter.
// Handlers call the ACTIVE adapter from FormatRegistry — they never import
// format packages directly.  This keeps the core server format-agnostic.
//
// To add a new format:
//   1. Create server/src/formats/<id>/adapter.ts implementing StoryFormatAdapter.
//   2. Register it in server/src/formats/registry.ts.
//   3. Done — no handler files need to change.
// ---------------------------------------------------------------------------

// The subset of workspace state that adapters need for feature computation.
// Add fields here as new adapter hooks require them.
export interface FormatContext {
  /** Resolved format id from StoryData or config, e.g. "sugarcube-2". */
  readonly formatId: string;
  /** Workspace passage names available at request time. */
  readonly passageNames: string[];
}

// Completion request data passed to the adapter.
export interface AdapterCompletionRequest {
  /** Full document text. */
  text: string;
  /** Cursor offset within text. */
  offset: number;
}

// Hover request data passed to the adapter.
export interface AdapterHoverRequest {
  /** Token type from the workspace index, e.g. "macro", "variable", "passage". */
  tokenType: string;
  /** Raw token name without sigils, e.g. "if", "myVar", "PassageName". */
  rawName: string;
}

// Diagnostic request data passed to the adapter.
export interface AdapterDiagnosticRequest {
  /** Full document text. */
  text: string;
  /** Document URI. */
  uri: string;
}

// ---------------------------------------------------------------------------
// The adapter interface every format must implement.
// ---------------------------------------------------------------------------

export interface StoryFormatAdapter {
  /** Canonical format id, lower-cased, matching StoryData format field. */
  readonly id: string;

  /** Human-readable display name shown in status bar and logs. */
  readonly displayName: string;

  // ── Completion ─────────────────────────────────────────────────────────────

  /**
   * Return format-specific completion items for the given position.
   * The adapter is responsible for detecting context (e.g. "inside <<",
   * "typing a variable sigil") from the raw text + offset.
   * Core workspace completions (passage names, user-defined variables/macros)
   * are added by the handler AFTER calling this — don't duplicate them.
   */
  provideFormatCompletions(req: AdapterCompletionRequest, ctx: FormatContext): CompletionItem[];

  /**
   * Given a macro/symbol name, returns the snippet body to insert.
   * Used by the handler when building completion items for user-defined symbols.
   * Return null to use a generic insertion.
   */
  buildMacroSnippet(name: string, hasBody: boolean): string | null;

  /**
   * Names of macros that can wrap content (have a corresponding closing tag).
   * Used to drive close-tag completion and folding range detection.
   */
  getBlockMacroNames(): ReadonlySet<string>;

  // ── Hover ──────────────────────────────────────────────────────────────────

  /**
   * Return markdown hover text for a builtin token, or null if unknown.
   * The handler calls this for tokens that aren't user-defined symbols.
   */
  provideBuiltinHover(req: AdapterHoverRequest, ctx: FormatContext): string | null;

  /**
   * Return markdown describing a variable sigil prefix, or null.
   * e.g. for SugarCube "$" → "SugarCube story variable", "_" → "temp variable"
   */
  describeVariableSigil(sigil: string): string | null;

  // ── Diagnostics ────────────────────────────────────────────────────────────

  /**
   * Return format-specific diagnostics for a document.
   * Core unknown-passage diagnostics are produced by the handler separately.
   */
  provideDiagnostics(req: AdapterDiagnosticRequest, ctx: FormatContext): Diagnostic[];

  // ── Virtual runtime prelude ────────────────────────────────────────────────

  /**
   * Return TypeScript/JS stubs injected into virtual documents for type-checking.
   * For SugarCube: declare State, Engine, SugarCube, setup, etc.
   * Return empty string if format has no virtual runtime.
   */
  getVirtualRuntimePrelude(): string;
}