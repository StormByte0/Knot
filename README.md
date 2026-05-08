# Knot — Twine IDE for VS Code

<p align="center">
  <strong>A next-generation integrated development environment for Twine and Twee interactive fiction projects.</strong>
</p>

---

## What is Knot?

Knot is a VS Code extension that transforms the experience of writing interactive fiction. Instead of treating Twine stories as isolated text files, Knot models your entire project as a **directed graph** — enabling real-time structural analysis, intelligent navigation, and narrative-aware diagnostics that no other Twine tooling provides.

The core language server is written in **Rust** for high-performance parsing, incremental graph recomputation, and low-latency response times. It communicates with VS Code through the Language Server Protocol (LSP), providing a rich set of standard and custom features.

### Key Differentiators

- **Graph-native architecture** — Your story is a directed graph, not a pile of files
- **Real-time structural analysis** — Broken links, unreachable passages, infinite loops detected as you type
- **Cross-format unification** — SugarCube, Harlowe, Chapbook, and Snowman all supported through a plugin system
- **Incremental graph recomputation** — Only affected regions are re-analyzed after each keystroke
- **Integrated visual story tooling** — Interactive Story Map, Play Mode, Debug View, and Profile View
- **Forward dataflow analysis** — Track variable initialization across passage boundaries

---

## Features

### Language Server (26 LSP Methods)

| Feature | Description |
|---------|-------------|
| **Completion** | Context-aware: passage names (`[[`), variables (`$`), SugarCube macros (`<<`) with snippets |
| **Hover** | Passage metadata: link count, variable count, tags, incoming references |
| **Go to Definition** | Navigate from `[[link]]` to the target passage header |
| **Go to Declaration** | Same as definition for Twine links |
| **Go to Implementation** | Find all passages that link *to* the current passage (reverse navigation) |
| **Go to Type Definition** | Navigate to the StoryData passage |
| **Find References** | All header definitions + link occurrences across the workspace |
| **Rename** | Rename a passage across all definitions and references simultaneously |
| **Document Symbols** | List all passages in a file as symbols with tag details |
| **Workspace Symbols** | Workspace-wide passage search with query filtering |
| **Signature Help** | SugarCube macro parameter hints with active parameter tracking |
| **Code Actions** | Quick-fix: create missing passage, initialize variable, add content template |
| **Code Lens** | Inline lens above passage headers showing link/reference counts |
| **Inlay Hints** | Variable initialization state annotations at passage entry points |
| **Folding Ranges** | Foldable passage body regions |
| **Document Links** | Clickable `[[links]]` that navigate to target passages |
| **Selection Range** | Hierarchical selection: link text → full link → passage body → passage header |
| **Formatting** | Normalize headers, trim whitespace, ensure blank lines between passages |
| **Range Formatting** | Range-restricted formatting |
| **On-Type Formatting** | Auto-close `[[` with `]]` and `<<` with `>>` |
| **Linked Editing** | Rename passage header and all matching links simultaneously |
| **Call Hierarchy** | Incoming/outgoing call hierarchy for passage navigation |
| **Pull Diagnostics** | Pull diagnostics model alongside the standard push model |
| **Semantic Tokens** | Full semantic highlighting for 10 token types and 4 modifiers |

### 13 Configurable Diagnostic Categories

| Diagnostic | Default | Description |
|-----------|---------|-------------|
| Broken Link | Warning | Links to non-existent passages |
| Unreachable Passage | Hint | Passages unreachable from the start passage |
| Infinite Loop | Warning | Cycles with no state mutation |
| Uninitialized Variable | Warning | Variables used before initialization |
| Unused Variable | Hint | Variables written but never read |
| Redundant Write | Hint | Variables written twice without an intervening read |
| Duplicate Passage Name | Error | Multiple passages with the same name |
| Empty Passage | Hint | Passages with no body content |
| Dead-End Passage | Info | Passages with no outgoing links |
| Invalid Passage Name | Warning | Passage names with problematic characters |
| Orphaned Passage | Info | Passages with only one incoming link |
| Complex Passage | Hint | Passages with 6+ outgoing links |
| Large Passage | Hint | Passages exceeding 500 words |

