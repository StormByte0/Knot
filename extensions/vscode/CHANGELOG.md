# Changelog

All notable changes to Knot v2 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [2.2.0] — Unreleased

Story Map foundations: header metadata now lives in the language model, and
reachability gets a manual escape hatch for variable-aliased navigation.

### Added

- **Manual reachability entries (`reachable` header metadata)** — static
  analysis can't see through variable-aliased navigation (`<<goto $next>>`,
  `<<link $label $target>>`), which produced persistent false
  "unreachable passage" warnings for dynamic stories. Marking a top-level
  entry passage in its header metadata — `:: Hub {"reachable":true}` —
  makes it act like a second start node: Knot runs its downward
  reachability analysis from the start passage AND every marked entry,
  suppressing the false positives for everything downstream while still
  flagging genuinely orphaned passages. Reachability is a tool-level
  property (the story engine never sees it), so the flag lives in the
  per-passage metadata block rather than the engine-facing tag bracket —
  story tags stay semantic, and other Twee tools treat the key as
  unknown (preserved) metadata. The unreachable-passage warning offers a
  `Mark '…' as reachable (manual entry)` quickfix that writes the flag
  into the passage header for you — merging with any existing
  position/group/color metadata instead of overwriting it. The flag
  round-trips through Story Map position saves, which merge into the
  existing metadata block instead of rewriting it. The Story Map renders
  manual entries with a green ring + corner marker and a "Manual entry
  point" tooltip; the unreachable-diagnostic message notes how many
  entry points were considered.
- **Header metadata in the language model** — passage position, group,
  color, size, and header line number are parsed once (by every format
  plugin, through the shared header parser) onto the `Passage` model
  instead of being re-derived from raw file text on every Story Map
  request. This also means the "open passage" navigation now jumps to the
  correct line for **closed** files, which previously always opened line 0.

### Fixed

- **StoryInterface highlighting while editing** — the incremental
  single-passage re-parse (what runs on every keystroke inside a
  passage) still skipped StoryInterface body tokens from when that mode
  was excluded from token serving, while the full-document parse served
  them. Each WithinPassage edit replaced the passage's token group with
  header-only tokens, so the body highlighting flickered off until the
  next full re-parse restored it. The incremental path now serves the
  same tokens as the full parse.
- **Unreachable-passage quickfixes now surface from the passage body**
  — VS Code only includes diagnostics intersecting the cursor in the
  code-action context, and the unreachable warning squiggles only the
  passage name in the header, so with the cursor in the passage body —
  where authors actually edit — no quickfix ever appeared. The server
  now derives the passage containing the cursor itself and offers the
  same fixes (add link / mark reachable) from anywhere in the passage,
  respecting diagnostic severity suppression.
- **StoryData positions from any file** — the Story Map position fallback
  read the *first* open document's text, silently dropping StoryData
  passage positions whenever StoryData lived in a different file. It now
  extracts from every open document that actually contains a StoryData
  passage.
- **Metadata through incremental edits** — editing a passage (including
  edits on the header line itself, like changing a position value) kept the
  pre-edit header metadata forever; the incremental path now re-derives
  it. Inserting or deleting lines inside one passage also shifts the
  recorded header line numbers of every later passage in the document.
- **Malformed metadata fields no longer hide their siblings** — the old
  reader aborted on the first unparseable field (a bad `position` value
  made `group`/`color`/`size` invisible); fields are now parsed
  independently, and position/size accept string (`"100,200"`), object
  (`{"x":..,"y":..}`), and array (`[100, 200]`) forms.

### Changed

- **Faster "save all positions"** — `knot/updatePositions` now groups
  updates per file and scans each file once instead of rescanning the
  whole file per passage (O(passages × lines) → O(lines)); drag-heavy
  sessions with large stories stop paying a quadratic tax.

---

## [2.1.0] — Full Release

Graduation from preview to the first full stable release of the Knot v2 line.
The headline feature is **story format versioning**: the SugarCube macro
catalog is now versioned end to end, and every user-facing feature is
filtered through the story's pinned `format-version` from StoryData.

### Added

