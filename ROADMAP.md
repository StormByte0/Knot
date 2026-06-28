# Knot — Roadmap

This document tracks features that are intentionally deferred to future
versions. Items here require architectural advances or significant new
work that is out of scope for the current release.

For smaller, near-term features (decompile, archive export, test mode,
etc.), see [PLANNED_FEATURES.md](./PLANNED_FEATURES.md).

---

## Knot Standalone Desktop Program

Knot currently depends on VS Code for the editor surface, event handling, and window management. The extension is structured to make a future migration to a self-contained desktop application straightforward — the language server, format plugins, and build pipeline are all decoupled from the VS Code shell.

A dedicated desktop program would offer a purpose-built environment for Twine and interactive fiction development, without requiring authors to learn the VS Code ecosystem. This is a significant undertaking and is planned for after the VS Code extension reaches feature maturity.

---

## Graph Simplification & Advanced Analysis

Knot's graph model is the foundation of its structural analysis. The current implementation prioritizes correctness; several advanced features depend on a more capable interpretation layer than exists today.

### Game Loop Visualization

The server already detects strongly connected components (SCCs) in the passage graph — cycles where the player can move between passages indefinitely. The detection logic lives in `crates/core/src/graph.rs`, and the data is sent to the client via the `game_loops` field in `KnotGraphResponse`. The Story Map does not yet render this information.

The challenge is presentation. A story can have multiple overlapping sub-loops, and showing them all at once overwhelms the graph layout. The detection also needs hardening — current SCC analysis identifies cycles but cannot reliably distinguish intentional game loops (hub-and-spoke patterns, shopping menus) from broken ones where the player cannot escape. A proper visualization needs to group related cycles and offer collapse/expand controls, so authors can focus on one loop at a time without losing the broader context.

### Infinite Loop Diagnostic

The old `DiagnosticKind::InfiniteLoop` was removed when game loop detection was added, because SCC analysis alone could not distinguish intentional loops from broken ones. A proper diagnostic needs to understand which passages offer exit conditions — a loop is only "infinite" if every path through it either returns to the loop or reaches a dead end that is not the story's conclusion. This requires analyzing the conditional structure inside passages (which `<<if>>` branches lead where), which is more nuanced than the current link-based graph.

The fix involves extending the graph model to track conditional edges (links that only fire under certain variable states) and adding a reachability pass that checks whether any path from a loop node can reach a terminal passage. This is non-trivial because TwineScript conditions can depend on variables set in other passages, requiring cross-passage dataflow analysis that the current variable tracker does not fully support.

### Graph Simplification Pass

Large stories (100+ passages) produce graphs that are hard to read even with good layout. A simplification pass could collapse linear chains (A → B → C where B has no other links) into single composite nodes, and group tightly-coupled subgraphs into clusters. This is a presentation-layer feature — the underlying graph stays the same, but the Story Map renders a simplified view that the author can expand on demand. This requires work in both the Story Map webview (new node types, cluster layout, expand/collapse state) and a server-side simplification algorithm.

---

## Advanced Variable Analysis

The current variable tracker knows where each variable is read and written, but it does not track the *values* flowing through those operations. Several diagnostic improvements depend on value-sensitive analysis.

### Type Inference

SugarCube variables are dynamically typed, but in practice most authors use them consistently — a variable is always a string, or always a number, or always an object with a known shape. Type inference could detect when a variable is used inconsistently (treated as a number in one passage and as a string in another), which is a common source of runtime errors. This requires extending the JS annotation pipeline to track inferred types through assignments and reads, then adding a new diagnostic for type mismatches. The oxc parser already produces AST that could support this, but the annotation layer would need significant new logic.

### Unreachable Code Detection

Inside passage bodies, SugarCube macros like `<<if>>`, `<<switch>>`, and `<<for>>` create branching structure. The parser already builds an AST representing this structure, but there is no analysis pass that checks for unreachable branches — an `<<if>>` whose condition can never be true, or a `<<switch>>` case shadowed by an earlier one. Detecting this requires evaluating constant conditions and tracking which macro bodies are reachable from the passage entry point. This is conceptually similar to the existing dead-end passage diagnostic, but applied within a single passage rather than across the graph.

---

## Format Plugin Maturity

SugarCube is the most complete format plugin, with full macro catalog, variable tracking, and special passage support. The other three formats (Harlowe, Chapbook, Snowman) currently have stub parsers only. Bringing them to feature parity is planned for future releases.
