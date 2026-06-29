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
* HTML structure validation
* Detection of malformed tags and invalid nesting
* CSS property and selector validation
* Diagnostics for embedded HTML and CSS inside passages

This requires dedicated parser integration and coordination with the
existing analysis pipeline.

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
