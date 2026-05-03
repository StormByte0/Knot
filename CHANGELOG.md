# Change Log

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
