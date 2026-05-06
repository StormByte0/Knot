# Knot v2 — Promises

This document defines what each layer **promises** (exports) and what it **expects** (imports). This is the contract that must be honored at every boundary.

---

## Layer 1: package.json

### Promises
| Promise Type | What's Promised |
|---|---|
| Commands | 13 commands the extension registers |
| Languages | `twine` language ID with `.tw`/`.twee` extensions |
| Grammars | Base + embedded grammars for Twee syntax |
| Semantic Tokens | Token types for passages, macros, variables, etc. |
| Configuration | Settings for project, build, lint, and format |
| Keybindings | Go-to-passage, build shortcuts |
| Activation | On `.tw`/`.twee` files in workspace |

### Expects
Nothing — this is the declaration layer.

---

## Layer 2: Client (Extension Host)

### Promises
| Export | What It Provides |
|---|---|
| `activate(context)` | Extension activation, language client startup |
| `deactivate()` | Clean shutdown |
| Command handlers | Implementation of all 13 commands declared in package.json |
| Status bar | Format indicator, server status |
| UI | Menu provider, format picker |

### Expects (Imports)
| From | What's Needed |
|---|---|
| `shared/protocol` | Custom LSP message types |
| `shared/enums` | Shared error codes, status enums |
| `vscode-languageclient` | LSP client library |

---

## Layer 2: Server (LSP Server Entry)

### Promises
| Export | What It Provides |
|---|---|
| LSP connection | Full language server protocol implementation |
| Feature registration | All LSP capabilities (completion, hover, semantic tokens, etc.) |
| Document sync | Full document synchronization |
| Thin dispatcher | `lspServer.ts` delegates all logic to handler modules |

### Expects (Imports)
| From | What's Needed |
|---|---|
| `core/` | Workspace index, parser, document store, CFG, flow graph, AST |
| `handlers/` | LSP handler implementations (9 handler modules) |
| `formats/formatRegistry` | `FormatRegistry` for format module resolution |
| `hooks/hookTypes` | Enum definitions |
| `shared/protocol` | Custom protocol types |

---

## Layer 3: Core

### Promises
| Export | What It Provides |
|---|---|
| `WorkspaceIndex` | Indexes all passages in workspace, accepts format data via enums |
| `Parser` | Parses Twee documents into AST, delegates body lexing to `FormatModule.lexBody()` |
| `DocumentStore` | Manages open document lifecycle |
| `IncrementalParser` | Re-parses only changed passages |
| `ReferenceIndex` | Tracks cross-references using `PassageRef[]` from `extractPassageRefs()` |
| `LinkGraph` | Directed graph of passage links (BFS reachability, orphan detection) |
| `SymbolTable` | Workspace-wide symbol table |
| `DiagnosticEngine` | Orchestrates diagnostics; enforces `DiagnosticCapability.rules` declaratively |
| `ASTBuilder` | Builds hierarchical AST from `BodyToken[]` using `format.macroBodyStyle` |
| `SyntaxAnalyzer` | 7 structural checks (unclosed macros, mismatched close tags, invalid nesting, etc.) |
| `SemanticAnalyzer` | 6 semantic checks (unknown macros, deprecated macros, unknown variables, unknown passage refs, scope violations, custom macro resolution) |
| `VirtualDocProvider` | Extracts embedded JS/CSS for language features in script/stylesheet passages and macro bodies |
| `ASTWorkspace` | Coordinator: AST → syntax analysis → semantic analysis → virtual docs |
| `CFGBuilder` | Per-passage control flow graph with variable state tracking |
| `StoryFlowGraphBuilder` | Cross-passage story flow with special passage virtual edges and variable state propagation |

