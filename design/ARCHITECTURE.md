# Knot v2 — Architecture

## Philosophy: Promise-Driven, Format-Agnostic

Knot v2 is built on a strict layered architecture where each layer **promises** (declares) what it provides and **imports** what it expects from the layer below. No layer may reach across boundaries. The single most important rule:

> **There is ZERO bleed-through from story formats to core infrastructure or handlers.**
> All format-specific data flows through enums, `FormatModule` capability bags, and declarative type definitions — never through direct imports or hardcoded references.

---

## The Twine Engine Layer

A critical distinction in Knot v2's architecture: **the Twine Engine is the universal base that ALL story formats sit on top of**. Core code models ONLY the Twine Engine. Format-specific behavior goes through `FormatModule`.

### What is Twine Engine vs Story Format

| Concept | Description |
|---|---|
| **Twine Engine** | The universal base layer shared by every story format. This is what the core parser, lexer, and index know about directly. |
| **Story Format** | A format (SugarCube, Harlowe, Chapbook, Snowman) that adds its own syntax, macros, variables, and semantics ON TOP of the Twine Engine base. |

Core code models ONLY the Twine Engine. When core encounters anything beyond the engine's universal features, it delegates to the active format module through the `FormatRegistry`.

### Twine Engine Features (what core knows about)

These are the ONLY syntax structures that core code parses directly, without asking a format module:

| Feature | Description |
|---|---|
| `::` passage headers | Passage name, optional `[tags]`, and body separator |
| `[[link]]` boundary detection | Core detects `[[` and `]]` delimiters — content interpretation is the format module's job |
| `[script]` tag | Twee 3 spec: passage contains JavaScript |
| `[stylesheet]` tag | Twee 3 spec: passage contains CSS |
| StoryData passage | JSON metadata passage used for format auto-detection |
| Start passage detection | Identifies the entry-point passage |
| Passage structure | Name, tags, body — the universal container |

Core can parse a `.twee` file and identify every passage, its header, tags, and body text without knowing which story format is active. That's the Twine Engine layer.

### Format-Specific Features (what core NEVER knows about)

These are entirely the domain of format modules. Core code MUST NOT contain any regex, constant, or logic that handles these:

| Feature | Why It's Format-Specific |
|---|---|
| Macro syntax and delimiters | `<<>>` (SugarCube) vs `(:)` (Harlowe) vs `<% %>` (Snowman) |
| Variable sigils and scoping | `$/_` (SugarCube) vs `$` only (Harlowe) — scope rules differ |
| Hook syntax | `[...]` with nametags is Harlowe-only |
| Link body interpretation | `|` vs `->` vs `<-` pipe/arrow syntax varies by format |
| Special passage types beyond Twee 3 | Widget passages, init passages, StoryInterface, etc. are format-specific |
| Macro kind (changer/command/instant) | Macro call structure classification; SugarCube uses changer style, Harlowe uses changer/command/instant |
| Template blocks | `{insert}` (Chapbook) or `<% %>` (Snowman) — format-specific |
| Passage reference extraction | Whether a passage is referenced via `<<goto>>`, `(go-to:)`, `Engine.play()`, `data-passage`, etc. is entirely format-specific |

### Data Flow Through FormatModule

Core never interprets format-specific syntax on its own. Instead, it follows a consistent **detect → delegate** pattern:

```
Core detects :: header  →  Parser checks SpecialPassageDef[] for O(1) name/tag lookup
Core detects [[...]]    →  Format's extractPassageRefs() resolves the link interior
Core extracts body text →  Parser calls format.lexBody(body, baseOffset) for tokenization
Core sees a sigil       →  Parser checks VariableCapability.sigils for scope classification
Core needs completion   →  Parser checks macroDelimiters + triggerChars for completion triggers
Core runs diagnostics   →  DiagnosticEngine enforces DiagnosticCapability.rules declaratively
Core builds link graph  →  Format's extractPassageRefs() is THE single source of truth for passage references
Core builds flow graph  →  StoryFlowGraph uses format.specialPassages for virtual edges and init variable state
```

Every one of these questions is answered through the `FormatModule` interface and its optional capability bags. Core NEVER makes assumptions about delimiters, sigils, or syntax — even something as seemingly universal as `$` for variables is determined by the format's `VariableCapability.sigils` declarations.

---

## Layered Design

