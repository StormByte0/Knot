# Knot v2 — FormatModule & Enum System

The FormatModule system is the **sole boundary** between format-agnostic core logic and format-specific data. It is the mechanism that ensures zero bleed-through.

---

## Design Principles

1. **Core asks, formats answer.** The core defines what it needs (`FormatModule` interface). Formats provide the answers (object literals implementing `FormatModule`).
2. **Enums, not strings.** Every category of data is represented by an enum defined in `hookTypes.ts`. No magic strings.
3. **Registry pattern.** Formats register at startup. Core resolves modules at runtime through `FormatRegistry`.
4. **One format active at a time per document.** A workspace may use one story format (or override per-project), but the registry supports discovery of all installed formats.
5. **Declarative over imperative.** Formats provide data declarations (`SpecialPassageDef[]`, `DiagnosticRuleDef[]`, `MacroDelimiters`, `MacroDef` boolean flags) rather than functions that core must call. Core derives what it needs at load time.
6. **Capability bags, not flat sub-providers.** Optional features are expressed as optional properties on `FormatModule`. Absent = unsupported. No stubs, no dead code.
7. **Object literals, not classes.** Every format exports a single `FormatModule` object literal. Capabilities are explicit. No hidden stub methods from class hierarchies.
8. **Single source of truth for passage references.** `extractPassageRefs()` is the ONLY way core learns about passage references. Core NEVER extracts passage references on its own.

---

## Enum Definitions (`hookTypes.ts`)

Enums remain the backbone of data interchange. Core and formats share these enum values to communicate without coupling.

### `MacroCategory`
Classifies what kind of macro a format provides. Used by core/handlers to categorize completions, hover results, and diagnostics without knowing the format. Slimmed to universal categories only; format-specific categories use `Custom` with a `categoryDetail` string.

```typescript
export enum MacroCategory {
  Navigation = 'navigation',   // Links, passage navigation
  Output     = 'output',       // Content rendering
  Control    = 'control',      // Flow control (if, for, while)
  Variable   = 'variable',     // Variable get/set
  Styling    = 'styling',      // CSS/style related
  System     = 'system',       // Save, load, settings
  Utility    = 'utility',      // General utility
  Custom     = 'custom',       // Format-specific extension point (use categoryDetail)
}
```

### `MacroKind`
How a macro relates to its body content. This is universal across formats:

- **Changer**: attaches to a body (SugarCube: `<<if>>...<</if>>`, Harlowe: `(if:)[...]`)
- **Command**: standalone action (SugarCube: `<<goto>>`, Harlowe: `(go-to:)`)
- **Instant**: silent side-effect (SugarCube: `<<set>>`, Harlowe: `(set:)`)

```typescript
export enum MacroKind {
  Changer = 'changer',
  Command = 'command',
  Instant = 'instant',
}
```

### `MacroBodyStyle`
How a format's macros delimit their body content. Determines how the AST builder assembles macro body nodes.

```typescript
export enum MacroBodyStyle {
  CloseTag = 'close-tag',   // <<macro>>body<</macro>> — SugarCube
  Hook     = 'hook',        // (macro:)[body] — Harlowe
  Inline   = 'inline',      // No macro bodies — Chapbook/Snowman/Fallback
}
```

### `PassageType`
Classifies what kind of passage it is at the Twine Engine level. Only universal types — format-specific passage types use `Custom` with a `typeId` string from `SpecialPassageDef`.

```typescript
export enum PassageType {
  Story     = 'story',       // Normal story passage — the default
  Stylesheet = 'stylesheet', // CSS passage (Twee 3 spec: [stylesheet] tag)
  Script    = 'script',      // JavaScript passage (Twee 3 spec: [script] tag)
  Start     = 'start',       // The starting passage
  StoryData = 'storydata',   // Story metadata passage (JSON)
  Custom    = 'custom',      // Format-specific passage type
}
```

### `PassageKind`
The fundamental kind of passage, used in `SpecialPassageDef`. Core recognizes `Script` and `Stylesheet` from Twee 3 spec tags; format-specific kinds use `Special`.

