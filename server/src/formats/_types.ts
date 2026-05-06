/**
 * Knot v2 — Format Module Standardized Types
 *
 * THE contract that every format module must fulfill.
 * Core and handlers ONLY know about these types.
 * Format modules implement FormatModule and export one object.
 *
 * DESIGN PRINCIPLES:
 *   - Enum-driven data interchange (MacroCategory, PassageType, LinkKind, etc.)
 *   - Capability bags for optional features (macros, variables, etc.)
 *   - Declarative data over imperative functions wherever possible
 *   - AST node types and token types come FROM formats
 *   - Core builds ASTs but is NOT responsible for symbol recognition
 *   - No dead code — absent capabilities are simply missing, not stubbed
 *
 * CRITICAL RULES:
 *   - No format-specific logic in core/handlers
 *   - All data flows through enums for isolation and modularity
 *   - Core asks, formats answer (never the reverse)
 *   - Dynamic import: core loads from /formats/{{detectedFormat}}/adapter
 */

import {
  MacroCategory,
  MacroKind,
  MacroBodyStyle,
  PassageKind,
  LinkKind,
  PassageRefKind,
} from '../hooks/hookTypes';

// ─── Source Range ─────────────────────────────────────────────────

export interface SourceRange {
  readonly start: number;
  readonly end: number;
}

// ─── Format-Provided AST Node Types ──────────────────────────────

/**
 * AST node type declaration from a format.
 * Core builds ASTs using these type IDs as labels.
 * Every format MUST include the baseline types (Document, PassageHeader,
 * PassageBody, Link, Text) plus any format-specific ones.
 */
export interface ASTNodeTypeDef {
  /** Unique type ID, e.g. 'MacroCall', 'Hook', 'VariableRef' */
  readonly id: string;
  /** Human-readable label for display */
  readonly label: string;
  /** Whether this node type can have children */
  readonly canHaveChildren: boolean;
  /** Which node type IDs can be direct children (null = any) */
  readonly childNodeTypeIds?: readonly string[];
}

/**
 * The full AST node type set from a format.
 * Includes the baseline types every format must provide,
 * plus format-specific types.
 */
export interface FormatASTNodeTypes {
  /** All node type definitions, keyed by ID */
  readonly types: ReadonlyMap<string, ASTNodeTypeDef>;
  /** Baseline: Document root */
  readonly Document: string;
  /** Baseline: :: Passage header */
  readonly PassageHeader: string;
  /** Baseline: Passage body content */
  readonly PassageBody: string;
  /** Baseline: [[link]] */
  readonly Link: string;
  /** Baseline: Plain text */
  readonly Text: string;
}

// ─── Token Types ─────────────────────────────────────────────────

/**
 * Token type declaration from a format.
 * Formats declare what token types their body lexer produces.
 * Core uses these to build correctly-typed AST nodes.
 */
export interface TokenTypeDef {
  /** Unique type ID, e.g. 'macro-call', 'hook-open', 'variable' */
  readonly id: string;
  /** Human-readable label */
  readonly label: string;
  /** Broad category for grouping */
  readonly category: 'delimiter' | 'identifier' | 'literal' | 'operator' | 'whitespace';
}

/**
 * A single token emitted by a format's body lexer.
 * Core handles :: headers and [[ ]] link boundaries.
 * Formats tokenize everything INSIDE passage bodies.
 */
export interface BodyToken {
  /** References a TokenTypeDef.id from this format */
  readonly typeId: string;
  /** Raw text of this token */
  readonly text: string;
  /** Character range in the passage body */
  readonly range: SourceRange;
  /** Parsed macro name (for macro-call / macro-close tokens) */
  readonly macroName?: string;
  /** Whether this is a closing token (e.g. <</if>>) */
  readonly isClosing?: boolean;
  /** Parsed variable name (for variable tokens) */
  readonly varName?: string;
  /** Variable sigil: '$' or '_' (for variable tokens) */
  readonly varSigil?: string;
}

// ─── Macro Definition ────────────────────────────────────────────

/**
 * A single macro definition from a format.
 * All properties are declarative — core derives what it needs.
 * Boolean flags replace parallel maps: instead of maintaining
 * separate navigationMacros / blockMacros / etc. maps,
 * core can derive Sets at format-load time from these flags.
 */
