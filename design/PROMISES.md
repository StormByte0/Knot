# Knot v2 — Promises

This document defines what each layer **promises** (exports) and what it **expects** (imports). This is the contract that must be honored at every boundary.

---

## Layer 1: package.json

### Promises
| Promise Type | What's Promised |
|---|---|
| Commands | 12 commands the extension registers |
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
| Command handlers | Implementation of all 12 commands declared in package.json |
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
| Feature registration | All LSP capabilities (completion, hover, etc.) |
| Document sync | Full document synchronization |

### Expects (Imports)
| From | What's Needed |
|---|---|
| `core/` | Workspace index, parser, document store |
| `handlers/` | LSP handler implementations |
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
| `Lexer` | Tokenizes Twee source (headers + boundaries) |
| `DocumentStore` | Manages open document lifecycle |
| `IncrementalParser` | Re-parses only changed passages |
| `ReferenceIndex` | Tracks cross-references between passages |
| `LinkGraph` | Directed graph of passage links |
| `SymbolTable` | Workspace-wide symbol table |
| `DiagnosticEngine` | Orchestrates diagnostics; enforces `DiagnosticCapability.rules` declaratively |

### Expects (Imports)
| From | What's Needed |
|---|---|
| `hooks/hookTypes` | `MacroCategory` (11 values: `Navigation`, `Output`, `Control`, `Variable`, `Widget`, `Styling`, `System`, `Dialog`, `Audio`, `Utility`, `Custom`), `MacroKind`, `MacroBodyStyle`, `PassageType` (9 values: `Story`, `Widget`, `Stylesheet`, `Script`, `Init`, `Start`, `StoryData`, `Custom`, `Other`), `PassageKind`, `LinkKind` (5 values: `Passage`, `External`, `Action`, `Back`, `Custom`), `FormatCapability` enums |
| `formats/_types` | `FormatModule` (the contract), `FormatASTNodeTypes`, `ASTNodeTypeDef`, `TokenTypeDef`, `BodyToken` (string `typeId` referencing `TokenTypeDef.id`), `MacroDef` (with boolean flags `isNavigation`, `isInclude`, `isConditional`, `isAssignment`, `passageArgPosition`, `hasBody`), `MacroSignatureDef`, `MacroArgDef`, `SpecialPassageDef` (declarative, replaces `classifyPassage()`), `LinkResolution`, `MacroDelimiters` (replaces getter methods), `DiagnosticRuleDef`, `DiagnosticResult` (with `ruleId: string`), `DiagnosticCapability`, `DiagnosticCheckContext`, `MacroCapability`, `VariableCapability`, `CustomMacroCapability`, `NavigationCapability`, `VariableSigilDef`, `SourceRange`, `CORE_DIAGNOSTIC_RULES` |
| `formats/formatRegistry` | `FormatRegistry` for resolving format modules at runtime (lazy loading, O(1) alias resolution, `detectFromStoryData()`) |
| `shared/enums` | Shared error codes |

### MUST NOT Import
- `formats/sugarcube/`, `formats/harlowe/`, `formats/fallback/` (any format directory)
- Any format name string literals

### Zero Format-Specific Guarantees
Core additionally promises:
- **Zero format-specific regex** — no hardcoded patterns for `<<>>`, `(:)`, `<% %>`, or any delimiter
- **Zero hardcoded delimiters** — macro delimiters are always from `FormatModule.macroDelimiters`
- **Zero hardcoded sigil logic** — `$` vs `_` scoping is always via `VariableCapability.sigils`
- **Zero format-specific trigger chars** — completion triggers are always from `VariableCapability.triggerChars` or derived from `macroDelimiters`
- **Zero hardcoded diagnostic rules in core** — format-specific rules come from `DiagnosticCapability.rules`; core provides only `CORE_DIAGNOSTIC_RULES` (Twine Engine level)
- **Zero hardcoded passage types** — no passage types beyond the Twee 3 spec values in `PassageType`; format-specific types use `SpecialPassageDef` with `PassageKind` + optional `typeId`
- **Zero classifyPassage() calls** — special passage classification is declarative via `SpecialPassageDef[]`; core builds a Map for O(1) lookup
- **Zero parallel macro maps** — `MacroDef` boolean flags replace separate navigation/block/assignment maps; core derives Sets at load time

---

## Layer 3: Handlers

### Promises
| Export | What It Provides |
|---|---|
| `CompletionHandler` | LSP completion, delegates macro/passage completion via FormatModule capability bags |
| `DefinitionHandler` | Go-to-definition for passages, macros, variables |
| `HoverHandler` | Hover info, delegates macro docs via FormatModule |
| `ReferencesHandler` | Find-all-references |
| `RenameHandler` | Rename passages and variables |
| `SymbolsHandler` | Document and workspace symbols |
| `DiagnosticsHandler` | LSP diagnostics, enforces `DiagnosticCapability.rules` |
| `CodeActionHandler` | Quick fixes and refactoring |
| `DocumentLinksHandler` | Clickable passage links |