```
┌─────────────────────────────────────────────────┐
│  Layer 1: package.json                          │
│  PROMISES: commands, settings, languages,       │
│  grammars, activation events                    │
│  IMPORTS: nothing (this is the declaration)     │
└──────────────────────┬──────────────────────────┘
                       │ promises
                       ▼
┌─────────────────────────────────────────────────┐
│  Layer 2: Client + Server entry                 │
│  PROMISES: extension activation, LSP features,  │
│  GUI, status bar, command handlers              │
│  IMPORTS: from core/, shared/                   │
└──────────────────────┬──────────────────────────┘
                       │ promises
                       ▼
┌─────────────────────────────────────────────────┐
│  Layer 3: Core (workspace index, handlers,      │
│  parser, CFG, flow graph, AST, diagnostics)     │
│  PROMISES: format-agnostic LSP operations       │
│  IMPORTS: from formats/_types (FormatModule,    │
│           capability bags) + hooks/hookTypes    │
│  NEVER IMPORTS: from format directories         │
└──────────────────────┬──────────────────────────┘
                       │ FormatModule + enums
                       ▼
┌─────────────────────────────────────────────────┐
│  Layer 4: Formats (sugarcube/, harlowe/,        │
│  chapbook/, snowman/, fallback/)                │
│  PROMISES: format-specific data via FormatModule│
│  IMPORTS: from formats/_types + hooks/hookTypes │
│  NEVER IMPORTS: from core/ or handlers/         │
└─────────────────────────────────────────────────┘
```

---

## Data Flow: Enums and FormatModule Capability Bags

The core never knows about specific formats. Instead:

1. **`hooks/hookTypes.ts`** defines **enums** that represent categories of format-provided data:
   - `MacroCategory` (8 values) — what kind of macro (navigation, output, control, variable, styling, system, utility, custom)
   - `MacroKind` (3 values) — what kind of macro body (changer, command, instant)
   - `MacroBodyStyle` (3 values) — how macro bodies are delimited (close-tag, hook, inline)
   - `PassageType` (6 values) — what kind of passage (story, stylesheet, script, start, storydata, custom)
   - `PassageKind` (4 values) — what kind of special passage (markup, script, stylesheet, special)
   - `LinkKind` (3 values) — what kind of link (passage, external, custom)
   - `PassageRefKind` (4 values) — how a passage is referenced (link, macro, api, implicit)

2. **`formats/_types.ts`** defines **the FormatModule contract** — the single interface every format module must implement:
   - `FormatModule` — the top-level object literal every format exports (replaces `IFormatProvider`)
   - Optional capability bags: `macros?`, `variables?`, `customMacros?`, `diagnostics?`, `snippets?`, `runtime?`
   - Absent bags = no dead code — core checks bag presence once at format-load time
   - Declarative data: `SpecialPassageDef[]`, `MacroDelimiters`, `DiagnosticRuleDef[]`, `TokenTypeDef[]`, `ASTNodeTypeDef[]`
   - Body lexing: `lexBody(input, baseOffset)` with `baseOffset` for correct SourceRange values
   - Passage reference extraction: `extractPassageRefs(body, bodyOffset) → PassageRef[]` — THE single source of truth

3. **`formats/formatRegistry.ts`** is the **FormatRegistry** where formats are registered and resolved (replaces `HookRegistry`):
   - Core calls `FormatRegistry.getModule(formatId)` to get a `FormatModule`
   - Core calls `FormatRegistry.detectFromStoryData(data)` for auto-detection
   - FormatRegistry provides lazy loading via `BUILTIN_LOADERS` and O(1) alias resolution
   - Core never imports from `formats/sugarcube/` or any format directory — it only knows about `FormatModule`

4. **Format modules export one object literal** conforming to `FormatModule`. No classes, no hidden stub methods. Capabilities are explicit — present or absent.

---

## Key Architectural Concepts

### FormatModule Replaces IFormatProvider

Every format exports a single `FormatModule` object literal instead of a class implementing `IFormatProvider`. Object literals make capabilities explicit; no hidden stub methods.

**Old pattern (deprecated):**
```typescript
class SugarCubeAdapter implements IFormatProvider {
  getMacroProvider(): IMacroProvider { ... }
  getPassageProvider(): IPassageProvider { ... }
  getDiagnosticProvider(): IDiagnosticProvider { ... }  // stubs for most formats
  getLinkProvider(): ILinkProvider { ... }
  getSyntaxProvider(): ISyntaxProvider { ... }
}
```