### Expects (Imports)
| From | What's Needed |
|---|---|
| `hooks/hookTypes` | `MacroCategory` (8 values: `Navigation`, `Output`, `Control`, `Variable`, `Styling`, `System`, `Utility`, `Custom`), `MacroKind` (3 values: `Changer`, `Command`, `Instant`), `MacroBodyStyle` (3 values: `CloseTag`, `Hook`, `Inline`), `PassageType` (6 values: `Story`, `Stylesheet`, `Script`, `Start`, `StoryData`, `Custom`), `PassageKind` (4 values: `Markup`, `Script`, `Stylesheet`, `Special`), `LinkKind` (3 values: `Passage`, `External`, `Custom`), `PassageRefKind` (4 values: `Link`, `Macro`, `API`, `Implicit`) enums |
| `formats/_types` | `FormatModule` (the contract), `FormatASTNodeTypes`, `ASTNodeTypeDef`, `TokenTypeDef`, `BodyToken` (string `typeId` referencing `TokenTypeDef.id`), `MacroDef` (with boolean flags `isNavigation`, `isInclude`, `isConditional`, `isAssignment`, `passageArgPosition`, `hasBody`), `MacroSignatureDef`, `MacroArgDef` (with `embeddedLanguage?`), `SpecialPassageDef` (declarative, replaces `classifyPassage()`), `PassageRef` (single source of truth for passage references), `PassageRefKind`, `LinkResolution`, `MacroDelimiters` (replaces getter methods), `DiagnosticRuleDef`, `DiagnosticResult` (with `ruleId: string`), `DiagnosticCapability`, `DiagnosticCheckContext`, `MacroCapability`, `VariableCapability`, `CustomMacroCapability`, `SnippetsCapability`, `RuntimeCapability`, `SnippetDef`, `RuntimeGlobalDef`, `VariableSigilDef`, `SourceRange`, `CORE_DIAGNOSTIC_RULES` |
| `formats/formatRegistry` | `FormatRegistry` for resolving format modules at runtime (lazy loading, O(1) alias resolution, `detectFromStoryData()`) |
| `shared/enums` | Shared error codes (`KnotErrorCode`), status enums (`KnotStatus`) |

### MUST NOT Import
- `formats/sugarcube/`, `formats/harlowe/`, `formats/chapbook/`, `formats/snowman/`, `formats/fallback/` (any format directory)
- Any format name string literals

### Zero Format-Specific Guarantees
Core additionally promises:
- **Zero format-specific regex** — no hardcoded patterns for `<<>>`, `(:)`, `<% %>`, or any delimiter
- **Zero hardcoded delimiters** — macro delimiters are always from `FormatModule.macroDelimiters`
- **Zero hardcoded sigil logic** — `$` vs `_` scoping is always via `VariableCapability.sigils`
- **Zero format-specific trigger chars** — completion triggers are always from `VariableCapability.triggerChars` or derived from `macroDelimiters`
- **Zero hardcoded diagnostic rules in core** — format-specific rules come from `DiagnosticCapability.rules`; core provides only `CORE_DIAGNOSTIC_RULES` (6 Twine Engine level rules)
- **Zero hardcoded passage types** — no passage types beyond the 6 `PassageType` values; format-specific types use `SpecialPassageDef` with `PassageKind.Special` + optional `typeId`
- **Zero classifyPassage() calls** — special passage classification is declarative via `SpecialPassageDef[]`; core builds a Map for O(1) lookup
- **Zero parallel macro maps** — `MacroDef` boolean flags replace separate navigation/block/assignment maps; core derives Sets at load time
- **Zero passage reference extraction in core** — `extractPassageRefs()` is the ONLY way core learns about passage references; no `[[` regex in core, no hardcoded navigation macro names
- **Zero format-specific special passage handling** — StoryInterface, StoryInit, PassageHeader are all handled through `SpecialPassageDef` with `typeId` lookups; no format name strings

---

## Layer 3: Handlers

### Promises
| Export | What It Provides |
|---|---|
| `CompletionHandler` | LSP completion + completion resolve, delegates via FormatModule capability bags |
| `DefinitionHandler` | Go-to-definition for passages, macros, variables |
| `HoverHandler` | Hover info, delegates macro docs via FormatModule |
| `ReferencesHandler` | Find-all-references |
| `RenameHandler` | Rename passages and variables |
| `SymbolsHandler` | Document and workspace symbols |
| `DiagnosticsHandler` | LSP diagnostics, enforces `DiagnosticCapability.rules` |
| `CodeActionHandler` | Quick fixes and refactoring |
| `DocumentLinksHandler` | Clickable passage links |
| `SemanticTokensHandler` | Semantic token highlighting using format-declared token types |

