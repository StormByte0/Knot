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
  * **IMPLEMENTED (plan.md Phase 3.2, 2026-09-15):** JS regions are parsed
    chunk-by-chunk when oxc reports a fatal error — healthy statements keep
    their tokens, broken statements get tolerant fallback-lexer tokens, and
    diagnostics are localized and capped. oxc was also bumped 0.134 → 0.150
    (Phase 3.1). See `knot-core/src/oxc/chunks.rs` and
    `js_annotate::analyze_module_resilient`.

## 6. Smarter Context-Aware Parsing

**Goal:** Improve editor suggestions and diagnostics by understanding content context more accurately.

* **Improved Zoning Logic:** Better identify syntax regions within passages so completions and diagnostics can adapt based on the type of content currently being edited.
  * **IMPLEMENTED (plan.md Phases 2–4, 2026-09-15):** full HTML stratum
    (atomic tag interiors, `@attr` directive expressions, raw-text
    `<script>`/`<style>` zones), a span-carrying HTML CST built on html5gum,
    HTML/CSS semantic tokens, and a per-byte `ZoneMap::language_at` authority
    consumed by the completion guard (`FormatPlugin::zone_analyze`).
