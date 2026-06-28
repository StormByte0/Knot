# Knot — Architecture

This document describes how Knot is structured, what each component does, and
how the pieces fit together. It is written for developers and curious users
who want to understand the system under the hood.

---

## Overview

Knot is a language server and VS Code extension for Twine/Twee interactive
fiction projects. The core insight is that a Twine story is not a pile of
text files — it is a **directed graph** of passages connected by links.
Knot models the project as a graph from the ground up, which enables
structural analysis (broken links, unreachable passages, dead ends, game
loops) that file-by-file tooling cannot provide.

The project is split into two main parts:

1. **Rust language server** (`crates/`) — a high-performance LSP server
   that parses twee files, builds the workspace graph, runs analysis, and
   handles all language features. Written in Rust for low latency and
   memory safety.

2. **VS Code extension** (`extensions/vscode/`) — the client that owns
   the UI: status bar, commands, webview panels (Story Map, Debug View,
   Profile View, Variable Tracking), build orchestration, and the Watch
   toggle. Communicates with the server over LSP.

The extension never parses twee files directly. Every language feature
goes through the server via standard LSP requests and a small set of
custom `knot/*` requests.

---

## Rust Workspace

The server is organized as a Cargo workspace with three crates:

```
crates/
├── core/       — workspace model, graph, analysis, document editing
├── formats/    — format plugins (SugarCube, Harlowe, Chapbook, Snowman)
└── server/     — LSP server, request handlers, client communication
```

### `knot-core`

The foundation. Defines the format-agnostic data model that all format
plugins produce and all analysis runs against:

- **`Workspace`** — owns all documents, the passage graph, configuration
  (`.vscode/knot.json`), and resolved story metadata (format, version,
  IFID from StoryData). This is the central state that everything else
  reads from.
- **`Document`** — a single parsed `.twee` file. Contains passages,
  their tags, links, variable operations, and a `Rope`-backed text
  buffer for incremental editing.
- **`Passage`** — a single passage with its header, body blocks
  (text, macros, expressions, headings), links, variable operations,
  and classification (special, metadata, normal).
- **`Block`** — the content within a passage body: plain text, macro
  invocations, inline expressions, headings, or incomplete/malformed
  segments.
- **`Graph`** — a `petgraph` directed graph where nodes are passages
  and edges are links. Supports incremental surgery (add/remove
  passages and links without rebuilding the whole graph), cycle
  detection (SCC analysis for game loops), and reachability analysis.
- **`Analysis`** — runs the diagnostic passes over the workspace:
  broken links, unreachable passages, uninitialized variables, unused
  variables, redundant writes, duplicate passage names, empty passages,
  dead-end passages, invalid passage names, complex passages (too many
  outgoing links), large passages (exceeds word count threshold).
- **`Editing`** — incremental document update logic. Applies text
  changes to the `Rope` and figures out which passages were affected,
  so only those need re-parsing.

### `knot-formats`

The format plugin system. Each Twine story format (SugarCube, Harlowe,
Chapbook, Snowman) has its own parser that produces the format-agnostic
`Document`/`Passage`/`Block` types defined in `knot-core`.

- **`FormatPlugin` trait** — defines the contract every format must
  implement: parsing, semantic token generation, special passage
  classification, macro catalog access, completion providers, hover
  providers.
- **`FormatRegistry`** — routes requests to the right plugin based on
  the workspace's detected format (from StoryData).
- **SugarCube** (`crates/formats/src/sugarcube/`) — the most complete
  plugin. Uses a recursive descent parser backed by `oxc` (a
  JavaScript parser) for the `<<script>>` and `<<set>>` bodies. Has a
  static macro catalog (~1200 lines of data), special passage
  definitions, CSS parsing, and a full JS annotation pipeline that
  tracks variable reads/writes across SugarCube macros.
- **Harlowe, Chapbook, Snowman** — full parsers, though variable
  tracking is partial in some.