```typescript
export enum PassageKind {
  Markup    = 'markup',      // Normal story passage (default, safe fallback)
  Script    = 'script',      // JavaScript passage (Twee 3 spec: [script] tag)
  Stylesheet = 'stylesheet', // CSS passage (Twee 3 spec: [stylesheet] tag)
  Special   = 'special',     // Format-specific special passage (widget, header, init, etc.)
}
```

### `LinkKind`
Classifies link types for the link graph and reference index. Only universal types — format-specific link kinds use `Custom`.

```typescript
export enum LinkKind {
  Passage  = 'passage',   // Internal passage link
  External = 'external',  // External URL
  Custom   = 'custom',    // Format-specific link type
}
```

### `PassageRefKind`
How a passage is referenced from within a passage body. Core NEVER detects these — the format's `extractPassageRefs()` is the single source of truth for ALL passage references.

```typescript
export enum PassageRefKind {
  Link     = 'link',      // [[ ]] syntax (every format has this)
  Macro    = 'macro',     // Format macro (<<goto>>, (go-to:), etc.)
  API      = 'api',       // JavaScript API call (Engine.play(), story.show(), etc.)
  Implicit = 'implicit',  // Implicit reference (data-passage, {embed passage:}, etc.)
}
```

---

## FormatModule Interface (`formats/_types.ts`)

`FormatModule` is THE contract that every format module must fulfill. It replaces the old `IFormatProvider` + sub-provider pattern with a single object literal and optional capability bags.

### Complete Interface

```typescript
export interface FormatModule {
  // ── Identity (required) ───────────────────────────────────────────
  readonly formatId: string;          // e.g. "sugarcube-2", "harlowe-3"
  readonly displayName: string;       // Human-readable name
  readonly version: string;           // Format version string
  readonly aliases: readonly string[];// Alternate names for O(1) resolution

  // ── AST & Token declarations (required) ───────────────────────────
  readonly astNodeTypes: FormatASTNodeTypes;
  readonly tokenTypes: readonly TokenTypeDef[];

  // ── Body lexing (required) ────────────────────────────────────────
  readonly lexBody: (input: string, baseOffset: number) => BodyToken[];

  // ── Passage reference extraction (required) ───────────────────────
  // THE single source of truth for "what passages does this body reference?"
  readonly extractPassageRefs: (body: string, bodyOffset: number) => PassageRef[];

  // ── Link resolution (required) ────────────────────────────────────
  readonly resolveLinkBody: (rawBody: string) => LinkResolution;

  // ── Special passages (required, declarative) ──────────────────────
  readonly specialPassages: readonly SpecialPassageDef[];

  // ── Macro body style (required) ───────────────────────────────────
  readonly macroBodyStyle: MacroBodyStyle;

  // ── Macro delimiters (required, declarative) ──────────────────────
  readonly macroDelimiters: MacroDelimiters;

  // ── Macro pattern (required) ──────────────────────────────────────
  readonly macroPattern: RegExp | null;

  // ── Capabilities (optional bags) ──────────────────────────────────
  readonly macros?: MacroCapability;
  readonly variables?: VariableCapability;
  readonly customMacros?: CustomMacroCapability;
  readonly diagnostics?: DiagnosticCapability;
  readonly snippets?: SnippetsCapability;
  readonly runtime?: RuntimeCapability;
}
```

### Why Object Literals Instead of Classes

The old `IFormatProvider` was a class-based interface with required sub-providers. This led to:
- Hidden stub methods (every format had to implement `IDiagnosticProvider` even if it had no rules)
- Parallel maps that needed manual synchronization
- Imperative functions where declarative data sufficed

The new `FormatModule` object literal approach:
- Makes capabilities **explicit** — present or absent, no hiding
- Eliminates stub code — absent bags are simply missing
- Makes data **declarative** — `SpecialPassageDef[]`, `DiagnosticRuleDef[]`, `MacroDelimiters`, `MacroDef` flags
- Enables **O(1) lookups** — core derives Maps/Sets at load time from declarations

---

## Supporting Types (`formats/_types.ts`)

### SourceRange

```typescript
export interface SourceRange {
  readonly start: number;
  readonly end: number;
}
```

### AST Node Types

