# Knot — Feature Roadmap

This document tracks functionality that is intentionally deferred to a future version.
Items here require architectural advances that are out of scope for the current release.

---

## 1. Graph Simplification & Advanced Analysis

These features depend on a more sophisticated graph simplification and interpretation
layer than what currently exists. They are blocked until that foundation is built.

### Game Loop Visualization

- **Status**: Server-side detection implemented (`GameLoopInfo`, SCC analysis in
  `crates/core/src/graph.rs`). Data is sent across the wire (`game_loops` field in
  `KnotGraphResponse`). Client receives but does not render it.
- **What's missing**: A Story Map UI that can display cycles with multiple sub-loops
  or branches without becoming visually overwhelming. The detection logic also needs
  hardening — current SCC analysis identifies cycles but doesn't distinguish game loops
  from problematic infinite loops reliably enough for visual treatment.
- **Files involved**: `crates/core/src/graph.rs:779` (detection),
  `crates/server/src/lsp_ext.rs:32` (wire format),
  `extensions/vscode/webview/src/types.ts:10,46-50` (client types),
  `extensions/vscode/webview/src/components/StoryMap.tsx` (rendering)

### InfiniteLoop Diagnostic

- **Status**: The old `DiagnosticKind::InfiniteLoop` was removed (replaced by
  `GameLoopInfo`). A proper InfiniteLoop diagnostic needs to distinguish between
  intentional game loops (normal Twine interaction patterns with state mutation) and
  actual infinite loops (cycles with no exit condition and no state change).
- **What's missing**: Graph simplification algorithm capable of determining whether a
  cycle has a viable exit path or mutation. The `has_mutation` field on `GameLoopInfo`
  is a step in this direction but insufficient on its own.

### Block Detection & Visualization

- **Status**: Placeholder `block: Option<String>` field exists across the full stack
  (`crates/core/src/graph.rs:1092`, `crates/server/src/lsp_ext.rs:78`,
  `extensions/vscode/src/types.ts:54-55`). Always `None` in practice.
- **What's missing**: A block detection algorithm that groups related passages into
  logical blocks (e.g., a choice hub and its consequence passages). Would enable
  passage block visualization in the Story Map.
- **Note**: The `SpecialPassageLayer::UserDefined` variant in
  `crates/core/src/passage.rs:160-176` is also reserved for this kind of
  user-defined grouping. It has semantic token support but no format plugin creates
  `UserDefined` passages.

---

## 2. Format Support: Full Harlowe, Snowman & Chapbook

All four format plugins (`Core`, `SugarCube`, `Harlowe`, `Snowman`, `Chapbook`) have
`impl FormatPlugin` implementations. However, the depth of analysis varies significantly:

- **SugarCube**: Most complete — macro parsing, variable extraction, semantic tokens,
  special passage classification, and validation all have real implementations.
- **Harlowe**: Parsing and special passage classification exist. Macro analysis and
  variable tracking are less comprehensive. Many SugarCube-level features are not
  yet provided.
- **Snowman**: Minimal plugin — basic passage parsing and special passage classification.
  No macro system (Snowman uses raw JavaScript), so variable tracking and semantic
  analysis would need a fundamentally different approach (JavaScript analysis).
- **Chapbook**: Parsing and classification exist. Chapbook's declarative modifier
  system (`{embed passage}`, `{reveal link}`) is not parsed for navigation links or
  variable tracking.

### Full format support means:

- **Harlowe**: Complete macro/hook parsing, variable tracking (`$variable` + datamap
  access), semantic tokens for hooks, macros, and variables, link detection including
  hidden links and variable-driven navigation.
- **Snowman**: JavaScript-aware variable analysis (at minimum, tracking `story.state`
  reads/writes), passage link detection in `story.show()` calls, semantic tokens for
  embedded JavaScript.
- **Chapbook**: Modifier parsing for passage links and reveals, variable tracking
  (`_variable` syntax), semantic tokens for modifiers and variables.