**New pattern:**
```typescript
export const sugarcubeModule: FormatModule = {
  formatId: 'sugarcube-2',
  displayName: 'SugarCube 2',
  version: '2.37.0',
  aliases: ['sugarcube', 'sc2'],
  astNodeTypes: { ... },
  tokenTypes: [ ... ],
  lexBody: (input, baseOffset) => [ ... ],
  extractPassageRefs: (body, bodyOffset) => [ ... ],
  resolveLinkBody: (rawBody) => { ... },
  specialPassages: [ ... ],
  macroBodyStyle: MacroBodyStyle.CloseTag,
  macroDelimiters: { open: '<<', close: '>>', closeTagPrefix: '/' },
  macroPattern: /<<([\w-]+)/,
  // Optional capability bags — present only when the format supports them:
  macros: { builtins: [...], aliases: new Map([...]) },
  variables: { sigils: [...], assignmentMacros: new Set([...]), ... },
  customMacros: { definitionMacros: new Set(['widget']), ... },
  diagnostics: { rules: [...], customCheck: ... },
  snippets: { templates: [...] },
  runtime: { globals: [...], virtualPrelude: '...' },
};
```

### Capability Bags Replace Flat Sub-Providers

Instead of required `IMacroProvider`, `IPassageProvider`, `IDiagnosticProvider` interfaces that every format must implement (even to stub), `FormatModule` has optional capability bags:

| Capability Bag | Present When | Absent Means |
|---|---|---|
| `macros?` | Format has macro syntax (SugarCube, Harlowe) | Format doesn't use macros (Chapbook uses inserts, Snowman uses templates) |
| `variables?` | Format has sigiled variables (`$/_`) | Format uses different variable syntax (Chapbook `var.name`, Snowman `s.name`) |
| `customMacros?` | Format supports user-defined macros (widget, macro:) | Format doesn't support custom macros |
| `diagnostics?` | Format has format-specific validation rules | No format-specific diagnostics beyond core rules |
| `snippets?` | Format provides snippet templates for autocompletion | No format-specific snippets |
| `runtime?` | Format exposes runtime globals (State, Engine, Config) | No runtime globals for completion/hover |

Core checks bag presence **once at format-load time** and caches the result. No per-request `if (provider.getDiagnosticProvider())` checks.

### extractPassageRefs() — Single Source of Truth

The `extractPassageRefs(body, bodyOffset) → PassageRef[]` method is THE single source of truth for ALL passage references. This replaces the old pattern where core tried to extract passage references on its own. Every downstream consumer uses this one method:

```
extractPassageRefs() → PassageRef[]
  ├── LinkGraph (BFS reachability, orphan detection)
  ├── ReferenceIndex (cross-reference tracking)
  ├── StoryFlowGraph (navigation edges, variable state flow)
  ├── DiagnosticEngine (unknown-passage, unreachable-passage)
  ├── RenameHandler (rename all references to a passage)
  ├── DefinitionHandler (go-to-definition for passage targets)
  └── DocumentLinksHandler (clickable passage links)
```

Every `PassageRef` carries a `PassageRefKind` telling core HOW the passage was referenced:
- `Link` — `[[ ]]` syntax (universal across all formats)
- `Macro` — format macro (`<<goto>>`, `(go-to:)`, etc.)
- `API` — JavaScript API call (`Engine.play()`, `story.show()`, etc.)
- `Implicit` — implicit reference (`data-passage`, `{embed passage:}`, etc.)

This means core NEVER interprets format-specific syntax to find passage references. The format module does it all.

### Declarative Special Passages Replace classifyPassage()

Instead of a `classifyPassage(name, tags)` function called per-passage, formats provide a `SpecialPassageDef[]` array. Core builds a `Map<string, SpecialPassageDef>` at load time for O(1) name/tag lookups.

```typescript
// Old: imperative, per-passage function call
const kind = passageProvider.classifyPassage(name, tags);

// New: declarative, O(1) map lookup
const specialMap = buildSpecialPassageMap(module.specialPassages);
const kind = specialMap.get(name)?.kind;
```

