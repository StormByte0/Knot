# Changelog

All notable changes to Knot v2 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [2.0.0] — In Development

### Added

#### Rust Language Server (New)
- Complete Rust language server implementation using `tower-lsp` + `tokio`
- Cargo workspace with three crates: `knot-core`, `knot-formats`, `knot-server`
- Binary communicates with VS Code over stdio via LSP

#### Unified Document Model
- Format-agnostic `Document` → `Passage` → `Block`/`Link`/`VarOp` type hierarchy
- `DocumentSnapshot` with `Rope`-backed incremental editing
- All files normalized into a single internal representation regardless of story format

#### Format Plugin System
- `FormatPlugin` trait defining parse, semantic token, and special passage contracts
- `FormatRegistry` with plugin discovery and routing
- Four complete format implementations:
  - **SugarCube 2** — Full parser with macro table, variable tracking, special passages
  - **Harlowe 3** — Full parser with partial variable tracking
  - **Chapbook 1** — Full parser (variable tracking unsupported)
  - **Snowman 1** — Full parser with full variable tracking
- Fault-tolerant parsing with `is_incomplete` recovery markers

#### Graph Mathematics Engine
- `PassageGraph` built on `petgraph::DiGraph`
- **Broken link detection** via edge-level `is_broken` flags
- **Unreachable passage detection** via BFS from start passage
- **Infinite loop detection** via Tarjan's SCC algorithm with state mutation analysis
- **Graph export** for Story Map visualization with node metadata and edge markers
- 17 `DiagnosticKind` variants for comprehensive narrative analysis

#### Analysis Engine
- Forward dataflow worklist algorithm for cross-passage variable tracking
- Must-analysis with intersection at join points for variable initialization
- Seeding from special passages (StoryInit, startup, Script)
- Variable diagnostics: uninitialized reads, unused variables, redundant writes
- Structural diagnostics: duplicate passage names, empty passages, dead-end passages, invalid passage names, orphaned passages, complex passages, large passages, missing start link
- Safety-bounded iteration: `passage_count * 10` max iterations

#### Incremental Editing Pipeline
- `graph_surgery()` for incremental in-place graph updates
- Set diff of old/new passage names (added/removed/modified)
- Edge stripping and re-addition for modified passages only
- `DebounceTimer` with configurable duration (default 50ms)
- `UpdateResult` tracking changes and re-analysis necessity

#### Workspace Model
- Single-project workspace with `KnotConfig` (compiler path, build config, diagnostic severities, ignore patterns, format override)
- `StoryMetadata` parsed from StoryData JSON (format, format-version, start passage, IFID)
- Format resolution priority: StoryData → knot.json config → heuristic → default (SugarCube)
- StoryData validation: missing, duplicate, missing start passage

#### Standard LSP Methods (26 implemented)
- `textDocument/completion` — Context-aware: passages (`[[`), variables (`$`), SugarCube macros (`<<`)
- `completionItem/resolve` — Markdown documentation for passage/variable/macro
- `textDocument/hover` — Passage metadata (links, vars, tags, incoming refs)
- `textDocument/definition` — Navigate from links to passage headers
- `textDocument/declaration` — Same as definition
- `textDocument/typeDefinition` — Navigate to StoryData passage
- `textDocument/implementation` — Passages that link to current passage
- `textDocument/references` — All header defs + link occurrences
- `textDocument/prepareRename` / `textDocument/rename` — Rename across all definitions and references
- `textDocument/documentSymbol` — Passages as DocumentSymbols with tag details
- `workspace/symbol` — Workspace-wide passage search
- `textDocument/signatureHelp` — SugarCube macro parameter hints
- `textDocument/codeAction` — Quick-fix: create passage, add link, init variable, add template
- `textDocument/codeLens` — Link/reference count lens above passage headers
- `textDocument/inlayHint` — Variable initialization state hints
- `textDocument/foldingRange` — Foldable passage body regions
- `textDocument/documentLink` — Clickable passage links
- `textDocument/selectionRange` — Hierarchical selection expansion
- `textDocument/formatting` — Header normalization, whitespace, blank lines
- `textDocument/rangeFormatting` — Range-restricted formatting
- `textDocument/onTypeFormatting` — Auto-close `[[`/`]]` and `<<`/`>>`
- `textDocument/linkedEditingRange` — Linked rename of header + all links
- `textDocument/prepareCallHierarchy` — Passage call hierarchy items
- `callHierarchy/incomingCalls` — Passages linking to current
- `callHierarchy/outgoingCalls` — Passages linked from current
- `textDocument/diagnostic` — Pull diagnostics model
- `textDocument/semanticTokens/full` — 10 token types, 4 modifiers with delta encoding

