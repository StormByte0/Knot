# Change Log


## 0.2.0 — Major Update

This release improves accuracy, performance, and flexibility. The internal architecture was redesigned, resulting in more reliable behavior across the extension.

---

### What’s New

**Improved Diagnostics**

* More accurate errors and warnings
* New checks for:

  * Deprecated macros
  * Missing required arguments
  * Invalid assignment usage
* Diagnostic rules can now be enabled, disabled, or customized

**Better Code Intelligence**

* Improved type inference for variables
* More reliable macro validation and structure checks
* More consistent results across large projects

**Performance Improvements**

* Smarter caching reduces unnecessary reprocessing
* Better handling of large workspaces
* Fewer duplicate or inconsistent results

---

### General Improvements

* More reliable “Go to Definition”, references, and rename
* Improved parsing of scripts, styles, and expressions
* Better handling of passage links and relationships
* Reduced edge-case bugs during editing

---

### Advanced / Extensibility

* New adapter system enables support for multiple story formats
* Architecture improvements make future updates easier

---

### Breaking Changes

* Story format adapters now require additional methods
* Some internal APIs now require an adapter parameter

(Does not affect normal usage.)

---

### Stability

* Expanded test coverage
* More consistent behavior across files and projects


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