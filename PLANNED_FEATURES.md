# Planned Features

This file tracks features that have been discussed and deferred for future
implementation. Each entry includes the use case, proposed approach, and
rough effort estimate. Entries that sit here for multiple releases without
progress should be deleted or promoted to a dedicated issue tracker.

---

## 1. Decompile HTML → Twee Source

**Use case**: Migrating from Twine GUI to Knot. Users have existing
`.html` files (compiled Twine stories) and want to extract the passages
into editable `.twee` source files.

**Proposed command**: `Knot: Decompile HTML to Twee`

**Approach**:
- User runs the command, picks an `.html` file via file picker
- Extension calls tweego with `-d` (decompile to Twee v3) and `-o` pointing
  at a new `.twee` file in the workspace
- Output filename derived from the HTML filename (e.g. `story.html` → `story.twee`)
- After decompile, the new file opens in the editor

**Dependencies**: None — tweego's `-d` flag is fully featured.

**Effort**: Small (1-2 hours). Pure extension-side work, no server changes.

---

## 2. Project Initialization Skeleton

**Use case**: New users creating a Knot project from scratch need a
starting point with the required special passages and a sensible structure.

**Current state**: `knot.initProject` exists but creates a minimal stub.
No skeleton generation, no special passage reference, no StorySettings
template for SugarCube.

**Proposed approach**:
- Generate a `00-meta.twee` file containing:
  - `StoryData` passage with a fresh IFID (use existing `knot.generateIfid`)
  - `StoryTitle` passage with a placeholder title
  - `StorySubtitle`, `StoryAuthor` passages (commented out)
- Generate a `01-start.twee` file with a `Start` passage containing
  placeholder text
- Generate a `99-special-passages-reference.twee` file with ALL special
  passage names (SugarCube-focused) as commented-out headers with
  descriptions, so users can uncomment what they need:
  ```
  :: StoryCaption
  // Shown in the sidebar UI. Updated on every passage change.
  
  :: StoryInit
  // Runs once when the story starts. Initialize variables here.
  
  :: PassageHeader
  // Prepended to every passage's content.
  
  ... etc
  ```
- Optionally generate a `.vscode/knot.json` with sensible defaults

**Dependencies**: None — pure extension-side work.

**Effort**: Medium (3-4 hours). The special passage reference is the bulk
of the work — needs research per format (SugarCube, Harlowe, Chapbook,
Snowman) to list all special passages with accurate descriptions.

---

## 3. Send Passage to New File

**Use case**: Users write many passages in a single `.twee` file during
drafting, then want to split them into separate files for organization.

**Proposed command**: `Knot: Move Passage to New File`
- Triggered from the editor context menu when the cursor is in a passage
- Asks for a filename (default: sanitized passage name)
- Removes the passage from the current file and creates a new `.twee`
  file with just that passage

**Dependencies**:
- Server support for "move passage" operation (currently no such LSP
  request exists). Would need a new `knot/passageMove` request that
  takes passage name, source file, and target file.
- Or: extension-side implementation using text manipulation (simpler but
  less robust — doesn't handle edge cases like passage metadata blocks)

**Effort**: Medium (4-6 hours). Server-side approach is cleaner but more
work; extension-side is faster but fragile.

---

## 4. Passage Organization (Bulk Operations)

**Use case**: Reorganizing passages across files — moving groups of
passages, renaming files, merging files, splitting by tag.

**Proposed approach**:
- Integrate with the Story Map webview — allow drag-and-drop of passages
  between "file groups" shown in the map
- Add a "Passage Organizer" panel (separate webview or tree view) that
  shows all passages grouped by file, with drag-and-drop reorganization
- Support bulk operations: select multiple passages, move to new/existing
  file, rename, retag

**Dependencies**:
- Significant server work — needs `knot/passageMove`, `knot/passageRename`,
  `knot/fileMerge`, `knot/fileSplit` LSP requests
- Story Map webview enhancements for drag-and-drop between file groups
- New tree view or webview for the organizer panel

**Effort**: Large (2-3 days). This is a major feature, not a quick win.

---

## 5. Twine Archive Export

**Use case**: Backing up a project as a Twine archive (XML format), or
migrating back to Twine GUI.

**Proposed command**: `Knot: Export Twine Archive`

**Approach**:
- User runs the command, picks an output location
- Extension calls tweego with `--archive-twine2` and `-o` pointing at the
  chosen `.html` (archive) file
- Archive format is XML, importable by Twine 2 GUI

**Dependencies**: None — tweego's `--archive-twine2` flag is fully featured.

**Effort**: Small (1 hour). Pure extension-side work.

---

## 6. Test Mode

**Use case**: Testing a story with the format's debug features enabled
(e.g. SugarCube's `Config.debug` view shows the passage hierarchy,
variables, and history).

**Proposed command**: `Knot: Play in Test Mode`

**Approach**:
- Same as `knot.play` but appends `-t` to the tweego args
- Could be a separate command, or a toggle in a "Play" dropdown
- Test mode only works for Twine 2-style formats (SugarCube, Harlowe, etc.)

**Dependencies**: None — tweego's `-t` flag is fully featured.

**Effort**: Small (1 hour). Pure extension-side work.

---

## 7. Module Bundling (`-m` flag)

**Use case**: Bundling assets (CSS, JS, fonts, images) from outside the
workspace source tree — e.g. a shared `~/twine-assets/` directory used
across multiple projects.

**Proposed setting**: `knot.build.moduleDirs` (array of strings, default [])
- Each entry is a path (relative or absolute) to a directory of modules
- Server adds one `-m <dir>` flag per entry

**Dependencies**:
- Server-side: add `module_dirs` field to `KnotBuildParams` and `BuildConfig`
- Extension-side: add setting to `contributes.configuration`

**Effort**: Small (2 hours).

---

## 8. Custom `<head>` Content (`--head` flag)

**Use case**: Injecting analytics tags, third-party SDK snippets, or meta
tags that don't belong in a stylesheet/script passage.

**Proposed setting**: `knot.build.headFile` (string, default empty)
- Path to an HTML file whose contents are appended to `<head>` in the
  compiled output
- Server passes `--head <path>` to tweego when set

**Dependencies**:
- Server-side: add `head_file` field to `KnotBuildParams` and `BuildConfig`
- Extension-side: add setting to `contributes.configuration`

**Effort**: Small (1 hour).

---

## Priority Guidance

**Ship next** (priority #1, blocks smooth onboarding):
- #2 Project Initialization Skeleton (onboarding — this is the top priority)

**Ship after** (quick wins, high value):
- #1 Decompile HTML → Twee (migration use case is critical for adoption)
- #5 Twine Archive Export (backup/migration)
- #6 Test Mode (debugging aid)
- #7 Module Bundling
- #8 Custom `<head>` Content

**Ship last** (large effort, plan carefully):
- #3 Send Passage to New File
- #4 Passage Organization (bulk operations)

**Not planned** (removed from roadmap):
- ~~Format Override~~ — not currently planned for implementation.