Special passages include format-specific entries like SugarCube's `StoryInit`, `PassageHeader`, `PassageFooter`, `StoryInterface`, and Harlowe's `Startup`, `Header`, `Footer`. The `StoryInterface` passage IS a special passage — it goes into the story flow graph as a virtual edge because it runs at engine startup before any passage renders.

### Story Flow Graph Uses Special Passages

The `StoryFlowGraph` adds virtual edges for special passages that run automatically by the engine:

- **Init passage** (`StoryInit`/`Startup`) → virtual edge to Start passage
- **Interface passage** (`StoryInterface`) → virtual edge to Start passage (runs at engine startup)
- **Header passage** (`PassageHeader`/`Header`) → virtual edge to every story passage

These virtual edges use `PassageRefKind.Implicit` because they represent engine behavior, not authored links. The graph also computes init variable state by propagating through StoryInit and PassageHeader CFGs before seeding the Start passage.

### Declarative Diagnostic Rules Replace IDiagnosticProvider

Instead of `IDiagnosticProvider` with imperative `checkMacroUsage()`/`checkPassageStructure()` methods (that were stubs for most formats anyway), formats provide `DiagnosticCapability.rules: DiagnosticRuleDef[]`. Core's diagnostic engine enforces the rules. A `customCheck?` escape hatch exists for complex rules that can't be expressed declaratively.

### MacroDelimiters Replaces Getter Methods

Instead of four separate getter methods (`getMacroCallPrefix`, `getMacroCallSuffix`, `getMacroClosePrefix`, `getMacroCloseSuffix`), formats provide a single declarative object:

```typescript
// Old: four imperative methods
getMacroCallPrefix(): string;     // '<<'
getMacroCallSuffix(): string;     // '>>'
getMacroClosePrefix(): string;    // '<</'
getMacroCloseSuffix(): string;    // '>>'

// New: one declarative object
macroDelimiters: { open: '<<', close: '>>', closeTagPrefix: '/' }
```

### MacroDef Boolean Flags Replace Parallel Maps

Instead of maintaining 6 parallel maps (`navigationMacros`, `blockMacros`, `assignmentMacros`, etc.), each `MacroDef` has boolean flags: `isNavigation`, `isInclude`, `isConditional`, `isAssignment`, `passageArgPosition`, `hasBody`. Core derives Sets at load time for O(1) lookups. No more keeping parallel maps in sync.

### BodyToken String TypeIds Replace AdapterTokenType Enum

Instead of `AdapterTokenType` enum, `BodyToken.typeId` is a string matching a `TokenTypeDef.id`. Formats declare their token types; core uses the declarations. This is more extensible — new formats can define new token types without modifying a shared enum.

### AST Node Types Come From Formats

`FormatASTNodeTypes` with `ASTNodeTypeDef[]` declarations. Core builds ASTs but isn't responsible for symbol recognition — formats declare what node types they produce.

---

## Core Analysis Pipeline

The core provides a full analysis pipeline that processes Twee documents from raw text to diagnostics:

```
RawPassage[]        ← core/parser.ts splits on :: headers
     ↓
BodyToken[]         ← format.lexBody(body, baseOffset) tokenizes passage bodies
     ↓
ASTNode tree        ← core/astBuilder.ts builds hierarchical AST using format.macroBodyStyle
     ↓
SyntaxAnalysis      ← core/syntaxAnalyzer.ts checks 7 structural error categories
     ↓
SemanticAnalysis    ← core/semanticAnalyzer.ts checks 6 semantic error categories
     ↓
PassageCFG          ← core/cfg.ts builds per-passage control flow graph
     ↓
StoryFlowGraph      ← core/storyFlowGraph.ts combines CFGs + special passage virtual edges
     ↓
VirtualDocs         ← core/virtualDocs.ts extracts embedded JS/CSS for language features
     ↓
Diagnostics         ← core/diagnosticEngine.ts merges all analysis phases
```

### Per-Passage CFG (Control Flow Graph)

The `CFGBuilder` in `core/cfg.ts` builds a control flow graph for each passage:

- **Basic blocks** — sequences of statements with a single entry and exit point
- **Navigation edges** — edges that navigate to other passages (`<<goto>>`, `(go-to:)`, `[[links]]`)
- **Variable state tracking** — tracks what values variables can hold at each point
- **Abstract value types** — `literal`, `type`, `range`, `truthy`, `falsy`, `union`, `unknown`