### Expects (Imports)
| From | What's Needed |
|---|---|
| `core/` | All core modules for data access |
| `hooks/hookTypes` | Enums for categorization |
| `formats/_types` | `FormatModule`, capability bag interfaces, `MacroDef`, `DiagnosticResult`, etc. |
| `formats/formatRegistry` | `FormatRegistry` for runtime format resolution |
| `shared/` | Protocol types, enums |

### MUST NOT Import
- `formats/sugarcube/`, `formats/harlowe/`, `formats/fallback/` (any format directory)

---

## Layer 4: Formats

### Promises (via FormatModule)
Each format module exports a single `FormatModule` object literal containing:
- **`formatId`** — unique format identifier string (e.g. `"harlowe3"`, `"sugarcube2"`)
- **`displayName`** — human-readable name (e.g. "SugarCube 2", "Harlowe 3")
- **`version`** — format version string
- **`aliases`** — alternate names for O(1) resolution (e.g. `["sugarcube", "sc2"]`)
- **`astNodeTypes`** — `FormatASTNodeTypes` declaring format-specific AST node kinds (including baseline: Document, PassageHeader, PassageBody, Link, Text)
- **`tokenTypes`** — `TokenTypeDef[]` declaring what token types `lexBody()` produces
- **`lexBody(input, baseOffset)`** — tokenizes passage body with correct SourceRange values. Core handles `::` headers and `[[ ]]` boundaries; format handles everything inside passage bodies.
- **`resolveLinkBody(rawBody)`** — resolves the interior of `[[...]]` into a `LinkResolution`
- **`specialPassages`** — `SpecialPassageDef[]` for declarative special passage classification (replaces `classifyPassage()`)
- **`macroBodyStyle`** — `MacroBodyStyle` enum value
- **`macroDelimiters`** — `MacroDelimiters` object (replaces getter methods)
- **`macroPattern`** — `RegExp | null` for detecting macro calls

Additionally, each format module may provide optional capability bags:
- **`macros?: MacroCapability`** — builtin macro catalog + alias mapping. Present for SugarCube, Harlowe. Absent for Chapbook (inserts), Snowman (templates).
- **`variables?: VariableCapability`** — variable sigils, assignment/comparison operators, patterns, trigger chars. Present for SugarCube (`$/_`), Harlowe (`$`). Absent for Chapbook (`var.name`), Snowman (`s.name`).
- **`customMacros?: CustomMacroCapability`** — user-defined macro support (e.g. SugarCube's `widget`). Absent for formats without custom macro support.
- **`navigation?: NavigationCapability`** — implicit passage reference patterns beyond `[[links]]`. Present for all formats with API-based navigation.
- **`diagnostics?: DiagnosticCapability`** — declarative `DiagnosticRuleDef[]` rules + optional `customCheck?` escape hatch. Core's diagnostic engine enforces the rules.

### MacroDef Boolean Flags
Each `MacroDef` carries its own classification flags, replacing parallel maps:
- `hasBody?` — whether this macro has a body (close-tag or hook style)
- `isNavigation?` — whether this macro navigates to another passage
- `isInclude?` — whether this macro includes/transcludes a passage
- `isConditional?` — whether this macro creates a conditional branch
- `isAssignment?` — whether this macro assigns to a variable
- `passageArgPosition?` — which argument position holds a passage name (0-indexed)

Core derives Sets at format-load time from these flags for O(1) lookups. No more 6 parallel maps to keep in sync.

### Expects (Imports)
| From | What's Needed |
|---|---|
| `formats/_types` | `FormatModule` interface, capability bag interfaces, `MacroDef`, `SpecialPassageDef`, `DiagnosticRuleDef`, `BodyToken`, `TokenTypeDef`, etc. |
| `hooks/hookTypes` | Enums to categorize data (`MacroCategory`, `MacroKind`, `PassageType`, `LinkKind`, `MacroBodyStyle`, `PassageKind`, etc.) |
| `formats/formatRegistry` | Registry to register themselves |

### MUST NOT Import
- `core/` (format modules are data providers, not consumers of core)
- `handlers/` (format modules don't handle LSP requests)
- Other format directories (formats are isolated from each other)

---

## Validation Checklist

Before any PR can merge, verify:
- [ ] No import from format directories (`formats/sugarcube/`, `formats/harlowe/`, `formats/fallback/`) in `core/` or `handlers/`
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
- [ ] Custom passage types use `SpecialPassageDef` with `PassageKind` + optional `typeId` — never extend the `PassageType` enum
- [ ] Custom macro categories use `MacroCategory.Custom` with a `categoryDetail` string — never extend the `MacroCategory` enum
- [ ] Custom link kinds use `LinkKind.Custom` with a `kindDetail` string — never extend the `LinkKind` enum