export interface MacroDef {
  /** Macro name WITHOUT delimiters, e.g. 'if', 'set', 'link-goto' */
  readonly name: string;
  /** Aliases / alternate names, e.g. 'loop' for 'for', 'v6m' for 'verbatim' */
  readonly aliases?: readonly string[];
  /** Which category this macro belongs to */
  readonly category: MacroCategory;
  /** Additional detail for Custom category */
  readonly categoryDetail?: string;
  /** Whether this is a changer, command, or instant */
  readonly kind: MacroKind;
  /** Human-readable description for hover/completion */
  readonly description: string;
  /** One or more valid call signatures */
  readonly signatures: readonly MacroSignatureDef[];
  /** Whether this macro is deprecated */
  readonly deprecated?: boolean;
  /** If deprecated, explanation and replacement */
  readonly deprecationMessage?: string;
  /** Valid child macro names (e.g. ['else', 'elseif'] for 'if') */
  readonly children?: readonly string[];
  /** Valid parent macro names (e.g. ['if'] for 'else') */
  readonly parents?: readonly string[];

  // ── Derived boolean flags ──────────────────────────────────────
  // Instead of 6 parallel maps, put flags on each macro definition.
  // Core derives Sets at format-load time for O(1) lookups.

  /** Whether this macro has a body (close-tag or hook style) */
  readonly hasBody?: boolean;
  /** Whether this macro navigates to another passage (e.g. goto, go-to) */
  readonly isNavigation?: boolean;
  /** Whether this macro includes/transcludes another passage (e.g. display, include) */
  readonly isInclude?: boolean;
  /** Whether this macro creates a conditional branch (e.g. if, unless) */
  readonly isConditional?: boolean;
  /** Whether this macro assigns to a variable (e.g. set, capture) */
  readonly isAssignment?: boolean;
  /** Which argument position holds a passage name (0-indexed) */
  readonly passageArgPosition?: number;
}

export interface MacroSignatureDef {
  readonly args: readonly MacroArgDef[];
  readonly returnType?: string;
  readonly description?: string;
}

export interface MacroArgDef {
  readonly name: string;
  readonly type: string;
  readonly required: boolean;
  readonly variadic?: boolean;
  readonly description?: string;
  /**
   * If this argument contains embedded source code in another language,
   * declare it here. This enables:
   *   - Semantic token highlighting for embedded JS/CSS/HTML within strings
   *   - Virtual document creation for string-embedded code regions
   *   - Diagnostics from the embedded language's analyzer
   *
   * Examples:
   *   - SugarCube <<run>> / <<print>>: 'javascript'
   *   - SugarCube <<style>> style string arg: 'css'
   *   - Any macro arg that appends HTML to the DOM: 'html'
   *
   * Omit if the argument is a plain string value (no embedded language).
   */
  readonly embeddedLanguage?: 'javascript' | 'css' | 'html';
}

// ─── Variable Sigil ──────────────────────────────────────────────

/**
 * Variable sigil definition from a format.
 * Declarative — no need for a describeSigil() function,
 * the description field covers it.
 */
export interface VariableSigilDef {
  /** The sigil character: '$', '_', etc. */
  readonly sigil: string;
  /** What scope this sigil represents */
  readonly kind: 'story' | 'temp';
  /** Human-readable description for hover */
  readonly description: string;
}

// ─── Special Passage ─────────────────────────────────────────────

/**
 * Declarative special passage definition from a format.
 * Core checks Twee 3 spec tags ([script], [stylesheet]) first,
 * then looks up the passage name in this list.
 *
 * This replaces classifyPassage(name, tags) → PassageKind | null.
 * Instead of calling a function per-passage, core does a single
 * O(1) name lookup in a Map derived from this list at format-load time.
 */
export interface SpecialPassageDef {
  /** The passage name that identifies this special passage */
  readonly name: string;
  /** What kind of passage this is */
  readonly kind: PassageKind;
  /** Human-readable description */
  readonly description: string;
  /** Analysis priority (lower = analyzed first; 0 = highest) */
  readonly priority?: number;
  /** The tag that identifies this passage type, if tag-based */
  readonly tag?: string;
  /** Format-specific type ID for Custom passage types (e.g. 'widget', 'header') */
  readonly typeId?: string;
}