### Expects (Imports)
| From | What's Needed |
|---|---|
| `core/` | All core modules for data access |
| `hooks/hookTypes` | Enums for categorization |
| `formats/_types` | `FormatModule`, capability bag interfaces, `MacroDef`, `PassageRef`, `DiagnosticResult`, etc. |
| `formats/formatRegistry` | `FormatRegistry` for runtime format resolution |
| `shared/` | Protocol types, enums |

### MUST NOT Import
- `formats/sugarcube/`, `formats/harlowe/`, `formats/chapbook/`, `formats/snowman/`, `formats/fallback/` (any format directory)

---

## Layer 4: Formats

### Promises (via FormatModule)
Each format module exports a single `FormatModule` object literal containing:
- **`formatId`** — unique format identifier string (e.g. `"sugarcube-2"`, `"harlowe-3"`)
- **`displayName`** — human-readable name (e.g. "SugarCube 2", "Harlowe 3")
- **`version`** — format version string
- **`aliases`** — alternate names for O(1) resolution (e.g. `["sugarcube", "sc2"]`)
- **`astNodeTypes`** — `FormatASTNodeTypes` declaring format-specific AST node kinds (including baseline: Document, PassageHeader, PassageBody, Link, Text)
- **`tokenTypes`** — `TokenTypeDef[]` declaring what token types `lexBody()` produces
- **`lexBody(input, baseOffset)`** — tokenizes passage body with correct SourceRange values. Core handles `::` headers and `[[ ]]` boundaries; format handles everything inside passage bodies.
- **`extractPassageRefs(body, bodyOffset)`** — THE single source of truth for passage references. Returns `PassageRef[]` covering [[ ]] links, navigation macros, API calls, and implicit references. Core NEVER extracts passage references on its own.
- **`resolveLinkBody(rawBody)`** — resolves the interior of `[[...]]` into a `LinkResolution`
- **`specialPassages`** — `SpecialPassageDef[]` for declarative special passage classification (replaces `classifyPassage()`). Includes StoryInterface, StoryInit, PassageHeader, PassageFooter, etc. — all special passages go into the graph.
- **`macroBodyStyle`** — `MacroBodyStyle` enum value
- **`macroDelimiters`** — `MacroDelimiters` object (replaces getter methods)
- **`macroPattern`** — `RegExp | null` for detecting macro calls

