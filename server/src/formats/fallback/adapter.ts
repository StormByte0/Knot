import type { CompletionItem, Diagnostic } from 'vscode-languageserver/node';
import type {
  StoryFormatAdapter,
  FormatContext,
  AdapterCompletionRequest,
  AdapterHoverRequest,
  AdapterDiagnosticRequest,
  BuiltinMacroInfo,
  ImplicitPassageRefPattern,
  PassageRefApiCall,
} from '../types';

// ---------------------------------------------------------------------------
// FallbackAdapter
//
// Used when the active story format is unknown or not yet supported.
// All methods return safe empty values — no SugarCube-specific behaviour.
// Users will see basic workspace features (passage nav, go-to-definition on
// user variables) but no format-specific completions or hover docs.
// ---------------------------------------------------------------------------

const EMPTY_SET: ReadonlySet<string> = new Set();
const EMPTY_MAP: ReadonlyMap<string, ReadonlySet<string>> = new Map();

export class FallbackAdapter implements StoryFormatAdapter {
  readonly id          = 'fallback';
  readonly displayName = 'Unknown Format';

  provideFormatCompletions(_req: AdapterCompletionRequest, _ctx: FormatContext): CompletionItem[] {
    return [];
  }

  buildMacroSnippet(_name: string, _hasBody: boolean): string | null {
    return null;
  }

  getBlockMacroNames(): ReadonlySet<string> {
    return EMPTY_SET;
  }

  provideBuiltinHover(_req: AdapterHoverRequest, _ctx: FormatContext): string | null {
    return null;
  }

  describeVariableSigil(_sigil: string): string | null {
    return null;
  }

  provideDiagnostics(_req: AdapterDiagnosticRequest, _ctx: FormatContext): Diagnostic[] {
    return [];
  }

  getVirtualRuntimePrelude(): string {
    return '';
  }

  getPassageArgMacros(): ReadonlySet<string> {
    return EMPTY_SET;
  }

  getPassageArgIndex(_macroName: string, _argCount: number): number {
    return -1;
  }

  getBuiltinMacros(): ReadonlyArray<BuiltinMacroInfo> {
    return [];
  }

  getBuiltinGlobals(): ReadonlyArray<{ name: string; description: string }> {
    return [];
  }

  getSpecialPassageNames(): ReadonlySet<string> {
    return EMPTY_SET;
  }

  isSpecialPassage(_name: string): boolean {
    return false;
  }

  getSystemPassageNames(): ReadonlySet<string> {
    return EMPTY_SET;
  }

  getVariableAssignmentMacros(): ReadonlySet<string> {
    return EMPTY_SET;
  }

  getMacroDefinitionMacros(): ReadonlySet<string> {
    return EMPTY_SET;
  }

  getInlineScriptMacros(): ReadonlySet<string> {
    return EMPTY_SET;
  }

  getAnalysisPriority(_passageName: string): number {
    return 10;
  }

  getMacroParentConstraints(): ReadonlyMap<string, ReadonlySet<string>> {
    return EMPTY_MAP;
  }

  // ── Virtual doc generation ────────────────────────────────────────────────

  storyVarToJs(name: string): string { return name; }
  tempVarToJs(name: string): string { return name; }
  getOperatorNormalization(): Readonly<Record<string, string>> { return {}; }

  // ── Format hints (parser / lexer) ─────────────────────────────────────────

  getVariableSigils(): ReadonlyArray<{ sigil: string; variableType: 'story' | 'temporary' }> {
    return [];
  }

  resolveVariableSigil(_sigil: string): 'story' | 'temporary' | null {
    return null;
  }

  getOperatorPrecedence(): Readonly<Record<string, number>> {
    return {};
  }

  getScriptTags(): ReadonlyArray<string> {
    return [];
  }

  getStylesheetTags(): ReadonlyArray<string> {
    return [];
  }

  getTempVarPrefix(): string {
    return '';
  }

  getAssignmentOperators(): ReadonlyArray<string> {
    return [];
  }

  getComparisonOperators(): ReadonlyArray<string> {
    return [];
  }

  getStoryDataPassageName(): string | null {
    return null;
  }

  // ── Implicit passage references ────────────────────────────────────────────

  getImplicitPassagePatterns(): ReadonlyArray<ImplicitPassageRefPattern> {
    return [];
  }

  getPassageRefApiCalls(): ReadonlyArray<PassageRefApiCall> {
    return [];
  }
}
