# Knot — Roadmap

This document tracks features that are intentionally deferred to future
versions. Items here require architectural advances or significant new
work that is out of scope for the current release.

For smaller, near-term features (passage management, storymap UX, project initialization, etc.), see [PLANNED_FEATURES.md](./PLANNED_FEATURES.md).

---

## Format Plugin Development

SugarCube is currently the only fully implemented format plugin in Knot,
with production-ready support for parsing, macro analysis, variable
tracking, completion, hover documentation, and special passage support.

The remaining supported story formats — Harlowe, Chapbook, and Snowman —
currently exist only as minimal placeholder implementations. While the
plugin architecture is already in place, each format requires a near
complete parser and analysis implementation written from the ground up.

Unlike incremental feature work, this is effectively the development of
three entirely new language plugins, each with its own syntax rules,
execution model, and authoring conventions.

Each format requires dedicated implementations for:

* **Harlowe 3** — Full macro parser, variable tracking, completion,
  hover documentation, expression analysis, and support for Harlowe's
  unique runtime model and `(macro:)` syntax.

* **Chapbook 1** — Modifier parser, completion, hover documentation,
  expression analysis, variable tracking, and support for Chapbook's
  distinct authoring syntax and modifier system.

* **Snowman 2** — Embedded JavaScript template analysis, expression
  parsing inside `<% %>` blocks, helper completions, variable tracking,
  and support for Snowman's JavaScript-centric execution model.

Estimated effort: if it's anything like sugarcube, several months of dedicated development per format.

---

## HTML & CSS Parser / Linter Integration

Twine projects frequently embed custom CSS stylesheets and HTML content
directly inside passages. While Knot already supports embedded JavaScript
analysis, dedicated support for HTML and CSS validation does not yet exist.

Future integration would allow Knot to provide first-class support for
these embedded languages directly inside the editor.

Planned improvements include:

* CSS syntax validation and linting
  * **IMPLEMENTED (plan.md Phase 2.5, 2026-09-22):** CSS parse errors are
    relayed on every CSS-bearing surface — `[stylesheet]` passages,
    `<style>` element bodies, `<<style>>`/`<<css>>` blocks — and an
    unknown-property lint (a curated 500+ property registry, custom and
    vendor-prefixed names always allowed) flags typos like `colr` on all
    surfaces including `style="…"` attribute values. See
    `knot-core/src/css/properties.rs` and
    `sugarcube/lsp/embedded_diags.rs`.
* CSS property and selector validation
  * Property validation: implemented (see above). Selector validation:
    **IMPLEMENTED (2026-09-22)** — a pseudo-name lint (curated
    pseudo-class/pseudo-element registries; legacy single-colon
    spellings, `@page` pseudos and vendor prefixes always allowed) flags
    unknown names like `:hvoer`/`::befor` — the typo class that makes
    browsers silently drop the whole rule — plus a placement lint for
    pseudo-elements that are not last in their compound
    (`div::before:hover`). Runs on every stylesheet surface via the
    real-parser token stream. See `knot-core/src/css/selectors.rs`.
* HTML attribute directives (`@attr="expr"` / `sc-eval:attr="expr"`)
  * **IMPLEMENTED (2026-09-22):** recognition is raw-source and
    case-sensitive (upstream parity: `SC-EVAL:x` is an ordinary
    attribute); `@data-setter`/`sc-eval:data-setter` and lone sigils
    (`@`, `sc-eval:`) are Error diagnostics mirroring the engine's
    `processAttributeDirectives` throws; the directive sigil gets its
    own `htmlDirective` token type (split from the base attribute name,
    themed in both themes); attribute-NAME positions inside tag
    interiors — including mid-typing unterminated tags — offer curated
    HTML attribute completions with `@`/`sc-eval:` families (`@data-setter`
    is never offered as a directive — it is an upstream error). See
    `sugarcube/lsp/html_completions.rs` and the directive section of
    `embedded_diags.rs`.
* HTML structure validation
  * **PARTIALLY IMPLEMENTED (plan.md Phase 2.5, 2026-09-22):** unclosed
    elements (Error — upstream renders an error box for the same shape),
    duplicate attributes and missing attribute values (Warning), and
    unterminated tags at passage end (Warning) are diagnosed inside
    passages and StoryInterface. Prose `<`, stray end tags, and literal
    regions (`<nowiki>`, `{{{ }}}`, raw bodies) are deliberately silent —
    matching both the WHATWG tokenizer's tolerance and SugarCube's
    behavior.
