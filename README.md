# knot Language Server

Professional language tooling for [Twine](https://twinery.org/) / [SugarCube 2](https://www.motoslave.net/sugarcube/2/) interactive fiction development. Works with `.tw` and `.twee` files.

## Features

### Core Language Intelligence

**Smart Diagnostics** — Real-time warnings for unknown macros, broken passage links, undefined variables, type mismatches, and StoryData validation errors, including cross-file analysis across your whole project.

**Go to Definition (F12)** — Jump to macro definitions, passage declarations, variable assignments, widget definitions, and JavaScript globals. Supports both `Ctrl+Click` and keyboard shortcut.

**Find References** — Find all usages of passages, story variables (`$var`), and custom macros/widgets across your entire workspace.

**Rename Symbol (F2)** — Safely rename passages, story variables, and custom macros with workspace-wide refactoring support. All links and references are automatically updated.

**IntelliSense Autocomplete** — Context-aware completions for:
- All built-in SugarCube macros (with auto-inserted close tags for block macros like `<<if>>...<</if>>`)
- Passage names in links `[[link->PassageName]]`
- Story variables (`$storyVar`) and temporary variables (`_tempVar`)
- Property access completion (`$player.stats.hp` shows available properties)
- Custom widgets and macros defined via `<<widget>>` or `Macro.add()`
- JavaScript globals defined in your script passages

**Hover Documentation** — Rich hover tooltips showing:
- Variable types and definitions with reference counts
- Macro documentation and usage hints
- Passage metadata with incoming link information
- Property path type inference (`$player.stats.hp` shows inferred type at each level)
- JavaScript global type information

**Type Inference** — Automatic type tracking for story variables. Hover over `$player.stats.hp` to see the inferred type chain from your `StoryInit` assignments. Supports nested objects and arrays.

**Semantic Highlighting** — Enhanced syntax highlighting powered by semantic analysis:
- Functions (macros, widgets)
- Classes (passages)
- Variables (story and temp vars)
- Operators (SugarCube operators)
- Strings, numbers, comments

### Document & Workspace Features

**Syntax Highlighting** — Full embedded JavaScript highlighting inside `[script]` passages and `<<script>>` blocks, CSS inside `[stylesheet]` passages, and SugarCube expression colouring inside macro arguments.

**Code Folding** — Fold passages, macro blocks (`<<if>>`, `<<for>>`, etc.), and comment blocks for easier navigation of large files.

**Outline View** — The VS Code Outline panel shows all passages in the current file, with `$variable` assignments and `<<widget>>` definitions listed as children.

**Workspace Symbols (Ctrl+T)** — Quick symbol search across all passages, widgets, macros, and story variables in your project.

**Workspace Awareness** — Full project understanding across all `.tw` and `.twee` files. Custom macros defined via `Macro.add()` in your Story JavaScript and widgets defined with `<<widget>>` are recognised everywhere.

**Document Symbols** — Per-file symbol extraction for breadcrumbs and outline navigation.

### Code Actions

**Quick Fixes** — Auto-generate missing passages when you reference an undefined passage link. Click the lightbulb or use `Ctrl+.` to create a new passage stub instantly.

### Build & Development Tools

**Tweego Integration** — Full integration with the Tweego CLI compiler:
- **Build** (`Ctrl+Shift+B`) — Compile your story to HTML with configurable output path
- **Watch Mode** — Automatically rebuild on file changes
- **Test Mode** — Build with test-specific configurations
- **Format Listing** — View available story formats installed on your system
- Configurable via settings: `tweego.path`, `tweego.outputFile`, `tweego.formatOverride`, `tweego.modulePaths`, `tweego.headFile`, `tweego.noTrim`, `tweego.logFiles`, `tweego.extraArgs`

**Custom Commands**:
| Command | Keybinding | Description |
|---|---|---|
| **knot: Go to Passage** | `Ctrl+Alt+P` (Mac: `Cmd+Alt+P`) | Quick-pick navigation to any passage with incoming link preview |
| **knot: Build** | `Ctrl+Shift+B` (Mac: `Cmd+Shift+B`) | Compile story with Tweego |
| **knot: Build (Test Mode)** | — | Build with test configuration |
| **knot: Start Watch Mode** | — | Enable automatic rebuilding on save |
| **knot: Stop Watch Mode** | — | Disable watch mode |
| **knot: List Available Story Formats** | — | Show installed Tweego story formats |
| **knot: Refresh All Documents** | — | Re-index the entire workspace |
| **knot: Restart Language Server** | — | Restart the server process |
| **knot: Show Language Server Output** | — | Open the output channel for debug logs |
| **knot: Open Settings** | — | Open knot configuration panel |
| **knot: Open Menu** | — | Unified command palette for all knot actions |

**Unified Status Bar Menu** — Click the `knot` status bar item to access a unified menu with all commands, passage navigation, build tools, and settings.

### Query API

The language server exposes custom LSP requests for external tooling:
- `knot/getPassages` — Retrieve all passages with metadata (name, URI, reference count, incoming links)
- `knot/getStoryData` — Get StoryData metadata (IFID, format, format version, start passage, total passage count)
- `knot/storyDataUpdated` — Notification broadcast when story data changes

## Quick Start

1. Open a folder containing your `.tw` or `.twee` files
2. The server activates automatically
3. The status bar shows `⟳ knot` while indexing and `✓ knot` when ready — click it to access output logs and server actions

## Example

```twee
:: StoryInit [startup]
<<set $player = {
  name: "Alex",
  stats: { hp: 100, mp: 50 }
}>>

:: Start
Welcome, <<print $player.name>>!
<<if $player.stats.hp gt 80>>
  You feel healthy.
<</if>>
[[Explore->Forest]]

:: Forest
The trees are dense.
[[Return->Start]]
```

## Supported File Types

- `.tw` — Twine source files
- `.twee` — Twee source files

Passages using SugarCube 2 syntax are supported in both Twine 2 editor exports and [tweego](https://www.motoslave.net/tweego/) CLI projects.

## Configuration

All settings are prefixed with `knot.`:

### Tweego Settings
| Setting | Type | Default | Description |
|---|---|---|---|
| `tweego.path` | string | `"tweego"` | Path to the Tweego executable |
| `tweego.outputFile` | string | `"dist/index.html"` | Output file path for compiled HTML |
| `tweego.formatOverride` | string | `""` | Story format ID to use, overriding `StoryData` |
| `tweego.modulePaths` | array | `[]` | Module directories (-m): CSS, JS, fonts bundled into `<head>` |
| `tweego.headFile` | string | `""` | File whose contents are appended to `<head>` (--head) |
| `tweego.noTrim` | boolean | `false` | Disable whitespace trimming (--no-trim) |
| `tweego.logFiles` | boolean | `false` | Log all processed input files (--log-files) |
| `tweego.extraArgs` | string | `""` | Extra arguments appended verbatim (e.g. `--twee2-compat`) |

## Development

```bash
# Install dependencies
npm install
npm --prefix client install
npm --prefix server install

# Build
npm run build

# Run tests
npm test

# Package
npm run package:vsix

# Watch mode (development)
npm run watch:client
npm run watch:server
```

## Architecture

- **Client** (`client/`) — VS Code extension providing UI integration, status bar, commands, and Tweego build tools
- **Server** (`server/`) — Language Server Protocol implementation with:
  - Lexer and parser for SugarCube/Twee syntax
  - Workspace indexer for cross-file analysis
  - Type inference engine for story variables
  - Format adapters for extensible story format support
  - Handler modules for LSP features (completion, hover, definition, etc.)

## License

MIT