All diagnostics include **related information** pointing to the source of the issue (e.g., the broken link location for broken-link diagnostics).

### Custom LSP Extensions (11 Methods)

| Method | Description |
|--------|-------------|
| `knot/graph` | Export passage graph with node metadata, unreachable flags, and broken edge markers |
| `knot/build` | Invoke Tweego compiler, stream output via notifications |
| `knot/play` | Build and return compiled HTML path for in-editor preview |
| `knot/variableFlow` | Per-variable write/read locations, initialized-at-start, unused status |
| `knot/debug` | 17-field debug info for a passage (variables, links, loops, reachability) |
| `knot/trace` | DFS execution trace with loop detection (configurable max depth) |
| `knot/profile` | 20+ workspace statistics: complexity, balance, distribution, tag stats |
| `knot/compilerDetect` | PATH + configured path search for Tweego |
| `knot/breakpoints` | Set, clear, and list debug breakpoints per passage |
| `knot/stepOver` | Single-step: show outgoing choices and variable operations |
| `knot/watchVariables` | Variable state at passage entry with dataflow information |

### VS Code Integration

| Feature | Description |
|---------|-------------|
| **Decorations API** | Gutter badges on passage headers, faded unreachable passages, wavy underlines on broken links |
| **Language Status API** | Native status indicator showing format, passage count, broken/unreachable counts |
| **Task Provider** | `knot/build` and `knot/watch` tasks, integrated with `Ctrl+Shift+B` |
| **Story Map** | Interactive Cytoscape.js graph visualization with 4 layout algorithms, search/filter, click-to-navigate |
| **Play Mode** | In-editor story preview with auto-rebuild, passage history, debug panel, and keyboard navigation |
| **Debug View** | Passage inspection with trace execution, step-over, breakpoints, and variable watch |
| **Profile View** | Workspace statistics dashboard with complexity metrics, structural balance, and tag analysis |

### Format Support

| Format | Parsing | Variable Tracking | Special Passages |
|--------|---------|-------------------|-----------------|
| SugarCube 2 | Full | Full | StoryInit, StoryCaption, StoryMenu, PassageReady, etc. |
| Harlowe 3 | Full | Partial | startup, metadata passages |
| Chapbook 1 | Full | Unsupported | Engine utility passages |
| Snowman 1 | Full | Full | Script passages |

---

## Getting Started

### Prerequisites

- **VS Code** 1.85.0 or later
- **Rust toolchain** (for building from source) — edition 2024
- **Node.js** 20+ (for building the extension)
- **Tweego** (optional, for build/play features) — automatically detected

### Installation

#### From VSIX (Pre-built)

