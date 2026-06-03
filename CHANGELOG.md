# Change Log

## 1.0.1 — Bug Fix

### Fixed
- Setting knot.lint.unreachablePassage to "off" now correctly suppresses unreachable passage warnings. Previously the setting was ignored and warnings always appeared.
- The "info" severity setting for lint rules now displays as info instead of falling through to warning.

## 1.0.0

First stable release. Full documentation available at [stormbyte0.github.io/Knot](https://stormbyte0.github.io/Knot/).

### Language Features

- **Smart Completions** — 50+ built-in SugarCube macro snippets with placeholder arguments, passage name completions inside macro arguments, story/temp variable completions with inferred type info, property chain completions (`$player.stats.`), close-tag suggestions (`<</`) using a stack-based nesting algorithm, and custom widget/Macro.add() detection
- **Go to Definition (F12)** — Jump to passage declarations, `<<widget>>` definitions, `Macro.add()` calls in script passages, variable assignments, and JS global declarations. Works across all workspace files
- **Find All References (Shift+F12)** — Find every reference to a passage, variable, or macro/widget across the entire workspace, including implicit references via `data-passage`, `Engine.play()`, and `Story.get()`
- **Rename (F2)** — Rename passages, variables, and widgets across all files. Updates `[[links]]`, macro arguments, passage headers, and definitions in a single atomic operation
- **Hover** — Rich hover information for macros (description, deprecation), variables (inferred type table, definition location, reference count), passages (incoming links grouped by source), and SugarCube globals (API description)
- **Type Inference** — Automatically infers types from `<<set>>` assignments. Supports `number`, `string`, `boolean`, `null`, `object` (with full property trees), and `array` (with element type). StoryInit is analyzed first, then special passages, then regular passages
- **Dynamic Passage Resolution** — Resolves variable-based navigation like `<<goto $destination>>` by tracking variable-to-string assignments workspace-wide. Includes `data-passage`, `Engine.play()`, and `Story.get()` implicit reference detection
- **Macro.add() Detection** — Detects `Macro.add()` calls in JavaScript using acorn AST walking. Detected macros receive full language support: completions, hover, go-to-definition, and find references

### Diagnostics

- **9 configurable diagnostic rules** with per-rule severity settings (`knot.lint.*`):
  - `unknownPassage` — links to undefined passages
  - `unknownMacro` — unrecognized macro names
  - `duplicatePassage` — duplicate passage definitions
  - `unreachablePassage` — passages not reachable from start via BFS
  - `typeMismatch` — comparison operators with mismatched types
  - `containerStructure` — invalid macro nesting (`<<elseif>>` outside `<<if>>`, etc.)
- **Always-on checks** (not configurable):
  - Deprecated macro usage (e.g. `<<click>>`)
  - Missing required macro arguments
  - Invalid assignment targets in `<<set>>`
  - StoryData validation (missing/invalid IFID, nonexistent start passage)
  - JavaScript syntax errors in script passages and `<<script>>` blocks

### Code Actions

- **Create Missing Passage** — Quick fix for "Unknown passage target" diagnostics; creates a passage stub at the end of the current file
- **Generate IFID** — Context-sensitive action that generates a UUID v4 using `crypto.randomUUID()` when a StoryData passage lacks an IFID
- **Replace Invalid IFID** — Replaces an invalid IFID value with a newly generated UUID v4

### Editor

- **Syntax Highlighting** — TextMate grammar for passage headers, macro tags, variables, operators, links, HTML, strings, numbers, and comments. Plus 7 semantic token types for macros (function), passages (class), variables, operators, strings, numbers, and comments
- **Embedded Language Injection** — Full JavaScript syntax in script-tagged passages, Story JavaScript, and `<<script>>` bodies. Full CSS in stylesheet-tagged passages
- **Smart Indentation** — Auto-indent after `<<if>>`, `<<for>>`, `<<widget>>`, etc.
- **Bracket Auto-close** — `<<` → `>>`, `[[` → `]]`, plus standard brackets and quotes
- **Surrounding Pairs** — Select text + `[[` wraps as `[[selected text]]`
- **Code Folding** — Passage bodies, block comments, and block macros (individually foldable when nested)

### Build Integration

- **Tweego build/watch/test** — Build (`Ctrl+Alt+B`), watch mode, and test mode directly from VS Code
- **Inline diagnostics** — Build errors appear as inline VS Code diagnostics
- **Status bar** — Build button, watch toggle, LSP status indicator with passage count and detected format
- **Verification** — `knot.verifyTweego` confirms installation; `knot.listFormats` shows available formats

### Configuration

- **Project settings** — Source directory, story formats directory, include/exclude patterns
- **Tweego settings** — Path, output file, format override, module paths, head file, no-trim, log-files, extra arguments
- **Lint settings** — Per-rule severity for 6 configurable diagnostic rules


## 0.2.0

Internal architecture redesign. Not released to Marketplace.

### Architecture

- Decomposed `WorkspaceIndex` into `DefinitionRegistry`, `ReferenceIndex`, `LinkGraph`, and `ParseCache` for clearer separation of concerns
- Introduced format adapter system (`StoryFormatAdapter` interface) enabling multi-format support. SugarCube 2 adapter provides the full feature set; fallback adapter provides safe no-op behavior
- Format resolution uses 4-step strategy: exact match, alias match, prefix match, fallback. Defaults to SugarCube 2

### Diagnostics

- Refactored `DiagnosticEngine` with per-rule severity configuration (`knot.lint.*` settings)
- Added `containerStructure` diagnostic for invalid macro nesting
- Added `typeMismatch` diagnostic for comparison operators with incompatible types
- Added always-on checks: deprecated macros, missing required arguments, assignment target errors, StoryData validation, JavaScript syntax errors

### Intelligence

- Dynamic passage reference resolution via variable tracking (`<<goto $dest>>`)
- Implicit passage reference detection: `data-passage` HTML attribute, `Engine.play()`, `Story.get()` JS API calls
- `Macro.add()` detection via acorn AST walking in JavaScript passages
- JS global type inference from `var`/`let`/`const` initializer expressions

### Performance

- 120ms debounced index coordinator coalesces rapid edits into single reanalysis passes
- Incremental parsing: only the affected passage is re-parsed on edit; other passage ASTs are reused
- LRU parse cache with 500-file limit
- File system watcher for external `.tw`/`.twee` changes


## 0.1.0

Initial preview release.

### Language Features
- Diagnostics: unknown macros, broken passage links, type mismatches, duplicate passage names
- Go to Definition for passages, widgets, custom macros, and story variables
- Find All References across workspace
- Rename refactoring for passages and story variables
- Autocomplete for SugarCube built-in macros, passage names, story variables, property access, custom widgets/macros, and close tags
- Hover documentation for macros, variables (with inferred type), and passages (with reference counts)
- Best-effort type inference for story variables assigned via `<<set>>`
- Semantic token highlighting for macros, passages, variables, operators
- Document symbols (Outline) and workspace symbol search (Ctrl+T)
- Code action: create missing passage
- Syntax highlighting with embedded JavaScript and CSS in script/stylesheet passages
- Code folding for passages and macro blocks

### Build Integration
- Tweego build, watch, and test commands
- Configurable tweego path, output, format override, module paths, head file, and extra arguments
- Verify tweego installation command
- List available story formats command

### Project Settings
- Source directory, story formats directory, include/exclude patterns