// ─── Passage Reference ──────────────────────────────────────────

/**
 * A single passage reference extracted from a passage body.
 * The format's extractPassageRefs() is the SINGLE SOURCE OF TRUTH
 * for all passage references — [[ ]] links, macros, API calls,
 * and implicit references. Core NEVER extracts passage references
 * on its own.
 *
 * Every downstream consumer (graph, diagnostics, workspace index,
 * rename, definition, document links) uses this one type.
 */
export interface PassageRef {
  /** The passage name being referenced */
  readonly target: string;
  /** How this reference was produced */
  readonly kind: PassageRefKind;
  /** Character range in the passage body (relative to body start) */
  readonly range: SourceRange;
  /** What produced this reference, e.g. '[[ ]]', '<<goto>>', '(go-to:)', 'data-passage' */
  readonly source: string;
  /** For LinkKind classification (resolved from resolveLinkBody for [[ ]] links) */
  readonly linkKind?: LinkKind;
}

// ─── Link Resolution ─────────────────────────────────────────────

/**
 * Result of resolving the body text inside [[...]].
 * Core detects [[ and ]] boundaries; the format resolves the interior.
 */
export interface LinkResolution {
  readonly target: string;
  readonly displayText?: string;
  readonly kind: LinkKind;
  readonly setter?: string;
  /** For Custom link kinds, additional format-specific detail */
  readonly kindDetail?: string;
}

// ─── Macro Delimiters ────────────────────────────────────────────

/**
 * Declarative macro delimiter configuration.
 * Core uses these to know what characters delimit macro calls,
 * instead of calling getMacroCallPrefix()/getMacroCallSuffix() etc.
 */
export interface MacroDelimiters {
  /** Opening delimiter, e.g. '<<' for SugarCube, '(' for Harlowe */
  readonly open: string;
  /** Closing delimiter, e.g. '>>' for SugarCube, ')' for Harlowe */
  readonly close: string;
  /** Close-tag prefix, e.g. '/' for SugarCube's <</macro>>. Omit if not applicable. */
  readonly closeTagPrefix?: string;
}

// ─── Diagnostic Rule ─────────────────────────────────────────────

/**
 * Declarative diagnostic rule from a format.
 * Core's diagnostic engine ENFORCES these rules; formats don't
 * run their own diagnostic pass. This is "core asks, format answers."
 */
export interface DiagnosticRuleDef {
  /** Unique string ID for this rule (e.g. 'unknown-macro', 'invalid-hook') */
  readonly id: string;
  /** Human-readable description of what this rule checks */
  readonly description: string;
  /** Default severity when the rule fires */
  readonly defaultSeverity: 'error' | 'warning' | 'info' | 'hint';
  /** Scope of the rule — where it applies */
  readonly scope: 'passage' | 'document' | 'workspace';
}

export interface DiagnosticResult {
  /** The rule ID that triggered this diagnostic */
  readonly ruleId: string;
  /** Human-readable message */
  readonly message: string;
  /** Severity level */
  readonly severity: 'error' | 'warning' | 'info' | 'hint';
  /** Character range in the document */
  readonly range?: SourceRange;
}

// ─── Capability Bags ─────────────────────────────────────────────

/**
 * Macro capability — present for formats with macro syntax.
 * Absent for: Chapbook (inserts, not macros), Snowman (templates)
 */
export interface MacroCapability {
  /** The full builtin macro catalog */
  readonly builtins: readonly MacroDef[];
  /** Alias → canonical name mapping */
  readonly aliases: ReadonlyMap<string, string>;
}

/**
 * Variable capability — present for formats with sigiled variables.
 * Absent for: Chapbook (var.name), Snowman (s.name)
 */
export interface VariableCapability {
  /** Variable sigil definitions */
  readonly sigils: readonly VariableSigilDef[];
  /** Macro names that assign to variables, e.g. 'set', 'capture' */
  readonly assignmentMacros: ReadonlySet<string>;
  /** Assignment operators (e.g. 'to', '=' for SugarCube; 'to' for Harlowe) */
  readonly assignmentOperators: readonly string[];
  /** Comparison operators that trigger type-mismatch checking */
  readonly comparisonOperators: readonly string[];
  /** Regex for detecting variable references. Captures: [1]=sigil, [2]=name */
  readonly variablePattern: RegExp;
  /** Characters that trigger variable completion */
  readonly triggerChars: readonly string[];
}

