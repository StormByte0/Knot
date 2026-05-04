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
// Builtin macro info returned by adapters.
//
// This is a self-contained interface so that types.ts doesn't need to import
// from format-specific modules. SugarCube's MacroDef extends this.
// ---------------------------------------------------------------------------

export interface BuiltinMacroInfo {
  name: string;
  description: string;
  hasBody: boolean;
  deprecated?: boolean;
  deprecationMessage?: string;
  container?: string;
  containerAnyOf?: string[];
  category?: string;
  args?: ReadonlyArray<{
    position: number;
    label: string;
    isPassageRef?: boolean;
    isSelector?: boolean;
    isVariable?: boolean;
    isRequired?: boolean;
    kind: 'expression' | 'string' | 'selector' | 'variable';
  }>;
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

  // ── Passage-arg macros ─────────────────────────────────────────────────────

  /** Names of macros whose arguments include a passage-name reference. */
  getPassageArgMacros(): ReadonlySet<string>;

  /** Given a macro name and argument count, return the index of the passage-name arg. */
  getPassageArgIndex(macroName: string, argCount: number): number;

  // ── Builtins ───────────────────────────────────────────────────────────────

  /** Builtin macro definitions for this format. */
  getBuiltinMacros(): ReadonlyArray<BuiltinMacroInfo>;

  /** Builtin global definitions for this format. */
  getBuiltinGlobals(): ReadonlyArray<{ name: string; description: string }>;

  // ── Special passages ───────────────────────────────────────────────────────

  /** Names of special/lifecycle passages. */
  getSpecialPassageNames(): ReadonlySet<string>;

  /** Whether a passage name indicates a special passage (lifecycle, system). */
  isSpecialPassage(name: string): boolean;

  /** Names of system passages that are always reachable (e.g. StoryData, Story JavaScript). */
  getSystemPassageNames(): ReadonlySet<string>;

  // ── Macro categories ───────────────────────────────────────────────────────

  /** Names of macros that assign story variables (e.g. 'set' in SugarCube). */
  getVariableAssignmentMacros(): ReadonlySet<string>;

  /** Names of macros that define reusable custom macros (e.g. 'widget' in SugarCube). */
  getMacroDefinitionMacros(): ReadonlySet<string>;

  /** Names of macros that contain inline script bodies (e.g. 'script' in SugarCube). */
  getInlineScriptMacros(): ReadonlySet<string>;

  // ── Analysis ordering ──────────────────────────────────────────────────────

  /** Priority for analysis ordering — lower runs first. Return 0 for highest priority. */
  getAnalysisPriority(passageName: string): number;

  // ── Structural constraints ─────────────────────────────────────────────────

  /** Map from macro name to the valid parent macro names it requires. */
  getMacroParentConstraints(): ReadonlyMap<string, ReadonlySet<string>>;

  // ── Virtual doc generation ────────────────────────────────────────────────

  /** Convert a story variable name (without sigil) to its JavaScript representation.
   *  SugarCube: State.variables.name */
  storyVarToJs(name: string): string;

  /** Convert a temp variable name (without sigil) to its JavaScript representation.
   *  SugarCube: temporary.name */
  tempVarToJs(name: string): string;

  /** Map of sugar operators to their JS equivalents for virtual doc generation. */
  getOperatorNormalization(): Readonly<Record<string, string>>;

  // ── Format hints (parser / lexer) ──────────────────────────────────────────

  /** Variable sigils used by this format. SugarCube: $ for story vars, _ for temp vars. */
  getVariableSigils(): ReadonlyArray<{ sigil: string; variableType: 'story' | 'temporary' }>;

  /** Resolve a sigil character to the variable type it represents, or null. */
  resolveVariableSigil(sigil: string): 'story' | 'temporary' | null;

  /** Operator precedence table for this format's sugar operators. Maps operator name to precedence number. */
  getOperatorPrecedence(): Readonly<Record<string, number>>;

  /** Names of passage-header tags that indicate a script passage. SugarCube: ['script'] */
  getScriptTags(): ReadonlyArray<string>;

  /** Names of passage-header tags that indicate a stylesheet passage. SugarCube: ['stylesheet', 'style'] */
  getStylesheetTags(): ReadonlyArray<string>;

  /** Whether the format uses a prefix for temporary/passage-scoped variables (e.g. '_' in SugarCube). */
  getTempVarPrefix(): string;

  /** Assignment operators for this format. SugarCube: ['to', '='] */
  getAssignmentOperators(): ReadonlyArray<string>;

  /** Comparison operators that should trigger type-mismatch warnings. SugarCube: ['gt', 'gte', 'lt', 'lte'] */
  getComparisonOperators(): ReadonlyArray<string>;

  /** Name of the special passage containing story metadata (JSON), or null if the format doesn't use one. SugarCube: 'StoryData' */
  getStoryDataPassageName(): string | null;

  // ── Implicit passage references ────────────────────────────────────────────

  /**
   * Return patterns that detect passage references in raw text (HTML attributes,
   * JavaScript API calls, etc.) that are not represented as Twine links or
   * macro passage-args.
   *
   * Each pattern MUST have exactly one capture group that extracts the passage
   * name. The `description` is used in diagnostics/hover to explain how the
   * reference was found.
   *
   * SugarCube examples:
   *   - data-passage="PassageName"  (HTML attribute)
   *   - Engine.play("PassageName") (JS API)
   *   - Engine.goto("PassageName") (JS API)
   *   - Story.get("PassageName")   (JS API)
   */
  getImplicitPassagePatterns(): ReadonlyArray<ImplicitPassageRefPattern>;

  /**
   * Return API call patterns that reference passages in expression trees.
   * Used to extract passage refs from parsed expressions like Engine.play("Name").
   *
   * Each entry describes an object+method pair where the first string argument
   * is a passage name reference.
   */
  getPassageRefApiCalls(): ReadonlyArray<PassageRefApiCall>;

  // ── Dynamic passage reference hints ─────────────────────────────────────

  /**
   * Return the names of macros that can dynamically navigate to a passage
   * when their passage-arg is a variable (not a string literal). The engine
   * will attempt to resolve the variable's known string values.
   *
   * This is a superset of getPassageArgMacros() — it should include any macro
   * whose body may cause navigation even without an explicit passage-arg, such
   * as macros that call Engine.play/goto internally.
   *
   * Example: In SugarCube, a <<widget "travel">> that contains <<goto $dest>>
   * means any call site <<travel>> could navigate to whatever $dest holds.
   */
  getDynamicNavigationMacros(): ReadonlySet<string>;
}

// ---------------------------------------------------------------------------
// Implicit passage reference pattern
// ---------------------------------------------------------------------------

export interface ImplicitPassageRefPattern {
  /** RegExp with exactly one capture group for the passage name. */
  pattern: RegExp;
  /** Human-readable description of what this pattern detects. */
  description: string;
}

/** Describes a JS API call pattern that references a passage by name. */
export interface PassageRefApiCall {
  /** The object name (e.g. 'Engine', 'Story'). */
  objectName: string;
  /** Method names on that object whose first string argument is a passage name. */
  methods: ReadonlyArray<string>;
}