Formats declare their AST node types. Core builds ASTs using these type IDs as labels.

```typescript
export interface ASTNodeTypeDef {
  readonly id: string;               // e.g. 'MacroCall', 'Hook', 'VariableRef'
  readonly label: string;            // Human-readable label
  readonly canHaveChildren: boolean;
  readonly childNodeTypeIds?: readonly string[];  // null = any
}

export interface FormatASTNodeTypes {
  readonly types: ReadonlyMap<string, ASTNodeTypeDef>;
  readonly Document: string;         // Baseline
  readonly PassageHeader: string;    // Baseline
  readonly PassageBody: string;      // Baseline
  readonly Link: string;             // Baseline
  readonly Text: string;             // Baseline
}
```

Every format MUST include the baseline types (Document, PassageHeader, PassageBody, Link, Text) plus any format-specific ones. Core builds ASTs but is NOT responsible for symbol recognition — formats declare what they produce.

### Token Types & Body Tokens

Formats declare what token types their body lexer produces. `BodyToken.typeId` is a string referencing a `TokenTypeDef.id` — no shared enum needed.

```typescript
export interface TokenTypeDef {
  readonly id: string;          // e.g. 'macro-call', 'hook-open', 'variable'
  readonly label: string;
  readonly category: 'delimiter' | 'identifier' | 'literal' | 'operator' | 'whitespace';
}

export interface BodyToken {
  readonly typeId: string;      // References a TokenTypeDef.id
  readonly text: string;
  readonly range: SourceRange;  // Correct offsets via lexBody baseOffset
  readonly macroName?: string;
  readonly isClosing?: boolean;
  readonly varName?: string;
  readonly varSigil?: string;
}
```

**Why string typeIds instead of an enum?** New formats can define new token types without modifying a shared enum. The type declaration lives with the format; core reads it.

### Macro Definition

All properties are declarative. Boolean flags replace parallel maps.

```typescript
export interface MacroDef {
  readonly name: string;
  readonly aliases?: readonly string[];
  readonly category: MacroCategory;
  readonly categoryDetail?: string;    // For Custom category
  readonly kind: MacroKind;
  readonly description: string;
  readonly signatures: readonly MacroSignatureDef[];
  readonly deprecated?: boolean;
  readonly deprecationMessage?: string;
  readonly children?: readonly string[];
  readonly parents?: readonly string[];

  // Boolean flags — core derives Sets at load time
  readonly hasBody?: boolean;
  readonly isNavigation?: boolean;
  readonly isInclude?: boolean;
  readonly isConditional?: boolean;
  readonly isAssignment?: boolean;
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
   * If this argument contains embedded source code in another language.
   * Enables semantic token highlighting for embedded JS/CSS/HTML,
   * virtual document creation, and diagnostics from the embedded language.
   * Examples: SugarCube <<run>> → 'javascript', <<style>> → 'css'
   * Omit if the argument is a plain string value (no embedded language).
   */
  readonly embeddedLanguage?: 'javascript' | 'css' | 'html';
}
```

**Boolean flags vs. parallel maps:**

| Old Pattern (6 parallel maps) | New Pattern (flags on MacroDef) |
|---|---|
| `navigationMacros: Set<string>` | `MacroDef.isNavigation?: boolean` |
| `blockMacros: Set<string>` | `MacroDef.hasBody?: boolean` |
| `assignmentMacros: Set<string>` | `MacroDef.isAssignment?: boolean` |
| `conditionalMacros: Set<string>` | `MacroDef.isConditional?: boolean` |
| `includeMacros: Set<string>` | `MacroDef.isInclude?: boolean` |
| `passageArgMap: Map<string, number>` | `MacroDef.passageArgPosition?: number` |

Core derives the Sets/Maps at format-load time from the flags. No more keeping 6 maps in sync.

### Variable Sigils

```typescript
export interface VariableSigilDef {
  readonly sigil: string;      // '$', '_', etc.
  readonly kind: 'story' | 'temp';
  readonly description: string;
}
```

Declarative — no need for a `classifySigil()` function. Core builds a Map at load time.

### Special Passage Definitions

