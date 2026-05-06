# Knot — Twine Language Support for VS Code

A language server for Twine development with multi-format support: diagnostics, completions, go-to-definition, rename, semantic tokens, and Tweego build integration.

## Features

### Language Server Protocol
- **Completions** — Macro names, passage names, and variables with format-aware triggers
- **Hover** — Macro documentation, passage previews, variable scope info
- **Go to Definition** — Navigate to passage definitions, macro implementations
- **Find References** — Find all references to passages and variables
- **Rename** — Rename passages and variables across the workspace
- **Document Symbols** — Passage outline in the breadcrumb bar
- **Semantic Tokens** — Syntax highlighting for macros, variables, hooks, and passage headers
- **Document Links** — Clickable passage links in `[[brackets]]`
- **Code Actions** — Quick fixes for common issues

### Diagnostics
- Unknown passage references
- Duplicate passage names
- Unreachable passages (from start)
- Conditionally reachable passages
- Dead conditional branches (always true/false)
- Unknown macros (format-specific)
- Deprecated macros
- Structural errors (unclosed macros, mismatched tags)

### Story Format Support
- **SugarCube 2** — Full macro catalog (70+ macros), variables (`$/_`), widgets, snippets
- **Harlowe 3** — Full macro catalog (250+ macros), hook syntax, changers/commands/instants
- **Chapbook 2** — Inserts, modifiers, YAML front matter
- **Snowman 3** — Template blocks, Underscore.js/Snowman JS API
- **Fallback** — Basic `[[link]]` support for any format

Format auto-detection from `StoryData` passage.

### Analysis Pipeline
- **AST** — Hierarchical abstract syntax tree per passage
- **Control Flow Graph** — Per-passage CFG with variable state tracking
- **Story Flow Graph** — Cross-passage flow with special passage virtual edges (StoryInit, PassageHeader, StoryInterface)
- **Link Graph** — BFS reachability analysis
- **Virtual Documents** — Embedded JavaScript/CSS language features in `[script]`/`[stylesheet]` passages

### Tweego Build Integration
- Build, test build, verify installation
- Watch mode for auto-rebuild
- Configurable output path, format override, module paths

## Getting Started

1. Install the extension
2. Open a `.tw` or `.twee` file
3. Knot auto-detects the story format from `StoryData`
4. If no `StoryData` exists, use **Knot: Select Story Format** command

## Commands

| Command | Keybinding |
|---|---|
| Knot: Go to Passage | `Ctrl+Alt+P` |
| Knot: Build | `Ctrl+Alt+B` |
| Knot: Select Story Format | `Ctrl+Alt+F` |
| Knot: Restart Language Server | — |
| Knot: Refresh All Documents | — |
| Knot: List Available Story Formats | — |
| Knot: Show Language Server Output | — |
| Knot: Open Settings | — |
| Knot: Verify Tweego Installation | — |
| Knot: Build (Test Mode) | — |
| Knot: Start Watch Mode | — |
| Knot: Stop Watch Mode | — |
| Knot: Open Menu | — |

## Configuration

| Setting | Default | Description |
|---|---|---|
| `knot.format.activeFormat` | `""` (auto-detect) | Story format ID |
| `knot.format.formatsDirectory` | `.storyformats` | Directory for custom format packages |
| `knot.lint.unknownPassage` | `warning` | Severity for unknown passage targets |
| `knot.lint.unknownMacro` | `warning` | Severity for unknown macro names |
| `knot.lint.duplicatePassage` | `error` | Severity for duplicate passage names |
| `knot.lint.unreachablePassage` | `warning` | Severity for unreachable passages |
| `knot.lint.containerStructure` | `error` | Severity for structural errors |
| `knot.lint.deprecatedMacro` | `warning` | Severity for deprecated macros |
| `knot.tweego.path` | `tweego` | Path to Tweego executable |
| `knot.tweego.outputFile` | `dist/index.html` | Output HTML file path |

## Architecture

Knot v2 uses a strict layered architecture with zero format bleed-through:

```
package.json → Client/Server → Core/Handlers → FormatModules
```

Core and handlers never import from format directories — all format-specific data flows through the `FormatModule` interface and `FormatRegistry`. See `design/` directory for full architecture documentation.

## Development

```bash
# Build
npm run build

# Watch
npm run watch:client
npm run watch:server

# Test
npm test
```

## License

MIT
