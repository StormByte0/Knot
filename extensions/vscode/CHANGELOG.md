# Changelog

All notable changes to Knot v2 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [2.0.0.1] — Marketplace Beta

First public beta release of Knot v2.0.0. Marked as `preview: true` on the
marketplace to signal pre-release status. SugarCube 2 is feature-complete;
Harlowe, Chapbook, and Snowman have placeholder implementations.

> **Version note:** The VS Code Marketplace doesn't accept semver pre-release
> suffixes (e.g. `2.0.0-beta.1`). Beta builds use a 4-number version
> (`2.0.0.1`, `2.0.0.2`, …) that sorts below the stable `2.0.0` release, so
> users on the beta will auto-update to stable when it ships.

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
- Four format implementations (varying maturity):
  - **SugarCube 2** — Production-quality parser with macro catalog, variable
    tracking, special passages, completion, and hover
  - **Harlowe 3** — Placeholder/skeleton implementation; `FormatPlugin` trait
    implemented but parser not yet production quality, link extraction not
    functional
  - **Chapbook 1** — Placeholder/skeleton implementation; same status as Harlowe
  - **Snowman 2** — Placeholder/skeleton implementation; same status as Harlowe
- Fault-tolerant parsing with `is_incomplete` recovery markers (SugarCube)

#### Graph Mathematics Engine
- `PassageGraph` built on `petgraph::DiGraph`
- **Broken link detection** via edge-level `is_broken` flags
- **Unreachable passage detection** via BFS from start passage
- **Dead-end passage detection** (passages with no outgoing links)
- **SCC computation** (Tarjan's algorithm) — data is computed and exported
  to the client, but the Story Map webview does not yet render game loop
  highlighting. Infinite loop diagnostic is planned but not yet
  implemented (requires conditional-edge tracking — see ROADMAP.md)
- **Graph export** for Story Map visualization with node metadata and edge markers
- 17 `DiagnosticKind` variants for comprehensive narrative analysis

#### Analysis Engine
- Forward dataflow worklist algorithm for cross-passage variable tracking (SugarCube)
- Must-analysis with intersection at join points for variable initialization
- Seeding from special passages (StoryInit, startup, Script)
- Variable diagnostics: uninitialized reads (ghost text on passage header),
  unused variables (warning, configurable via settings), redundant writes
- Structural diagnostics: duplicate passage names, empty passages, dead-end
  passages, invalid passage names, orphaned passages, complex passages, large
  passages, missing start link
- Safety-bounded iteration: `passage_count * 10` max iterations
- Note: reliable cross-passage flow analysis requires graph flow detection
  that is not yet implemented. Some diagnostics are conservative until that
  work lands — see ROADMAP.md.

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
- `textDocument/completion` — Context-aware: passages (`[[`), variables (`$`), SugarCube macros (`<<`) (SugarCube only; other formats do not yet have macro catalogs)
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
- `textDocument/signatureHelp` — SugarCube macro parameter hints (SugarCube only)
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
- `knot/graph` — Passage graph export with metadata, unreachable flags, broken edges, and SCC game-loop data (note: Story Map does not yet render game-loop highlighting)
- `knot/build` — Invoke Tweego compiler, stream output via `knot/buildOutput` notifications
- `knot/play` — Build and return compiled HTML path
- `knot/variableFlow` — Per-variable write/read locations, initialized-at-start, unused status (SugarCube only)
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
- 14 commands: openStoryMap, build, play, playFromPassage, toggleWatch, restartServer, reindexWorkspace, detectCompiler, configureStoryFormats, openManagedStorage, openTweegoFolder, openPassageByName, openSettings, initProject
- 4 keybindings declared in `package.json` (F5=Play, Shift+F5=Play from Passage, F6=Build, Ctrl+Shift+M=Story Map) — scoped to `resourceLangId == 'twee'`; may conflict with VS Code defaults in practice
- **Status Bar Cluster** — Five items: Story Map, Build, Watch (toggle auto-rebuild on save), Play (open in browser), Settings
- **Watch Toggle** — Background save watcher for `.tw`/`.twee`/`.js`/`.css`/`.html` files with build output logging
- **Play** — Opens compiled HTML in the default system browser; builds first if Watch is off, opens existing HTML if Watch is on
- **Decorations API** — Gutter badges on passage headers, faded unreachable passages, wavy red underlines on broken links
- **Language Status API** — Native status indicator with periodic refresh showing format, passage count, broken/unreachable counts, and format support level (✓ for SugarCube, ◐ for other formats)
- **Task Provider** — `knot/build` task using Pseudoterminal
- **Story Map Webview** — `@xyflow/react` + `dagre` graph visualization with click-to-navigate (single click jumps to passage in editor), special-passage node coloring, dead-end highlighting (yellow double border), unreachable passage grouping. All edges render in gray — no edge-type color differentiation. No focus mode or right-click context menu.
- **Debug View** — Passage inspection with trace, step-over, breakpoints, variable watch, diagnostics listing
- **Profile View** — Workspace statistics dashboard with graph health, variable stats, link distribution, complexity metrics, structural balance, tag analysis
- Crash recovery: auto-retry up to 3 times with 2s delay, restart/disable dialog on max retries
- 10 configurable diagnostic severity settings
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
- ~120 built-in SugarCube macro signatures with parameter descriptions
- Used by signature help and completion providers

#### Build Pipeline
- **Source/format separation** — workspace is purely game files; story formats live in the extension-managed folder. No more `format.js` getting bundled as a passage.
- **StoryTitle-derived output filename** — compiled HTML is named after the `StoryTitle` passage (sanitized), matching Twine GUI behavior. Falls back to `index.html`.
- **Build stats logging** — Tweego's `-l` flag is always passed; passage and word counts are parsed and logged as `Knot: Build stats — N passages, N words`.
- **Build flags setting** — `knot.build.flags` array for additional Tweego command-line flags, merged with `.vscode/knot.json` flags.
- **Auto-download** — Tweego and story formats are downloaded automatically on first build. No manual setup required.
- **Versioned format cache** — story formats cached per version in `<globalStorage>/storyformats/<id>@<version>/`.

### Changed

#### Settings Reorganization
- Dropped the redundant "Knot — " prefix from all section titles
- Reordered sections for progressive disclosure: Build → Diagnostics → Indexing → Status & Paths → Advanced
- "General" section renamed to "Advanced" and demoted to the bottom
- Read-only managed paths gathered into the "Status & Paths" section
- All setting descriptions rewritten for clarity (action-first phrasing, trimmed verbose explanations)

#### Settings Renamed
- `knot.tweegoPath` → `knot.build.tweegoPath` (namespace consistency)
- `knot.storyformats.path` → `knot.build.storyformatsPath` (same)
- One-time migration shim copies old values forward on activation

#### Source Directory Resolution
- Workspace root is always the source directory — no more `src/` auto-detection
- `knot.build.sourceDir` still works as an explicit override
- Source directory validation rejects toolchain directories (contains a tweego binary)

#### Story Formats Resolution
- Project-local `<workspace>/storyformats/` is no longer supported
- Story formats resolve from: user setting → versioned managed cache → error with download hint
- Log messages renamed from "TWEEGO_PATH" to "story formats search path" for clarity

### Fixed

#### Cross-Platform Path Handling
- Fixed hardcoded `\\storyformats\\` backslashes in error messages — now uses `PathBuf::join().display()` for correct separators on all platforms
- Fixed string-based path normalization in the indexing exclude-pattern matcher — now uses `PathBuf::strip_prefix` instead of string manipulation

[2.0.0.1]: https://github.com/StormByte0/Knot/releases/tag/v2.0.0.1
