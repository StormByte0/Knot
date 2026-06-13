# Knot IDE — Architectural Vision & Technical Specification

## Overview

Knot is a next-generation integrated development environment for Twine and Twee projects inside Visual Studio Code. The project is designed to move beyond basic syntax highlighting and macro completion into a fully integrated narrative engineering platform.

The long-term goal is to establish Knot as the primary development environment for all major Twine ecosystems, including SugarCube, Harlowe, Chapbook, and Snowman.

At its core, Knot combines:

* A high-performance Rust-based language server
* A graph-aware narrative precompiler
* Real-time workspace analysis
* Cross-format parsing and diagnostics
* Visual story architecture tooling
* Integrated compiler workflows

Unlike existing Twine extensions, Knot is designed around a unified graph model capable of performing structural and semantic analysis across an entire narrative workspace.

---

# Core Product Goals

## Zero-Configuration Development

Knot is designed to minimize setup friction and provide a fully operational development environment immediately after installation.

The onboarding system automatically:

* Detects missing compiler dependencies
* Downloads and installs Tweego when required
* Generates project scaffolding
* Configures workspace defaults
* Initializes a standardized project structure

Target workflow:

> Open VS Code → Install Knot → Begin writing within 60 seconds.

---

## Graph-Driven Narrative Intelligence

Knot treats a story as a directed graph rather than isolated text files.

This enables:

* Broken link detection
* Infinite traversal loop analysis
* Unreachable passage detection
* Cross-passage variable tracking
* Structural flow visualization
* Workspace-wide semantic analysis

The graph engine continuously updates as the user edits passages in real time.

---

## Multi-Format Workspace Support

Knot supports all major Twine story formats, but each workspace is restricted to a single active project and a single declared story format.

Supported targets include:

* SugarCube
* Harlowe
* Chapbook
* Snowman

A Knot workspace represents exactly one Twine project.

This ensures:

* Deterministic parser routing
* Predictable graph analysis
* Stable variable semantics
* Simpler workspace indexing
* Lower memory overhead
* Faster incremental updates

Opening multiple Twine projects simultaneously inside the same workspace is intentionally unsupported.

Users working on multiple projects are expected to use separate VS Code windows or workspaces.

Cross-format parsing inside a single workspace is intentionally unsupported.

Each format remains isolated behind a dedicated parser plugin while sharing a unified analysis engine.

---

# System Architecture

## High-Level Structure

Knot is divided into two independent runtime layers:

| Layer                | Responsibility                       |
| -------------------- | ------------------------------------ |
| VS Code Client       | UI, onboarding, commands, webviews   |
| Rust Language Server | Parsing, graph analysis, diagnostics |

This separation ensures the extension host remains lightweight while all heavy computation runs in a dedicated native process.

---

# Distribution Architecture

The Rust language server ships as a platform-specific native binary bundled directly inside the VSIX package.

VS Code platform targeting is handled through `@vscode/vsce --target` builds.

| Target       | Binary          |
| ------------ | --------------- |
| win32-x64    | knot-server.exe |
| darwin-arm64 | knot-server     |
| darwin-x64   | knot-server     |
| linux-x64    | knot-server     |
| linux-arm64  | knot-server     |

The TypeScript client bootstraps the correct binary dynamically at activation time.

```ts
const serverPath = await getPlatformBinary(context.extensionPath);

const serverOptions: ServerOptions = {
    command: serverPath,
    args: ['--stdio']
};
```

## Packaging Pipeline

The release pipeline uses a GitHub Actions matrix build to:

* Compile Rust binaries for all supported platforms
* Package target-specific VSIX artifacts
* Attach release assets automatically
* Validate binary execution during CI

## Fallback Behavior

If the native server fails to execute:

* Knot falls back to TextMate grammar highlighting
* Advanced analysis features are disabled gracefully
* The user receives a recoverable warning notification

## Binary Budget

Target binary size:

* Less than 15MB compressed per platform

This keeps extension installation and activation fast.

---

# VS Code Client Layer

The client layer is responsible for user interaction and editor integration.

## Responsibilities

### Workspace & Onboarding

The client detects whether required compiler tooling exists on the system and can automatically install dependencies when necessary.

