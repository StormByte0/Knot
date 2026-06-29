# Knot: Planned Features

## 1. Project Setup & Onboarding

**Goal:** Reduce friction when starting new projects.

* **Project Starter Templates:** Auto-generate essential structure (metadata, story ID, `Start` passage, reference files) upon workspace creation.
* **Default Configs:** Optional workspace configuration with recommended defaults.

## 2. Passage Management & Organization

**Goal:** Streamline restructuring as stories grow.

* **Move to New File:** Extract selected passages into individual files with automatic cleanup of the source file.
* **Bulk Operations:** Move passages between existing files, split large files, or merge multiple files.

## 3. Story Map UX Improvements

**Goal:** Make navigation and editing more intuitive.

* **Interactive Graph:** Support for double-click navigation, multi-selection, and right-click context menus for quick edits.

## 4. Improved Syntax Highlighting

**Goal:** Make styled and formatted text easier to distinguish while writing.

* **Markup Highlighting:** Add special highlighting for formatted text regions so styling and markup effects are easier to identify visually.

## 5. JavaScript Parser Improvements

**Goal:** Improve resilience when handling JavaScript syntax errors.

* **Error Recovery:** Prevent JavaScript parsing errors from breaking syntax highlighting and analysis for the rest of the file after an invalid JavaScript block.

## 6. Smarter Context-Aware Parsing

**Goal:** Improve editor suggestions and diagnostics by understanding content context more accurately.

* **Improved Zoning Logic:** Better identify syntax regions within passages so completions and diagnostics can adapt based on the type of content currently being edited.