* Detection of malformed tags and invalid nesting
  * Malformed tags: implemented (see above). Invalid nesting (e.g.
    `<p>` inside `<span>`) remains future work — it needs honest tree
    reasoning beyond the wikifier's model.
* Diagnostics for embedded HTML and CSS inside passages
  * **IMPLEMENTED (plan.md Phase 2.5, 2026-09-22)** — including a
    tree-builder fix that lets macros inside HTML elements pair with
    their close tags (wikified-content parity; previously `<<if>>`
    inside a `<div>` never paired, and macro diagnostics were blind
    inside HTML elements).

This requires dedicated parser integration and coordination with the
existing analysis pipeline.

---

## Story Format Versioning (SugarCube)

**IMPLEMENTED (2026-09-22, v2.1.0).** The static macro catalog is now a
versioned union catalog: every macro that ever existed in SugarCube 2.x
carries `added_in` / `deprecated_in` / `removed_in` plus per-era
descriptors (description + args), sourced from the official v2 docs
per-macro History sections, the docs upgrade guides, release notes
2.31.0–2.37.3, and the 2.37.3 engine source.

Key design points:

* The raw union catalog is **not** for user-facing features — completions,
  hover, signature help, and diagnostics resolve through
  `macros_at(version)` / `descriptor_at(version)` / `status_at(version)` so
  only the macros and descriptions relevant to the story's pinned
  `format-version` are surfaced. Structural parsing (tree building,
  pairing) stays version-blind so removed macros still parse while being
  flagged.
* The server pushes the StoryData `format-version` into the plugin
  (`set_story_version`) at indexing and on metadata change; editing it
  triggers a full reindex. Unknown/missing versions fail open to the
  latest known release.
* New diagnostics: `sc-macro-removed` (Error, with removal version +
  replacement + downgrade guidance) and `sc-macro-not-in-version`
  (Warning, with the version the macro was added in); `sc-deprecated`
  now reports the deprecation version.
* Version-aware "elements": special passages (`StoryDisplayTitle`
  2.31.0, `[init]` 2.36.0) filter the same way.
* Future extension points: versioning the globals/methods surface (the
  2.37.0 `Save.*` deprecations etc.) follows the same descriptor pattern.

---

## Graph Simplification & Advanced Analysis

Knot's graph model is the foundation of its structural analysis. The
current implementation prioritizes correctness, but several advanced
analysis features require deeper graph interpretation.

### Game Loop Visualization

The server already computes strongly connected components (SCCs) using
Tarjan's algorithm, but the Story Map does not currently visualize this
information.

Future work would allow loops to be grouped visually, collapsed, and
explored without overwhelming the graph layout.

### Infinite Loop Diagnostic

A loop is only problematic if the player cannot escape it.

Proper diagnostics require analyzing passage conditions, conditional
branches, and variable-dependent transitions across multiple passages.

This requires deeper flow analysis than the current graph model supports.

### Graph Simplification Pass

Large stories can produce graphs that quickly become difficult to read.

A simplification layer could:

* Collapse linear passage chains into composite nodes
* Group tightly coupled subgraphs into clusters
* Allow expand/collapse interactions for simplified graph views

This also serves as foundational infrastructure for more advanced
cross-passage static analysis in future versions.

---

## Advanced Variable Analysis

Knot currently tracks where variables are read and written across the
project, but does not understand the actual execution order in which
those operations occur.

For meaningful static analysis, simply knowing *where* a variable was
modified is not enough — the system must understand *when* and *under*
*what execution path* those changes happen.

This makes deeper variable analysis fundamentally dependent on improved
graph flow analysis.

### Type Inference

Variables in Twine are dynamically typed, but most projects use them
consistently in practice.

Future analysis could detect inconsistent variable usage, such as values
being treated as numbers in one path and strings in another.

### Unreachable Code Detection

Passage logic often creates branches where certain conditions or code
paths may never execute.

Future analysis could detect unreachable branches caused by impossible
conditions or conflicting execution paths.

These features depend on understanding passage flow order and variable
state transitions throughout the graph.

---

## Knot Standalone Desktop Program

Knot currently depends on Visual Studio Code for the editor surface, event handling,
and window management. The extension architecture is intentionally
structured so a future migration to a dedicated desktop application
remains possible.

A standalone desktop application would provide a purpose-built
environment for Twine and interactive fiction development without
requiring authors to use VS Code directly.

This is a long-term goal that depends on the extension reaching feature
maturity and on community support making sustained development possible.

No development timeline is currently planned.