Features include:

* Tweego detection
* Binary download management
* Progress notifications
* Project scaffolding
* Workspace initialization

---

### UI Integration

The extension contributes:

* Command palette actions
* Sidebar panels
* Status bar indicators
* Story graph webviews
* Build and compile actions

---

### Story Graph Rendering

Narrative flow visualization is rendered through a dedicated webview using technologies such as:

* Cytoscape.js
* D3.js

The graph data itself is generated by the Rust language server and exposed through custom LSP requests.

---

# Reliability Model

Because the analysis engine runs as a native process, the client must handle process failures safely.

## Crash Recovery

The TypeScript client monitors the Rust server process and automatically restarts it if it exits unexpectedly.

After restart:

* Workspace configuration is re-sent
* Open documents are re-synchronized
* Workspace indexing resumes automatically

## Graceful Degradation

If the server crashes repeatedly:

* Knot disables advanced analysis temporarily
* Basic syntax highlighting remains active
* A status bar warning is shown to the user

Example:

> Knot analysis engine unavailable. Click to restart.

## Panic Isolation

The Rust server isolates request-handler panics using:

```rust
std::panic::catch_unwind
```

This prevents graph analysis failures from terminating the entire LSP transport loop.

---

# Rust Language Server

## Why Rust

The language server is implemented as a native Rust binary rather than Node.js or WebAssembly.

This provides:

* True multithreading
* Low-latency graph recomputation
* Native filesystem performance
* Predictable memory usage
* Efficient incremental parsing

The architecture closely mirrors tooling such as rust-analyzer.

---

## Recommended Rust Stack

| Purpose                  | Crate        |
| ------------------------ | ------------ |
| LSP implementation       | tower-lsp    |
| Async runtime            | tokio        |
| Graph algorithms         | petgraph     |
| Incremental text storage | ropey        |
| Serialization            | serde_json   |
| Lexing/parsing           | logos, regex |
| Logging                  | tracing      |

---

# Unified Document Model

Every file is normalized into a format-agnostic internal representation.

```rust
struct Document {
    uri: Url,
    format: StoryFormat,
    passages: Vec<Passage>,
}

struct Passage {
    name: String,
    tags: Vec<String>,
    span: Range,
    body: Vec<Block>,
    links: Vec<Link>,
    vars: Vec<VarOp>,
    is_special: bool,
}
```

Format plugins are responsible only for parsing source text into this structure.

The core engine owns:

* Global graph construction
* Workspace indexing
* Cross-file diagnostics
* Graph mathematics
* Dataflow analysis

---

# Workspace Model

Knot is designed around a single-project workspace model.

Each VS Code workspace is expected to contain exactly one Twine project and exactly one authoritative `StoryData` passage.

Multi-project workspaces are intentionally unsupported.

If multiple independent Twine projects are detected:

* Knot emits workspace diagnostics
* Graph analysis is disabled until the conflict is resolved
* Users are instructed to open projects in separate VS Code windows

This restriction simplifies:

* Graph ownership
* Incremental analysis
* Variable flow tracking
* Format resolution
* Build orchestration
* Cache invalidation

It also guarantees that all analysis operates against a single coherent narrative graph.

## StoryData Authority

The `StoryData` passage is the authoritative source for:

* Story format
* Entry point
* IFID metadata
* Tag color configuration
* Story-level metadata

Knot follows the Twee 3 specification and treats `StoryData` as the canonical project contract.

If `StoryData` is missing or malformed, Knot emits a warning diagnostic and falls back to heuristic detection.

Example:

> Missing StoryData passage. Assuming SugarCube 2.

## Format Resolution Order

Workspace-wide resolution:

| Priority | Source                   | Purpose                             |
| -------- | ------------------------ | ----------------------------------- |
| 1        | StoryData passage        | Story format, entry point, IFID     |
| 2        | knot.json / project.json | Knot-specific tooling configuration |
| 3        | Heuristic scan           | Fallback inference                  |
| 4        | Default                  | SugarCube 2                         |

`.vscode/knot.json` never overrides the story format declared by `StoryData`.

Knot-specific workspace configuration is loaded from:

```text
.vscode/knot.json
```