```typescript
export interface SpecialPassageDef {
  readonly name: string;        // Passage name that identifies this type
  readonly kind: PassageKind;
  readonly description: string;
  readonly priority?: number;   // Analysis priority (lower = first; 0 = highest)
  readonly tag?: string;        // Tag that identifies this type, if tag-based
  readonly typeId?: string;     // Format-specific type ID (e.g. 'init', 'header', 'footer', 'interface', 'widget')
}
```

**Replaces `classifyPassage(name, tags)`**: Instead of calling a function per-passage, core does a single O(1) name lookup in a Map derived from this list at format-load time. Core pre-classifies Twee 3 spec tags (`[script]`→Script, `[stylesheet]`→Stylesheet), then checks the format's `SpecialPassageDef[]` for format-specific passages.

The `typeId` field is critical for the story flow graph. The graph looks up special passages by `typeId` — not by passage name — because passage names differ across formats (e.g. SugarCube's `StoryInit` vs Harlowe's `Startup` both have `typeId: 'init'`). Known typeIds:

| typeId | Meaning | SugarCube | Harlowe |
|---|---|---|---|
| `'init'` | Runs once at engine startup | StoryInit | Startup |
| `'header'` | Runs before every passage body | PassageHeader | Header |
| `'footer'` | Runs after every passage body | PassageFooter | Footer |
| `'interface'` | Defines HTML structure at startup | StoryInterface | (none) |
| `'widget'` | Defines custom macros (tag-based) | (tag: widget) | (none) |

### Passage Reference

```typescript
export interface PassageRef {
  readonly target: string;       // The passage name being referenced
  readonly kind: PassageRefKind; // How this reference was produced
  readonly range: SourceRange;   // Character range in the passage body (relative to body start)
  readonly source: string;       // What produced this reference, e.g. '[[ ]]', '<<goto>>', '(go-to:)', 'data-passage'
  readonly linkKind?: LinkKind;  // For [[ ]] links, resolved from resolveLinkBody
}
```

**This is the single source of truth** for all passage references. Core NEVER extracts passage references on its own — no `[[` regex in core, no hardcoded macro names for navigation. Every downstream consumer (LinkGraph, ReferenceIndex, StoryFlowGraph, diagnostics, rename, definition, document links) uses this one type.

### Link Resolution

```typescript
export interface LinkResolution {
  readonly target: string;
  readonly displayText?: string;
  readonly kind: LinkKind;
  readonly setter?: string;
  readonly kindDetail?: string;  // For Custom link kinds
}
```

Core detects `[[` and `]]` boundaries; format resolves the interior via `resolveLinkBody()`.

### Macro Delimiters

```typescript
export interface MacroDelimiters {
  readonly open: string;             // e.g. '<<' for SugarCube, '(' for Harlowe
  readonly close: string;            // e.g. '>>' for SugarCube, ')' for Harlowe
  readonly closeTagPrefix?: string;  // e.g. '/' for SugarCube's <</macro>>
}
```

**Replaces four getter methods**: `getMacroCallPrefix()`, `getMacroCallSuffix()`, `getMacroClosePrefix()`, `getMacroCloseSuffix()` → one declarative object.

| Property | SugarCube | Harlowe | Fallback |
|---|---|---|---|
| `open` | `'<<'` | `'('` | `''` |
| `close` | `'>>'` | `')'` | `''` |
| `closeTagPrefix` | `'/'` | `undefined` | `undefined` |

### Diagnostic Rules

```typescript
export interface DiagnosticRuleDef {
  readonly id: string;                // e.g. 'unknown-macro', 'invalid-hook'
  readonly description: string;
  readonly defaultSeverity: 'error' | 'warning' | 'info' | 'hint';
  readonly scope: 'passage' | 'document' | 'workspace';
}

export interface DiagnosticResult {
  readonly ruleId: string;            // References DiagnosticRuleDef.id
  readonly message: string;
  readonly severity: 'error' | 'warning' | 'info' | 'hint';
  readonly range?: SourceRange;
}
```

**Replaces `IDiagnosticProvider`**: Instead of imperative `checkMacroUsage()`/`checkPassageStructure()` methods (that were stubs for most formats), formats provide `DiagnosticCapability.rules: DiagnosticRuleDef[]`. Core's diagnostic engine enforces the rules. A `customCheck?` escape hatch exists for complex rules.