#### Story Format Versioning (SugarCube)
- **Versioned macro catalog** — every entry now carries its lifecycle
  (`added_in`, `deprecated_in`, `removed_in`) plus per-era descriptors
  (description + argument signature), so the same macro is described
  correctly for the version the story is pinned to (e.g. `<<script>>` shows
  no `language` parameter before SugarCube 2.37.0; `<<for>>` documents
  only its era's forms).
- **Version-filtered completions** — the builtin completion list only
  offers macros that exist at the story's version: `<<type>>`,
  `<<numberbox>>`, `<<done>>`, `<<do>>`/`<<redo>>` etc. are hidden for
  stories pinned below their introduction; the six macros removed in
  SugarCube 2.37.0 (`<<click>>`, `<<display>>`, `<<forget>>`,
  `<<remember>>`, `<<setplaylist>>`, `<<stopallaudio>>`) are offered again
  for older stories and hidden on 2.37+.
- **Version-aware lifecycle diagnostics** — new `sc-macro-removed` (Error:
  states the removal version, the deprecation window, the replacement, and
  downgrade guidance) and `sc-macro-not-in-version` (Warning: states the
  version the macro was added in). The `sc-deprecated` hint now reports
  the deprecation version.
- **Version-aware hover and signature help** — hovers resolve the
  era-appropriate description and arguments, and show tombstone notes for
  macros that don't exist at the story's version.
- **Version-aware special passages** — `StoryDisplayTitle` (added 2.31.0)
  and the `[init]` tag (added 2.36.0) are only special for stories on
  versions that actually have them.
- **StoryData `format-version` change → reindex** — editing the pinned
  version now triggers a full reparse so all completions, tokens, and
  diagnostics immediately reflect the new version (IFID edits remain
  in-place, as they affect nothing).
- **Fail-open version policy** — stories without a parseable
  `format-version` (or with a version newer than the catalog tracks)
  are treated as the latest known SugarCube; unusual metadata degrades
  to the full current surface, never to bogus diagnostics.
- **Version-filtered close-tag completions and sub-macro scoping** — the
  `<</` close-tag fallback list and the parent-constraint map (which
  scopes sub-macros like `<<option>>` to their containers) are filtered
  through the same version gate, so only relevant macros and elements
  are shown.
- **Version-gated HTML attribute directives** — the `@attr` / `sc-eval:attr`
  evaluation-directive completion families exist only from SugarCube
  2.21.0 and are suppressed for older stories (plain attributes are
  always offered).

#### HTML Highlighting
- **StoryInterface passages are now highlighted** — the interface parse
  mode builds the full unified AST but previously served no tokens,
  leaving shell files (e.g. `01-shell.twee`) completely unhighlighted.
  Tag names, attribute names, `=`s, quoted values, entities, and
  delimiters now flow through the semantic-token pipeline with the
  `htmlTag` / `htmlAttribute` / `htmlEntity` / `htmlDelimiter` /
  `htmlDirective` theme colors (present in both Knot Dark and Knot Light,
  and mapped to standard TextMate scopes for third-party themes).

### Fixed

#### HTML Parser
- **False `Unclosed HTML tag` errors on nested same-name elements** — the
  close-tag search matched the FIRST `</name>` in the remaining source,
  so an outer wrapper (`<div id="app-shell">`) stole an inner closer
  (`<div id="scene-canvas"></div>`'s own `</div>`), and the bounded
  content slice then hid every real closer below it — cascading bogus
  errors on well-formed StoryInterface documents. The search is now
  nesting-aware: same-name start tags increment a depth counter, HTML
  comments and `<script>`/`<style>` bodies are consumed as units (a
  closer spelled inside them is not markup), start-tag extents are
  scanned quote-aware (a closer inside a quoted attribute value is part
  of the attribute), and raw-text elements keep upstream's flat
  first-closer semantics.

### Changed
- SugarCube version data is sourced from the official v2 docs per-macro
  History sections, the docs upgrade guides (≥2.37.0 … ≥2.8.0), release
  notes 2.31.0–2.37.3, and the 2.37.3 engine source itself.
- Versions compare strictly as `(major, minor, patch)` tuples — SugarCube
  does not follow semver (2.37.0 shipped removals in a "minor" bump).
- The language server's reported version now derives from the workspace
  `Cargo.toml` at compile time — it can never drift from the extension
  version again.

---

## [2.0.0] — Marketplace Beta (Pre-release)

First public beta release of Knot v2.0.0. Marked as `preview: true` on the
marketplace to signal pre-release status. SugarCube 2 is feature-complete;
Harlowe, Chapbook, and Snowman have placeholder implementations.

> **Version note:** This beta is published with the `--pre-release` flag
> (`vsce publish --pre-release`), which sets `preRelease: true` in the VSIX
> manifest. The Marketplace displays it in a separate **Pre-release** tab —
> users opt in by clicking "Switch to Pre-release Version" on the
> Marketplace page. When stable `2.0.0` ships (without `--pre-release`),
> beta users auto-update to it. The 4-component `2.0.0.1` scheme previously
> documented here does not work: modern `vsce` rejects 4-component versions
> as non-semver, and the Marketplace itself rejects semver prerelease tags
> like `2.0.0-beta.1`.

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

[2.0.0]: https://github.com/StormByte0/Knot/releases/tag/v2.0.0
