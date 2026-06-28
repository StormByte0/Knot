# Knot — Roadmap

This document tracks features that are intentionally deferred to future
versions. Items here require architectural advances or significant new
work that is out of scope for the current release.

For smaller, near-term features (decompile, archive export, test mode,
project initialization, etc.), see [PLANNED_FEATURES.md](./PLANNED_FEATURES.md).

---

## Knot Standalone Desktop Program

Knot currently depends on VS Code for the editor surface, event handling,
and window management. The extension is structured to make a future
migration to a self-contained desktop application straightforward — the
language server, format plugins, and build pipeline are all decoupled
from the VS Code shell.

A dedicated desktop program would offer a purpose-built environment for
Twine and interactive fiction development, without requiring authors to
learn the VS Code ecosystem. This is a long-term goal that depends on
the VS Code extension reaching feature maturity and on community
support making sustained development possible. No timeline is committed.

---

## Graph Simplification & Advanced Analysis

Knot's graph model is the foundation of its structural analysis. The
current implementation prioritizes correctness; several advanced features
depend on a more capable interpretation layer than exists today.

### Game Loop Visualization

The server computes strongly connected components (SCCs) in the passage
graph using Tarjan's algorithm — cycles where the player can move
between passages indefinitely. The detection logic lives in
`crates/core/src/graph.rs`, and the data is sent to the client via the
`game_loops` field in `KnotGraphResponse`. However, the Story Map webview
does not yet render this information.

The challenge is presentation. A story can have multiple overlapping
sub-loops, and showing them all at once overwhelms the graph layout. The
detection also needs hardening — current SCC analysis identifies cycles
but cannot reliably distinguish intentional game loops (hub-and-spoke
patterns, shopping menus) from broken ones where the player cannot
escape. A proper visualization needs to group related cycles and offer
collapse/expand controls, so authors can focus on one loop at a time
without losing the broader context.

### Infinite Loop Diagnostic

SCC analysis alone cannot distinguish intentional loops from broken
ones. A proper diagnostic needs to understand which passages offer exit
conditions — a loop is only "infinite" if every path through it either
returns to the loop or reaches a dead end that is not the story's
conclusion. This requires analyzing the conditional structure inside
passages (which `<<if>>` branches lead where) and tracking conditional
edges — links that only fire under certain variable states.

This is non-trivial because TwineScript conditions can depend on
variables set in other passages, requiring cross-passage dataflow
analysis that the current variable tracker does not fully support.
This feature depends on the graph flow detection work above.

### Graph Simplification Pass

Large stories (100+ passages) produce graphs that are hard to read even
with good layout. A simplification pass could collapse linear chains
(A → B → C where B has no other links) into single composite nodes, and
group tightly-coupled subgraphs into clusters. This is a
presentation-layer feature — the underlying graph stays the same, but
the Story Map renders a simplified view that the author can expand on
demand. This requires work in both the Story Map webview (new node
types, cluster layout, expand/collapse state) and a server-side
simplification algorithm.

---

## Advanced Variable Analysis

The current variable tracker knows where each variable is read and
written, but it does not track the *values* flowing through those
operations. Several diagnostic improvements depend on value-sensitive
analysis.

### Type Inference

SugarCube variables are dynamically typed, but in practice most authors
use them consistently — a variable is always a string, or always a
number, or always an object with a known shape. Type inference could
detect when a variable is used inconsistently (treated as a number in
one passage and as a string in another), which is a common source of
runtime errors.

This is a difficult feature to implement reliably. The oxc parser
already produces AST that could support it, but the annotation layer
would need significant new logic, and the false-positive rate must be
low enough to be useful. This item is a candidate for future work but
is not committed to a timeline.

### Unreachable Code Detection (within passages)

Inside passage bodies, SugarCube macros like `<<if>>`, `<<switch>>`,
and `<<for>>` create branching structure. The parser already builds an
AST representing this structure, but there is no analysis pass that
checks for unreachable branches — an `<<if>>` whose condition can never
be true, or a `<<switch>>` case shadowed by an earlier one.

This depends on type inference (above) to evaluate constant conditions,
and is therefore also a candidate rather than a committed item.

---

## Format Plugin Maturity

SugarCube is the only format with a production-quality plugin today —
full macro catalog, variable tracking, special passages, completion,
and hover. The other three formats (Harlowe, Chapbook, Snowman) have
placeholder/skeleton implementations only. The `FormatPlugin` trait is
implemented for each, but the parsers have not been completed to
production quality and link extraction is not yet functional.

Bringing them to feature parity is planned. Each format needs:

- **Harlowe 3** — Macro catalog (~60 builtins with Harlowe's distinct
  `(macro:)` syntax), variable tracking (Harlowe uses a different
  scoping model than SugarCube), completion & hover.
- **Chapbook 1** — Modifier catalog (Chapbook's equivalent of macros —
  `~` and `{}` syntax), embedded JS expression analysis, completion &
  hover.
- **Snowman 2** — Embedded JS analysis inside `<% %>` ERB template
  blocks, completion for Underscore.js helpers, variable tracking.

Estimated effort: 3–4 months of dedicated development per format.