This file is reserved exclusively for IDE tooling preferences and never influences story compilation.

Configuration includes:

* Compiler paths
* Build configuration
* Diagnostic severities
* Ignore lists
* Tooling preferences

## StoryData Discovery

Knot searches the workspace for a valid `StoryData` passage.

Rules:

* Exactly one `StoryData` passage must exist
* The first valid `StoryData` becomes authoritative
* Duplicate `StoryData` passages generate diagnostics
* Missing `StoryData` triggers heuristic fallback mode

## StoryData Entry Point

Reachability analysis uses the `start` field declared in `StoryData`.

```json
:: StoryData
{
  "ifid": "A1B2C3D4-E5F6-7890-1234-567890ABCDEF",
  "format": "SugarCube",
  "format-version": "2.36.1",
  "start": "Prologue"
}
```

If `start` is omitted:

* Knot defaults to `Start`

If the declared start passage does not exist:

* Knot emits an error diagnostic on the `StoryData` passage

Example:

> Start passage 'Prologue' not found in workspace.

## Metadata Passages

`StoryData` and `StoryTitle` are treated as metadata passages.

They do not contribute:

* Narrative links
* Variable operations
* Flow edges
* Semantic narrative content

## StoryData Validation

Knot validates StoryData against the supported format registry.

Validation includes:

* Format existence
* IFID correctness
* JSON validity
* Entry point existence
* Tag-color schema validation

Tag colors declared in `StoryData` are forwarded to client-side graph rendering and visualization systems.

## Unsupported Formats

If `StoryData.format` specifies a format outside Knot's supported registry:

* The Core Engine does not initialize
* Graph analysis is disabled
* Semantic diagnostics are withheld
* Only TextMate grammar highlighting remains active

Knot enters passive mode and surfaces a single diagnostic on the `StoryData` passage.

Example:

> Unsupported story format: "MyCustomFormat". Knot analysis is disabled.

Supported formats:

* SugarCube
* Harlowe
* Chapbook
* Snowman

---

# Special Passage System

Knot maintains a format-aware special passage registry similar to its system macro registry.

This allows each format plugin to declare canonical passage behaviors defined by the story format itself.

Examples include:

* SugarCube → StoryInit, StoryCaption, StoryMenu, PassageReady
* Harlowe → startup and metadata passages
* Chapbook → engine-defined utility passages

The registry is owned by the format plugin layer rather than the Core Engine.

## Universal Metadata Passages

The Core Engine recognizes only two universal metadata passages:

* StoryData
* StoryTitle

These passages are excluded from:

* Narrative graph construction
* Link extraction
* Variable flow analysis
* Runtime traversal semantics

## Format-Specific Special Passages

Format plugins may declare special passage definitions through a structured registry.

Example:

```rust
struct SpecialPassageDef {
    name: String,
    behavior: SpecialPassageBehavior,
    contributes_variables: bool,
    participates_in_graph: bool,
    execution_priority: Option<i32>,
}
```

This allows plugins to define:

* Startup execution order
* Variable initialization semantics
* Visibility rules
* Graph participation behavior
* Build-time handling
* Semantic token overrides

The Core Engine remains format-agnostic and consumes only normalized metadata exposed by the plugin.

## Passage Classification

Special passages are still represented through the shared `Passage` structure.

```rust
is_special: true
```

However, their runtime meaning is delegated entirely to the owning format plugin.

For example:

* SugarCube may treat `StoryInit` as a pre-entry execution node contributing variable initialization state
* `StoryCaption` may be excluded from reachability diagnostics
* `PassageReady` may inject execution hooks into flow analysis

The Core Engine never hardcodes these semantics directly.

## Plugin Sovereignty

All execution-order behavior, startup semantics, and special-passage lifecycle rules remain within the owning format plugin.

This preserves:

* Strict format isolation
* Accurate engine semantics
* Extensible format behavior
* Core Engine neutrality

---

# Format Plugin System

Each story format is implemented as an isolated parser module.

## Plugin Responsibilities

A plugin must:

* Parse passage boundaries
* Extract links
* Detect variable reads/writes
* Generate semantic tokens
* Produce format-specific diagnostics

The plugin does not understand workspace structure or other files.

---

## Fault-Tolerant Parsing