/**
 * Custom macro capability — present for formats that support user-defined macros.
 * Absent for: Chapbook, Snowman
 */
export interface CustomMacroCapability {
  /** Macro names that define new macros (e.g. 'widget' for SugarCube) */
  readonly definitionMacros: ReadonlySet<string>;
  /** Patterns for detecting custom macro definitions in script passages */
  readonly scriptPatterns: readonly {
    readonly pattern: RegExp;
    readonly macroNameGroup: number;
    readonly description: string;
  }[];
  /** Whether calling a custom macro includes its body's links at the call site */
  readonly expandsBodyLinks: boolean;
}

export interface DiagnosticCapability {
  /** Format-specific diagnostic rules (declarative) */
  readonly rules: readonly DiagnosticRuleDef[];
  /**
   * Optional custom check for diagnostics that can't be expressed declaratively.
   * Use sparingly — prefer declarative rules.
   */
  readonly customCheck?: (context: DiagnosticCheckContext) => readonly DiagnosticResult[];
}

// ─── Snippet Template ────────────────────────────────────────────

/**
 * A snippet template for autocompletion.
 * Uses VS Code snippet syntax for tab stops and placeholders.
 */
export interface SnippetDef {
  /** Unique key for this snippet, e.g. 'if', 'for-range' */
  readonly key: string;
  /** Short prefix that triggers completion, e.g. 'if', 'for' */
  readonly prefix: string;
  /** Human-readable description shown in completion list */
  readonly description: string;
  /** Body using VS Code snippet syntax ($1, ${1:default}, $0, etc.) */
  readonly body: readonly string[];
  /** Category for grouping in the completion list */
  readonly category?: string;
}

// ─── Runtime Global ──────────────────────────────────────────────

/**
 * A runtime global variable/object exposed by the story format.
 * Used for completion, hover, and diagnostic support.
 */
export interface RuntimeGlobalDef {
  /** The global name, e.g. 'State', 'Engine', 'Config' */
  readonly name: string;
  /** Human-readable description for hover/completion */
  readonly description: string;
  /** Whether this is an object with members (true) or a simple value (false) */
  readonly hasMembers: boolean;
  /** If hasMembers, the known member names and their descriptions */
  readonly members?: readonly {
    readonly name: string;
    readonly description: string;
    readonly type?: string;
  }[];
}

export interface SnippetsCapability {
  /** Snippet templates for autocompletion */
  readonly templates: readonly SnippetDef[];
}

export interface RuntimeCapability {
  /** Runtime globals exposed by the format */
  readonly globals: readonly RuntimeGlobalDef[];
  /** JavaScript code that sets up a virtual runtime for analysis */
  readonly virtualPrelude?: string;
}

export interface DiagnosticCheckContext {
  readonly passageNames: ReadonlySet<string>;
  readonly formatId: string;
  readonly body: string;
  readonly bodyTokens: readonly BodyToken[];
}

// ─── THE FormatModule ────────────────────────────────────────────

/**
 * THE standardized export shape for every format module.
 *
 * Every format directory exports exactly one object conforming to this
 * interface. The shape is always the same, even if internal
 * implementations differ completely.
 *
 * Required fields = the non-negotiable contract.
 * Optional capability bags = present only when the format supports that feature.
 * Absent bags are simply missing — no stubs, no dead code.
 *
 * Usage:
 *   import { harloweModule } from './formats/harlowe/adapter';
 *   // or dynamic:
 *   const mod = await import(`./formats/${detectedFormat}/adapter`);
 *   const format: FormatModule = mod.default;
 */
export interface FormatModule {
  // ── Identity (required) ───────────────────────────────────────────
  readonly formatId: string;
  readonly displayName: string;
  readonly version: string;
  readonly aliases: readonly string[];

  // ── AST & Token declarations (required) ───────────────────────────
  // Core needs these to build AST nodes with correct type labels.
  // Formats declare what they produce; core uses the declarations.
  readonly astNodeTypes: FormatASTNodeTypes;
  readonly tokenTypes: readonly TokenTypeDef[];

