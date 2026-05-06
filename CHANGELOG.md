# Knot — Changelog

## 2.0.0

Complete architectural overhaul from v1.

### Architecture
- **FormatModule system** — Object literal exports replace class-based IFormatProvider adapters. Formats declare capabilities explicitly; absent bags mean unsupported features (no stubs, no dead code).
- **FormatRegistry** — Lazy loading, O(1) alias resolution, `detectFromStoryData()` auto-detection.
- **Zero format bleed-through** — Core and handlers never import from format directories. All format-specific data flows through `FormatModule` capability bags.
- **Declarative special passages** — `SpecialPassageDef[]` with O(1) Map lookup replaces `classifyPassage()` function calls.
- **Declarative diagnostic rules** — `DiagnosticRuleDef[]` with string IDs replaces `DiagnosticRule` enum. Core engine enforces rules; `customCheck?` escape hatch for complex rules.
- **extractPassageRefs()** — Single source of truth for all passage references (links, macros, API calls, implicit). Core never extracts passage references on its own.

### Story Format Support
- **SugarCube 2** — 70 macros, 24 snippets, 11 runtime globals, widget support, `$/_` variables, `<<>>` delimiters, close-tag body style
- **Harlowe 3** — 257 macros across 6 category files, hook syntax, `$` variables, `()` delimiters, hook body style
- **Chapbook 2** — Inserts and modifiers, YAML front matter, `{}` delimiters
- **Snowman 3** — Template blocks, Underscore.js/Snowman API, `<% %>` delimiters
- **Fallback** — Basic `[[link]]` support for any format

### Analysis Pipeline
- **AST Builder** — Hierarchical AST with 3 nesting strategies (close-tag, hook, inline) based on `macroBodyStyle`
- **Syntax Analyzer** — 7 structural error categories (unclosed macros, mismatched close tags, invalid nesting, etc.)
- **Semantic Analyzer** — 6 semantic checks (unknown macros, deprecated macros, unknown variables, unknown passage refs, scope violations, custom macro resolution)
- **Control Flow Graph** — Per-passage CFG with variable state propagation, abstract value types (literal, range, truthy, falsy, union)
- **Story Flow Graph** — Cross-passage flow with special passage virtual edges (StoryInit → Start, PassageHeader → every passage, StoryInterface → Start), conditional reachability, dead condition detection
- **Virtual Documents** — Embedded JS/CSS language features in `[script]`/`[stylesheet]` passages and `<<script>>`/`<<style>>` macro bodies

### LSP Features
- Completions with resolve (macro signatures, passage previews, variable scope)
- Hover documentation
- Go-to-definition for passages and macros
- Find-all-references
- Rename for passages and variables
- Document and workspace symbols
- Semantic tokens (macros, passages, variables, hooks)
- Document links (clickable passage links)
- Code actions
- Custom protocol requests: `knot/refreshDocuments`, `knot/listPassages`, `knot/listFormats`, `knot/selectFormat`

### Enums (Slimmed)
- `MacroCategory` — 8 values (Navigation, Output, Control, Variable, Styling, System, Utility, Custom)
- `MacroKind` — 3 values (Changer, Command, Instant)
- `MacroBodyStyle` — 3 values (CloseTag, Hook, Inline)
- `PassageType` — 6 values (Story, Stylesheet, Script, Start, StoryData, Custom)
- `PassageKind` — 4 values (Markup, Script, Stylesheet, Special)
- `LinkKind` — 3 values (Passage, External, Custom)
- `PassageRefKind` — 4 values (Link, Macro, API, Implicit)

### Breaking Changes from v1
- `IFormatProvider` → `FormatModule` (object literal, not class)
- `HookRegistry` → `FormatRegistry` (lazy loading, alias resolution)
- `adapter.ts` → `index.ts` with modular file structure per format
- `DiagnosticRule` enum removed — rules use string IDs
- `FormatCapability` enum removed — capability presence checked via bag existence
- `NavigationCapability` bag removed — replaced by `extractPassageRefs()` method
- `classifyPassage()` removed — replaced by `SpecialPassageDef[]` declarative lookup
- `RawPassage.rawLinks` removed — replaced by `extractPassageRefs()` → `PassageRef[]`
- `Parser.extractMacroNames()` removed — macro extraction goes through `lexBody()`
