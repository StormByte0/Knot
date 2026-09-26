# Knot — A Twine IDE that actually understands your story

[![Visual Studio Marketplace Version](https://img.shields.io/visual-studio-marketplace/v/stormbyte.knot?label=VS%20Code%20Marketplace)](https://marketplace.visualstudio.com/items?itemName=stormbyte.knot)
[![VS Code Engine](https://img.shields.io/badge/VS%20Code-%E2%89%A5%201.85.0-007ACC?logo=visualstudiocode)](https://marketplace.visualstudio.com/items?itemName=stormbyte.knot)
[![License](https://img.shields.io/badge/license-Knot%20Source--Available-6f42c1)](LICENSE)
[![Discord](https://img.shields.io/badge/Discord-Join%20the%20server-5865F2?logo=discord&logoColor=white)](https://discord.gg/YYm9nGbtZY)
[![Patreon](https://img.shields.io/badge/Patreon-Become%20a%20Patron-FF424D?logo=patreon)](https://www.patreon.com/StormByte0)
[![Ko-fi](https://img.shields.io/badge/Ko--fi-Buy%20a%20coffee-FF5E5B?logo=ko-fi&logoColor=white)](https://ko-fi.com/stormbyte0)

Knot is a development environment for [Twine](https://twinery.org/) and Twee interactive fiction, built around a language server written in Rust. Instead of bolting regexes onto a text editor, Knot models your project the way Twine does: a directed graph of passages, connected by links, with variables flowing across them. That structural model powers navigation, diagnostics, and insight that pattern matching simply cannot provide.

The bulk of Knot lives in the Rust language server, which talks to editors through the Language Server Protocol. The VS Code extension is its first host — but the same Rust core can power other editors, and in time become a standalone IDE for Twine.

## In action

**Story Map** — your whole project as an interactive graph. Nodes are passages, edges are links; click any node to jump to it in the editor.

![Story Map](extensions/vscode/media/demos/storymap.gif)

**Passage Diagnostics** — a dedicated panel inspecting links, variables, macros, and complexity, plus real-time diagnostics as you type.

![Passage Diagnostics](extensions/vscode/media/demos/passage-diagnostics.gif)

**Variable Tracking** — where every variable is set, read, and how it flows across passages. JavaScript-aware, powered by [oxc](https://oxc.rs/).

![Variable Tracking](extensions/vscode/media/demos/variable-tracking.gif)

**Build & Play** — one-click compile via [Tweego](https://www.motoslave.net/tweego/), which Knot downloads and manages for you. Zero setup.

![Build & Play](extensions/vscode/media/demos/build-and-play.gif)

## What Knot does today

- **Structural understanding, not just regex.** Knot parses your project into passages and the links between them, so it can answer "where is this variable set?", "is this passage reachable?", and "which passages link here?" — questions pure text matching gets wrong.
- **Version-aware SugarCube 2 support.** The macro catalog is versioned end to end: completions, hover docs, signature help, and diagnostics are filtered through the `format-version` pinned in your `StoryData`. Using a macro that was removed in SugarCube 2.37.0? You get an error with the removal version, the replacement, and downgrade guidance — not a silent runtime failure.
- **Variable tracker.** See where every variable is set, read, and how it flows across passages. For SugarCube, tracking is JavaScript-aware via the [oxc](https://oxc.rs/) parser, so it understands `<<set $x to 1>>`, `<<run $x++>>`, and JS inside `<<script>>` blocks.
- **Passage diagnostics.** Real-time diagnostics cover broken links, unreachable passages, uninitialized variables, duplicate passage names, and dead ends — each configurable to `error`/`warning`/`info`/`hint`/`off`.
- **Embedded CSS linting.** Parse errors on every stylesheet surface (`[stylesheet]` passages, `<style>` bodies, `<<style>>`/`<<css>>` blocks), an unknown-property lint backed by a 500+ property registry, and pseudo-class/pseudo-element typo detection — the mistakes that make browsers silently drop a whole rule.
- **Embedded HTML validation.** Unclosed elements, duplicate attributes, missing attribute values, and unterminated tags are diagnosed inside passages and StoryInterface, with completions for `@attr` / `sc-eval:attr` evaluation directives.
- **Story Map.** An interactive graph visualization of your project, built on [@xyflow/react](https://reactflow.dev/) with automatic layout via [dagre](https://github.com/dagrejs/dagre). Click any node to jump to it in the editor. Manual entry points (`"reachable": true` header metadata) keep dynamic, variable-aliased navigation from producing false "unreachable" warnings (unreleased — in the repository build, not yet in a Marketplace release).
- **The Knot IDE sidebar.** A dedicated activity-bar panel with three views: Passage Diagnostics, Variable Tracking, and Project Info (workspace statistics).
- **Incremental analysis.** Only affected passages are re-parsed after each keystroke, so large projects stay responsive.
- **Integrated build pipeline.** One-click build and play via Tweego — auto-downloaded, with story formats cached per version. No manual compiler setup, no PATH fiddling.
- **Watch mode.** Toggle auto-rebuild on save: edit, save, refresh the browser.

## Quick start

1. Install **Knot** from the [VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=stormbyte.knot) (or search "Knot" in the Extensions panel).
2. Open a folder containing `.tw` or `.twee` files.
3. Click **Build** in the status bar — Knot downloads Tweego and the right story format on first run.
4. Click **Play** to open the compiled story in your browser.

Full walkthrough: [Onboarding](https://stormbyte0.github.io/Knot/onboarding.html) · [Feature deep-dive](https://stormbyte0.github.io/Knot/features.html)

## Story format support

| Format | Status | Notes |
|---|---|---|
| **SugarCube 2** | ✅ Full support | Versioned macro catalog (73-entry union, filtered by your story's `format-version`), JS-aware variable tracking, embedded CSS & HTML linting, special passages, completion, hover, signature help, diagnostics, Story Map, build pipeline |
| **Harlowe 3** | ◐ Placeholder | Build pipeline works (via Tweego); language features not yet built out |
| **Chapbook 1** | ◐ Placeholder | Build pipeline works (via Tweego); language features not yet built out |
| **Snowman 2** | ◐ Placeholder | Build pipeline works (via Tweego); language features not yet built out |

Knot auto-detects the format from your `StoryData` passage and activates the right language features. Bringing the remaining formats to parity is the main development priority — see the [roadmap](ROADMAP.md).

## Support Knot's development 💛

**Knot is free — including for commercial use — and it will stay that way.** It is built and maintained by a single developer on nights and weekends, and right now its funding is **exactly zero**: no patrons, no tips, nothing. Every hour that goes into Knot is volunteered.

That's sustainable only so long as the volunteer keeps volunteering. If Knot saves you time — catches a broken link before your players do, untangles a variable bug that would have cost you an evening — consider putting a fraction of that saved time back into the project:

[![Patreon](https://img.shields.io/badge/Patreon-Become%20a%20Patron-FF424D?style=for-the-badge&logo=patreon)](https://www.patreon.com/StormByte0)
[![Ko-fi](https://img.shields.io/badge/Ko--fi-Buy%20a%20coffee-FF5E5B?style=for-the-badge&logo=ko-fi&logoColor=white)](https://ko-fi.com/stormbyte0)

- **[Patreon](https://www.patreon.com/StormByte0)** — recurring support ($5 / $10 / $25 tiers) with backer-only dev updates and roadmap voting at the Insider tier.
- **[Ko-fi](https://ko-fi.com/stormbyte0)** — a one-time tip of any size. Three dollars buys the coffee that funds the next commit.

Donations directly fund what users ask for most: completing the Harlowe, Chapbook, and Snowman format plugins, the [planned features](PLANNED_FEATURES.md), and the ongoing maintenance that keeps Knot working as Twine, SugarCube, and VS Code evolve. Your name (if you want it) goes into [SUPPORTERS.md](SUPPORTERS.md) and the [sponsors page](https://stormbyte0.github.io/Knot/sponsors.html).

## Community

- **[Discord](https://discord.gg/YYm9nGbtZY)** — support, dev updates, and feature requests. The fastest way to reach the developer.
- **[GitHub Issues](https://github.com/StormByte0/Knot/issues)** — the standard channel for bug reports.

## Documentation

- **[Documentation site](https://stormbyte0.github.io/Knot/)** — onboarding, feature deep-dives, and support tiers.
- **[Extension README](extensions/vscode/README.md)** — commands, settings, troubleshooting, and project configuration.
- **[Extension CHANGELOG](extensions/vscode/CHANGELOG.md)** — release history, plus an *Unreleased* section covering work already merged to `main`.
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — how the Rust server, format plugins, and extension fit together.
- **[ROADMAP.md](ROADMAP.md)** / **[PLANNED_FEATURES.md](PLANNED_FEATURES.md)** — where Knot goes next.
- **[SUPPORTERS.md](SUPPORTERS.md)** — the public thank-you list.

## Building from source

Knot is a Rust workspace with three crates (`core`, `formats`, `server`) plus a VS Code extension:

```
crates/
├── core/       — workspace model, graph, analysis, document editing
├── formats/    — format plugins (SugarCube, Harlowe, Chapbook, Snowman)
└── server/     — LSP server, request handlers, client communication
```

Requires a stable Rust toolchain (rustfmt + clippy for the CI gates) and VS Code 1.85+ for the extension host:

```
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full component map and the extension README for packaging details.

## Built with

- [Tweego](https://www.motoslave.net/tweego/) — the format-agnostic compiler Knot uses to build stories
- [oxc](https://oxc.rs/) — the JavaScript parser powering SugarCube variable analysis
- [tower-lsp](https://github.com/ebkalderon/tower-lsp) — the LSP server framework Knot's Rust server is built on
- [petgraph](https://github.com/petgraph/petgraph) — graph data structures for passage analysis
- [@xyflow/react](https://reactflow.dev/) — Story Map graph visualization
- The Rust and VS Code extension ecosystems

## License

This project is licensed under the **Knot Source-Available License** — free to use for any purpose, including commercial use, but source redistribution, forks, and competing products are not permitted. See [LICENSE](LICENSE) for the full terms.

---

Knot is a passion project. If it makes your interactive fiction workflow better, [say thanks with a coffee](https://ko-fi.com/stormbyte0) — or just tell a fellow author about it.