The key architectural principle is **format ownership**: each plugin
owns its syntax, its special passages, its macros, and its semantic
tokens. The server never hardcodes SugarCube syntax — it asks the
plugin.

### `knot-server`

The LSP server built on `tower-lsp` + `tokio`. Handles all client
communication and delegates to `knot-core` and `knot-formats` for the
actual work.

- **`state.rs`** — `ServerState`, the server's mutable state. Holds
  the `Workspace`, the `FormatRegistry`, the language client handle,
  and the extension's global storage path.
- **`handlers/`** — LSP request handlers, organized by concern:
  - `sync.rs` — `did_open`, `did_change`, `did_close`, file watching,
    workspace indexing.
  - `completion.rs`, `hover.rs`, `navigation.rs`, `semantic.rs`,
    `structure.rs` — standard LSP features.
  - `build.rs` — the `knot/build` and `knot/play` custom requests.
    Resolves the tweego binary, story formats directory, source
    directory, output filename, and runs tweego.
  - `profile.rs`, `passage_diagnostics.rs` — custom requests for the
    extension's webview panels.
- **`lsp_ext.rs`** — definitions for all custom `knot/*` request and
  notification types.
- **`helpers/`** — shared utilities: compiler resolution (`which`/
  `where`), story formats directory discovery, indexing logic, URI
  conversion.

---

## VS Code Extension

The client side, written in TypeScript. Owns all UI and orchestrates
the server.

```
extensions/vscode/src/
├── extension.ts           — activation, wiring, lifecycle
├── binaryResolution.ts    — find the knot-server binary for the platform
├── commands.ts            — all knot.* command registrations
├── statusBarItems.ts      — [Story Map] [Build] [Watch] [Play] [⚙]
├── watchState.ts          — singleton: background save watcher state
├── taskProvider.ts        — "Build Story" task for VS Code's task system
├── storyMapProvider.ts    — Story Map webview (graph visualization)
├── debugViewProvider.ts   — Passage Diagnostics webview
├── profileViewProvider.ts — Project Info webview
├── variableFlowProvider.ts— Variable Tracking webview
├── notifications.ts       — custom LSP notification handlers
├── decorations.ts         — editor decorations (gutter badges, fades)
├── languageStatus.ts      — Language Status API indicator
├── navigation.ts          — cross-panel navigation coordination
├── crashRecovery.ts       — automatic server restart on crash
├── types.ts               — shared TypeScript types + LSP client
└── utils.ts               — build request params, managed path helpers
```

### Build Pipeline

The build flow is the most complex orchestration in the extension:

1. **Source**: the workspace root is the source directory. Users put
   all game files (`.twee`, `.js`, `.css`, assets) directly in the
   workspace. Story formats live separately in the extension-managed
   folder — this keeps the workspace purely game files and prevents
   `format.js` from being bundled as a passage.

2. **Tweego resolution**: `knot.build.tweegoPath` setting →
   `.vscode/knot.json` → PATH lookup → managed download.

3. **Story formats resolution**: `knot.build.storyformatsPath` setting
   → versioned managed cache (`<globalStorage>/storyformats/<id>@<ver>/`)
   → error with download hint.

4. **Output filename**: derived from the `StoryTitle` passage
   (sanitized), falling back to `index.html`. This matches Twine GUI
   behavior.

5. **Tweego invocation**: the server assembles args (`--start` if
   needed, `-l` for stats, `-o` for output, merged flags from settings
   + `.vscode/knot.json`, source path) and runs tweego with `cwd` set
   to the workspace root. `TWEEGO_PATH` env var is set when story
   formats are in the managed cache or a user-configured path.

6. **Output streaming**: tweego's stdout/stderr is streamed to the
   extension's "Knot Build" output channel via `knot/buildOutput`
   notifications. The stats line (`Passages: N | Words: N`) is parsed
   and re-emitted as `Knot: Build stats — N passages, N words`.