#### Custom LSP Extensions (11 methods)
- `knot/graph` — Passage graph export with metadata, unreachable flags, broken edges
- `knot/build` — Invoke Tweego compiler, stream output via `knot/buildOutput` notifications
- `knot/play` — Build and return compiled HTML path
- `knot/variableFlow` — Per-variable write/read locations, initialized-at-start, unused status
- `knot/debug` — 17-field passage debug info
- `knot/trace` — DFS execution trace with loop detection (configurable max depth, default 50)
- `knot/profile` — 20+ workspace statistics (complexity, balance, distribution, tags)
- `knot/compilerDetect` — PATH + configured path search for Tweego
- `knot/breakpoints` — Set/clear/list debug breakpoints per passage
- `knot/stepOver` — Single-step outgoing choices and variable operations
- `knot/watchVariables` — Variable state at passage entry with dataflow
- `knot/indexProgress` notification — Workspace indexing progress
- `knot/buildOutput` notification — Streamed compiler output

#### VS Code Extension
- Binary bootstrap system: platform-specific `knot-server` resolution with fallback to TextMate
- TextMate grammar for Twee syntax highlighting (`syntaxes/twee.tmLanguage.json`)
- Language configuration for bracket matching, comment toggling, and word patterns
- 9 commands: openStoryMap, build, play, playFromPassage, restartServer, reindexWorkspace, detectCompiler, openPassageByName, toggleAutoRebuild
- 4 keybindings: F5 (Play), Shift+F5 (Play from Passage), F6 (Build), Ctrl+Shift+M (Story Map)
- **Decorations API** — Gutter badges on passage headers, faded unreachable passages, wavy red underlines on broken links
- **Language Status API** — Native status indicator with periodic refresh showing format, passage count, broken/unreachable counts
- **Task Provider** — `knot/build` and `knot/watch` tasks using Pseudoterminal
- **Story Map Webview** — Cytoscape.js graph with 4 layouts (dagre, breadthfirst, cose, circle), search/filter, click-to-navigate, color-coded nodes
- **Play Mode** — In-editor story preview with auto-rebuild, passage history sidebar, debug panel, keyboard navigation (Alt+R/B/←/→/G), format-aware passage tracking
- **Debug View** — Passage inspection with trace, step-over, breakpoints, variable watch, diagnostics listing
- **Profile View** — Workspace statistics dashboard with graph health, variable stats, link distribution, complexity metrics, structural balance, tag analysis
- Crash recovery: auto-retry up to 3 times with 2s delay, restart/disable dialog on max retries
- 13 configurable diagnostic severity settings
- Editor defaults for Twee files: word wrap, tab size 4, format on save, bracket pair colorization

#### CI/CD
- GitHub Actions workflow with 5 jobs: check, test, fmt, clippy, build
- 5-platform binary matrix: win32-x64, darwin-arm64, darwin-x64, linux-x64, linux-arm64
- Platform-specific VSIX packaging with `@vscode/vsce --target`
- Cross-compilation support for Linux ARM64
- Rust cache via `Swatinem/rust-cache@v2`

#### Semantic Token System
- 10 token types: PassageHeader, Macro, Variable, Link, String, Number, Boolean, Comment, Tag, Keyword
- 4 token modifiers: Definition, Deprecated, ReadOnly, ControlFlow
- Delta encoding for efficient token transfer

#### SugarCube Macro Table
- 30 built-in SugarCube macro signatures with parameter descriptions
- Used by signature help and completion providers

[2.0.0]: https://github.com/StormByte0/Knot/releases/tag/v2.0.0