---

## Capability Bags (`formats/_types.ts`)

Optional properties on `FormatModule`. Present only when the format supports that feature. Absent = no dead code.

### MacroCapability

```typescript
export interface MacroCapability {
  readonly builtins: readonly MacroDef[];
  readonly aliases: ReadonlyMap<string, string>;  // alias → canonical name
}
```

Present for: SugarCube, Harlowe. Absent for: Chapbook (inserts, not macros), Snowman (templates).

### VariableCapability

```typescript
export interface VariableCapability {
  readonly sigils: readonly VariableSigilDef[];
  readonly assignmentMacros: ReadonlySet<string>;
  readonly assignmentOperators: readonly string[];
  readonly comparisonOperators: readonly string[];
  readonly variablePattern: RegExp;
  readonly triggerChars: readonly string[];
}
```

Present for: SugarCube (`$/_`), Harlowe (`$`). Absent for: Chapbook (`var.name`), Snowman (`s.name`).

**Replaces `classifyVariableSigil()`**: Core builds a `Map<string, VariableSigilDef>` from `sigils` at load time for O(1) lookup. The `triggerChars` replace `getVariableTriggerChars()`. The `variablePattern` replaces `getVariablePattern()`.

| Property | SugarCube | Harlowe | Fallback |
|---|---|---|---|
| `sigils` | `[{sigil:'$', kind:'story'}, {sigil:'_', kind:'temp'}]` | `[{sigil:'$', kind:'story'}]` | `[]` |
| `triggerChars` | `['$', '_']` | `['$']` | `[]` |

### CustomMacroCapability

```typescript
export interface CustomMacroCapability {
  readonly definitionMacros: ReadonlySet<string>;  // e.g. 'widget' for SugarCube
  readonly scriptPatterns: readonly {
    readonly pattern: RegExp;
    readonly macroNameGroup: number;
    readonly description: string;
  }[];
  readonly expandsBodyLinks: boolean;
}
```

Present for: SugarCube (widget). Absent for: Chapbook, Snowman.

### DiagnosticCapability

```typescript
export interface DiagnosticCapability {
  readonly rules: readonly DiagnosticRuleDef[];
  readonly customCheck?: (context: DiagnosticCheckContext) => readonly DiagnosticResult[];
}

export interface DiagnosticCheckContext {
  readonly passageNames: ReadonlySet<string>;
  readonly formatId: string;
  readonly body: string;
  readonly bodyTokens: readonly BodyToken[];
}
```

Rules are **declarative** — core's diagnostic engine enforces them. `customCheck?` is an escape hatch for rules that can't be expressed declaratively. Use sparingly.

### SnippetsCapability

```typescript
export interface SnippetsCapability {
  readonly templates: readonly SnippetDef[];
}

export interface SnippetDef {
  readonly key: string;           // Unique key, e.g. 'if', 'for-range'
  readonly prefix: string;       // Trigger prefix, e.g. 'if', 'for'
  readonly description: string;  // Shown in completion list
  readonly body: readonly string[]; // VS Code snippet syntax ($1, ${1:default}, $0, etc.)
  readonly category?: string;    // Grouping category
}
```

Present for: SugarCube, Harlowe, Chapbook. Absent for: Snowman, Fallback.

### RuntimeCapability

```typescript
export interface RuntimeCapability {
  readonly globals: readonly RuntimeGlobalDef[];
  readonly virtualPrelude?: string;
}

export interface RuntimeGlobalDef {
  readonly name: string;          // e.g. 'State', 'Engine', 'Config'
  readonly description: string;
  readonly hasMembers: boolean;
  readonly members?: readonly {
    readonly name: string;
    readonly description: string;
    readonly type?: string;
  }[];
}
```

Present for: SugarCube (State, Engine, Config, Dialog, etc.), Harlowe, Chapbook, Snowman. Absent for: Fallback.

The `virtualPrelude` is JavaScript code that sets up a virtual runtime environment for analysis. This enables the semantic analyzer to understand runtime globals without actually executing format code.

