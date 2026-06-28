# Knot — Roadmap

This document tracks features that are intentionally deferred to future
versions. Items here require architectural advances or significant new
work that is out of scope for the current release.

For smaller, near-term features (decompile, archive export, test mode,
etc.), see [PLANNED_FEATURES.md](./PLANNED_FEATURES.md).

---

## Graph Simplification & Advanced Analysis

Knot's graph model is the foundation of its structural analysis, but
the current implementation focuses on correctness over sophistication.
Several advanced features depend on a more capable graph interpretation
layer than what exists today.

### Game Loop Visualization

The server already detects strongly connected components (SCCs) in the
passage graph — these represent potential game loops where the player
can cycle between passages indefinitely. The detection logic exists in
`crates/core/src/graph.rs` and the data is sent to the client via the
`game_loops` field in `KnotGraphResponse`. The Story Map webview does
not yet render this data visually.

The challenge is presentation: a story can have multiple overlapping
sub-loops, and showing them all at once overwhelms the graph layout.
The detection logic also needs hardening — current SCC analysis
identifies cycles but does not reliably distinguish intentional game
loops (hub-and-spoke patterns, shopping menus) from problematic
infinite loops where the player cannot escape. A proper visualization
needs to group related cycles and offer collapse/expand controls so
authors can focus on one loop at a time without losing the broader
graph context.

### Infinite Loop Diagnostic

The old `DiagnosticKind::InfiniteLoop` was removed when game loop
detection was added, because SCC analysis could not reliably
distinguish intentional loops from broken ones. A proper infinite
loop diagnostic needs to understand which passages offer exit
conditions — a loop is only "infinite" if every path through it
either returns to the loop or reaches a dead end that is not the
story's conclusion. This requires analyzing the conditional structure
inside passages (which `<<if>>` branches lead where), which is more
nuanced than the current link-based graph.

The fix involves extending the graph model to track conditional edges
(links that only fire under certain variable states) and adding a
reachability pass that checks whether any path from a loop node can
reach a terminal passage. This is non-trivial because TwineScript
conditions can depend on variables set in other passages, requiring
cross-passage dataflow analysis that the current variable tracker
does not fully support.

### Graph Simplification Pass

Large stories (100+ passages) produce graphs that are hard to read
even with good layout. A simplification pass could collapse
linear chains (A → B → C where B has no other links) into single
composite nodes, and group tightly-coupled subgraphs into clusters.
This is a presentation-layer feature — the underlying graph stays
the same, but the Story Map renders a simplified view that the
author can expand on demand. This requires significant work in the
Story Map webview (new node types, cluster layout, expand/collapse
state management) and a server-side simplification algorithm.

---

## Advanced Variable Analysis

The current variable tracker knows where each variable is read and
written, but it does not track the *values* flowing through those
operations. Several diagnostic improvements depend on value-sensitive
analysis.

### Type Inference

SugarCube variables are dynamically typed, but in practice most
authors use them consistently (a variable is always a string, or
always a number, or always an object with a known shape). Type
inference could detect when a variable is used inconsistently —
treated as a number in one passage and as a string in another —
which is a common source of runtime errors. This requires extending
the JS annotation pipeline to track inferred types through
assignments and reads, then adding a new diagnostic kind for type
mismatches. The oxc parser already produces AST that could support
this, but the annotation layer would need significant new logic.

### Unreachable Code Detection

Inside passage bodies, SugarCube macros like `<<if>>`, `<<switch>>`,
and `<<for>>` create branching structure. The parser already builds
an AST that represents this structure, but there is no analysis pass
that checks for unreachable branches — an `<<if>>` whose condition
can never be true, or a `<<switch>>` case that is shadowed by an
earlier case. Detecting this requires evaluating constant conditions
and tracking which macro bodies are reachable from the passage entry
point. This is conceptually similar to the existing dead-end passage
diagnostic but applied within a single passage rather than across
the graph.

---

## Format Plugin Maturity

SugarCube is the most complete format plugin, with full macro
catalog, variable tracking, and special passage support. The other
three formats (Harlowe, Chapbook, Snowman) have functional parsers
but lack deeper analysis.

### Harlowe Variable Tracking

Harlowe uses a different variable syntax (`$var` vs SugarCube's
`$var`) and different macro structure. The parser extracts passages
and links correctly, but variable tracking is partial — Harlowe's
`(set: $var to value)` syntax is parsed, but more complex patterns
like `(a:, )` array construction and macro chains are not fully
tracked. Completing this requires extending the Harlowe parser's
AST and adding the corresponding annotation passes, mirroring what
SugarCube already does.

### Chapbook and Snowman JavaScript Analysis

Chapbook and Snowman both embed JavaScript directly in passages
(Chapbook via `{% script %}` blocks, Snowman via `<% %>` ERB-style
tags). The oxc-based JS annotation pipeline that powers SugarCube's
variable tracking could be reused for these formats, but the
integration points are different — each format wraps JS in different
container syntax that the parser must extract first. This is a
medium-effort feature that would bring variable tracking parity to
all four formats.

---

## Story Map Enhancements

The Story Map is functional but has room to grow as a navigation
and authoring tool.

### Passage Editing from the Map

Currently the Story Map is read-only — you can click a node to
navigate to the passage in the editor, but you cannot edit passage
content or rename passages directly from the graph. Allowing inline
rename (double-click a node, type a new name, all links update) and
quick content edits (expand a node to show its first few lines,
edit them inline) would make the map a more central authoring
surface. This requires new `knot/passageRename` and
`knot/passageEdit` LSP requests, plus significant webview work for
the inline editing UX.

### Multi-Story Support

Knot currently assumes one story per workspace. Some authors
maintain story collections or shared universes across multiple
`.html` outputs. Supporting multiple `StoryData` passages in a
single workspace — each defining a separate story with its own
format, start passage, and graph — would let authors work on
related stories together. This requires changes to the workspace
model (which currently keys on a single `metadata` field) and
the Story Map (which would need story-switching controls).