The CFG uses `MacroDef` boolean flags (`isConditional`, `isNavigation`, `isAssignment`) and `format.extractPassageRefs()` to identify branching and navigation without any format-specific logic.

### Story Flow Graph

The `StoryFlowGraphBuilder` in `core/storyFlowGraph.ts` combines per-passage CFGs into a workspace-wide graph:

- **Inter-passage edges** — navigation between passages (links, macros, API calls)
- **Virtual edges** — special passages that run automatically (StoryInit, PassageHeader, StoryInterface)
- **Variable state flow** — propagates variable states across passages via BFS
- **Conditional reachability** — distinguishes "always reachable" from "only reachable via conditional links"
- **Dead condition detection** — finds conditions that are always true/false given known variable state
- **Fixed-point analysis** — iterates until variable states stabilize

### Virtual Documents

The `VirtualDocProvider` in `core/virtualDocs.ts` extracts embedded language regions:

- `[script]` passages → JavaScript virtual documents
- `[stylesheet]` passages → CSS virtual documents
- SugarCube `<<script>>`/`<<style>>` macro bodies → JS/CSS virtual documents
- Snowman `<% %>` template blocks → JavaScript virtual documents
- Diagnostic remapping — virtual document diagnostics are mapped back to the source Twee document

---

## Directory Structure