Additionally, each format module may provide optional capability bags:
- **`macros?: MacroCapability`** — builtin macro catalog + alias mapping. Present for SugarCube (70 macros), Harlowe (257 macros). Absent for Chapbook (inserts), Snowman (templates).
- **`variables?: VariableCapability`** — variable sigils, assignment/comparison operators, patterns, trigger chars. Present for SugarCube (`$/_`), Harlowe (`$`). Absent for Chapbook (`var.name`), Snowman (`s.name`).
- **`customMacros?: CustomMacroCapability`** — user-defined macro support (e.g. SugarCube's `widget`). Absent for formats without custom macro support.
- **`diagnostics?: DiagnosticCapability`** — declarative `DiagnosticRuleDef[]` rules + optional `customCheck?` escape hatch. Core's diagnostic engine enforces the rules.
- **`snippets?: SnippetsCapability`** — snippet templates for autocompletion using VS Code snippet syntax.
- **`runtime?: RuntimeCapability`** — runtime globals (State, Engine, Config, etc.) + optional virtual prelude for analysis.

### MacroDef Boolean Flags
Each `MacroDef` carries its own classification flags, replacing parallel maps:
- `hasBody?` — whether this macro has a body (close-tag or hook style)
- `isNavigation?` — whether this macro navigates to another passage
- `isInclude?` — whether this macro includes/transcludes a passage
- `isConditional?` — whether this macro creates a conditional branch
- `isAssignment?` — whether this macro assigns to a variable
- `passageArgPosition?` — which argument position holds a passage name (0-indexed)

Core derives Sets at format-load time from these flags for O(1) lookups. No more 6 parallel maps to keep in sync.

### MacroArgDef Embedded Language
Each `MacroArgDef` may declare an `embeddedLanguage?` field:
- `'javascript'` — SugarCube `<<run>>`, `<<print>>`, Snowman `<% %>` blocks
- `'css'` — SugarCube `<<style>>` body
- `'html'` — Any macro arg that appends HTML to the DOM

This enables semantic token highlighting for embedded code, virtual document creation, and diagnostics from the embedded language's analyzer.

### Format Directory Structure
Each format module uses `index.ts` (not `adapter.ts`) as its entry point. Macros/inserts are split by category for maintainability:

```
formats/sugarcube/
├── index.ts              # sugarcubeModule: FormatModule
├── lexer.ts              # Body lexer
├── specialPassages.ts    # StoryInit, PassageHeader, PassageFooter, StoryInterface
├── macros-helpers.ts     # m(), mc(), sig(), arg() helper functions
├── macros-control.ts     # if, for, while, switch, etc.
├── macros-navigation.ts  # goto, return, back, etc.
├── macros-index.ts       # Aggregates all macro categories
├── snippets.ts           # Snippet templates
└── runtime.ts            # Runtime globals
```

### Expects (Imports)
| From | What's Needed |
|---|---|
| `formats/_types` | `FormatModule` interface, capability bag interfaces, `MacroDef`, `SpecialPassageDef`, `DiagnosticRuleDef`, `BodyToken`, `TokenTypeDef`, `PassageRef`, etc. |
| `hooks/hookTypes` | Enums to categorize data (`MacroCategory`, `MacroKind`, `PassageType`, `LinkKind`, `MacroBodyStyle`, `PassageKind`, `PassageRefKind`, etc.) |
| `formats/formatRegistry` | Registry to register themselves |

### MUST NOT Import
- `core/` (format modules are data providers, not consumers of core)
- `handlers/` (format modules don't handle LSP requests)
- Other format directories (formats are isolated from each other)

---

## Validation Checklist

Before any PR can merge, verify:
- [ ] No import from format directories (`formats/sugarcube/`, `formats/harlowe/`, etc.) in `core/` or `handlers/`
- [ ] No format name string in `core/` or `handlers/` source
- [ ] All format data flows through `FormatModule` capability bags accessed via `FormatRegistry`
- [ ] All enums used in core are from `hooks/hookTypes` or `shared/enums`
- [ ] No `DiagnosticRule` enum references — diagnostics use `DiagnosticRuleDef` with string `id` and `DiagnosticResult` with `ruleId: string`
- [ ] Every command in package.json has a handler in client
- [ ] Every LSP capability in package.json has a handler in server
- [ ] No format-specific regex in `core/` or `handlers/` (no `<<>>`, `(:)`, `<% %>` patterns)
- [ ] No hardcoded delimiters in `core/` or `handlers/` — all from `FormatModule.macroDelimiters`
- [ ] No hardcoded sigil logic in `core/` or `handlers/` — all via `VariableCapability.sigils`
- [ ] No `classifyPassage()` calls — special passages are declarative via `SpecialPassageDef[]`
- [ ] No parallel macro maps — classification is via `MacroDef` boolean flags; core derives Sets at load time
- [ ] Every format module exports a valid `FormatModule` object with all required fields
- [ ] Optional capability bags are present only when the format supports that feature (no stubs)
- [ ] `BodyToken.typeId` is a string referencing a `TokenTypeDef.id` from the same format module
- [ ] `lexBody(input, baseOffset)` uses `baseOffset` for correct SourceRange values
- [ ] `DiagnosticCapability.rules` are declarative; `customCheck?` is used sparingly as an escape hatch
- [ ] Custom passage types use `SpecialPassageDef` with `PassageKind.Special` + optional `typeId` — never extend the `PassageType` enum
- [ ] Custom macro categories use `MacroCategory.Custom` with a `categoryDetail` string — never extend the `MacroCategory` enum
- [ ] Custom link kinds use `LinkKind.Custom` with a `kindDetail` string — never extend the `LinkKind` enum
- [ ] All passage references come from `extractPassageRefs()` — no `[[` regex in core, no hardcoded navigation macro names
- [ ] Special passages are handled in the graph through `specialPassages` exports — including StoryInterface, StoryInit, PassageHeader
- [ ] StoryFlowGraph uses `typeId` lookups (not format name strings) to find special passages
- [ ] `PassageRef` with `PassageRefKind` is the single source of truth for passage reference classification
- [ ] Format entry points use `index.ts` (not `adapter.ts`) with modular file structure
- [ ] `MacroArgDef.embeddedLanguage?` is declared for args containing embedded JS/CSS/HTML code