All parsers must support incomplete and invalid syntax during live editing.

### Recovery Rules

* Passage boundaries act as parser synchronization points
* Errors inside one passage must not corrupt subsequent passages
* Incomplete macros and strings generate partial AST nodes
* The lexer must never hard-fail

Example:

```rust
Node {
    is_incomplete: true
}
```

Incomplete nodes are excluded from graph analysis until they become valid.

---

## Benefits

This architecture allows new formats to inherit existing graph analysis automatically.

Adding a new format only requires:

1. A parser
2. Link extraction
3. Variable extraction
4. Semantic token generation

All graph-level tooling becomes immediately available.

---

# SugarCube Macro Discovery

Custom macro discovery is implemented entirely inside the SugarCube format plugin.

The Core Engine does not perform JavaScript parsing or regex-based macro scanning.

The SugarCube plugin analyzes:

* Script passages
* Workspace `.js` files
* User-defined macro registration helpers

The plugin identifies patterns such as:

* `Macro.add()`
* `Macro.delete()`
* `registerMacros()`
* Custom wrapper registration functions

Discovered symbols are reported back through the standard plugin symbol interface.

```rust
struct SugarCubeSymbols {
    custom_macros: Vec<MacroDef>,
}
```

This architecture keeps JavaScript runtime semantics isolated within the SugarCube plugin boundary.

JavaScript analysis runs asynchronously to avoid blocking passage parsing.

---

# Real-Time Editing Pipeline

Knot is designed around incremental, non-blocking updates.

## Editing Workflow

### 1. File Change

The editor sends a `didChange` event.

### 2. Debounce

The server waits briefly to avoid excessive recomputation.

### 3. Incremental Parse

Only modified passages are re-parsed.

### 4. Graph Surgery

Graph updates are performed incrementally:

* Remove edges from modified or deleted passages
* Insert edges from updated passages
* Preserve unaffected graph structure

The graph is updated in-place rather than rebuilt entirely.

### 5. Analysis Invalidation

Only affected graph regions are recomputed.

#### Reachability

BFS is rerun only if entry-point connectivity changes.

#### SCC / Loop Detection

Tarjan recomputation is limited to the affected connected component.

#### Variable Flow

Dataflow analysis is rerun only on reachable subgraphs impacted by the edit.

### 6. Diagnostics Return

Updated diagnostics and semantic tokens are returned to VS Code.

---

# Graph Mathematics Engine

## Passage Graph

The workspace is represented as a directed graph:

* Nodes → passages
* Edges → links between passages

This graph powers all structural analysis features.

---

## Loop Detection

Knot uses Tarjan’s Strongly Connected Components algorithm to identify traversal cycles.

Potential infinite loops are flagged when:

* A cycle exists
* No state mutation occurs inside the cycle

This prevents false positives in state-driven loops.

---

## Unreachable Passage Detection

Starting from the configured entry point, Knot walks the graph and identifies passages that cannot be reached during execution.

These passages are surfaced as informational diagnostics.

---

## Variable Flow Analysis

Knot performs forward dataflow analysis to track variable initialization across passage paths.

This enables diagnostics such as:

> Variable `$gold` may be used before initialization.

This feature is primarily targeted at SugarCube and Snowman, where state flow is statically traceable.

---

# Multi-Format Variable Models

Different Twine formats use fundamentally different execution models.

Knot abstracts this using per-format variable tracking implementations.

| Format    | Cross-Passage Tracking |
| --------- | ---------------------- |
| SugarCube | Full support           |
| Snowman   | Full support           |
| Harlowe   | Partial support        |
| Chapbook  | Unsupported            |

This avoids unreliable diagnostics in formats that cannot be safely analyzed statically.

---

# Language Server Capabilities

Knot implements a comprehensive set of standard LSP features alongside its custom extensions. The server advertises the following capabilities during the `initialize` handshake.

## Standard LSP Methods