```
knot_v2/
├── package.json                          # Layer 1: Promises
├── tsconfig.json
├── language-configuration.json
│
├── design/                               # Architecture & design docs
│   ├── ARCHITECTURE.md                   # This file
│   ├── PROMISES.md                       # What each layer promises
│   └── HOOK_SYSTEM.md                    # FormatModule & enum system deep-dive
│
├── assets/
│   └── icon.png                          # Extension icon
│
├── syntaxes/
│   ├── twee.tmLanguage.json              # Base Twee grammar
│   └── twee-script.tmLanguage.json       # Embedded JS/CSS grammar
│
├── client/                               # Layer 2: Extension host side
│   ├── package.json
│   ├── tsconfig.json
│   ├── build.js
│   └── src/
│       ├── extension.ts                  # Activation entry point
│       ├── statusBar.ts                  # Status bar UI
│       ├── commands/
│       │   ├── index.ts                  # Command registry & re-exports
│       │   ├── lspCommands.ts            # LSP control commands
│       │   └── buildCommands.ts          # Tweego build commands
│       └── ui/
│           ├── index.ts                  # UI re-exports
│           └── menuProvider.ts           # Command palette menu
│
├── server/                               # Layer 2-3: Language server
│   ├── package.json
│   ├── tsconfig.json
│   ├── tsconfig.test.json
│   ├── build.js
│   └── src/
│       ├── server.ts                     # Server entry point
│       ├── lspServer.ts                  # Thin LSP dispatcher (delegates to handlers)
│       │
│       ├── core/                         # Layer 3: Format-agnostic core
│       │   ├── index.ts
│       │   ├── workspaceIndex.ts         # Workspace-wide passage indexing
│       │   ├── parser.ts                 # Twee passage parser (uses FormatRegistry)
│       │   ├── documentStore.ts          # Document lifecycle management
│       │   ├── incrementalParser.ts      # Incremental re-parsing
│       │   ├── referenceIndex.ts         # Cross-reference tracking (uses PassageRef[])
│       │   ├── linkGraph.ts              # Passage link graph (BFS reachability)
│       │   ├── symbolTable.ts            # Symbol table for workspace
│       │   ├── diagnosticEngine.ts       # Uses FormatModule.diagnostics rules
│       │   ├── ast.ts                    # AST node types and tree utilities
│       │   ├── astBuilder.ts             # Builds hierarchical AST from BodyToken[]
│       │   ├── astWorkspace.ts           # Coordinators: AST → syntax → semantic → virtual docs
│       │   ├── syntaxAnalyzer.ts         # 7 structural error checks
│       │   ├── semanticAnalyzer.ts       # 6 semantic error checks
│       │   ├── virtualDocs.ts            # Virtual document extraction (JS/CSS)
│       │   ├── cfg.ts                    # Per-passage control flow graph
│       │   └── storyFlowGraph.ts         # Cross-passage story flow with variable state
│       │
│       ├── handlers/                     # Layer 3: LSP handlers (format-agnostic)
│       │   ├── index.ts                  # Handler registration + re-exports
│       │   ├── completions.ts            # Completion + completion resolve handler
│       │   ├── definition.ts             # Go-to-definition handler
│       │   ├── hover.ts                  # Hover handler
│       │   ├── references.ts             # Find-references handler
│       │   ├── rename.ts                 # Rename handler
│       │   ├── symbols.ts                # Document/workspace symbols handler
│       │   ├── diagnostics.ts            # Diagnostics handler
│       │   ├── codeActions.ts            # Code action handler
│       │   ├── documentLinks.ts          # Document link handler
│       │   └── semanticTokens.ts         # Semantic tokens handler
│       │
│       ├── hooks/                        # Enum definitions + deprecated re-exports
│       │   ├── index.ts                  # Re-exports
│       │   ├── hookTypes.ts              # Enums: MacroCategory, MacroKind, PassageRefKind, etc.
│       │   ├── hookRegistry.ts           # DEPRECATED — re-exports FormatRegistry
│       │   └── formatHooks.ts            # DEPRECATED — re-exports from formats/_types
│       │
│       └── formats/                      # Layer 4: Format implementations
│           ├── _types.ts                 # FormatModule type system (THE contract)
│           ├── formatRegistry.ts         # FormatRegistry (resolution, detection, loading)
│           ├── index.ts                  # Re-exports
│           ├── fallback/
│           │   ├── index.ts              # fallbackModule: FormatModule
│           │   └── lexer.ts              # Fallback body lexer
│           ├── sugarcube/
│           │   ├── index.ts              # sugarcubeModule: FormatModule
│           │   ├── lexer.ts              # SugarCube body lexer
│           │   ├── snippets.ts           # Snippet templates
│           │   ├── runtime.ts            # Runtime globals (State, Engine, Config, etc.)
│           │   ├── specialPassages.ts     # StoryInit, PassageHeader, PassageFooter, StoryInterface
│           │   ├── macros-helpers.ts      # m(), mc(), sig(), arg() helper functions
│           │   ├── macros-control.ts      # if, for, while, switch, etc.
│           │   ├── macros-navigation.ts   # goto, return, back, etc.
│           │   ├── macros-variable.ts     # set, capture, unset, etc.
│           │   ├── macros-output.ts       # print, include, etc.
│           │   ├── macros-revision.ts     # replace, append, prepend, etc.
│           │   ├── macros-styling.ts      # addclass, removeclass, toggleclass, etc.
│           │   ├── macros-audio.ts        # audio, cacheaudio, etc.
│           │   ├── macros-form.ts         # textbox, checkbox, radiobutton, etc.
│           │   ├── macros-timed.ts        # repeat, stop, wait, etc.
│           │   ├── macros-misc.ts         # debug, nobr, etc.
│           │   └── macros-index.ts        # Aggregates all macro categories
│           ├── harlowe/
│           │   ├── index.ts              # harloweModule: FormatModule
│           │   ├── lexer.ts              # Harlowe body lexer
│           │   ├── snippets.ts           # Snippet templates
│           │   ├── runtime.ts            # Runtime globals
│           │   ├── specialPassages.ts     # Startup, Header, Footer
│           │   ├── macros-helpers.ts      # Helper functions
│           │   ├── macros-basics.ts       # Basic macros
│           │   ├── macros-data.ts         # Data macros
│           │   ├── macros-display.ts      # Display macros
│           │   ├── macros-navigation.ts   # Navigation macros
│           │   ├── macros-interactive.ts  # Interactive macros
│           │   ├── macros-advanced.ts     # Advanced macros
│           │   └── macros-index.ts        # Aggregates all macro categories
│           ├── chapbook/
│           │   ├── index.ts              # chapbookModule: FormatModule
│           │   ├── lexer.ts              # Chapbook body lexer
│           │   ├── snippets.ts           # Snippet templates
│           │   ├── runtime.ts            # Runtime globals
│           │   ├── specialPassages.ts     # Special passages
│           │   ├── diagnostics.ts         # Chapbook-specific diagnostics
│           │   ├── inserts-output.ts      # Output inserts
│           │   ├── inserts-control.ts     # Control inserts
│           │   ├── inserts-navigation.ts  # Navigation inserts
│           │   ├── inserts-interactive.ts # Interactive inserts
│           │   ├── inserts-debug.ts       # Debug inserts
│           │   ├── inserts-modifiers.ts   # Modifier inserts
│           │   ├── inserts-helpers.ts     # Helper functions
│           │   └── inserts-index.ts       # Aggregates all insert categories
│           └── snowman/
│               ├── index.ts              # snowmanModule: FormatModule
│               ├── lexer.ts              # Snowman body lexer
│               ├── snippets.ts           # Snippet templates
│               ├── runtime.ts            # Runtime globals
│               └── diagnostics.ts        # Snowman-specific diagnostics
│
├── shared/                               # Cross-cutting types
│   ├── index.ts
│   ├── protocol.ts                       # Custom LSP protocol extensions
│   └── enums.ts                          # Shared enums (KnotStatus, KnotErrorCode)
│
└── tests/
    ├── server/
    │   ├── core/
    │   │   ├── storyDataDetection.test.ts
    │   │   ├── parser.test.ts
    │   │   └── boundary.test.ts
    │   ├── formats/
    │   │   ├── harlowe/adapter.test.ts
    │   │   └── adapterStandardization.test.ts
    │   └── hooks/
    │       └── hookRegistry.test.ts
    └── helpers/
        └── testFixtures.ts
```