  // ── Body lexing (required) ────────────────────────────────────────
  // Core splits on :: headers only. Format tokenizes everything
  // inside passage bodies (macros, hooks, variables, links, etc.)
  // baseOffset = character offset of the body start in the document.
  readonly lexBody: (input: string, baseOffset: number) => BodyToken[];

  // ── Passage reference extraction (required) ───────────────────────
  // THE single source of truth for "what passages does this body reference?"
  // Covers [[ ]] links, navigation macros, API calls, and implicit refs.
  // Core NEVER extracts passage references on its own.
  // Every downstream consumer (graph, diagnostics, rename, etc.) uses this.
  readonly extractPassageRefs: (body: string, bodyOffset: number) => PassageRef[];

  // ── Link resolution (required) ────────────────────────────────────
  // Resolves the body text inside [[...]] to determine target, display
  // text, and link kind. Used by extractPassageRefs for [[ ]] links
  // and by handlers that need link semantics (hover, completion).
  readonly resolveLinkBody: (rawBody: string) => LinkResolution;

  // ── Special passages (required, declarative) ──────────────────────
  // Core pre-classifies [script]→Script and [stylesheet]→Stylesheet
  // per the Twee 3 spec. This list adds format-specific special passages.
  // Empty array = format has no special passages beyond Twee 3 spec.
  readonly specialPassages: readonly SpecialPassageDef[];

  // ── Macro body style (required) ───────────────────────────────────
  readonly macroBodyStyle: MacroBodyStyle;

  // ── Macro delimiters (required) ───────────────────────────────────
  // Declarative — replaces getMacroCallPrefix/Suffix/ClosePrefix/Suffix
  readonly macroDelimiters: MacroDelimiters;

  // ── Macro pattern (required) ──────────────────────────────────────
  // Regex for detecting macro calls in body text.
  // Captures: [1] = macro name. Null if format has no macro syntax.
  readonly macroPattern: RegExp | null;

  // ── Capabilities (optional bags) ──────────────────────────────────
  // Present only when the format supports that feature.
  // Core checks bag presence ONCE at format-load time.

  /** Macro catalog, aliases, and derived data */
  readonly macros?: MacroCapability;
  /** Variable sigils, assignment, and comparison operators */
  readonly variables?: VariableCapability;
  /** Custom macro definitions (widget, macro:, etc.) */
  readonly customMacros?: CustomMacroCapability;
  /** Format-specific diagnostic rules */
  readonly diagnostics?: DiagnosticCapability;
  /** Snippet templates for autocompletion */
  readonly snippets?: SnippetsCapability;
  /** Runtime globals and virtual prelude */
  readonly runtime?: RuntimeCapability;
}

// ─── Core Diagnostic Rules (Twine Engine Level) ──────────────────

/**
 * Core diagnostic rules that run regardless of the active format.
 * These are NOT format-specific — they check Twine engine level issues.
 * Format-specific rules come from DiagnosticCapability.rules.
 */
export const CORE_DIAGNOSTIC_RULES: DiagnosticRuleDef[] = [
  {
    id: 'duplicate-passage',
    description: 'Two or more passages share the same name',
    defaultSeverity: 'error',
    scope: 'workspace',
  },
  {
    id: 'unknown-passage',
    description: 'A link targets a passage that does not exist',
    defaultSeverity: 'warning',
    scope: 'passage',
  },
  {
    id: 'unreachable-passage',
    description: 'A passage cannot be reached from the start passage',
    defaultSeverity: 'warning',
    scope: 'workspace',
  },
  {
    id: 'conditionally-reachable',
    description: 'A passage is only reachable via conditional links (may be unreachable depending on game state)',
    defaultSeverity: 'hint',
    scope: 'workspace',
  },
  {
    id: 'dead-if-branch',
    description: 'A conditional branch can never execute because the condition is always false given the known variable state',
    defaultSeverity: 'hint',
    scope: 'passage',
  },
  {
    id: 'dead-else-branch',
    description: 'An else branch can never execute because the condition is always true given the known variable state',
    defaultSeverity: 'hint',
    scope: 'passage',
  },
];