| Method | Description |
|---|---|
| `textDocument/completion` | Context-aware completion: passage names (`[[`), variables (`$`), SugarCube macros (`<<`). Supports snippets, sort/filter text, and `completionItem/resolve`. |
| `textDocument/hover` | Passage metadata hover: link count, variable count, tags, incoming links. |
| `textDocument/declaration` | Same as definition for Twine — navigates from links to passage headers. |
| `textDocument/definition` | Go-to-definition: navigates from `[[link]]` to the target passage header. |
| `textDocument/typeDefinition` | Navigates to the StoryData passage (the type declaration for the story). |
| `textDocument/implementation` | Shows all passages that link to the passage under the cursor (reverse navigation). |
| `textDocument/references` | Finds all references to a passage: header definition + all link occurrences. |
| `textDocument/rename` | Renames a passage across all definitions and references. Supports `prepareRename`. |
| `textDocument/documentSymbol` | Lists all passages in a document as symbols with tag details. |
| `workspace/symbol` | Workspace-wide passage search with query filtering. |
| `textDocument/signatureHelp` | SugarCube macro signature information with parameter hints. |
| `textDocument/codeAction` | Quick-fix actions: create missing passage, initialize variable, add content template. |
| `textDocument/codeLens` | Inline lens above passage headers showing link/reference counts. |
| `textDocument/inlayHint` | Variable initialization state hints at passage entry points. |
| `textDocument/foldingRange` | Foldable passage body regions. |
| `textDocument/documentLink` | Clickable passage links that navigate to the target passage. |
| `textDocument/selectionRange` | Hierarchical selection: link text → full link → passage body. |
| `textDocument/formatting` | Basic Twee formatting: normalize headers, trim whitespace, blank lines between passages. |
| `textDocument/rangeFormatting` | Range-restricted formatting. |
| `textDocument/onTypeFormatting` | Auto-close `[[` with `]]` and `<<` with `>>`. |
| `textDocument/linkedEditingRange` | Linked editing: rename passage name in header and all matching links simultaneously. |
| `textDocument/prepareCallHierarchy` | Prepare call hierarchy for passage navigation. |
| `callHierarchy/incomingCalls` | Incoming navigation: passages that link to the current passage. |
| `callHierarchy/outgoingCalls` | Outgoing navigation: passages linked from the current passage. |
| `textDocument/diagnostic` | Pull diagnostics model alongside existing push model. |
| `textDocument/semanticTokens/full` | Full semantic token highlighting for 10 token types and 4 modifiers. |

## Diagnostic Features

Knot provides 13 configurable diagnostic categories:

| Category | Default Severity | Description |
|---|---|---|
| `broken-link` | Warning | Links to non-existent passages |
| `unreachable-passage` | Hint | Passages unreachable from start |
| `infinite-loop` | Warning | Cycles with no state mutation |
| `uninitialized-variable` | Warning | Variables used before initialization |
| `unused-variable` | Hint | Variables written but never read |
| `redundant-write` | Hint | Variables written twice without intervening read |
| `duplicate-passage-name` | Error | Multiple passages with the same name |
| `empty-passage` | Hint | Passages with no body content |
| `dead-end-passage` | Info | Passages with no outgoing links |
| `invalid-passage-name` | Warning | Passage names with problematic characters |
| `orphaned-passage` | Info | Passages with only one incoming link |
| `complex-passage` | Hint | Passages with 6+ outgoing links |
| `large-passage` | Hint | Passages exceeding 500 words |

All diagnostics include `related_information` pointing to the source of the issue (e.g., the broken link location for broken-link diagnostics).

## VS Code Client Surface

The VS Code extension integrates with native editor APIs:

| API | Description |
|---|---|
| **Decorations API** | Gutter badges on passage headers, faded unreachable passages, wavy underlines on broken links |
| **Language Status API** | Native language status indicator showing format, passage count, broken/unreachable counts |
| **Task Provider** | `knot` task type with `build` and `watch` tasks, integrated with `Ctrl+Shift+B` |
| **Story Map Webview** | Cytoscape.js-based interactive graph visualization |
| **Debug View** | Passage inspection panel with trace, step-over, breakpoints, and variable watch |
| **Profile View** | Workspace statistics with complexity metrics and structural balance analysis |
| **Play Mode** | In-editor story preview with auto-rebuild, history sidebar, and debug panel |

---

# Knot-Specific LSP Extensions

Knot extends the standard Language Server Protocol with custom requests and notifications.

## Graph Export