### Core Diagnostic Rules

Core provides `CORE_DIAGNOSTIC_RULES` that run regardless of the active format. These check Twine Engine level issues:

```typescript
export const CORE_DIAGNOSTIC_RULES: DiagnosticRuleDef[] = [
  { id: 'duplicate-passage', ... },
  { id: 'unknown-passage', ... },
  { id: 'unreachable-passage', ... },
  { id: 'conditionally-reachable', ... },
  { id: 'dead-if-branch', ... },
  { id: 'dead-else-branch', ... },
];
```

| Rule ID | Description | Severity | Scope |
|---|---|---|---|
| `duplicate-passage` | Two or more passages share the same name | error | workspace |
| `unknown-passage` | A link targets a passage that does not exist | warning | passage |
| `unreachable-passage` | A passage cannot be reached from the start passage | warning | workspace |
| `conditionally-reachable` | A passage is only reachable via conditional links | hint | workspace |
| `dead-if-branch` | A conditional branch can never execute (condition always false) | hint | passage |
| `dead-else-branch` | An else branch can never execute (condition always true) | hint | passage |

Format-specific rules come from `DiagnosticCapability.rules`. Both use the same `DiagnosticRuleDef` structure with string `id`.

---

## FormatRegistry (`formats/formatRegistry.ts`)

The registry is the runtime mechanism. FormatModules register at startup; core resolves them at runtime. Replaces the old `HookRegistry`.

```typescript
export class FormatRegistry {
  private modules: Map<string, FormatModule>;
  private aliasMap: Map<string, string>;       // alias → canonical formatId
  private loaders: Map<string, () => FormatModule>;  // lazy loaders

  /** Register a format module */
  register(module: FormatModule): void;

  /** Unregister a format by ID */
  unregister(formatId: string): void;

  /** Get a format module by ID or alias (O(1) resolution) */
  getModule(formatId?: string): FormatModule | undefined;

  /** Get the currently active format module */
  getActiveFormat(): FormatModule | undefined;

  /** Set the active format by ID or alias */
  setActiveFormat(formatId: string): void;

  /** Get all available format IDs */
  getAvailableFormats(): string[];

  /** Check if a format is registered */
  hasModule(formatId?: string): boolean;

  /** Auto-detect format from StoryData passage JSON */
  detectFromStoryData(data: unknown): FormatModule | undefined;
}
```

### Key Differences from HookRegistry

| Feature | HookRegistry (old) | FormatRegistry (new) |
|---|---|---|
| Registry value | `IFormatProvider` (class instance) | `FormatModule` (object literal) |
| Resolution | By formatId only | By formatId OR alias (O(1)) |
| Detection | Manual | `detectFromStoryData()` auto-detect |
| Loading | Eager (all formats loaded at startup) | Lazy (BUILTIN_LOADERS with lazy loaders) |
| Sub-provider access | `provider.getMacroProvider()` etc. | `module.macros` etc. (direct property) |
| Capability check | `provider.capabilities.has(X)` per-request | Check bag presence once at load time |
| Future extensibility | N/A | Async imports for external formats |

### BUILTIN_LOADERS (Lazy Loading)

```typescript
const BUILTIN_LOADERS: Map<string, () => FormatModule> = new Map([
  ['sugarcube-2', () => sugarcubeModule],
  ['harlowe-3',   () => harloweModule],
  ['chapbook-2',  () => chapbookModule],
  ['snowman-3',   () => snowmanModule],
  ['fallback',    () => fallbackModule],
]);
```

Lazy loaders ensure format code is only loaded when needed. Future: async imports for external format packages.

---

## Data Flow Example: Completion

1. User types `<<` in a `.tw` file
2. `handlers/completions.ts` receives the LSP completion request
3. Handler calls `FormatRegistry.getActiveFormat()` → gets `FormatModule`
4. Handler checks `module.macros` — if present, uses `builtins` for macro completion
5. Handler checks `module.macroDelimiters` for trigger character matching
6. Handler filters by `MacroCategory`, builds `CompletionItem[]`
7. On resolve, handler enriches the item with `macro.signatures`, `macro.description`, format-specific delimiters
8. Returns completion list — **handler never knew which format was active**

