# Contributing to Knot

Thank you for your interest in contributing to Knot! This document provides guidelines and instructions for contributing to the project.

---

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Project Architecture](#project-architecture)
- [Development Workflow](#development-workflow)
- [Code Standards](#code-standards)
- [Testing](#testing)
- [Submitting Changes](#submitting-changes)
- [Reporting Issues](#reporting-issues)
- [Adding a New Format Plugin](#adding-a-new-format-plugin)

---

## Code of Conduct

Be respectful, constructive, and inclusive. We are all here to build great tooling for the interactive fiction community. Harassment, discrimination, and toxic behavior will not be tolerated.

---

## Getting Started

1. **Fork** the repository on GitHub
2. **Clone** your fork locally:
   ```bash
   git clone https://github.com/<your-username>/Knot.git
   cd Knot
   git checkout ver_2
   ```
3. **Create a branch** for your changes:
   ```bash
   git checkout -b feature/my-feature
   ```

---

## Development Setup

### Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust | Stable (edition 2024) | Language server |
| Node.js | 20+ | VS Code extension |
| VS Code | 1.85.0+ | Extension host |
| Tweego | Any | Build/play testing (optional) |

### Build the Project

```bash
# Build all Rust crates
cargo build --workspace

# Build the VS Code extension
cd extensions/vscode
npm install
npm run compile
```

### Run in Development Mode

1. Open the `extensions/vscode/` folder in VS Code
2. Press `F5` to launch the Extension Development Host
3. In the new window, open a folder with `.tw` or `.twee` files
4. Knot should activate and the language server should start

To use a locally built Rust server during development:

```bash
# Build the server in debug mode
cargo build -p knot-server

# In your VS Code settings (Extension Development Host):
# Set "knot.server.path" to the full path of target/debug/knot-server
```

---

## Project Architecture

Before contributing, please read [ARCHITECTURE.md](./ARCHITECTURE.md) thoroughly. It is the definitive technical specification for the project and describes every major subsystem.

### Crate Overview

| Crate | Responsibility | Key Files |
|-------|---------------|-----------|
| `knot-core` | Document model, graph engine, analysis, editing | `document.rs`, `graph.rs`, `analysis.rs`, `editing.rs`, `passage.rs`, `workspace.rs` |
| `knot-formats` | Format plugin system and parsers | `plugin.rs`, `sugarcube/`, `harlowe/`, `chapbook/`, `snowman/` |
| `knot-server` | LSP server implementation | `handlers.rs`, `lib.rs`, `lsp_ext.rs`, `state.rs` |
| `extensions/vscode` | VS Code extension client | `extension.ts`, `storyMapProvider.ts`, `playModeProvider.ts`, `debugViewProvider.ts`, `profileViewProvider.ts` |

### Design Principles

- **Format plugins are isolated** — The core engine is format-agnostic. Format-specific logic lives exclusively in the `knot-formats` crate.
- **Single-project workspace model** — One Twine project per workspace, one StoryData passage.
- **Incremental graph surgery** — Graph updates are performed in-place using set diff of passage names, not rebuilt from scratch.
- **Fault-tolerant parsing** — Parsers must never hard-fail. Incomplete syntax produces partial AST nodes with `is_incomplete: true`.
- **StoryData is authoritative** — The StoryData passage is the canonical source for format, entry point, and IFID. Configuration in `knot.json` never overrides it.

---

## Development Workflow

### Branching

- **`ver_2`** is the active development branch for Knot v2
- **`master`** is reserved for stable releases
- Create feature branches from `ver_2`: `feature/your-feature-name`
- Create fix branches from `ver_2`: `fix/issue-description`

### Commit Messages

Use clear, descriptive commit messages. The conventional commit format is encouraged:

```
feat(server): add support for document highlights
fix(core): resolve graph surgery edge case with duplicate passage names
docs(readme): update build instructions for cross-compilation
test(formats): add snapshot tests for Harlowe parser
```

### Keeping Your Branch Updated

```bash
git remote add upstream https://github.com/StormByte0/Knot.git
git fetch upstream
git rebase upstream/ver_2
```

---

## Code Standards

### Rust Code

- **Formatting:** All code must pass `cargo fmt --all -- --check`
- **Linting:** All code must pass `cargo clippy --workspace -- -D warnings`
- **Edition:** Rust edition 2024
- **Error handling:** Use `Result` types properly. Do not use `unwrap()` in production code paths — use `unwrap_or_default()`, `ok()`, or proper error propagation.
- **Locking:** The server uses `RwLock` for shared state. Acquire write locks only when mutating, and drop them before any `await` points or client communication.
- **No version bumps:** Do not modify version numbers in `Cargo.toml` or `package.json`. The maintainers handle version management.

### TypeScript Code

- **Formatting:** Follow the existing code style in the project
- **Strict mode:** TypeScript strict mode is enabled (`tsconfig.json`)
- **API usage:** Use the VS Code API through the `vscode` module. Use `vscode-languageclient` for LSP communication.
- **Webview security:** All webview HTML must use `nonce` attributes and Content Security Policy headers. Never load scripts from untrusted sources.

### General

- **DRY principle:** Avoid duplicating logic. If you see repeated patterns (especially link-parsing code), extract them into shared helper functions.
- **Comments:** Add comments for non-obvious logic. Document public APIs with doc comments (`///` in Rust, `/** */` in TypeScript).
- **No panics in production:** Rust code in the server should use `std::panic::catch_unwind` for request handlers. Never intentionally panic in a handler.

---

## Testing

### Running Tests

```bash
# All Rust tests
cargo test --workspace

# Specific crate
cargo test -p knot-core
cargo test -p knot-formats
cargo test -p knot-server

# With output
cargo test --workspace -- --nocapture

# Specific test
cargo test -p knot-core test_story_data_parsing
```

### Test Categories

| Category | Location | Framework |
|----------|----------|-----------|
| Core analysis tests | `crates/knot-core/src/analysis_tests.rs` | Built-in `#[test]` |
| Format integration tests | `crates/knot-formats/src/integration_tests.rs` | Built-in `#[test]` |
| Workspace tests | `crates/knot-core/src/workspace.rs` | Built-in `#[test]` (inline module) |
| Snapshot tests | (where applicable) | `insta` |
| Property tests | (where applicable) | `proptest` |

### Writing Tests

- **Unit tests** go in the same file as the code they test, in a `#[cfg(test)] mod tests` block, or in a companion `*_tests.rs` file.
- **Integration tests** go in `crates/knot-formats/src/integration_tests.rs`.
- **Snapshot tests** use the `insta` crate. When adding new snapshot tests, run with `cargo insta test` and review with `cargo insta review`.
- **Property-based tests** use `proptest`. These are especially valuable for graph invariants and parser edge cases.

### Test Coverage Expectations

- All new features must include tests
- Bug fixes should include a test that would have caught the bug
- Format parser changes must include parsing test cases for valid, invalid, and incomplete syntax

---

## Submitting Changes

### Pull Request Process

1. **Ensure all checks pass** locally:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace -- -D warnings
   cargo test --workspace
   cd extensions/vscode && npm run compile && npm run lint
   ```

2. **Push your branch** to your fork:
   ```bash
   git push origin feature/my-feature
   ```

3. **Open a Pull Request** against the `ver_2` branch of `StormByte0/Knot`

4. **Fill out the PR template** with:
   - What the change does
   - Why it is needed
   - How it was tested
   - Any breaking changes or migration notes

5. **Address review feedback** promptly and push fixes to the same branch

### PR Requirements

- [ ] All CI checks pass (check, test, fmt, clippy)
- [ ] New code has tests
- [ ] No version bumps in `Cargo.toml` or `package.json`
- [ ] No unnecessary dependencies added
- [ ] Documentation comments on new public APIs
- [ ] ARCHITECTURE.md updated if the change affects the system architecture

---

## Reporting Issues

### Bug Reports

When filing a bug report, please include:

1. **VS Code version** and **OS**
2. **Knot extension version**
3. **Steps to reproduce** the issue
4. **Expected vs. actual behavior**
5. **Relevant log output** from the Output panel (select "Knot" from the dropdown)
6. **Sample .tw/.twee files** that trigger the issue (minimal reproduction if possible)

### Feature Requests

Feature requests are welcome. Please describe:

1. **The problem** you are trying to solve
2. **The proposed solution** and how it would work
3. **Alternatives** you have considered
4. **Which story format(s)** it applies to

---

## Adding a New Format Plugin

Knot uses a plugin system for story format support. Adding a new format involves implementing the `FormatPlugin` trait defined in `crates/knot-formats/src/plugin.rs`.

### Steps

1. **Create a new module** under `crates/knot-formats/src/`:
   ```
   myformat/
   └── mod.rs
   ```

2. **Implement the `FormatPlugin` trait:**
   ```rust
   use crate::plugin::{FormatPlugin, ParseResult, SemanticToken, FormatDiagnostic};
   use knot_core::passage::StoryFormat;

   pub struct MyFormatPlugin;

   impl FormatPlugin for MyFormatPlugin {
       fn format(&self) -> StoryFormat {
           StoryFormat::Custom("MyFormat".into())
       }

       fn parse(&self, text: &str) -> ParseResult {
           // Parse the text and return passages, semantic tokens, diagnostics
           todo!()
       }

       fn parse_passage(&self, text: &str) -> ParseResult {
           // Parse a single passage
           todo!()
       }

       fn special_passages(&self) -> Vec<SpecialPassageDef> {
           // Declare format-specific special passages
           vec![]
       }

       fn is_special_passage(&self, name: &str) -> bool {
           // Check if a passage name is special in this format
           false
       }

       fn display_name(&self) -> &str {
           "MyFormat"
       }
   }
   ```

3. **Register the plugin** in `FormatRegistry::with_defaults()` in `plugin.rs`:
   ```rust
   registry.register(Box::new(myformat::MyFormatPlugin));
   ```

4. **Add the module declaration** in `crates/knot-formats/src/lib.rs`:
   ```rust
   pub mod myformat;
   ```

5. **Write tests** in `integration_tests.rs` covering:
   - Passage boundary detection
   - Link extraction
   - Variable read/write detection
   - Semantic token generation
   - Invalid/incomplete syntax recovery
   - Special passage behavior

6. **Update ARCHITECTURE.md** to document the new format support level

### Key Requirements for Format Plugins

- **Fault-tolerant parsing:** Never hard-fail. Incomplete syntax should produce partial AST nodes.
- **Passage boundaries as sync points:** Errors in one passage must not corrupt subsequent passages.
- **Variable tracking declaration:** Declare whether the format supports cross-passage variable tracking (`StoryFormat` flags).
- **Special passage definitions:** Declare all format-specific special passages with their behavior, variable contribution, and graph participation flags.

---

## Questions?

If you have questions about contributing, feel free to:

- Open a [GitHub Discussion](https://github.com/StormByte0/Knot/discussions)
- Ask in a [GitHub Issue](https://github.com/StormByte0/Knot/issues)
- Reach out to the maintainer (@StormByte0)

Thank you for contributing to Knot and helping make interactive fiction development better!