1. Download the appropriate VSIX for your platform from [Releases](https://github.com/StormByte0/Knot/releases)
2. In VS Code, open the Command Palette (`Ctrl+Shift+P` / `Cmd+Shift+P`)
3. Run **Extensions: Install from VSIX...**
4. Select the downloaded `.vsix` file

Platform-specific VSIX packages are available for:

| Platform | Target |
|----------|--------|
| Windows x64 | `win32-x64` |
| macOS Apple Silicon | `darwin-arm64` |
| macOS Intel | `darwin-x64` |
| Linux x64 | `linux-x64` |
| Linux ARM64 | `linux-arm64` |

#### From Source

```bash
# Clone the repository
git clone https://github.com/StormByte0/Knot.git
cd Knot
git checkout ver_2

# Build the Rust language server
cargo build --release -p knot-server

# Copy the binary to the extension directory
cp target/release/knot-server extensions/vscode/bin/

# Build and package the VS Code extension
cd extensions/vscode
npm install
npm run compile
npx vsce package
```

Then install the resulting `.vsix` file manually.

### First Use

1. Open a folder containing `.tw` or `.twee` files in VS Code
2. Knot activates automatically when it detects Twee files
3. The language server indexes your workspace and begins providing diagnostics
4. Open the **Story Map** from the Knot icon in the Activity Bar

---

## Build Instructions

### Building the Rust Server

```bash
# Debug build (faster compilation, slower runtime)
cargo build -p knot-server

# Release build (optimized, recommended)
cargo build --release -p knot-server
```

The binary is output at `target/debug/knot-server` or `target/release/knot-server`.

### Building the VS Code Extension

```bash
cd extensions/vscode

# Install dependencies
npm install

# Compile TypeScript
npm run compile

# Watch for changes during development
npm run watch
```

### Running Tests

```bash
# Run all Rust tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p knot-core
cargo test -p knot-formats
cargo test -p knot-server

# Run with output
cargo test --workspace -- --nocapture
```

### Linting & Formatting

```bash
# Check Rust formatting
cargo fmt --all -- --check

# Run Clippy lints
cargo clippy --workspace -- -D warnings

# Lint TypeScript
cd extensions/vscode
npm run lint
```

---

## Packaging & Distribution

### Packaging a Platform-Specific VSIX

Knot ships platform-specific VSIX packages because the Rust binary is compiled per target.

```bash
# 1. Build the Rust server for your target
cargo build --release -p knot-server --target <TARGET>

# 2. Copy the binary into the extension
cp target/<TARGET>/release/knot-server extensions/vscode/bin/

# 3. Package the VSIX
cd extensions/vscode
npx vsce package --target <VSCE-TARGET>
```

**Target mapping:**

| Rust Target | VSCE Target | OS |
|-------------|-------------|-----|
| `x86_64-pc-windows-msvc` | `win32-x64` | Windows |
| `aarch64-apple-darwin` | `darwin-arm64` | macOS Apple Silicon |
| `x86_64-apple-darwin` | `darwin-x64` | macOS Intel |
| `x86_64-unknown-linux-gnu` | `linux-x64` | Linux |
| `aarch64-unknown-linux-gnu` | `linux-arm64` | Linux ARM |

### Cross-Compilation (Linux ARM64)

```bash
# Install the cross-compilation toolchain
sudo apt-get install -y gcc-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-gnu

# Build
cargo build --release -p knot-server --target aarch64-unknown-linux-gnu
```

### CI/CD Pipeline

The project includes a GitHub Actions workflow (`.github/workflows/ci.yml`) that:

1. **Checks** the workspace compiles (`cargo check`)
2. **Runs tests** (`cargo test --workspace`)
3. **Validates formatting** (`cargo fmt --check`)
4. **Runs Clippy** with warnings as errors
5. **Builds release binaries** for all 5 platforms
6. **Packages platform-specific VSIX** artifacts and uploads them

The CI triggers on push to `ver_2` and `master` branches, and on pull requests to `ver_2`.

---

## Configuration

### VS Code Settings

Knot contributes the following settings, accessible via **Settings > Extensions > Knot**:

| Setting | Default | Description |
|---------|---------|-------------|
| `knot.experimental.rustServer` | `true` | Use the native Rust language server |
| `knot.server.path` | `""` | Override path to the knot-server binary |
| `knot.trace.server` | `"off"` | LSP communication trace level |
| `knot.diagnostics.*` | (varies) | Severity for each of the 13 diagnostic categories |

Each diagnostic category can be set to `"error"`, `"warning"`, `"info"`, `"hint"`, or `"off"`.

### Workspace Configuration (`.vscode/knot.json`)

Place a `knot.json` file in your workspace's `.vscode/` directory for project-specific settings:

```json
{
  "compiler_path": "/usr/local/bin/tweego",
  "build": {
    "output_dir": "dist",
    "flags": ["--module=scripts"]
  },
  "diagnostics": {
    "broken-link": "error",
    "unreachable-passage": "warning",
    "uninitialized-variable": "off"
  },
  "ignore_patterns": ["archived/**"],
  "format_override": "SugarCube"
}
```

> **Note:** `.vscode/knot.json` never overrides the story format declared by `StoryData`. It is reserved for IDE tooling preferences only.

### Keyboard Shortcuts

| Shortcut | Command | Condition |
|----------|---------|-----------|
| `F5` | Play Story | Twee files |
| `Shift+F5` | Play from This Passage | Twee files |
| `F6` | Build Project | Twee files |
| `Ctrl+Shift+M` / `Cmd+Shift+M` | Open Story Map | Twee files |

---

## Architecture

Knot is divided into two independent runtime layers:

| Layer | Responsibility |
|-------|---------------|
| **VS Code Client** (TypeScript) | UI, commands, webviews, decorations, status bar |
| **Rust Language Server** | Parsing, graph analysis, diagnostics, dataflow |

The Rust server communicates with VS Code over stdio using the Language Server Protocol. The TypeScript client bootstraps the correct platform-specific binary at activation time.

### Crate Structure

```
knot-core       — Unified document model, graph engine, analysis, editing
knot-formats    — Format plugin system (SugarCube, Harlowe, Chapbook, Snowman)
knot-server     — LSP server implementation (handlers, custom extensions, state)
```

### Key Design Principles

- **Single-project workspace model** — One Twine project per workspace, one StoryData passage
- **Format plugin isolation** — Each format is an isolated parser; the core engine is format-agnostic
- **Incremental graph surgery** — Graph updates are performed in-place, not rebuilt from scratch
- **Fault-tolerant parsing** — Parsers never hard-fail; incomplete syntax produces partial AST nodes
- **StoryData authority** — The StoryData passage is the canonical source for format, entry point, and IFID

For the full technical specification, see [ARCHITECTURE.md](./ARCHITECTURE.md).

---

## Performance Targets

| Operation | Target | Max Tolerance |
|-----------|--------|---------------|
| Initial workspace index (500 files) | <2s | <5s |
| Incremental update after keystroke | <50ms | <100ms |
| Go-to-definition response | <20ms | <50ms |
| Graph export for Story Map | <100ms | <300ms |
| Memory footprint | <200MB | <500MB |

---

## Project Structure

```
.
├── ARCHITECTURE.md          # Technical specification
├── Cargo.toml               # Workspace root
├── crates/
│   ├── knot-core/           # Document model, graph, analysis, editing
│   │   └── src/
│   │       ├── analysis.rs  # Dataflow engine + diagnostic detectors
│   │       ├── document.rs  # Unified document model
│   │       ├── editing.rs   # Incremental graph surgery
│   │       ├── graph.rs     # Passage graph + Tarjan SCC + BFS reachability
│   │       ├── passage.rs   # Core types (Passage, Link, VarOp, Block)
│   │       └── workspace.rs # Workspace model, config, StoryData validation
│   ├── knot-formats/        # Format plugin system
│   │   └── src/
│   │       ├── plugin.rs    # FormatPlugin trait + SemanticToken system
│   │       ├── sugarcube/   # SugarCube 2 parser
│   │       ├── harlowe/     # Harlowe 3 parser
│   │       ├── chapbook/    # Chapbook 1 parser
│   │       └── snowman/     # Snowman 1 parser
│   └── knot-server/         # LSP server
│       └── src/
│           ├── handlers.rs  # 26 LSP handlers + 11 custom methods + helpers
│           ├── lib.rs       # Server entry point + method registration
│           ├── lsp_ext.rs   # Custom knot/* request/response types
│           ├── main.rs      # Binary entry point
│           └── state.rs     # Server state (workspace, formats, debounce)
├── extensions/
│   └── vscode/              # VS Code extension
│       ├── src/
│       │   ├── extension.ts          # Activation, client, commands, status bar
│       │   ├── storyMapProvider.ts   # Cytoscape.js Story Map webview
│       │   ├── playModeProvider.ts   # In-editor story playtesting
│       │   ├── debugViewProvider.ts  # Passage debug sidebar
│       │   └── profileViewProvider.ts # Workspace profile sidebar
│       ├── syntaxes/        # TextMate grammar
│       └── package.json     # Extension manifest
└── .github/workflows/ci.yml # CI/CD pipeline
```

---

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for guidelines on how to contribute to Knot.

---

## License

This project is licensed under the **MIT License**. See [LICENSE](./LICENSE) for details.

---

## Links

- **Repository:** [https://github.com/StormByte0/Knot](https://github.com/StormByte0/Knot)
- **Issues:** [https://github.com/StormByte0/Knot/issues](https://github.com/StormByte0/Knot/issues)
- **Twine:** [https://tweecode.com](https://tweecode.com)
- **Tweego:** [https://www.motoslave.net/tweego/](https://www.motoslave.net/tweego/)