---

## Data Flow Example: Diagnostics

1. Document changes, `core/diagnosticEngine.ts` runs
2. Engine runs `CORE_DIAGNOSTIC_RULES` (format-agnostic: duplicate-passage, unknown-passage, unreachable-passage, conditionally-reachable, dead-if-branch, dead-else-branch)
3. Engine checks `module.diagnostics` — if present, iterates `DiagnosticCapability.rules`
4. Engine enforces each `DiagnosticRuleDef` declaratively (no calling format methods)
5. If `DiagnosticCapability.customCheck` is defined, engine calls it as an escape hatch
6. Engine merges format-specific results with core results
7. Publishes diagnostics — **engine never knew which format was active**

---

## Data Flow Example: Passage Reference Extraction

1. Parser extracts a passage body after a `::` header
2. Core calls `format.extractPassageRefs(body, bodyOffset)` — THE single source of truth
3. Format returns `PassageRef[]`, each with `target`, `kind` (PassageRefKind), `range`, `source`
4. `PassageRefKind.Link` — `[[ ]]` links (the format resolves the interior via `resolveLinkBody`)
5. `PassageRefKind.Macro` — navigation macros (`<<goto>>`, `(go-to:)`)
6. `PassageRefKind.API` — JavaScript API calls (`Engine.play()`)
7. `PassageRefKind.Implicit` — implicit references (`data-passage`, `{embed passage:}`)
8. Every downstream consumer uses the same `PassageRef[]` array — **no duplicate extraction logic**

---

## Data Flow Example: Story Flow Graph with Special Passages

1. `StoryFlowGraphBuilder.buildAndAnalyze()` is called with all passages
2. For each passage, `CFGBuilder.buildPassageCFG()` builds a per-passage control flow graph
3. `buildStoryFlowEdges()` creates inter-passage navigation edges from `PassageRef[]`
4. `addSpecialPassageVirtualEdges()` looks up `format.specialPassages` by `typeId`:
   - `typeId: 'init'` → virtual edge from init passage to Start
   - `typeId: 'interface'` → virtual edge from interface passage to Start (StoryInterface IS a special passage)
   - `typeId: 'header'` → virtual edges from header passage to every story passage
5. `computeInitVariableState()` propagates through init and header CFGs to seed Start's variable state
6. BFS reachability with variable state flow, conditional reachability detection
7. Dead condition detection using known variable states
8. **Graph never knew which format was active** — it used `typeId` lookups, not format names

---

## Data Flow Example: Passage Classification

1. Parser extracts a passage name and tags from a `::` header
2. Core checks Twee 3 spec tags first: `[script]` → Script, `[stylesheet]` → Stylesheet
3. Core looks up the passage name in the `SpecialPassageDef` map (built from `module.specialPassages` at load time)
4. If found, uses the `SpecialPassageDef.kind` and optional `typeId`
5. If not found, it's a regular `PassageType.Story`
6. **No per-passage function call** — O(1) Map lookup from declarative data

---

## Data Flow Example: Body Lexing

1. Core parses `::` headers and identifies passage boundaries
2. Core detects `[[ ]]` link boundaries
3. For the passage body, core calls `module.lexBody(bodyText, baseOffset)`
4. `baseOffset` = character offset of the body start in the document
5. Format returns `BodyToken[]` with correct `SourceRange` values (offset by `baseOffset`)
6. Each `BodyToken.typeId` references a `TokenTypeDef.id` from `module.tokenTypes`
7. Core uses these tokens to build AST nodes using `module.astNodeTypes`
8. **Core never interprets format-specific syntax** — it only reads the tokens the format produces

---

## Data Flow Example: Semantic Tokens

1. VS Code requests semantic tokens for a `.twee` document
2. `handlers/semanticTokens.ts` receives the request
3. Handler tokenizes passage headers using `::` regex (Twine engine level)
4. Handler calls `format.lexBody()` for each passage body
5. Handler maps `BodyToken.typeId` to LSP semantic token types:
   - `'macro-call'`/`'macro-close'` → function (only known macros highlighted)
   - `'variable'` → variable
   - `'hook-open'`/`'hook-close'` → operator
