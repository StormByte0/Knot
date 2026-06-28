# Knot — Twine IDE for VS Code

A next-generation integrated development environment for Twine and Twee interactive fiction projects. Knot models your entire story as a directed graph, enabling real-time structural analysis, intelligent navigation, and narrative-aware diagnostics that no other Twine tooling provides.

The core language server is written in Rust for high-performance parsing, incremental graph recomputation, and low-latency response times. It communicates with VS Code through the Language Server Protocol.

> **⚠️ Under active development.** Knot is pre-release software. Only **SugarCube 2** currently has full language features (macro catalog, JS-aware variable tracking, special passages, completion, hover). **Harlowe**, **Chapbook**, and **Snowman** have placeholder/skeleton implementations only — the `FormatPlugin` trait is implemented but the parsers are not yet production quality and link extraction is not functional. The build pipeline works for all formats (it delegates to Tweego, which is format-agnostic). See the [Supported Story Formats](#supported-story-formats) matrix below. Expect breaking changes and rough edges until v2.0.0 ships.

---

## Key Features

- **Graph-based architecture** — your story is modeled as a directed graph of passages connected by links. The Story Map visualizes passage relationships and highlights structural issues.
- **Real-time diagnostics** — broken links, unreachable passages, uninitialized variables, duplicate passage names, dead ends, and more — updated as you type.
- **SugarCube-focused analysis** — full macro catalog, JS-aware variable tracking, and special passage support for SugarCube 2. Other formats are planned.
- **Incremental analysis** — only affected passages are re-parsed after each keystroke. Large projects stay responsive.
- **Integrated build pipeline** — one-click build and play via Tweego. No manual compiler setup — Knot downloads and manages Tweego and story formats for you.
- **Watch mode** — toggle auto-rebuild on save. Edit in VS Code, refresh in your browser.
- **Variable tracking** — see where every variable is set, read, and how it flows across passages.
- **Passage diagnostics** — inspect any passage's links, variables, macros, and complexity metrics in a dedicated panel.

---

## Quick Start

### 1. Install Knot

Install from the VS Code Marketplace or by searching "Knot" in the Extensions panel (`Ctrl+Shift+X`).

### 2. Open a Twine Project

Open a folder containing `.tw` or `.twee` files. If you don't have a project yet, Knot will detect the empty workspace and offer to initialize one for you.

A minimal Twine project needs at least two passages:

```
:: StoryData
{
    "ifid": "D674C58C-DEFA-4F70-B7A2-27742230C0FC",
    "format": "SugarCube",
    "format-version": "2.37.0"
}

:: Start
Welcome to your story. [[Continue]].

:: Continue
You made it!
```

The `StoryData` passage tells Knot (and Tweego) which story format to use. The `Start` passage is where the player begins. Knot auto-detects the format from `StoryData` and activates the right language features.

### 3. Build and Play

Click **Build** in the status bar to compile your project into an HTML file. The first build will prompt Knot to download Tweego and the required story format — this happens automatically, no manual setup needed.

Click **Play** to open the compiled story in your default browser. If Watch is off, Play builds first; if Watch is on, Play just opens the already-fresh HTML.

> **Note on keyboard shortcuts:** F5 (Play), F6 (Build), Ctrl+Shift+M (Story Map), and Shift+F5 (Play from Passage) are declared in `package.json` but are scoped to only fire when a `.twee` file is the active editor. In practice these shortcuts may conflict with VS Code defaults; the status bar buttons and Command Palette (`Ctrl+Shift+P`) are the reliable ways to trigger these actions.

### 4. Enable Watch (Optional)

Click **Watch** in the status bar to toggle auto-rebuild on save. When Watch is on, every save of a `.tw`, `.twee`, `.js`, or `.css` file triggers a rebuild in the background. Build progress appears in the "Knot Build" output channel.

This is the recommended workflow for active development: turn Watch on, open Play in your browser, edit in VS Code, and refresh the browser after each save.

### 5. Explore the Story Map

Click **Story Map** in the status bar to open an interactive graph of your project. Nodes are passages, edges are links. Click any node to jump to that passage in the editor. The map updates in real time as you edit.

---

## Status Bar

Knot adds five items to the left side of the status bar:

| Item | Icon | Action |
|---|---|---|
| Story Map | $(compass) | Open the graph visualization |
| Build | $(tools) | Compile the project with Tweego |
| Watch | $(eye) / $(eye-closed) | Toggle auto-rebuild on save |
| Play | $(play) | Open compiled HTML in browser |
| Settings | $(gear) | Open Knot settings |

> **Keyboard shortcuts** (F5, F6, Ctrl+Shift+M, Shift+F5) are declared in `package.json` but may not reliably trigger due to VS Code default keybinding conflicts. Use the status bar buttons or Command Palette instead.

---

## Commands

All commands are available via the Command Palette (`Ctrl+Shift+P`), prefixed with "Knot:".

| Command | Description |
|---|---|
| **Knot: Build Project** | Compile the project to HTML via Tweego |
| **Knot: Play Story** | Build (if needed) and open in default browser |
| **Knot: Play from This Passage** | Build with a specific start passage, then open |
| **Knot: Toggle Watch** | Turn auto-rebuild on save on/off |
| **Knot: Open Story Map** | Open the passage graph visualization |
| **Knot: Open Passage by Name** | Quick-pick a passage and jump to it |
| **Knot: Configure Story Formats** | Manage installed story formats |
| **Knot: Configure Build Toolchain** | Set up or download Tweego |
| **Knot: Detect Compiler** | Check if Tweego is available on PATH |
| **Knot: Re-index Workspace** | Force a full re-index of all files |
| **Knot: Restart Language Server** | Restart the Rust LSP server |
| **Knot: Initialize Project** | Create a basic project skeleton |
| **Knot: Open Managed Storage Folder** | Open the folder where Knot stores Tweego and formats |
| **Knot: Open Tweego Folder** | Open the folder containing the managed Tweego binary |
| **Knot: Open Extension Settings** | Open the VS Code Settings UI filtered to Knot |

---

## Settings

Settings are organized into sections in the VS Code Settings UI. The most important ones for getting started:

### Build

| Setting | Default | Description |
|---|---|---|
| `knot.build.outputDir` | `"build"` | Where the compiled HTML is written, relative to the workspace root |
| `knot.build.sourceDir` | `""` | Subdirectory containing source files (empty = workspace root) |
| `knot.build.tweegoPath` | `""` | Override Tweego binary path (empty = auto-resolve) |
| `knot.build.storyformatsPath` | `""` | Override story formats folder (empty = use managed folder) |
| `knot.build.flags` | `[]` | Additional Tweego command-line flags |

### Diagnostics

Each diagnostic can be set to `error`, `warning`, `info`, `hint`, or `off`. Defaults are tuned for a balance of catching real issues without being noisy.

### Indexing

| Setting | Default | Description |
|---|---|---|
| `knot.indexing.maxFiles` | `1000` | Maximum files to index before stopping with a warning |
| `knot.indexing.exclude` | `[]` | Glob patterns to exclude from indexing |

### Status & Paths

Read-only settings showing where Knot has installed things:
- `knot.managed.storagePath` — extension storage root
- `knot.managed.tweegoPath` — downloaded Tweego binary
- `knot.managed.storyformatsPath` — managed formats cache
- `knot.resolved.tweegoPath` — the actual binary that will be used for the next build

### Advanced

| Setting | Default | Description |
|---|---|---|
| `knot.server.path` | `""` | Override the knot-server binary (for debugging) |
| `knot.trace.server` | `"off"` | LSP trace level for debugging |

---

## Project Configuration

In addition to VS Code settings, Knot reads project-level configuration from `.vscode/knot.json`. This file is checked into the repo and is useful for settings that should be shared across all contributors (or just persisted with the project):

```json
{
    "build": {
        "source_dir": "src",
        "output_dir": "dist",
        "flags": ["--no-trim"]
    },
    "ignore": ["build/**", "node_modules"],
    "max_files": 500
}
```

VS Code settings take priority over `.vscode/knot.json` for the same field. For `build.flags`, both sets are merged.

---

## How the Build Pipeline Works

Knot's build pipeline is designed to "just work" with zero manual setup:

1. **Tweego is auto-downloaded** on first build into the extension's global storage. You never need to install it yourself.
2. **Story formats are auto-downloaded** based on your project's `StoryData` format and version. The format is cached per version in `<globalStorage>/storyformats/<id>@<version>/`.
3. **The workspace is the source directory** — put all your `.twee`, `.js`, `.css`, and asset files directly in the workspace. Story formats live separately in the managed folder, so there's no risk of `format.js` getting bundled as a passage.
4. **The output filename is derived from `StoryTitle`** — if your story is called "The Lost City", the build produces `The_Lost_City.html`. Falls back to `index.html` if no `StoryTitle` passage exists.
5. **Build stats are logged** — every build prints `Passages: N | Words: N` to the Knot Build output channel, so you can track your project size at a glance.

Build output appears in the "Knot Build" output channel (`View → Output → Knot Build`). The server logs every resolution decision (which tweego binary, which formats directory, which source path) so you can debug build failures.

---

## Supported Story Formats

Knot currently has **SugarCube 2** as its only production-quality format plugin with full language features. The other three Twine formats (Harlowe, Chapbook, Snowman) have placeholder/skeleton implementations — the `FormatPlugin` trait is implemented for each, but the parsers have not been completed to production quality and link extraction is not yet functional. The build pipeline works for all formats because it delegates to Tweego, which is format-agnostic.

| Format | Status | What works | What doesn't (yet) |
|---|---|---|---|
| **SugarCube 2** | ✅ Full support | Full macro catalog (~120 builtins), JS-aware variable tracking via oxc, special passages, completion, hover, diagnostics, Story Map, build pipeline | — |
| **Harlowe 3** | ◐ Placeholder only | `FormatPlugin` trait implemented; build pipeline (via Tweego) | Parser quality, link extraction, Story Map visualization, macro catalog, variable tracking, completion, hover |
| **Chapbook 1** | ◐ Placeholder only | `FormatPlugin` trait implemented; build pipeline (via Tweego) | Parser quality, link extraction, Story Map visualization, macro catalog, variable tracking, completion, hover |
| **Snowman 2** | ◐ Placeholder only | `FormatPlugin` trait implemented; build pipeline (via Tweego) | Parser quality, link extraction, Story Map visualization, ERB template detection, macro catalog, variable tracking, completion, hover |

Knot auto-detects the format from your `StoryData` passage. For non-SugarCube formats, the Language Status indicator in the lower-right of the editor will show a `◐` marker indicating the format is not yet fully supported. If you're writing a SugarCube story, all language features activate automatically.

Bringing Harlowe, Chapbook, and Snowman to full parity is planned — see [ROADMAP.md](../../ROADMAP.md) for details.

---

## Requirements

- VS Code 1.85.0 or later
- An internet connection on first build (to download Tweego and story formats — after that, everything works offline)

No manual installation of Tweego, Node.js, or any other dependency is required.

---

## Troubleshooting

**Build fails with "No Tweego compiler found"**
Run the `Knot: Configure Build Toolchain` command to download Tweego automatically, or set `knot.build.tweegoPath` to point at a local Tweego binary.

**Build fails with "story format not found"**
Run the `Knot: Configure Story Formats` command. If your `StoryData` specifies a format and version, Knot will offer a one-click download.

**Diagnostics not updating**
Try `Knot: Re-index Workspace` to force a full reparse. If that doesn't help, `Knot: Restart Language Server` will restart the Rust server.

**Play opens a stale HTML file**
Make sure Watch is toggled on (the eye icon in the status bar should be open, not closed). If Watch is off, Play only builds if no HTML exists yet — otherwise it opens the existing file.

**Language server crashes repeatedly**
Knot has automatic crash recovery, but if it keeps happening, set `knot.trace.server` to `"verbose"`, reproduce the crash, then check the "Knot" output channel for the stack trace. Report it via GitHub Issues.

---

## Roadmap

Knot is under active development. See [ROADMAP.md](../../ROADMAP.md) for long-term architectural goals and [PLANNED_FEATURES.md](../../PLANNED_FEATURES.md) for near-term feature candidates.

---

## Support the Project

Knot is a passion project. If Knot makes your interactive fiction workflow better, consider supporting its continued development on **Patreon** or **Ko-fi**:

- **[Become a Patron →](https://patreon.com/StormByte0)**
- **[Buy a coffee on Ko-fi →](https://ko-fi.com/stormbyte0)**

Support funds:
- Completing the Harlowe, Chapbook, and Snowman format plugins
- Building the planned features in [PLANNED_FEATURES.md](../../PLANNED_FEATURES.md) (project initialization, decompile, passage organization, and more)
- Ongoing maintenance, bug fixes, and Twine/SugarCube version tracking
- Keeping Knot free to use for everyone, including commercial Twine authors

Patreon tiers and benefits are being set up — check the Patreon page for the latest. Every supporter gets listed in the Credits below.

---

## Credits

### Author

- **StormByte0** — [GitHub](https://github.com/StormByte0) · [Patreon](https://patreon.com/StormByte0) · [Ko-fi](https://ko-fi.com/stormbyte0)

### Patrons

Knot is funded by its community of supporters. Patrons will be listed here once the support program launches.

*Become a patron to see your name here — [patreon.com/StormByte0](https://patreon.com/StormByte0)*

### Built With

- [Tweego](https://www.motoslave.net/tweego/) — the compiler Knot uses to build stories
- [oxc](https://oxc-project.github.io/) — JavaScript parser powering SugarCube variable analysis
- [tower-lsp](https://github.com/silvanshade/lsp-server) — LSP server framework for Rust
- [petgraph](https://github.com/petgraph/petgraph) — graph data structures for passage analysis
- [@xyflow/react](https://reactflow.dev/) — Story Map graph visualization
- The Rust and VS Code extension ecosystems

---

## License

This project is licensed under the **Knot Source-Available License**. You are free to use Knot for any purpose, including commercial use, but you may not redistribute the source, create forks, or use it to build a competing product. See [LICENSE](../../LICENSE) for the full terms.

---

## Links

- **Repository:** [https://github.com/StormByte0/Knot](https://github.com/StormByte0/Knot)
- **Issues:** [https://github.com/StormByte0/Knot/issues](https://github.com/StormByte0/Knot/issues)
- **Patreon:** [https://patreon.com/StormByte0](https://patreon.com/StormByte0)
- **Ko-fi:** [https://ko-fi.com/stormbyte0](https://ko-fi.com/stormbyte0)
- **Tweego:** [https://www.motoslave.net/tweego/](https://www.motoslave.net/tweego/)
- **Twine:** [https://twinery.org/](https://twinery.org/)