### Key Directory Notes

- **`formats/_types.ts`** is the single source of truth for the FormatModule contract. Core and handlers import types from here — never from format directories.
- **`formats/formatRegistry.ts`** provides `FormatRegistry` with lazy loading, O(1) alias resolution, and `detectFromStoryData()`. Core uses this instead of `HookRegistry`.
- **`hooks/hookRegistry.ts`** and **`hooks/formatHooks.ts`** are DEPRECATED — they re-export from `formats/formatRegistry.ts` and `formats/_types.ts` respectively for backward compatibility during migration.
- **`hooks/hookTypes.ts`** remains the authoritative source for enums used across all layers.
- **Format modules use `index.ts`** (not `adapter.ts`) as their entry point. Each format has a modular file structure splitting macros/inserts by category.
- **`core/cfg.ts`** and **`core/storyFlowGraph.ts`** provide control flow and story flow analysis. Both use `FormatModule` exports (never format directories).
- **`lspServer.ts`** is a thin dispatcher — all LSP logic lives in handler modules registered via `handlers/index.ts`.

---

## Critical Rules

1. **Core (`server/src/core/`) and handlers (`server/src/handlers/`) MUST NOT import from format directories** (e.g. `formats/sugarcube/`, `formats/harlowe/`). They import types from `formats/_types.ts` and resolve modules via `FormatRegistry`.
2. **All format data reaches core through `FormatModule` capability bags** — accessed via `FormatRegistry.getModule()`.
3. **Formats register via `FormatRegistry.register()`, core reads via `FormatRegistry.getModule()`** — same registry pattern, new names.
4. **No file in `core/` or `handlers/` may contain the string "sugarcube" or any format name**
5. **Tests for core/handlers must use mock FormatModule objects, never real format adapters**
6. **Every enum used in core must be defined in `hooks/hookTypes.ts` or `shared/enums.ts`**
7. **Capability bags are optional — absent bags mean the feature is unsupported** (no stubs, no dead code)
8. **Diagnostic rules are declarative (`DiagnosticRuleDef[]`)** — core's diagnostic engine enforces them. Use `customCheck?` sparingly as an escape hatch.
9. **Special passages are declarative (`SpecialPassageDef[]`)** — core builds a Map for O(1) lookup. No `classifyPassage()` function.
10. **`lexBody(input, baseOffset)` must use `baseOffset`** for correct `SourceRange` values in all emitted `BodyToken`s.
11. **`extractPassageRefs(body, bodyOffset)` is THE single source of truth** for all passage references. Core NEVER extracts passage references on its own — no `[[` regex in core, no hardcoded macro names for navigation.
12. **Special passages go into the graph** — `StoryInterface`, `StoryInit`, `PassageHeader`, etc. are all handled through `specialPassages` exports with virtual edges in the story flow graph.

---

## Build & Development

- **Build**: esbuild bundles client and server separately
- **Test**: mocha for server tests — written after the full working body is complete
- **Dynamic format loading**: `BUILTIN_LOADERS` uses lazy loaders. Future: async imports for external formats.
- **Handler-first development**: LSP handlers are the entry points that consume FormatModule exports
- **Populate stubs first, tests after**: All handler stubs and core stubs must be fully implemented before writing tests