6. Builds `SemanticTokens` with delta encoding
7. **Handler never knew which format was active** — it used format-declared token type IDs

---

## Deprecated: hooks/hookRegistry.ts and hooks/formatHooks.ts

These files still exist for backward compatibility during migration:

- **`hooks/hookRegistry.ts`** — re-exports `FormatRegistry` from `formats/formatRegistry.ts`
- **`hooks/formatHooks.ts`** — re-exports types from `formats/_types.ts`

New code should import directly from `formats/formatRegistry` and `formats/_types`.

---

## Adding a New Format

To add a new story format (e.g., ImaginaryFormat):

1. Create `server/src/formats/imaginary/` directory with:
   - `index.ts` — exports `imaginaryModule: FormatModule`
   - `lexer.ts` — body lexer implementing `lexBody()`
   - Other files as needed (snippets, runtime, macros split by category, etc.)
2. Export a `imaginaryModule: FormatModule` object literal with:
   - Required fields: `formatId`, `displayName`, `version`, `aliases`, `astNodeTypes`, `tokenTypes`, `lexBody`, `extractPassageRefs`, `resolveLinkBody`, `specialPassages`, `macroBodyStyle`, `macroDelimiters`, `macroPattern`
   - Optional capability bags: only the ones the format supports
3. Add a lazy loader to `BUILTIN_LOADERS` in `formatRegistry.ts`
4. **That's it.** No changes to core, handlers, or client.

The new format is instantly available with zero core modifications. Absent capability bags mean core automatically knows which features to skip.

---

## Migration Summary: Old → New

| Old Pattern | New Pattern | Why |
|---|---|---|
| `IFormatProvider` (class) | `FormatModule` (object literal) | Explicit capabilities, no hidden stubs |
| `IMacroProvider` interface | `macros?: MacroCapability` bag | Optional — absent = no macros |
| `IPassageProvider` interface | `specialPassages: SpecialPassageDef[]` | Declarative, O(1) lookup |
| `IDiagnosticProvider` interface | `diagnostics?: DiagnosticCapability` bag | Declarative rules, optional |
| `ILinkProvider` interface | `resolveLinkBody()` + `extractPassageRefs()` | Direct functions on FormatModule |
| `ISyntaxProvider` interface | Declarative properties + `lexBody()` | MacroDelimiters, VariableCapability, etc. |
| `HookRegistry` | `FormatRegistry` | Lazy loading, O(1) alias resolution, auto-detect |
| `hookRegistry.register(id, provider)` | `FormatRegistry.register(module)` | Module carries its own identity |
| `hookRegistry.getProvider()` | `FormatRegistry.getModule()` | Returns FormatModule directly |
| `classifyPassage(name, tags)` | `SpecialPassageDef[]` + Map lookup | Declarative, O(1), no per-passage call |
| `getMacroCallPrefix/Suffix/ClosePrefix/Suffix()` | `macroDelimiters: MacroDelimiters` | One declarative object |
| `classifyVariableSigil(sigil)` | `VariableCapability.sigils` + Map | Declarative, O(1) lookup |
| `AdapterTokenType` enum | `BodyToken.typeId: string` | Extensible without shared enum |
| 6 parallel macro maps | `MacroDef` boolean flags | Core derives Sets at load time |
| `DiagnosticRule` enum | `DiagnosticRuleDef` with string `id` | Extensible, format-defined rules |
| `FormatDiagnosticRule` type | `DiagnosticRuleDef` | Unified rule definition format |
| Core extracts passage refs | `extractPassageRefs() → PassageRef[]` | Single source of truth, no format bleed |
| `FormatCapability` enum | Capability bag presence check | Simpler — absent bag = unsupported |
| `adapter.ts` entry point | `index.ts` with modular files | Better organization, split by category |
| `MacroKind.Container`/`SelfClose` | `MacroKind.Changer`/`Command`/`Instant` | Cleaner, format-agnostic classification |
| `PassageRefKind` didn't exist | `PassageRefKind` (Link/Macro/API/Implicit) | Required for single source of truth |
| `NavigationCapability` bag | `extractPassageRefs()` method | Method replaces separate capability bag |