```ts
interface KnotGraphRequest {
    method: 'knot/graph';
    params: {
        workspaceUri: string;
    };
}

interface KnotGraphResponse {
    nodes: Array<{
        id: string;
        label: string;
        file: string;
        line: number;
    }>;

    edges: Array<{
        source: string;
        target: string;
    }>;

    layout?: 'dagre' | 'force';
}
```

## Indexing Progress

```ts
interface KnotIndexProgress {
    method: 'knot/indexProgress';
    params: {
        totalFiles: number;
        parsedFiles: number;
    };
}
```

## Build Request

```ts
method: 'knot/build'
```

Triggers compilation and streams compiler output back through notifications.

## Play Request

```ts
method: 'knot/play'
```

Returns the compiled HTML entry point for embedded preview webviews.

---

# Story Map & Visual Navigation

Knot includes a live interactive graph visualization system.

Features include:

* Passage relationship visualization
* Click-to-navigate nodes
* Structural overview
* Link inspection
* Workspace-scale narrative mapping

The visualization updates in real time as the story changes.

---

# Build & Compiler Integration

Knot acts as a front-end orchestration layer for external Twine compilers.

Supported targets may include:

* Tweego
* Extwee
* Twee2

Users can configure custom build pipelines while maintaining integrated diagnostics and project tooling.

---

# Configuration & Diagnostic Control

Knot provides configurable analysis severity and suppression systems.

Example configuration:

```json
{
  "diagnostics": {
    "broken-link": "error",
    "unreachable-passage": "warning",
    "uninitialized-variable": "off"
  }
}
```

Inline suppression directives are also supported.

---

# Performance SLAs

| Operation                           | Target | Max Tolerance |
| ----------------------------------- | ------ | ------------- |
| Initial workspace index (500 files) | <2s    | <5s           |
| Incremental update after keystroke  | <50ms  | <100ms        |
| Go-to-definition response           | <20ms  | <50ms         |
| Graph export for Story Map          | <100ms | <300ms        |
| Memory footprint                    | <200MB | <500MB        |

These targets serve as regression thresholds during CI validation.

---

# Quality Assurance

## Parser Tests

Snapshot-based AST tests using the `insta` crate.

## Graph Tests

Property-based graph invariant testing using `proptest`.

## LSP Integration Tests

The Rust server is launched in `--stdio` mode and validated through JSON-RPC interaction tests.

## VS Code Integration Tests

Client-side command and webview tests use `@vscode/test-electron`.

---

# Development Roadmap

## Phase 0 — Rust Infrastructure

* Rust LSP scaffold
* SugarCube parser parity
* Existing feature migration
* Logging and crash recovery

---

## Phase 0.5 — Compatibility Bridge

The TypeScript language server remains available behind a feature flag.

```json
"knot.experimental.rustServer": true
```

Both servers run side-by-side in CI and diagnostic output is diffed to verify parity.

---

## Phase 1 — Graph Core

* Global graph engine
* Broken link diagnostics
* Reachability analysis
* Loop detection

---

## Phase 2 — Story Visualization

* Interactive graph webview
* Passage navigation
* Graph export APIs

---

## Phase 3 — Variable Analysis

* Dataflow engine
* Cross-passage variable diagnostics
* State-aware analysis

---

## Phase 4 — Format Expansion

* Harlowe support
* Chapbook support
* Snowman support

---

## Phase 5 — IDE Features

* In-editor play mode
* Debug tooling
* Advanced linting
* Workspace profiling

---

## Migration Strategy

### Compatibility Window

The Rust server must maintain diagnostic parity with the TypeScript server during the transition period.

### Feature Flag Rollout

Rust remains opt-in until stability targets are met.

### Legacy Sunset

The TypeScript server remains available as a fallback for one major release after Rust becomes default.

---

# Positioning

Knot is designed to move beyond traditional Twine syntax tooling into a complete narrative engineering platform.

Its primary differentiators are:

* Graph-native architecture
* Real-time structural analysis
* Cross-format unification
* Incremental graph recomputation
* Advanced narrative diagnostics
* Integrated visual story tooling

The long-term objective is to establish Knot as the standard professional IDE for interactive fiction development within VS Code.
