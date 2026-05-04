# Knot

Language support for [Twine](https://twinery.org/) / [SugarCube 2](https://www.motoslave.net/sugarcube/2/) development in VS Code. Works with `.tw` and `.twee` files.

This is an early preview. Some features are incomplete or may have rough edges — see [Current Limitations](#current-limitations) below.

## Full Documentation

See the [complete feature documentation](https://stormbyte0.github.io/Knot/) for every feature, configuration reference, and tips.

## What It Does

**Diagnostics** — Warns about unknown macros, broken passage links, type mismatches in comparisons, and duplicate passage names across your project. Six lint rules are individually configurable &mdash; set severity to error, warning, info, or off via `knot.lint.*` settings.

**Go to Definition (F12)** — Jump to passage declarations, `<<widget>>` definitions, `Macro.add()` calls in script passages, and variable assignments. Works across files.

**Find All References** — Find where a passage is linked, where a story variable (`$var`) is used, or where a custom macro is called.

**Rename (F2)** — Rename passages and story variables. Updates references across all files in the workspace.

**Autocomplete** — Completions for:
- Built-in SugarCube macros (with snippet bodies for block macros)
- Passage names inside `<<goto "">>`, `<<link "">>`, etc.
- Story variables (`$var`) and their properties (e.g. `$player.stats.`)
- Custom widgets and macros
- Close-tag suggestions (`<</`) for currently open block macros

**Hover** — Hover over a macro to see its description. Hover over `$var` to see its inferred type and where it was defined. Hover over a passage link to see incoming reference counts.

**Type Inference** — Tracks types assigned to story variables via `<<set>>`. When you write `<<set $player = {name: "Alex", hp: 100}>>`, the extension remembers the shape so property completions on `$player.` work later. This is best-effort — complex expressions, mutations through `<<run>>`, and dynamically constructed objects may not be tracked accurately.

**Semantic Highlighting** — Macros, passages, variables, and SugarCube operators get distinct colors beyond what the TextMate grammar provides.

**Outline & Workspace Symbols** — Passages, widgets, and variable assignments appear in the Outline panel. `Ctrl+T` searches across all files.

**Code Actions** — "Create missing passage" quick fix when you reference a passage that doesn't exist yet.

**Tweego Integration** — Build, watch, and test your project via the [Tweego](https://www.motoslave.net/tweego/) CLI. Configurable output path, module directories, format override, and extra arguments.

## Commands

| Command | Shortcut | What it does |
|---|---|---|
| Go to Passage | `Ctrl+Alt+P` | Quick-pick to any passage |
| Build | `Ctrl+Alt+B` | Run tweego to compile |
| Build (Test Mode) | — | Build with test configuration |
| Start/Stop Watch | — | Auto-rebuild on file changes |
| Verify Tweego | — | Check if tweego is found and working |
| List Story Formats | — | Show available formats |
| Refresh All Documents | — | Re-index the workspace |
| Restart Language Server | — | Restart the server |
| Show Output | — | Open the log output channel |
| Open Settings | — | Open knot settings |
| Open Menu | — | Unified command palette |

## Quick Start

1. Open a folder with `.tw` or `.twee` files
2. The extension activates automatically
3. The status bar shows indexing progress — click it for logs

## Example

```twee
:: StoryInit
<<set $player = {
  name: "Alex",
  stats: { hp: 100, mp: 50 }
}>>

:: Start
Welcome, <<print $player.name>>!
<<if $player.stats.hp gt 80>>
  You feel healthy.
<</if>>
[[Go exploring->Forest]]

:: Forest
The trees close in around you.
[[Return->Start]]
```

## Configuration

All settings are under `knot.`:

### Tweego

| Setting | Default | Description |
|---|---|---|
| `knot.tweego.path` | `"tweego"` | Path to the tweego executable |
| `knot.tweego.outputFile` | `"dist/index.html"` | Compiled HTML output path (relative to workspace) |
| `knot.tweego.formatOverride` | `""` | Override the story format from StoryData |
| `knot.tweego.modulePaths` | `[]` | Module directories (`-m`) bundled into `<head>` |
| `knot.tweego.headFile` | `""` | File appended to `<head>` (`--head`) |
| `knot.tweego.noTrim` | `false` | Disable whitespace trimming (`--no-trim`) |
| `knot.tweego.logFiles` | `false` | Log processed files (`--log-files`) |
| `knot.tweego.extraArgs` | `""` | Extra CLI arguments (e.g. `--twee2-compat`) |

### Project

| Setting | Default | Description |
|---|---|---|
| `knot.project.storyFilesDirectory` | `"src"` | Source directory passed to tweego |
| `knot.project.storyFormatsDirectory` | `".storyformats"` | Story format packages (added to `TWEEGO_PATH`) |
| `knot.project.include` | `[]` | Directories to include in indexing (empty = all) |
| `knot.project.exclude` | `[]` | Glob patterns to exclude from indexing |

### Linting

| Setting | Default | Description |
|---|---|---|
| `knot.lint.unknownPassage` | `"warning"` | Severity for links to undefined passages |
| `knot.lint.unknownMacro` | `"warning"` | Severity for unrecognized macro names |
| `knot.lint.duplicatePassage` | `"error"` | Severity for duplicate passage definitions |
| `knot.lint.typeMismatch` | `"error"` | Severity for type mismatches in comparisons |
| `knot.lint.unreachablePassage` | `"warning"` | Severity for passages unreachable from start |
| `knot.lint.containerStructure` | `"error"` | Severity for invalid macro nesting |

All lint settings accept: `"error"`, `"warning"`, `"info"`, or `"off"`.

## Current Limitations

- **SugarCube 2 only.** Harlowe, Chapbox, and other story formats are not supported. The format adapter system exists for future expansion, but no other adapters are implemented yet.
- **Type inference is best-effort.** It tracks `<<set $var to ...>>` and `<<set $var = ...>>` assignments. Variables mutated through `<<run>>`, JavaScript, or indirect assignment may lose their inferred type. There is no type annotation syntax — types are inferred from assignment patterns only.
- **No story map or graph view.** There is no visual passage-link graph. The Outline panel and `Go to Passage` command are the current navigation options.
- **No debugger.** There is no interactive debugging support for SugarCube stories.
- **Configurable diagnostics.** Six lint rules (`knot.lint.*`) can be set to error, warning, info, or off. Additional always-on checks (deprecated macros, missing required arguments, assignment target errors, StoryData validation, JavaScript syntax) cannot be suppressed.
- **Incremental parsing is passage-granular, not character-granular.** After an edit, the affected passage is re-parsed, then the whole workspace is reanalyzed. This is fast for small-to-medium projects but may lag on very large workspaces.

## Supported File Types

- `.tw`
- `.twee`

Compatible with Twine 2 editor exports and [tweego](https://www.motoslave.net/tweego/) CLI projects using SugarCube 2 syntax.

## Development

```bash
npm install
npm --prefix client install
npm --prefix server install

npm run build          # build client + server
npm test               # run server tests
npm run package:vsix   # build + package as .vsix
```

## License

MIT