### Status Bar Cluster

Five items on the left side of the status bar:

- **Story Map** — opens the graph visualization webview
- **Build** — one-shot build (`F6`)
- **Watch** — toggle background auto-rebuild on save
  (`$(eye)` / `$(eye-closed)`)
- **Play** — open compiled HTML in the default browser (`F5`). If
  Watch is ON, opens the existing HTML; if OFF, builds first.
- **⚙** — extension settings

The Watch toggle runs a `vscode.workspace.onDidSaveTextDocument`
watcher for `.tw`/`.twee`/`.js`/`.css`/`.html` files. On each save it
sends a `knot/build` request and logs the result to the build output
channel. The watcher state is session-scoped (not persisted across
reloads).

### Webview Panels

Four webview panels provide visual tooling:

- **Story Map** — an interactive directed graph of all passages,
  rendered with `@xyflow/react` + `dagre` for layout. Nodes are
  passages, edges are links. Supports click-to-navigate, focus, and
  layout customization.
- **Passage Diagnostics** — shows detailed info about the passage
  under the cursor: links, variables, macros, complexity metrics.
- **Project Info** — workspace-level stats: passage count, word count,
  format, IFID, story format version.
- **Variable Tracking** — shows variable flow across passages:
  where each variable is set, read, and how it propagates.

---

## Format Detection

The workspace's story format is detected from the `StoryData`
passage's `format` field. When the server parses a `StoryData`
passage, it extracts the format name (e.g. "SugarCube"), version
(e.g. "2.37.0"), IFID, and start passage. This metadata is stored in
`Workspace.metadata` and drives:

- Which `FormatPlugin` handles parsing and language features
- Which story format the build pipeline downloads into the managed
  cache (if missing)
- The versioned managed cache path: `<globalStorage>/storyformats/sugarcube-2@2.37.0/`

If no `StoryData` passage exists, the server defaults to SugarCube
and notifies the client via `knot/formatDetected`. The extension may
prompt the user to initialize a project.

---

## Configuration

Knot reads configuration from two sources, merged at build time:

1. **VS Code Settings** (`knot.*` settings) — the primary user-facing
   configuration. Visible in the Settings UI, organized into sections:
   Build, Diagnostics, Indexing, Status & Paths, Advanced.

2. **`.vscode/knot.json`** — project-local configuration, checked into
   the repo. Supports `compiler_path`, `storyformats_path`, `build`
   (source_dir, output_dir, flags), `diagnostics` (severity overrides),
   `ignore` (glob patterns), `max_files`, `format` (override),
   `special_passages` (user-defined).

VS Code settings take priority over `.vscode/knot.json` for the same
field. Some fields (like `build.flags`) are merged — both sets apply.

---

## Build Output

When tweego runs, the server streams output to the extension's
"Knot Build" output channel. The server prepends diagnostic lines
showing the resolution decisions:

```
Knot: Tweego binary: /home/user/.vscode/extensions/knot/.../tweego
Knot: Compiling source from: /home/user/project
Knot: Story formats: using managed cache at .../sugarcube-2@2.37.0
Knot: Story formats search path = .../sugarcube-2@2.37.0
<tweego stdout>
Knot: Build stats — 42 passages, 12345 words
```

This makes it easy to debug build failures — every resolution step
is visible.

---

## Platform Support

Knot supports Windows, macOS, and Linux on x64 and arm64. The
extension bundles pre-compiled `knot-server` binaries for each
platform. Tweego is downloaded on first build into the extension's
global storage, so users do not need to install it manually.

Path handling is cross-platform: the server uses `PathBuf` for all
path manipulation, `cfg!(windows)` for platform-specific behavior
(like the `.exe` suffix), and `to_file_path()`/`to_string_lossy()`
for URI conversion. The `force_relative` helper strips Windows drive
prefixes and leading separators so settings like `sourceDir` work
consistently across platforms.
