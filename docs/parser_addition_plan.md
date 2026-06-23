# Knot Block-Level Markup & Macro Catalog — Implementation Plan

> **Document purpose**: This is the single source of truth for the block-level markup and macro catalog overhaul. Read this document at the start of every session to catch up on what was previously done. Append to the **Worklog** and **Phase Status** sections at the end of each session.
>
> **Last updated**: 2026-06-23 (initial creation)
> **Author**: Super Z (planning phase)
> **Status**: PLANNING — awaiting user sign-off before any code changes

---

## Table of Contents

1. [How to Use This Document](#1-how-to-use-this-document)
2. [Executive Summary](#2-executive-summary)
3. [Research Findings (Authoritative)](#3-research-findings-authoritative)
4. [Current State Analysis — What's Wrong in Knot](#4-current-state-analysis--whats-wrong-in-knot)
5. [Architectural Decisions](#5-architectural-decisions)
6. [Phased Implementation Plan](#6-phased-implementation-plan)
7. [Per-Phase Detailed Specifications](#7-per-phase-detailed-specifications)
8. [Risk Analysis](#8-risk-analysis)
9. [Open Questions](#9-open-questions)
10. [Phase Status](#10-phase-status)
11. [Worklog](#11-worklog)

---

## 1. How to Use This Document

### For the AI agent (start of every session)

1. **Read this entire document first** — especially Section 10 (Phase Status) and Section 11 (Worklog) to understand what has been completed.
2. Check the current phase in Section 10. If a phase is `IN_PROGRESS`, resume from where the last session left off (the worklog entry will say what was last done).
3. Follow the per-phase spec in Section 7 for the current phase.
4. **Do NOT skip ahead** — phases have dependencies. Phase 1 (foundation) must complete before Phase 2, etc.
5. After completing a phase (or a significant sub-task within a phase), append to Section 10 (mark phase status) and Section 11 (worklog entry).
6. Zip modified files to `/home/z/my-project/download/` after each phase per the workflow rules.

### For the user

- Section 3 contains the frozen research findings — cite this when reviewing implementation correctness.
- Section 4 documents every known bug/gap in the current Knot codebase.
- Section 6 is the high-level roadmap; Section 7 has the implementation-level detail per phase.
- Section 9 lists questions that still need your input (will be updated as questions are resolved).

### Conventions

- **SugarCube version targeted**: v2.37.3 (latest stable as of research date). There is NO SugarCube v3.x.
- **Source-of-truth URLs**:
  - Official docs: `https://www.motoslave.net/sugarcube/2/docs/`
  - Source code: `https://github.com/tmedwards/sugarcube-2` (default branch: `develop`)
- **Citation format**: `[SRC:parserlib.js:1123]` means SugarCube source file `src/markup/parserlib.js` line 1123. `[DOCS:markup#heading]` means the official markup docs, heading section.

---

## 2. Executive Summary

### The problem

Knot's SugarCube parser currently has **zero support for block-level markup** (headings, horizontal rules, lists, blockquotes, tables, code blocks) and **incorrect/incomplete macro catalog entries** for ~20 macros. Two of these gaps are critical bugs:

1. **Macros execute inside `{{{...}}}` code blocks** — SugarCube treats code block content as raw/literal text, but Knot's parser lets `<<set>>` inside `{{{...}}}` mutate variables.
2. **`<<checkbox>>` / `<<radiobutton>>` arg schemas are fundamentally wrong** — extra "label" arg, wrong positions, swapped checked/unchecked values for checkbox.

### The scope

This plan covers:

- **Block-level markup**: headings (`!`), horizontal rules (`----`), lists (`*`/`#`), blockquotes (`>` and `<<<`), tables (`|...|`), code blocks (`{{{...}}}`), inline code (`{{{...}}}` inline form).
- **Macro catalog fixes**: correct arg schemas for ~20 macros, remove non-existent macros (`<<style>>`, `<<css>>`), add missing macros (`<<silent>>`).
- **Architectural refactor**: line-start tracking in the parser, generalized raw-body mechanism, new AST node variants, new semantic token types.

### Critical correction from research

**SugarCube does NOT use Marked.js or any Markdown parser.** It uses a TiddlyWiki-derived "Wikifier" with its own grammar. This means:

- Lists are `*`/`**`/`***` and `#`/`##`/`###` ONLY — NO Markdown `-`/`+`/`1.` syntax.
- NO mixed markers like `*#` (TiddlyWiki supports this; SugarCube's regex deliberately does not).
- NO leading whitespace allowed before block markers (SugarCube is stricter than Markdown's 3-space rule).
- NO backtick `` `code` `` inline code — SugarCube uses `{{{...}}}` for BOTH inline and block code (disambiguated by whether `{{{` is followed by a newline).
- Tables use TiddlyWiki syntax with `h`/`f`/`c`/`k` row-type suffixes, NOT GFM pipe-table delimiter rows.
- Blockquote has TWO forms: `>` (line-style, documented) and `<<<`...`<<<` (block-style, undocumented but in source).

### Execution approach

Seven phases, ordered by dependency and priority:

| Phase | Focus | Risk | Est. effort |
|---|---|---|---|
| 1 | Foundation: line-start tracking + AST variants + token types (no behavior change) | Medium — refactor of `parse_body` | Large |
| 2 | Critical bugs: code blocks (`{{{...}}}`) + checkbox/radiobutton | Low — well-scoped | Medium |
| 3 | Headings (`!`) + inline code (recursive content) | Low | Medium |
| 4 | Simple blocks: horizontal rule, blockquote (both forms) | Low | Small |
| 5 | Lists (`*`/`#`) with nesting | Medium — depth tracking | Medium |
| 6 | Tables (TiddlyWiki syntax) | Medium — multi-line, row types | Large |
| 7 | Macro catalog overhaul (all ~20 macros + remove `<<style>>`/`<<css>>` + add `<<silent>>`) | Medium — many small changes | Large |

---

## 3. Research Findings (Authoritative)

> This section is FROZEN — it documents verified SugarCube behavior with citations. Do not edit during implementation; if new findings emerge, add them to Section 11 (Worklog) and update Section 4 if needed.

### 3.1 SugarCube's markup engine

**SugarCube does NOT use Marked.js, CommonMark, or any Markdown parser.** It uses a TiddlyWiki-derived parser called the **Wikifier**.

- Source: `src/markup/wikifier.js` (engine + profile system), `src/markup/parserlib.js` (parser handlers), `src/markup/lexer.js` (generic lexer).
- The `cmark-gfm-js` entry in `package.json` is a **devDependency** for build tooling — NOT used at runtime.
- Confirmed by full-text search of markup docs: zero occurrences of "markdown", "marked", or "marked.js".
- The v1 docs explicitly state: *"SugarCube primarily uses TiddlyWiki markup … for its markup language."* [DOCS:v1-markup]

### 3.2 Profile system (critical for raw zones)

Each parser declares a `profiles` array. Two profiles are compiled:

- `'all'` — every parser (used for top-level passage wikification).
- `'core'` — parsers with no `profiles` array, or with `'core'` in it (used for inline contexts where block markup should be suppressed — e.g. inside inline code, links).

Block-level parsers (heading, list, table, both blockquote forms, `monospacedByBlock`) declare `profiles: ['block']`, so they are **excluded from the `'core'` profile**. [SRC:wikifier.js:42]

### 3.3 Universal anchor rule

**Every block parser's `match` regex begins with `^` and uses `gm` flags.** `^` matches only at column 0 (start of line / start of string). **No block construct in SugarCube tolerates leading whitespace.** This is the opposite of Markdown's 3-space rule. [SRC:parserlib.js — all block parsers]

### 3.4 Processing model — single-pass interleaved scan

There is **no separate macro pass and no separate markdown pass**. The Wikifier runs a single-pass scan where all parsers compete equally. The earliest-matching parser at each cursor position wins.

Two handler shapes determine whether nested constructs execute:

| Handler shape | Examples | Macros/vars/links inside? |
|---|---|---|
| `w.subWikify(<element>, terminator)` — recursively re-wikifies | heading, list, blockquote, table, customStyle (`@@`), formatByChar styles (`''`, `//`, etc.) | **YES — executed** |
| `.text(content)` or `.append(rawHtml)` — emits content without re-wikifying | monospacedByBlock, formatByChar `{{{` case, verbatimText, verbatimHtml, comments | **NO — literal/raw** |

[SRC:wikifier.js:123-218, parserlib.js — all handlers]

### 3.5 Headings (`!` … `!!!!!!`)

- **Syntax**: `^!{1,6}` — 1 to 6 exclamation marks at column 0. [SRC:parserlib.js:1123-1140]
- **Levels**: `!`=h1, `!!`=h2, `!!!`=h3, `!!!!`=h4, `!!!!!`=h5, `!!!!!!`=h6.
- **Required space after markers?** NO. `!Heading` and `! Heading` are both valid; the space becomes part of the rendered text.
- **Leading whitespace?** NONE allowed. One leading space disables the heading.
- **7th `!`**: `!!!!!!!` matches as h6 with the 7th `!` becoming the first character of heading text.
- **Macro/variable/link processing INSIDE headings**: **YES — all processed.** The handler calls `w.subWikify(<hN>, '\n')`, which recursively runs the full Wikifier (including macro, nakedVariable, link, formatByChar parsers).
- **HTML output**: `<h1>` … `<h6>`. Level = `w.matchLength` (number of `!`).
- **Behavior verification**: For `! Some <<set $x to 1>> heading`:
  - `<<set $x to 1>>` **executes**, mutating `$x` and producing no visible output.
  - Rendered HTML: `<h1>Some  heading</h1>` (gap where `<<set>>` was).
  - [SRC:parserlib.js:1132 — `w.subWikify(...)` call]

### 3.6 Horizontal rules (`----`)

- **Syntax**: `^----+\s*$` — 4 or more dashes, alone on the line (only trailing whitespace allowed). [SRC:parserlib.js:1009-1017]
- **3 dashes (`---`)?** NOT valid — renders as literal text (likely `—` + `-` due to the `emdash` parser converting `--` → U+2014).
- **Leading whitespace?** None allowed.
- **HTML output**: `<hr>` (void element).
- **Profile**: `'core'` (unusual for a block element, but the `^----+\s*$` requirement effectively limits it to block usage).

### 3.7 Lists (`*` / `#`)

- **Syntax**: `^(?:(?:\*+)|(?:#+))` — a run of all-`*` or all-`#` at column 0. [SRC:parserlib.js:1307-1375]
- **Unordered**: `*` = ul level 1, `**` = ul level 2, `***` = ul level 3, etc. (no hardcoded depth limit).
- **Ordered**: `#` = ol level 1, `##` = ol level 2, `###` = ol level 3, etc.
- **Nesting mechanism**: depth = **count of marker characters**, NOT indentation. `**` is always level 2 regardless of what precedes it.
- **Mixed markers (`*#`)?** **NOT supported.** The regex `^(?:(\*+)|(#+))` matches either a run of all-`*` or all-`#` — it does NOT match mixed sequences. `*#` would match only the first `*` (level-1 ul), and the `#` would be literal list-item text.
- **Same-depth type switching**: Supported. `*` then `**` (ul→ul nested), then `##` at the same depth as `**` switches the type ul→ol for that depth. [SRC:parserlib.js:1355-1362]
- **Required space after marker?** NO. `*item` and `* item` both work; the space becomes part of the item text.
- **Leading whitespace?** None allowed.
- **Standard Markdown list syntax?** **NOT supported.** No `-`, `+`, `1.`, no indentation-based nesting.
- **HTML output**: `<ul>`/`<ol>` wrapping `<li>` elements, properly nested by depth.
- **Macro/variable/link processing INSIDE list items**: **YES.** `w.subWikify(<li>, '\n')` is called for each item.
- **Docs coverage**: Only single-level examples documented. Multi-level behavior is source-only.

### 3.8 Blockquotes

**Two separate parsers exist.**

#### 3.8.1 Line-style blockquote (`>`, `>>`, etc.)

- **Syntax**: `^>+` — one or more `>` at column 0. [SRC:parserlib.js:60-111]
- **Levels**: `>` = level 1, `>>` = level 2, `>>>` = level 3, etc. (no hardcoded limit).
- **Required space after `>`?** NO. `>Text` and `> Text` both work.
- **Leading whitespace?** None allowed.
- **HTML output**: Nested `<blockquote>` elements. A `<br>` is appended after each line's content within a blockquote level.
- **Multi-line behavior**: Every line of a multi-line blockquote must begin with `>`. No "lazy continuation" (a line without `>` ends the blockquote).
- **Macro/variable/link processing**: **YES.** Each line's content is `subWikify`'d.
- **Docs coverage**: Documented. [DOCS:markup#blockquote]

#### 3.8.2 Block-style blockquote (`<<<` ... `<<<`) — UNDOCUMENTED

- **Syntax**: `^<<<\n` opens; `^<<<\n` closes. [SRC:parserlib.js:39-58]
- **Behavior**: A line consisting of exactly `<<<` opens a blockquote; another `<<<` line closes it. Everything between (may span multiple paragraphs) is wrapped in a single `<blockquote>`.
- **Opening/closing `<<<`**: Must be on its own line (the regex requires `^<<<\n`).
- **Macro/variable/link processing**: **YES** (via `subWikify`).
- **Docs coverage**: **NOT documented** in official v2 markup docs. Source-only. This is a TiddlyWiki holdover.

### 3.9 Tables — TiddlyWiki syntax (UNDOCUMENTED)

- **Syntax**: `^\|(?:[^\n]*)\|(?:[fhck]?)$` — each row is a line starting with `|`, ending with `|`, optionally followed by a one-letter row-type suffix. [SRC:parserlib.js:1142-1305]
- **Cells**: Separated by `|`.
- **Row-type suffix** (after the closing `|`):

| Suffix | Meaning | HTML container |
|---|---|---|
| (none) | body row | `<tbody>` |
| `h` | header row group | `<thead>` |
| `f` | footer row group | `<tfoot>` |
| `c` | caption (entire row content is caption text) | `<caption>` |
| `k` | CSS class assignment (row content is the class name; applied to `<table>`) | — |

- **Header cells**: A cell whose content begins with `!` renders as `<th>` instead of `<td>`.
- **Colspan**: A cell containing only `>` merges with the next cell.
- **Rowspan**: A cell containing only `~` extends the cell from the row above downward.
- **Alignment via whitespace**: Leading/trailing spaces within a cell set `text-align`:
  - Space on both sides → `center`
  - Leading space only → `right`
  - Trailing space only → `left`
- **Inline CSS**: `#id`, `.class`, `prop:value;` work in cell prefixes via `Wikifier.helpers.inlineCss()`.
- **HTML output**: `<table>` containing `<thead>`/`<tbody>`/`<tfoot>`/`<caption>`, with `<tr>`, `<th>`, `<td>`.
- **Macro/variable/link processing INSIDE cells**: **YES.** Each cell content is `w.subWikify($cell, cellTerminator)`. [SRC:parserlib.js:1274]
- **GFM pipe-table support?** NO. GFM requires a `| --- | --- |` delimiter row after the header; SugarCube uses the `h` suffix instead. A GFM delimiter row would be parsed as a normal body row whose cells contain `---`.
- **Docs coverage**: **NOT documented** in official v2 markup docs. Source-only.

### 3.10 Code blocks (`{{{...}}}`)

**Two forms exist, disambiguated by position:**

#### 3.10.1 Block code

- **Syntax**: Opening `{{{\n` (three braces immediately followed by a newline, alone on the line). Closing `}}}` alone on its own line. [SRC:parserlib.js:873-893, name `monospacedByBlock`]
- **Lookahead**: `/^\{\{\{\n((?:^[^\n]*\n)+?)(^\}\}\}$\n?)/gm`
- **Content**: Everything between the opening `{{{`+newline and the closing `}}}` line.
- **Content handling**: **RAW / LITERAL.** The handler uses `jQuery(...).text(match[1])`:
  - HTML-escapes the content (`<`, `>`, `&` → entities).
  - Does NOT call `subWikify` — no macros, no variable interpolation, no links, no SugarCube markup.
- **HTML output**: `<pre><code>…</code></pre>`.
- **Language hint?** NO. The opening regex `^\{\{\{\n` requires the newline immediately after `{{{`, leaving no room for a language identifier. SugarCube does not perform syntax highlighting on code blocks.
- **Leading whitespace on closing `}}}`?** Not allowed — the closing regex is `^\}\}\}$`, requiring `}}}` at column 0.

#### 3.10.2 Inline code

- **Syntax**: `{{{...}}}` appearing inline (i.e. `{{{` NOT at the start of a line / NOT immediately followed by a newline). [SRC:parserlib.js:895-944, `formatByChar` parser, `{{{` case]
- **Lookahead**: `/\{\{\{((?:.|\n)*?)\}\}\}/gm` — non-greedy, first `}}}` closes it.
- **Content handling**: **RAW / LITERAL.** Same as block code: `.text(match[1])`, no `subWikify`.
- **HTML output**: `<code>…</code>` (no `<pre>` wrapper).
- **Can appear mid-paragraph?** YES — that is its primary use. The inline form is in the `'core'` profile, so it fires anywhere in running text.
- **Backtick `` `code` `` support?** **NO.** There is no backtick parser anywhere in `parserlib.js`. A backtick in SugarCube source is literal text.
- **`{{...}}` (double-brace) support?** **NO.** That is TiddlyWiki macro-transclusion syntax, not used by SugarCube for code.

#### 3.10.3 Behavior verification

For `{{{ <<set $x to 1>> }}}` (single line, inline code):
- `<<set $x to 1>>` **does NOT execute**. `$x` is unchanged.
- Rendered HTML: `<code>&lt;&lt;set $x to 1&gt;&gt;</code>` (HTML-escaped literal text).

For multi-line block code:
```
{{{
<<set $x to 1>>
}}}
```
- `<<set $x to 1>>` **does NOT execute**. `$x` is unchanged.
- Rendered HTML: `<pre><code>&lt;&lt;set $x to 1&gt;&gt;\n</code></pre>`.

### 3.11 Other markup constructs (for awareness)

These exist in SugarCube but are **out of scope** for this plan unless explicitly added later:

| Construct | Syntax | Profile | Raw zone? | Documented? |
|---|---|---|---|---|
| Verbatim text | `"""..."""` or `<nowiki>...</nowiki>` | core | YES — `.text()`, no `subWikify` | YES |
| Verbatim HTML | `<html>...</html>` | core | YES — `.append(rawHtml)`, no `subWikify` | YES |
| Verbatim script tag | `<script>...</script>` | core | YES — raw passthrough | (source only) |
| Style tag | `<style>...</style>` | core | YES — raw passthrough | (source only) |
| SVG tag | `<svg>...</svg>` | core | YES — with attribute directives | (source only) |
| Custom style (inline) | `@@style;text@@` | core | NO — `subWikify` on text | YES |
| Custom style (block) | `@@style;\ntext\n@@` | core | NO — `subWikify` on text | YES |
| Emdash | `--` → `—` | core | n/a | YES |
| Double dollar | `$$` → literal `$` | core | n/a | YES |
| Templates | `?name` | core | n/a | YES |
| HTML char ref | `&entity;` / `&#NNN;` | core | n/a | YES |

### 3.12 SugarCube macro catalog — complete inventory

**59 builtin macros + 8 sub-macros** documented in official v2 docs (v2.37.3).

#### Full macro list (categorized)

**Variables** (`#macros-variables`):
- `capture variableList` — block, body required. v2.14.0.
- `set expression` — inline. v2.0.0.
- `unset variableList` — inline. v2.0.0 (+object props v2.37.0).

**Scripting** (`#macros-scripting`):
- `run expression` — inline. Identical to `<<set>>`.
- `script [language]` — block, body required, **RAW BODY**. `language` ∈ {`JavaScript`, `TwineScript`} (default JS, case-insensitive). v2.0.0; `language` opt v2.37.0.

**Display** (`#macros-display`):
- `= expression` — inline. Alias for `<<print>>`.
- `- expression` — inline. Like `<<print>>` but HTML-encodes output.
- `do [tag tags] [element tag]` — block, body required. **NEW v2.37.0.** Pairs with `<<redo>>`.
- `include passageName [elementName]` *or* `include linkMarkup [elementName]` — inline. v2.15.0.
- `nobr` — block, body required. Strips leading/trailing newlines, collapses others to single spaces.
- `print expression` — inline.
- `redo [tags]` — inline. **NEW v2.37.0.**
- `silent` — block, body required. **NEW v2.37.0.** Replacement for `<<silently>>`. Discards body output.
- `silently` — block, body required. **DEPRECATED v2.37.0** → use `<<silent>>`.
- `type speed [start delay] [class classes] [element tag] [id ID] [keep|none] [skipkey key]` — block, body required. v2.32.0.

**Control** (`#macros-control`):
- `if conditional` — block, body required.
- ↳ `elseif conditional` — sub-macro of `if`, body required.
- ↳ `else` — sub-macro of `if`, body required.
- `for [conditional]` *or* `for [init] ; [conditional] ; [post]` *or* `for [[keyVariable ,] valueVariable] range collection` — block, body required. Uses `range` keyword (NOT `in`/`to`).
- ↳ `break` — sub-macro of `for`, inline.
- ↳ `continue` — sub-macro of `for`, inline.
- `switch expression` — block, body required. v2.7.2.
- ↳ `case valueList` — sub-macro of `switch`, body required. **Variadic** (space-separated values).
- ↳ `default` — sub-macro of `switch`, body required.

**Interactive** (`#macros-interactive`):
- `button linkText [passageName]` *or* `button linkMarkup` *or* `button imageMarkup` — block, body required. v2.8.0. Identical to `<<link>>` but `<button>`.
- `checkbox receiverName uncheckedValue checkedValue [autocheck|checked]` — inline. v2.0.0 (`autocheck` v2.32.0).
- `cycle receiverName [once] [autoselect]` — block, body required. v2.29.0.
- ↳ `option label [value [selected]]` — sub-macro of `cycle`/`listbox`, inline. `selected` requires `value`.
- ↳ `optionsfrom collection` — sub-macro of `cycle`/`listbox`, inline.
- `link linkText [passageName]` *or* `link linkMarkup` *or* `link imageMarkup` — block, body required. v2.8.0.
- `linkappend linkText [transition|t8n]` — block, body required. v2.0.0.
- `linkprepend linkText [transition|t8n]` — block, body required. v2.0.0.
- `linkreplace linkText [transition|t8n]` — block, body required. v2.0.0.
- `listbox receiverName [autoselect]` — block, body required. v2.26.0. NO `once` keyword (unlike cycle).
- `numberbox receiverName defaultValue [passage] [autofocus]` — inline. v2.32.0.
- `radiobutton receiverName checkedValue [autocheck|checked]` — inline. v2.0.0 (`autocheck` v2.32.0).
- `textarea receiverName defaultValue [autofocus]` — inline. v2.0.0. **NO `passage` arg.**
- `textbox receiverName defaultValue [passage] [autofocus]` — inline. v2.0.0.

**Links** (`#macros-links`):
- `back [linkText [passageName]]` *or* `back linkMarkup` *or* `back imageMarkup` — inline. v2.0.0 (passageName v2.37.0).
- `return [linkText [passageName]]` *or* `return linkMarkup` *or* `return imageMarkup` — inline. v2.0.0 (passageName v2.37.0).
- `actions passageList` *or* `actions linkMarkupList` *or* `actions imageMarkupList` — inline. **Variadic. DEPRECATED v2.37.0.**
- `choice passageName [linkText]` *or* `choice linkMarkup` *or* `choice imageMarkup` — inline. **DEPRECATED v2.37.0.**

**DOM** (`#macros-dom`):
- `addclass selector classNames` — inline.
- `append selector [transition|t8n]` — block, body required.
- `copy selector` — inline.
- `prepend selector [transition|t8n]` — block, body required.
- `remove selector` — inline.
- `removeclass selector [classNames]` — inline. `classNames` optional (omitted = remove all).
- `replace selector [transition|t8n]` — block, body required.
- `toggleclass selector classNames` — inline.

**Audio** (`#macros-audio`):
- `audio trackIdList actionList` — inline. Both variadic. v2.0.0.
- `cacheaudio trackId sourceList` — inline. `sourceList` variadic. v2.0.0.
- `createaudiogroup groupId` — block, body required. `groupId` must begin with `:`. v2.19.0.
- ↳ `track trackId` — sub-macro of `createaudiogroup`, inline.
- `createplaylist listId` — block, body required. v2.8.0.
- ↳ `track trackId actionList` — sub-macro of `createplaylist`, inline. `actionList` = `volume level` (opt), `own` (opt keyword).
- `masteraudio actionList` — inline. v2.8.0.
- `playlist listId actionList` *or* `playlist actionList` — inline. v2.0.0.
- `removeaudiogroup groupId` — inline. v2.28.0.
- `removeplaylist listId` — inline. v2.8.0.
- `waitforaudio` — inline. v2.8.0.

**Miscellaneous** (`#macros-miscellaneous`):
- `done` — block, body required. v2.35.0. Body runs when incoming passage finishes rendering.
- `goto passageName` *or* `goto linkMarkup` — inline. v2.0.0.
- `repeat delay [transition|t8n]` — block, body required. v2.0.0.
- ↳ `stop` — sub-macro of `repeat`, inline.
- `timed delay [transition|t8n]` — block, body required. v2.0.0.
- ↳ `next [delay]` — sub-macro of `timed`, body required.
- `widget widgetName [container]` — block, body required. v2.0.0 (`container` v2.36.0).

#### Removed macros (do NOT exist in v2.37.x)

| Macro | Removed in | Replacement |
|---|---|---|
| `<<click>>` | v2.37.0 | `<<link>>` |
| `<<display>>` | v2.37.0 | `<<include>>` |
| `<<forget>>` | v2.37.0 | `forget()` function |
| `<<remember>>` | v2.37.0 | `memorize()` & `recall()` functions |
| `<<setplaylist>>` | v2.37.0 | `<<createplaylist>>` |
| `<<stopallaudio>>` | v2.37.0 | `<<audio ":all" stop>>` / `<<masteraudio>>` |

#### Macros that DO NOT EXIST (common misconceptions)

| Name | Reality |
|---|---|
| `<<style>>` | NOT a macro. `stylesheet` is a special passage tag. |
| `<<css>>` | NOT a macro. |
| `<<code>>` | NOT a macro. `{{{...}}}` is a markup feature, not a macro. |
| `<<verbatim>>` | NOT a macro. `"""..."""` and `<nowiki>...</nowiki>` are markup features. |
| `<<html>>` | NOT a macro. `<html>...</html>` is a markup feature. |

#### Audio action keyword reference

For `<<audio>>`, `<<playlist>>`, `<<masteraudio>>`:

**Audio actions** (`<<audio trackIdList actionList>>`):
`fadein`, `fadeout`, `fadeoverto seconds level`, `fadeto level`, `goto passage`, `load` (v2.28.0), `loop`, `mute`, `pause`, `play`, `stop`, `time seconds`, `unload` (v2.28.0), `unloop`, `unmute`, `volume level`.

**Playlist actions** (`<<playlist listId actionList>>`):
`fadein`, `fadeout`, `fadeoverto seconds level`, `fadeto level`, `load`, `loop`, `mute`, `pause`, `play`, `shuffle`, `skip`, `stop`, `unload`, `unloop`, `unmute`, `unshuffle`, `volume level`.

**Master audio actions** (`<<masteraudio actionList>>`):
`load`, `mute`, `muteonhide`, `nomuteonhide`, `stop`, `unload`, `unmute`, `volume level`.

### 3.13 SugarCube argument types

The docs distinguish these argument types (used throughout Section 7 specifications):

| # | Type | Example | Notes |
|---|---|---|---|
| 1 | Expression (JS/TwineScript) | `<<set $x to 5>>` | Single expression. |
| 2 | String (quoted) | `"Cakes"` | Quoted literal. |
| 3 | Variable name (quoted) | `"$foo"`, `"$foo.bar"` | Quoted variable NAME (not value). Required by form macros. |
| 4 | Variable ($story / _temp) | `$gold`, `_counter` | Auto-substituted to value. |
| 5 | Link markup | `[[Go West]]` | "regular syntax only, no setters". |
| 6 | Image markup | `[img[home.png][HQ]]` | "regular syntax only, no setters". |
| 7 | Passage name / reference | `"Go West"` | String name or variable holding one. |
| 8 | Selector (CSS/jQuery) | `"#pie"`, `".joe"` | All DOM macros. |
| 9 | Number | `100`, `0.5` | Integer/float. |
| 10 | Boolean | `true`, `false` | As expression values. |
| 11 | Keyword (bareword flag) | `autofocus`, `selected`, `keep` | Many — each macro documents its own. |
| 12 | CSS time value | `5s`, `500ms` | `type`, `timed`, `next`, `repeat`. |
| 13 | Action / action keyword | `play`, `volume 0.5` | Variadic action lists for audio macros. |
| 14 | List / valueList (space-sep) | `<<case "red" "auburn">>` | Variadic. |
| 15 | Comma-separated variable list | `<<capture $a, $b, _c>>` | `capture` and `unset`. |
| 16 | Backquote expression | `` <<link `"Wake " + $friend`>> `` | Evaluates contents as single discrete arg. |
| 17 | Language keyword | `JavaScript`, `TwineScript` | `<<script>>` only. |
| 18 | HTML element tag | `div`, `span` | `include`, `type`, `do`. |
| 19 | Class list (space-sep) | `"foo bar"` | `type`, `addclass`, `toggleclass`. |
| 20 | Track/group ID | `"bgm_space"`, `":playing"` | Audio macros. Group IDs begin with `:`. |

---

## 4. Current State Analysis — What's Wrong in Knot

> This section documents every known bug/gap in the current Knot codebase (branch `ver_3`), cross-referenced against Section 3 research findings.

### 4.1 Parser (`crates/formats/src/sugarcube/parser/core.rs`)

#### 4.1.1 No block-level markup support

The `parse_body()` loop has 15 dispatch arms on `bytes[i]`. **None of them handle**: `!` (headings), `*`/`#` (lists), `>` (blockquotes), `-` (horizontal rules), `|` (tables), `{`/`{{`/`{{{` (code blocks).

All of these fall through to the catch-all `_ =>` arm and become plain `Text` nodes with `is_prose: true`.

#### 4.1.2 No line-start tracking

The parser is a single linear byte walk with no concept of "this token is at the start of a line." Only two local `bytes[i-1] == b'\n'` peeks exist (in the `//` heuristic). This means block-level constructs cannot be reliably detected.

**Impact**: Even if arms for `!`/`*`/`#`/`>`/`-`/`|` are added, they need line-start awareness. SugarCube requires column-0 anchoring (NO leading whitespace allowed — see §3.3).

#### 4.1.3 Critical bug — macros execute inside `{{{...}}}`

Because there's no `b'{'` arm, `{{{ <<set $x to 1>> }}}` is parsed as:
1. `{{{ ` → plain Text (prose).
2. `<<set $x to 1>>` → real Macro node, **executes and mutates `$x`**.
3. ` }}}` → plain Text (prose).

**Expected behavior** (per §3.10): The entire `{{{ <<set $x to 1>> }}}` should be a single raw code block node with NO macro execution.

#### 4.1.4 `TextFormat` content is not recursively parsed

`''bold''`, `//italic//`, `__underline__`, `==strike==`, `~~sub~~`, `^^super^^` all capture content as a raw string and do NOT recursively parse it. So `''<<set $x to 1>>''` silently swallows the macro.

**Note**: This is a latent bug but **out of scope** for this plan — SugarCube's `formatByChar` parser DOES `subWikify` the content [SRC:parserlib.js:900+], so `''<<set>>''` should execute. However, fixing this is a separate concern from block-level markup. Flagged for future work.

#### 4.1.5 `parse_raw_body` is hardcoded to `script`/`style`/`css`

`macro_parser.rs:154` hardcodes the raw-body check:
```rust
if name.eq_ignore_ascii_case("script") || name.eq_ignore_ascii_case("style") || name.eq_ignore_ascii_case("css") {
```

**Problem 1**: `<<style>>` and `<<css>>` are **NOT real SugarCube macros** (see §3.12). They should not be in the catalog or the parser. SugarCube uses `stylesheet` special passage tags and `<style>...</style>` markup, not `<<style>>`/`<<css>>` macros.

**Problem 2**: The raw-body mechanism should be catalog-driven (a `MacroDef.body_is_raw: bool` field), not hardcoded in the parser.

#### 4.1.6 `MacroClose` is in the AST enum but never reaches downstream

The tree builder consumes all `MacroClose` nodes. Their span info is preserved on the parent `Macro`'s `close_span`/`close_name_span`. **This is the correct pattern** — any new flat-then-paired block construct should follow it.

### 4.2 AST (`crates/formats/src/sugarcube/ast.rs`)

#### 4.2.1 Missing variants

The `AstNode` enum has 9 variants: `Text`, `Macro`, `Expression`, `Link`, `Comment`, `InlineStyle`, `TextFormat`, `MacroClose`, `Error`.

**Missing** (needed for block-level markup):
- `Heading { level: u8, children: Vec<AstNode>, span }` — recursive content (per §3.5).
- `HorizontalRule { span }` — no content.
- `ListItem { depth: u8, ordered: bool, marker: String, children: Vec<AstNode>, span }` — flat model (no `List` wrapper; nesting reconstructed by depth).
- `Blockquote { depth: u8, children: Vec<AstNode>, span }` — for line-style `>`/`>>`.
- `BlockquoteBlock { children: Vec<AstNode>, span }` — for block-style `<<<...<<<`.
- `Table { header: Option<TableRow>, rows: Vec<TableRow>, caption: Option<String>, class: Option<String>, span }`.
- `TableRow { cells: Vec<TableCell>, row_type: TableRowType, span }`.
- `TableCell { children: Vec<AstNode>, is_header: bool, colspan: bool, rowspan: bool, span }`.
- `CodeBlock { content: String, span }` — raw content, no children (for block form `{{{\n...\n}}}`).
- `InlineCode { content: String, span }` — raw content (for inline form `{{{...}}}`).

#### 4.2.2 `TextFormatKind` lacks `Code`

The `TextFormatKind` enum has `Bold`, `Italic`, `Underline`, `Strike`, `Sub`, `Super`. There's no `Code` variant. **Recommendation**: Use a separate `InlineCode` AST variant instead of extending `TextFormatKind`, because inline code has different semantics (raw content vs. formatted content).

### 4.3 Semantic tokens (`crates/formats/src/plugin.rs`, `crates/formats/src/sugarcube/lsp/token_builder.rs`)

#### 4.3.1 Missing token types

22 token types exist. **Missing** (needed for block-level markup):
- `Heading` → wire name `"heading"`
- `HorizontalRule` → wire name `"horizontalRule"`
- `ListMarker` → wire name `"listMarker"` (for the `*`/`#`/`**`/`##` markers; content gets normal prose/macro tokens per user's Q2 answer).
- `Blockquote` → wire name `"blockquote"` (for the `>`/`>>` markers; content recurses).
- `Table` → wire name `"table"` (for `|` delimiters and row-type suffixes; cells recurse).
- `CodeBlock` → wire name `"codeBlock"` (single token over full span, per user's Q3 answer).
- `InlineCode` → wire name `"inlineCode"` (single token over full span).

#### 4.3.2 `SemanticToken.modifier` is `Option<Modifier>`, not a bitset

Only one modifier per token. This means a deprecated block macro name cannot simultaneously be tagged with `BlockDepthN`. **Limitation accepted** — not changing this in the plan.

#### 4.3.3 Enum declaration order ≠ legend order

`all_types()` returns variants in a different order than the enum declaration. Legend indices follow `all_types()` order. **Rule for new variants**: append to the end of `all_types()` to avoid renumbering existing indices.

#### 4.3.4 `InlineStyle` recurses with depth reset

`token_builder.rs:509` calls `build_semantic_tokens` (NOT `_at_depth`) for `InlineStyle` children, resetting depth to 0. **For new block constructs with children** (Heading, ListItem, Blockquote, TableCell), use `build_semantic_tokens_at_depth` with the surrounding depth to preserve `BlockDepthN` modifiers on nested macros.

#### 4.3.5 `build_diagnostics` is a separate AST walk

New AST variants that can produce diagnostics (e.g. unclosed code block) must be added in **both** `build_semantic_tokens_at_depth` AND `build_diagnostics`.

### 4.4 TextMate grammar & VS Code scopes

#### 4.4.1 Grammar is intentionally minimal

`sugarcube.tmLanguage.json` L9-15 explicitly states it only handles: embedded language regions, passage headers, comments, macro delimiters. All narrative markup relies on server-side semantic tokens.

**User decision (Q1)**: "keep textmate minimal. we can handle these as semantic tokens." → **No new TextMate patterns** will be added for block-level markup. All highlighting via semantic tokens only.

#### 4.4.2 `semanticTokenScopes` duplicated 5×

Each new token type needs an entry in all 5 language blocks in `package.json` (`twee`, `twee-sugarcube`, `twee-harlowe`, `twee-chapbook`, `twee-snowman`). All 5 must be edited.

### 4.5 Macro catalog (`crates/formats/src/sugarcube/macros/catalog.rs`)

#### 4.5.1 Critical — `<<checkbox>>` arg schema is wrong

**Current** (WRONG):
```rust
args: Some(&[
    MacroArgDef { position: 0, label: "label",     kind: String,   is_variable: false, is_required: false },
    MacroArgDef { position: 1, label: "variable",  kind: Variable, is_variable: true,  is_required: false },
    MacroArgDef { position: 2, label: "checked",   kind: String,   is_variable: false, is_required: false },
    MacroArgDef { position: 3, label: "unchecked", kind: String,   is_variable: false, is_required: false },
])
```

**Correct** (per §3.12):
```rust
args: Some(&[
    MacroArgDef { position: 0, label: "receiverName",  kind: Variable, is_variable: true,  is_required: true },
    MacroArgDef { position: 1, label: "uncheckedValue", kind: String,  is_variable: false, is_required: true },
    MacroArgDef { position: 2, label: "checkedValue",   kind: String,  is_variable: false, is_required: true },
    // Optional: autocheck | checked keyword (mutually exclusive) — needs new Keyword arg kind
])
```

**Also broken**: `CHECKBOX_FORMS` snippet and `checkbox` snippet have checked/unchecked values swapped.

#### 4.5.2 Critical — `<<radiobutton>>` arg schema is wrong

**Current** (WRONG):
```rust
args: Some(&[
    MacroArgDef { position: 0, label: "label",    kind: String,   is_variable: false, is_required: false },
    MacroArgDef { position: 1, label: "variable", kind: Variable, is_variable: true,  is_required: false },
    MacroArgDef { position: 2, label: "value",    kind: String,   is_variable: false, is_required: false },
])
```

**Correct** (per §3.12):
```rust
args: Some(&[
    MacroArgDef { position: 0, label: "receiverName", kind: Variable, is_variable: true,  is_required: true },
    MacroArgDef { position: 1, label: "checkedValue", kind: String,   is_variable: false, is_required: true },
    // Optional: autocheck | checked keyword (mutually exclusive)
])
```

#### 4.5.3 Non-existent macros in catalog

`<<style>>` and `<<css>>` are in the Knot catalog but **DO NOT EXIST** as SugarCube macros (see §3.12). They should be removed from the catalog AND from the parser's hardcoded raw-body check (`macro_parser.rs:154`).

#### 4.5.4 Missing macros

- `<<silent>>` (NEW v2.37.0, replacement for `<<silently>>`) — not in catalog.
- `<<do>>` / `<<redo>>` (NEW v2.37.0) — not in catalog.
- `<<choice>>` (deprecated v2.37.0 but still present) — not in catalog.

#### 4.5.5 Removed macros still in catalog

Need to verify and remove if present: `<<click>>`, `<<display>>`, `<<forget>>`, `<<remember>>`, `<<setplaylist>>`, `<<stopallaudio>>`. (Investigation needed — the research agent didn't confirm which of these are in the Knot catalog.)

#### 4.5.6 Arg schema gaps (18 macros)

Per research, these macros have incomplete arg schemas:

| Macro | Current | Correct (per §3.12) |
|---|---|---|
| `checkbox` | 4 args, wrong order | 3 required + optional keyword |
| `radiobutton` | 3 args, wrong order | 2 required + optional keyword |
| `textbox` | 2 args | 4 args (add `passage`, `autofocus`) |
| `numberbox` | 2 args | 4 args (add `passage`, `autofocus`) |
| `textarea` | 2 args | 3 args (add `autofocus`; NO passage) |
| `type` | 1 arg | 7 optional args (speed required) |
| `widget` | 1 arg | 2 args (add `container` keyword) |
| `include` | 1 arg | 2 args (add `elementName`) |
| `link` / `button` | 2 args | 3 forms (linkText+passage, linkMarkup, imageMarkup) |
| `goto` | 1 arg | 2 forms (passageName, linkMarkup) |
| `back` / `return` | 2 args | 3 forms |
| `audio` | 1 arg | 2 variadic lists (trackIdList, actionList) |
| `masteraudio` | 0 args | 1 variadic list (actionList) |
| `playlist` | 0 args | 2 forms (listId+actionList, actionList) |
| `cacheaudio` | 0 args | 2 args (trackId, sourceList variadic) |
| `track` (in playlist) | 1 arg | 2 args (trackId, actionList) |
| `case` | 0 args | 1 variadic (valueList) |
| `option` | 2 args | 3 args (add `selected` keyword) |
| `actions` | 1 arg | 1 variadic (passageList) |
| `for` | 0 args | 3 forms (conditional, C-style, range) |
| `script` | 0 args | 1 optional (language) |

#### 4.5.7 `MacroArgKind` enum too limited

Current (4 variants): `Expression`, `String`, `Selector`, `Variable`.

**Missing** (per §3.13): `Number`, `Link`, `Image`, `Keyword`, `Boolean`, `CssTime`, `Action`, `PassageRef`, `ElementTag`, `ClassList`, `TrackId`.

**Recommendation**: Add the most impactful ones (`Keyword`, `Link`, `Image`, `Number`) in Phase 7. Others can be modeled as `String` or `Expression` for now.

### 4.6 Documentation

#### 4.6.1 Deprecated docs (DO NOT reference)

Per handoff context, these are outdated and must NOT be used:
- All `.md` files in `docs/` folder
- `ROADMAP.md`, `RICH_HOVER_PLAN.md`, `IMPLEMENTATION_PLAN.md`, `PROSE_AND_PARSER_ISSUES.md`, `Macro_contextual_extraction.md`, `hover-token-based-plan.md` (root)

#### 4.6.2 `ARCHITECTURE.md` (root)

This is the only non-deprecated doc. It should be updated in Phase 7 (or a dedicated doc phase) to document the new AST variants and token types.

### 4.7 Architecture audit findings (post-Phase 2 sanity check)

After Phase 2, an architecture audit verified that ALL downstream consumers walk the AST produced by the parser — NOT the raw body text. This is the core architectural invariant: **the parser produces a proper AST, and everything downstream (tokens, links, var_ops, diagnostics, graph) consumes that AST**.

#### What was verified (clean)

| Consumer | File | Walks AST? | Handles new variants? |
|---|---|---|---|
| Semantic tokens | `token_builder.rs` `build_semantic_tokens_at_depth` | YES — `match node { ... }` on `AstNode` | YES — `CodeBlock`/`InlineCode` emit tokens; others are no-op placeholders |
| Diagnostics | `token_builder.rs` `build_diagnostics` | YES — `if let` chains on `AstNode` | Silently skips new variants (no diagnostics for code blocks yet — OK) |
| Links | `extraction.rs` `extract_links_recursive` | YES — `match node { ... }` with `_ => {}` catch-all | YES — `CodeBlock`/`InlineCode` fall into catch-all (correct: links inside code blocks are literal) |
| Var ops | `extraction.rs` `extract_var_ops_recursive` | YES — `match node { ... }` with `_ => {}` catch-all | YES — same as above |
| JS annotation | `js_annotate.rs` `annotate_inline_js` | YES — `match node { ... }` with `_ => {}` catch-all | YES — skips code blocks (no JS to annotate inside raw content) |
| Widget arg count | `registry_populate.rs` `extract_widget_arg_count` | YES — `match node { ... }` | YES — placeholder arms added in Phase 1 |
| Graph body blocks | `passage_build.rs` `build_body_blocks` | YES — `match node { ... }` | YES — placeholder arms (code blocks don't produce `Block` entries — correct) |
| Macro arg classification | `macro_parser.rs` `parse_structured_args` | Operates on the **args string** already isolated by the parser (NOT raw body text) | YES — `is_quoted_variable_name` helper added in Phase 2b works on `ArgToken`s from the isolated args |

3 new **architecture invariant tests** (in `core.rs`) verify that `ast.links` and `ast.var_ops` are empty when a link/variable appears inside a code block:
- `arch_code_block_content_not_extracted_as_link`
- `arch_code_block_content_not_extracted_as_var_op`
- `arch_block_code_content_not_extracted_as_link_or_var`

#### What was found (one gap — deferred)

**`extract_data_passage_refs`** in `extraction.rs:453` scans the RAW body text for `data-passage="..."` attributes (after `strip_comments`). It does NOT walk the AST, so a `data-passage` attribute inside a `{{{...}}}` code block would be incorrectly extracted as a passage reference.

- This is a **pre-existing limitation** (before Phase 2a, code blocks were plain text, so `data-passage` inside would also have been extracted). Phase 2a did NOT regress this.
- The fix: make `extract_data_passage_refs` walk the AST and only scan `Text` nodes that are NOT inside a `CodeBlock`/`InlineCode` (or, more generally, not inside any raw zone).
- **Deferred** to a future phase because:
  1. `data-passage` inside code blocks is rare in practice.
  2. The fix requires threading "am I inside a raw zone?" context through the AST walk — a non-trivial refactor that belongs in its own phase.
  3. Phase 2a's critical bug (macros executing inside code blocks) is already fixed; this is a lesser issue.

**Action item**: Add a deferred task to Section 6 (future phase or Phase 7 sub-phase) to refactor `extract_data_passage_refs` to walk the AST.

---

## 5. Architectural Decisions

> These decisions are LOCKED based on user answers (Q1-Q7) and research findings. Do not change without explicit user approval.

### AD-1: Line-start tracking via column counter

**Decision**: Thread a `col: usize` counter through `parse_body()` and helper functions. Reset to 0 on `\n`. Increment by char width otherwise.

**Rationale**: SugarCube requires column-0 anchoring for ALL block constructs (NO leading whitespace allowed — see §3.3). A column counter is the minimal change that supports this. It's simpler than pre-scanning line starts into a `Vec<usize>`.

**Impact**: Modifies the signature of `parse_body()` (likely adds a `col` parameter or uses a small `ParseContext` struct). Every arm that calls `flush_text` must respect it. Moderate refactor of `core.rs`.

**Why not `bytes[i-1] == b'\n'` peek?** It can't handle the start-of-input case (`i == 0`) cleanly, and it doesn't compose well when nested (e.g. inside an `InlineStyle` body). A column counter is composable.

### AD-2: Raw vs recursive body per construct

| Construct | Body handling | Rationale |
|---|---|---|
| `{{{...}}}` block code | **Raw** (no parse) | §3.10.1 — `.text()`, no `subWikify` |
| `{{{...}}}` inline code | **Raw** (no parse) | §3.10.2 — `.text()`, no `subWikify` |
| `----` horizontal rule | N/A (no body) | §3.6 |
| `! heading` | **Recursive** | §3.5 — `subWikify` called |
| `* list item` | **Recursive** | §3.7 — `subWikify` called |
| `>` blockquote (line) | **Recursive** | §3.8.1 — `subWikify` called |
| `<<<...<<<` blockquote (block) | **Recursive** | §3.8.2 — `subWikify` called |
| `| table cell |` | **Recursive** | §3.9 — `subWikify` called |

### AD-3: New AstNode variants — shapes

```rust
// In ast.rs, add to AstNode enum (all spans are body-relative Range<usize>):

Heading {
    level: u8,                    // 1..=6
    children: Vec<AstNode>,       // recursively parsed (macros execute, per §3.5)
    span: Range<usize>,           // covers `!` through end of line
},

HorizontalRule {
    span: Range<usize>,           // covers `----` (and trailing whitespace)
},

ListItem {
    depth: u8,                    // 1 = top level, 2 = nested, etc. (marker char count)
    ordered: bool,                // true for #, false for *
    marker: String,               // "*", "**", "#", "###", etc.
    children: Vec<AstNode>,       // recursively parsed
    span: Range<usize>,           // covers marker through end of line
},

Blockquote {
    depth: u8,                    // 1 = >, 2 = >>
    children: Vec<AstNode>,       // recursively parsed
    span: Range<usize>,           // covers `>` through end of line
},

BlockquoteBlock {
    children: Vec<AstNode>,       // recursively parsed (between <<< and <<<)
    open_span: Range<usize>,      // the opening <<< line
    close_span: Option<Range<usize>>,  // the closing <<< line (None if unclosed)
    span: Range<usize>,           // full span including open and close
},

Table {
    header: Option<TableRow>,     // rows with `h` suffix
    rows: Vec<TableRow>,          // body rows (no suffix)
    footer: Option<TableRow>,     // rows with `f` suffix
    caption: Option<String>,      // from `c` suffix row
    caption_span: Option<Range<usize>>,
    class: Option<String>,        // from `k` suffix row
    class_span: Option<Range<usize>>,
    span: Range<usize>,
},
TableRow {
    cells: Vec<TableCell>,
    row_type: TableRowType,       // Body, Header, Footer, Caption, Class
    span: Range<usize>,
},
TableCell {
    children: Vec<AstNode>,       // recursively parsed
    is_header: bool,              // true if cell content starts with `!`
    colspan: bool,                // true if cell content is just `>`
    rowspan: bool,                // true if cell content is just `~`
    span: Range<usize>,
},

CodeBlock {
    content: String,              // raw, no parse (per §3.10.1)
    span: Range<usize>,           // covers {{{ through }}}
},

InlineCode {
    content: String,              // raw, no parse (per §3.10.2)
    span: Range<usize>,           // covers {{{ through }}}
},

// New supporting enums:
enum TableRowType { Body, Header, Footer, Caption, Class }
```

### AD-4: New SemanticTokenType variants

Add to `plugin.rs` (append to `all_types()` at the end to preserve existing legend indices):

| Variant | Wire name | Legend index (next available) |
|---|---|---|
| `Heading` | `"heading"` | 22 |
| `HorizontalRule` | `"horizontalRule"` | 23 |
| `ListMarker` | `"listMarker"` | 24 |
| `Blockquote` | `"blockquote"` | 25 |
| `BlockquoteBlock` | `"blockquoteBlock"` | 26 |
| `Table` | `"table"` | 27 |
| `CodeBlock` | `"codeBlock"` | 28 |
| `InlineCode` | `"inlineCode"` | 29 |

### AD-5: Token emission strategy (per user Q2 — split spans)

For constructs with recursive content (Heading, ListItem, Blockquote, BlockquoteBlock, TableCell):

- The **marker** gets its own token type (e.g. `Heading` for the `!` run, `ListMarker` for `**`, `Blockquote` for `>>`, `Table` for `|`).
- The **content** is recursively tokenized via `build_semantic_tokens_at_depth` (preserving depth for nested macros).
- This matches the user's Q2 answer: "for `! Some <<set>> heading`, `some` and `heading` would be of same type [prose] and `<<set>>` would be a macro."

For raw constructs (CodeBlock, InlineCode):

- A **single token** over the full span (per user Q3 answer: "single block is good enough").
- The token type is `CodeBlock` or `InlineCode` (new types, per user Q3: "a new token type could be worth it").

For HorizontalRule:

- A single `HorizontalRule` token over the `----` span.

### AD-6: TextMate grammar — NO additions

Per user Q1: "keep textmate minimal. we can handle these as semantic tokens."

**No new patterns** will be added to `sugarcube.tmLanguage.json` or `twee.tmLanguage.json` for block-level markup. All highlighting via server-side semantic tokens.

### AD-7: `semanticTokenScopes` — 5× entries

Each new token type needs an entry in all 5 language blocks in `package.json`. Proposed scope mappings:

| Token type (wire name) | TextMate scope(s) |
|---|---|
| `heading` | `markup.heading.twee` |
| `horizontalRule` | `punctuation.separator.hr.twee` |
| `listMarker` | `punctuation.definition.list.twee` |
| `blockquote` | `punctuation.definition.quote.twee` |
| `blockquoteBlock` | `punctuation.definition.quote.twee` |
| `table` | `punctuation.definition.table.twee` |
| `codeBlock` | `markup.raw.code.twee` |
| `inlineCode` | `markup.raw.inline.twee` |

### AD-8: Generalize raw-body mechanism

Move the hardcoded `script`/`style`/`css` check (`macro_parser.rs:154`) to a catalog-driven field:

```rust
// In types.rs, add to MacroDef:
pub body_is_raw: bool,   // true for <<script>> (and any future raw-body macros)
```

**Then**:
1. Set `body_is_raw: true` ONLY for `script` (the only real raw-body macro per §3.12).
2. Remove `<<style>>` and `<<css>>` from the catalog entirely (they don't exist).
3. Remove the hardcoded name check in `macro_parser.rs:154`; replace with `if def.body_is_raw { ... }`.
4. Remove the `is_prose_rendering_macro` entries for `style`/`css` in `tree_builder.rs:405-410` (keep `script`, `silently`, `silent`, `done`).

### AD-9: List model — flat, depth-based

**Decision**: Use a flat `ListItem` model (no `List` wrapper). Each `ListItem` carries `depth: u8` and `ordered: bool`. Nesting is reconstructed by consumers based on depth changes (same pattern as block macros being flat-then-paired).

**Rationale**: Simpler parser, simpler token builder. SugarCube's source does build nested `<ul>`/`<ol>` at render time, but the AST doesn't need to — consumers can reconstruct nesting from depth.

### AD-10: Block code vs inline code disambiguation

**Decision**: In the parser, when `{{{` is encountered:
- If at column 0 AND immediately followed by `\n` → **block code** (`CodeBlock` variant). Scan for `^}}}$` (own line).
- Otherwise → **inline code** (`InlineCode` variant). Scan for next `}}}` (non-greedy).

**Rationale**: Matches SugarCube's disambiguation exactly (§3.10).

### AD-11: Phase ordering — small, reviewable chunks

Per user Q5: "split things into small handle-able phases."

Seven phases, each independently reviewable and shippable:
1. Foundation (no behavior change).
2. Critical bugs (code blocks + checkbox/radiobutton).
3. Headings + inline code.
4. Horizontal rule + blockquotes.
5. Lists.
6. Tables.
7. Macro catalog overhaul.

**Each phase ends with**: `cargo test` passing, modified files zipped to `/home/z/my-project/download/`, worklog appended.

---

## 6. Phased Implementation Plan

### Phase 1 — Foundation (no behavior change)

**Goal**: Add line-start tracking, new AST variants, new token types, new scope mappings. No parser behavior changes yet — all new variants are unused.

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — add `col: usize` tracking (thread through `parse_body` and `flush_text`).
- `crates/formats/src/sugarcube/ast.rs` — add 10 new `AstNode` variants + supporting enums.
- `crates/formats/src/plugin.rs` — add 8 new `SemanticTokenType` variants.
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — add 8 new match arms (emit nothing for now, or minimal tokens).
- `extensions/vscode/package.json` — add scope mappings for 8 new token types in all 5 language blocks.
- `crates/formats/src/sugarcube/parser/mod.rs` — update `parse_body` signature if needed.

**Acceptance criteria**:
- `cargo build --release` succeeds.
- `cargo test` passes (no regressions).
- No behavior change in parsing output (all new variants are unused).

### Phase 2 — Critical bugs

**Goal**: Fix the two critical bugs.

**Sub-phase 2a — Code blocks (`{{{...}}}`)**:
- Add `b'{'` arm in `core.rs` for `{{{`.
- Disambiguate block vs inline (per AD-10).
- Emit `CodeBlock` or `InlineCode` AST nodes with raw content.
- Token builder emits single `CodeBlock`/`InlineCode` token over full span.
- **This fixes the critical bug**: macros no longer execute inside `{{{...}}}`.

**Sub-phase 2b — checkbox/radiobutton catalog fix**:
- Rewrite `<<checkbox>>` arg schema (3 required + optional keyword).
- Rewrite `<<radiobutton>>` arg schema (2 required + optional keyword).
- Fix `CHECKBOX_FORMS` snippet (swap checked/unchecked).
- Fix `checkbox` snippet (swap checked/unchecked).
- Verify `RADIOBUTTON_FORMS` is correct (research says it is).

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — add `b'{'` arm + `parse_code_block` / `parse_inline_code` helpers.
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — emit tokens for `CodeBlock`/`InlineCode` variants.
- `crates/formats/src/sugarcube/macros/catalog.rs` — fix checkbox/radiobutton.
- `crates/formats/src/sugarcube/macros/snippets.rs` — fix checkbox snippet.
- `crates/formats/src/sugarcube/macros/completion_forms.rs` — fix CHECKBOX_FORMS.

**Acceptance criteria**:
- `{{{ <<set $x to 1>> }}}` produces a single `CodeBlock`/`InlineCode` node; `$x` is NOT mutated.
- `<<checkbox "$x" "unchecked" "checked">>` parses with correct arg positions.
- `cargo test` passes, including new tests for both fixes.

### Phase 3 — Headings + inline code

**Goal**: Add heading support (`!` ... `!!!!!!`) with recursive content.

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — add `b'!'` arm (line-start anchored, requires `col == 0`).
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — emit `Heading` token for the `!` run; recurse into `children` with surrounding depth.
- New test cases in `analysis_tests.rs` (or equivalent).

**Acceptance criteria**:
- `! Some <<set $x to 1>> heading` produces a `Heading` node with `level: 1` and `children: [Text("Some "), Macro("set"), Text(" heading")]`.
- The `<<set>>` IS parsed (executes in SugarCube) — this is correct behavior per §3.5.
- Token output: `Heading` token on `!`, then prose/macro tokens for content.
- `cargo test` passes.

### Phase 4 — Horizontal rule + blockquotes

**Goal**: Add `----` horizontal rule and both blockquote forms (`>` line-style, `<<<` block-style).

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — add `b'-'` arm (line-start, requires `^----+\s*$` pattern) and `b'>'` arm (line-start `>` run).
- The `<<<` block form needs special handling: detect `^<<<\n` at line start, scan for closing `^<<<\n`.
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — emit `HorizontalRule`, `Blockquote`, `BlockquoteBlock` tokens; recurse into blockquote children.

**Acceptance criteria**:
- `----` alone on a line produces `HorizontalRule` node.
- `---` (3 dashes) does NOT produce a horizontal rule (falls through to text).
- `> Some text` produces `Blockquote { depth: 1, children: [Text("Some text")] }`.
- `>> Nested` produces `Blockquote { depth: 2, ... }`.
- `<<<\n...\n<<<` produces `BlockquoteBlock` with recursive children.
- `cargo test` passes.

### Phase 5 — Lists

**Goal**: Add `*`/`#` list support with depth-based nesting.

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — add `b'*'` and `b'#'` arms (line-start, scan marker run, parse content recursively).
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — emit `ListMarker` token for the marker run; recurse into children.

**Acceptance criteria**:
- `* item` produces `ListItem { depth: 1, ordered: false, marker: "*", children: [Text("item")] }`.
- `** sub` produces `ListItem { depth: 2, ... }`.
- `# item` produces `ListItem { depth: 1, ordered: true, ... }`.
- `*#` does NOT produce a mixed marker — only `*` is consumed, `#` becomes text.
- `  * item` (leading space) does NOT produce a list item (falls through to text).
- Content inside list items IS recursively parsed (macros execute).
- `cargo test` passes.

### Phase 6 — Tables

**Goal**: Add TiddlyWiki-style table support (`|...|` rows with `h`/`f`/`c`/`k` suffixes, `!` header cells, `>` colspan, `~` rowspan).

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — add `b'|'` arm (line-start, parse row + suffix).
- New helper `parse_table_row` to handle a single `|...|[fhck]?$` line.
- New helper `parse_table_cell` to handle cell content (recursive).
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — emit `Table` tokens for `|` delimiters and row-type suffixes; recurse into cell children.

**Acceptance criteria**:
- `| cell1 | cell2 |` produces `Table { rows: [TableRow { cells: [TableCell, TableCell], row_type: Body }] }`.
- `|!header|h` produces a header row with `is_header: true` cells.
- `|caption|c` sets `caption` field on the table.
- `|classname|k` sets `class` field on the table.
- `|>` cell sets `colspan: true`.
- `|~` cell sets `rowspan: true`.
- Cell content IS recursively parsed (macros execute).
- `cargo test` passes.

### Phase 7 — Macro catalog overhaul

**Goal**: Fix all ~20 macro arg schemas, remove non-existent macros, add missing macros, add new `MacroArgKind` variants, generalize raw-body mechanism.

**Sub-phases** (each independently shippable):

**7a — Generalize raw-body mechanism**:
- Add `body_is_raw: bool` to `MacroDef`.
- Set `body_is_raw: true` for `script` only.
- Remove `<<style>>` and `<<css>>` from catalog.
- Remove hardcoded check in `macro_parser.rs:154`.
- Update `tree_builder.rs` `is_prose_rendering_macro` (remove `style`/`css`, keep `script`/`silently`/`silent`/`done`).

**7b — Add missing macros**:
- `<<silent>>` (NEW v2.37.0).
- `<<do>>` / `<<redo>>` (NEW v2.37.0).
- `<<choice>>` (deprecated but present).
- Verify and remove any removed macros still in catalog (`click`, `display`, `forget`, `remember`, `setplaylist`, `stopallaudio`).

**7c — Add new `MacroArgKind` variants**:
- `Keyword` (for `autofocus`, `selected`, `keep`, `container`, `autocheck`, `checked`, `once`, `autoselect`, etc.).
- `Link` (for `[[...]]` link markup args).
- `Image` (for `[img[...]]` image markup args).
- `Number` (for numeric args like `<<numberbox>>` default, `<<audio>>` volume).
- Update `parse_structured_args` to handle new kinds.

**7d — Fix arg schemas (batch)**:
- Form macros: `checkbox`, `radiobutton`, `textbox`, `numberbox`, `textarea`, `cycle`, `listbox`, `option`, `optionsfrom`.
- Link macros: `link`, `button`, `back`, `return`, `actions`, `choice`, `goto`, `include`.
- Audio macros: `audio`, `masteraudio`, `playlist`, `cacheaudio`, `track`.
- Control macros: `case`, `for`, `switch`.
- Output macros: `type`, `widget`, `script`, `do`, `redo`.
- DOM macros: `addclass`, `removeclass`, `toggleclass` (verify `classNames` is variadic).

**7e — Update snippets and completion forms**:
- Verify all snippets match the corrected arg schemas.
- Update `completion_forms.rs` for any macros whose forms changed.

**Files modified**:
- `crates/formats/src/types.rs` — add `MacroArgKind` variants, `body_is_raw` field.
- `crates/formats/src/sugarcube/macros/catalog.rs` — rewrite ~20 entries, add 3, remove 2.
- `crates/formats/src/sugarcube/macros/snippets.rs` — update snippets.
- `crates/formats/src/sugarcube/macros/completion_forms.rs` — update forms.
- `crates/formats/src/sugarcube/parser/macro_parser.rs` — generalize raw-body check, update `parse_structured_args`.
- `crates/formats/src/sugarcube/parser/tree_builder.rs` — update `is_prose_rendering_macro`.

**Acceptance criteria**:
- `<<style>>` and `<<css>>` are no longer recognized as macros.
- `<<silent>>`, `<<do>>`, `<<redo>>` are recognized.
- All corrected macros have correct arg schemas (verified by tests).
- `cargo test` passes.

---

## 7. Per-Phase Detailed Specifications

> This section contains the implementation-level detail for each phase. It will be expanded as phases are entered. For now, it references the high-level plan in Section 6.

### 7.1 Phase 1 — Foundation (DETAILED)

#### 7.1.1 Line-start tracking refactor

**Current** `parse_body` signature (`core.rs:20`):
```rust
pub(super) fn parse_body(text: &str, offset: usize) -> Vec<AstNode>
```

**Proposed** (option A — minimal change):
```rust
pub(super) fn parse_body(text: &str, offset: usize) -> Vec<AstNode> {
    parse_body_internal(text, offset, 0)  // col starts at 0
}

fn parse_body_internal(text: &str, offset: usize, initial_col: usize) -> Vec<AstNode> {
    let mut col = initial_col;
    // ... in the loop, on '\n': col = 0; otherwise: col += char_len
    // ... arms that need line-start check: if col == 0 { ... }
}
```

**Proposed** (option B — context struct, preferred for extensibility):
```rust
struct ParseCtx {
    offset: usize,
    col: usize,
}

pub(super) fn parse_body(text: &str, offset: usize) -> Vec<AstNode> {
    parse_body_with_ctx(text, &mut ParseCtx { offset, col: 0 })
}

fn parse_body_with_ctx(text: &str, ctx: &mut ParseCtx) -> Vec<AstNode> {
    // ... use ctx.col for line-start checks
    // ... update ctx.col on each char
}
```

**Recommendation**: Option B — cleaner, easier to extend, avoids parameter proliferation.

**`flush_text` impact**: `flush_text` doesn't need `col` (it just slices text), but it should be called with the same `text_start`/`end` semantics. No signature change needed.

**Recursive calls**: `parse_inline_style` calls `parse_body` recursively (`core.rs:346-350`). The recursive call should pass the correct `col` (the column at the start of the inline style body). This requires `parse_inline_style` to track `col` too.

#### 7.1.2 New AST variants — exact code

Add to `crates/formats/src/sugarcube/ast.rs` after the `Error` variant (before the closing `}` of the enum):

```rust
/// Headings: `!` through `!!!!!!` (1-6 levels).
/// Content is recursively parsed (macros execute per SugarCube source).
Heading {
    level: u8,
    children: Vec<AstNode>,
    span: Range<usize>,
},

/// Horizontal rule: `----` (4+ dashes alone on a line).
HorizontalRule {
    span: Range<usize>,
},

/// List item: `*`/`**`/`#`/`##` etc. at line start.
/// Flat model — nesting reconstructed by `depth` field.
ListItem {
    depth: u8,
    ordered: bool,
    marker: String,
    children: Vec<AstNode>,
    span: Range<usize>,
},

/// Line-style blockquote: `>`/`>>`/etc. at line start.
Blockquote {
    depth: u8,
    children: Vec<AstNode>,
    span: Range<usize>,
},

/// Block-style blockquote: `<<<\n...\n<<<` (undocumented but in source).
BlockquoteBlock {
    children: Vec<AstNode>,
    open_span: Range<usize>,
    close_span: Option<Range<usize>>,
    span: Range<usize>,
},

/// TiddlyWiki-style table.
Table {
    header: Option<TableRow>,
    rows: Vec<TableRow>,
    footer: Option<TableRow>,
    caption: Option<String>,
    caption_span: Option<Range<usize>>,
    class: Option<String>,
    class_span: Option<Range<usize>>,
    span: Range<usize>,
},

TableRow {
    cells: Vec<TableCell>,
    row_type: TableRowType,
    span: Range<usize>,
},

TableCell {
    children: Vec<AstNode>,
    is_header: bool,
    colspan: bool,
    rowspan: bool,
    span: Range<usize>,
},

/// Block code: `{{{\n...\n}}}` (raw content, no macro processing).
CodeBlock {
    content: String,
    span: Range<usize>,
},

/// Inline code: `{{{...}}}` mid-line (raw content, no macro processing).
InlineCode {
    content: String,
    span: Range<usize>,
},
```

Add supporting enum (after `TextFormatKind` or near it):

```rust
/// Row type for TiddlyWiki table rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableRowType {
    /// No suffix — body row (`<tbody>`).
    Body,
    /// `h` suffix — header row (`<thead>`).
    Header,
    /// `f` suffix — footer row (`<tfoot>`).
    Footer,
    /// `c` suffix — caption (`<caption>`).
    Caption,
    /// `k` suffix — CSS class assignment.
    Class,
}
```

#### 7.1.3 New SemanticTokenType variants — exact code

In `crates/formats/src/plugin.rs`:

1. Add to the `SemanticTokenType` enum (in the "Narrative content" section or a new "Block markup" section):
```rust
Heading,
HorizontalRule,
ListMarker,
Blockquote,
BlockquoteBlock,
Table,
CodeBlock,
InlineCode,
```

2. Add to `all_types()` (APPEND AT END to preserve indices 0-21):
```rust
SemanticTokenType::Heading,          // 22
SemanticTokenType::HorizontalRule,   // 23
SemanticTokenType::ListMarker,       // 24
SemanticTokenType::Blockquote,       // 25
SemanticTokenType::BlockquoteBlock,  // 26
SemanticTokenType::Table,            // 27
SemanticTokenType::CodeBlock,        // 28
SemanticTokenType::InlineCode,       // 29
```

3. Add to `lsp_name()`:
```rust
Self::Heading => "heading",
Self::HorizontalRule => "horizontalRule",
Self::ListMarker => "listMarker",
Self::Blockquote => "blockquote",
Self::BlockquoteBlock => "blockquoteBlock",
Self::Table => "table",
Self::CodeBlock => "codeBlock",
Self::InlineCode => "inlineCode",
```

#### 7.1.4 Token builder — placeholder arms

In `crates/formats/src/sugarcube/lsp/token_builder.rs`, add to `build_semantic_tokens_at_depth` (before the `MacroClose` arm at L522):

```rust
// Phase 1: placeholder arms — emit nothing yet.
// These will be filled in during Phases 2-6.
ast::AstNode::Heading { .. } => {}
ast::AstNode::HorizontalRule { .. } => {}
ast::AstNode::ListItem { .. } => {}
ast::AstNode::Blockquote { .. } => {}
ast::AstNode::BlockquoteBlock { .. } => {}
ast::AstNode::Table { .. } => {}
ast::AstNode::TableRow { .. } => {}  // note: TableRow/TableCell may not be top-level
ast::AstNode::TableCell { .. } => {}
ast::AstNode::CodeBlock { .. } => {}
ast::AstNode::InlineCode { .. } => {}
```

**Note**: `TableRow` and `TableCell` are not variants of `AstNode` — they're separate types used as fields of `Table`. The token builder will handle them inside the `Table` arm. Remove the `TableRow`/`TableCell` arms from the placeholder list.

#### 7.1.5 VS Code package.json — scope mappings

In `extensions/vscode/package.json`, add to EACH of the 5 language blocks in `semanticTokenScopes`:

```json
"heading": ["markup.heading.twee"],
"horizontalRule": ["punctuation.separator.hr.twee"],
"listMarker": ["punctuation.definition.list.twee"],
"blockquote": ["punctuation.definition.quote.twee"],
"blockquoteBlock": ["punctuation.definition.quote.twee"],
"table": ["punctuation.definition.table.twee"],
"codeBlock": ["markup.raw.code.twee"],
"inlineCode": ["markup.raw.inline.twee"]
```

#### 7.1.6 Phase 1 acceptance test

```bash
cd /home/z/my-project/Knot
cargo build --release --manifest-path crates/server/Cargo.toml
cargo test
```

Both must succeed. No parsing behavior change (all new variants are unused).

---

### 7.2 Phase 2 — Critical bugs (DETAILED)

#### 7.2.1 Code block parser — `b'{'` arm

In `core.rs`, add a new arm in the `match bytes[i]` block (before the catch-all `_ =>`):

```rust
b'{' if i + 2 < len && bytes[i + 1] == b'{' && bytes[i + 2] == b'{' => {
    let start = i;
    i += 3;
    
    // Disambiguate block vs inline (per AD-10):
    // - Block: col == 0 AND immediately followed by '\n'
    // - Inline: otherwise
    let is_block = ctx.col == 0 && i < len && bytes[i] == b'\n';
    
    let node = if is_block {
        parse_code_block(text, &mut i, offset + start)
    } else {
        parse_inline_code(text, &mut i, offset + start)
    };
    
    flush_text(text, &mut text_start, start, offset, &mut nodes);
    Some(node)
}
```

#### 7.2.2 `parse_code_block` helper

```rust
/// Parse a block code section: `{{{\n...\n}}}`.
/// Content is raw (no macro processing).
fn parse_code_block(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    // *i is positioned just after `{{{` (at the `\n`)
    let content_start = *i + 1;  // skip the newline
    
    // Scan for `^}}}$` (own line, column 0)
    let rest = &text[content_start..];
    let mut close_offset = None;
    let mut line_start = 0usize;
    
    for (idx, ch) in rest.char_indices() {
        if ch == '\n' {
            let next_line_start = idx + 1;
            if rest[next_line_start..].starts_with("}}}") {
                // Check that }}} is alone on its line
                let after_braces = next_line_start + 3;
                let after_bytes = rest.as_bytes();
                if after_braces >= rest.len()
                    || after_bytes[after_braces] == b'\n'
                    || (after_bytes[after_braces] == b'\r' && after_braces + 1 < rest.len() && after_bytes[after_braces + 1] == b'\n')
                {
                    close_offset = Some(next_line_start);
                    break;
                }
            }
            line_start = next_line_start;
        }
    }
    
    let (content, span_end) = if let Some(close_off) = close_offset {
        let content = text[content_start..content_start + close_off].to_string();
        // Skip past the closing `}}}` and its trailing newline
        let end = content_start + close_off + 3;
        let end = if end < text.len() && text.as_bytes()[end] == b'\n' { end + 1 } else { end };
        (content, end)
    } else {
        // Unclosed — consume rest of text
        let content = text[content_start..].to_string();
        (content, text.len())
    };
    
    *i = span_end;
    AstNode::CodeBlock {
        content,
        span: span_start..span_start + (span_end - span_start),
    }
}
```

#### 7.2.3 `parse_inline_code` helper

```rust
/// Parse inline code: `{{{...}}}` (non-greedy, first `}}}` closes).
/// Content is raw (no macro processing).
fn parse_inline_code(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    // *i is positioned just after `{{{`
    let content_start = *i;
    
    // Non-greedy scan for `}}}`
    let rest = &text[content_start..];
    let close_offset = rest.find("}}}");
    
    let (content, span_end) = if let Some(close_off) = close_offset {
        let content = text[content_start..content_start + close_off].to_string();
        let end = content_start + close_off + 3;
        (content, end)
    } else {
        // Unclosed — consume rest of text
        let content = text[content_start..].to_string();
        (content, text.len())
    };
    
    *i = span_end;
    AstNode::InlineCode {
        content,
        span: span_start..span_start + (span_end - span_start),
    }
}
```

#### 7.2.4 Token builder — CodeBlock and InlineCode arms

In `token_builder.rs`, replace the placeholder arms:

```rust
ast::AstNode::CodeBlock { span, .. } => {
    tokens.push(SemanticToken {
        start: body_offset_in_passage + span.start,
        length: span.end - span.start,
        token_type: SemanticTokenType::CodeBlock,
        modifier: None,
    });
}

ast::AstNode::InlineCode { span, .. } => {
    tokens.push(SemanticToken {
        start: body_offset_in_passage + span.start,
        length: span.end - span.start,
        token_type: SemanticTokenType::InlineCode,
        modifier: None,
    });
}
```

#### 7.2.5 checkbox/radiobutton catalog fix

In `catalog.rs`, replace the `checkbox` and `radiobutton` entries:

```rust
MacroDef {
    name: "checkbox",
    description: "Creates a checkbox, used to modify the value of the variable with the given name.",
    body: BodyRequirement::Never,
    kind: MacroKind::Inline,
    args: Some(&[
        MacroArgDef {
            position: 0,
            label: "receiverName",
            is_passage_ref: false,
            is_selector: false,
            is_variable: true,
            is_required: true,
            kind: MacroArgKind::Variable,
        },
        MacroArgDef {
            position: 1,
            label: "uncheckedValue",
            is_passage_ref: false,
            is_selector: false,
            is_variable: false,
            is_required: true,
            kind: MacroArgKind::String,
        },
        MacroArgDef {
            position: 2,
            label: "checkedValue",
            is_passage_ref: false,
            is_selector: false,
            is_variable: false,
            is_required: true,
            kind: MacroArgKind::String,
        },
        // NOTE: `autocheck` and `checked` keywords (mutually exclusive) added in Phase 7c
        // when MacroArgKind::Keyword exists.
    ]),
    deprecated: false,
    deprecation_message: None,
    category: MacroCategory::Forms,
    container: None,
    container_any_of: None,
    body_is_raw: false,  // added in Phase 7a
},

MacroDef {
    name: "radiobutton",
    description: "Creates a radio button, used to modify the value of the variable with the given name.",
    body: BodyRequirement::Never,
    kind: MacroKind::Inline,
    args: Some(&[
        MacroArgDef {
            position: 0,
            label: "receiverName",
            is_passage_ref: false,
            is_selector: false,
            is_variable: true,
            is_required: true,
            kind: MacroArgKind::Variable,
        },
        MacroArgDef {
            position: 1,
            label: "checkedValue",
            is_passage_ref: false,
            is_selector: false,
            is_variable: false,
            is_required: true,
            kind: MacroArgKind::String,
        },
        // NOTE: `autocheck` and `checked` keywords added in Phase 7c
    ]),
    deprecated: false,
    deprecation_message: None,
    category: MacroCategory::Forms,
    container: None,
    container_any_of: None,
    body_is_raw: false,
},
```

**Note**: The `body_is_raw` field doesn't exist yet (added in Phase 7a). For Phase 2, either:
- (a) Add `body_is_raw: bool` to `MacroDef` now (small change), OR
- (b) Defer the `body_is_raw` field to Phase 7a and omit it here.

**Recommendation**: Option (a) — add the field now with `false` for all existing entries, `true` for `script`. This avoids a second pass over the catalog in Phase 7a.

#### 7.2.6 Snippet and completion form fixes

In `snippets.rs`:
```rust
// BEFORE (wrong):
"checkbox" => Some(r#"checkbox "${1:\$var}" "${2:checked}" "${3:unchecked}">>"#),

// AFTER (correct):
"checkbox" => Some(r#"checkbox "${1:\$var}" "${2:unchecked}" "${3:checked}">>"#),
```

In `completion_forms.rs`, `CHECKBOX_FORMS`:
```rust
// BEFORE (wrong):
MacroCompletionForm {
    label: r#"<<checkbox "$var" "checked" "unchecked">>"#,
    detail: "Checkbox bound to variable (checked/unchecked values)",
    snippet: r#"checkbox "${1:\$var}" "${2:checked}" "${3:unchecked}">>"#,
    sort_priority: 0,
},

// AFTER (correct):
MacroCompletionForm {
    label: r#"<<checkbox "$var" "unchecked" "checked">>"#,
    detail: "Checkbox bound to variable (unchecked/checked values)",
    snippet: r#"checkbox "${1:\$var}" "${2:unchecked}" "${3:checked}">>"#,
    sort_priority: 0,
},
```

#### 7.2.7 Phase 2 acceptance tests

New tests to add (in `analysis_tests.rs` or a new `parser_tests.rs`):

```rust
#[test]
fn test_code_block_does_not_execute_macros() {
    let text = "{{{ <<set $x to 1>> }}}";
    let ast = parse_passage_body(text, 0, ParseMode::Normal);
    assert_eq!(ast.nodes.len(), 1);
    match &ast.nodes[0] {
        AstNode::InlineCode { content, .. } => {
            assert_eq!(content, " <<set $x to 1>> ");
        }
        other => panic!("expected InlineCode, got {:?}", other),
    }
    // Verify no Macro nodes were produced
    assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::Macro { .. })));
}

#[test]
fn test_block_code_multiline() {
    let text = "{{{\n<<set $x to 1>>\n}}}";
    let ast = parse_passage_body(text, 0, ParseMode::Normal);
    assert_eq!(ast.nodes.len(), 1);
    match &ast.nodes[0] {
        AstNode::CodeBlock { content, .. } => {
            assert_eq!(content, "<<set $x to 1>>\n");
        }
        other => panic!("expected CodeBlock, got {:?}", other),
    }
}

#[test]
fn test_checkbox_arg_schema() {
    let text = r#"<<checkbox "$color" "red" "blue">>"#;
    let ast = parse_passage_body(text, 0, ParseMode::Normal);
    // Verify the variable is at position 0, not position 1
    // (detailed assertion depends on structured_args shape)
}
```

---

### 7.3-7.7 Phases 3-7

(Detailed specs for Phases 3-7 will be expanded when each phase is entered. The high-level plan in Section 6 is sufficient for now. The pattern established in Phase 1 and Phase 2 — exact code, file paths, acceptance tests — will be followed for each subsequent phase.)

**Phase 3 (Headings + inline code)** key points:
- `b'!'` arm at line start (`ctx.col == 0`).
- Scan `!` run (1-6 chars; 7th+ becomes content).
- Content = rest of line, recursively parsed via `parse_body_with_ctx` with correct `col`.
- Token: `Heading` on `!` run, then recurse into children.

**Phase 4 (HR + blockquotes)** key points:
- `b'-'` arm at line start: check if rest of line matches `^----+\s*$`.
- `b'>'` arm at line start: scan `>` run, content = rest of line (recursive).
- `<<<` block form: special case in `b'<'` arm? Or separate arm? Actually `<<<` starts with `<` which currently triggers the `<<` macro arm. Need to check `bytes[i+2]` — if `b'<'` (not `b'<'`+`b'<'`... wait, `<<<` is `<`,`<`,`<`. The current `<<` arm checks `bytes[i+1] == b'<'`. For `<<<`, `bytes[i+1] == b'<'` is true, so it enters the macro arm. Need to add a check: if `bytes[i+2] == b'<'` AND at line start AND followed by `\n`, it's a blockquote block, not a macro.
- This is a **disambiguation challenge** — flag for careful handling in Phase 4.

**Phase 5 (Lists)** key points:
- `b'*'` and `b'#'` arms at line start.
- Scan marker run (`*+` or `#+`).
- Content = rest of line (recursive).
- Depth = marker char count.

**Phase 6 (Tables)** key points:
- `b'|'` arm at line start.
- Parse row: `|cell|cell|...|[fhck]?$`.
- Group consecutive rows into a `Table` node.
- Cell content recursive; `!` prefix → header cell; `>` → colspan; `~` → rowspan.

**Phase 7 (Macro catalog)** key points:
- Sub-phases 7a-7e as described in Section 6.
- Each sub-phase independently shippable.

---

## 8. Risk Analysis

### 8.1 High risk

#### 8.1.1 Line-start tracking refactor (Phase 1)

**Risk**: The `parse_body` function is called from multiple places (top-level, `parse_inline_style` recursion). Threading `col` through all paths without breaking existing behavior is non-trivial.

**Mitigation**: Use a `ParseCtx` struct (AD-1 option B). Test extensively with existing test suite. Phase 1 has NO behavior change — if tests pass, the refactor is correct.

#### 8.1.2 `<<<` blockquote disambiguation (Phase 4)

**Risk**: `<<<` starts with `<<`, which currently triggers the macro arm. A naive `b'<'` arm check would break macro parsing.

**Mitigation**: In the `b'<'` arm (currently checking `bytes[i+1] == b'<'`), add an additional check: if `ctx.col == 0` AND `bytes[i+2] == b'<'` AND (i+3 < len AND bytes[i+3] == b'\n'), it's a blockquote block — dispatch to `parse_blockquote_block` instead of `parse_macro`. This check must happen BEFORE the macro parsing logic.

#### 8.1.3 Table parsing complexity (Phase 6)

**Risk**: TiddlyWiki tables have many features (row types, colspan, rowspan, alignment, inline CSS). Implementing all at once is error-prone.

**Mitigation**: Implement basic table parsing first (body rows + header cells), then add row-type suffixes, then colspan/rowspan, then alignment/CSS. Each sub-feature gets its own test.

### 8.2 Medium risk

#### 8.2.1 Token legend index stability

**Risk**: Adding new `SemanticTokenType` variants could shift legend indices if not appended correctly, breaking existing token caches.

**Mitigation**: ALWAYS append new variants to the end of `all_types()`. Verify with a test that asserts existing indices are unchanged.

#### 8.2.2 `body_is_raw` field addition (Phase 7a)

**Risk**: Adding a field to `MacroDef` requires updating all 52+ catalog entries.

**Mitigation**: Use `Default` impl or a constructor helper. Add the field with `false` for all entries except `script`.

### 8.3 Low risk

#### 8.3.1 Code block parser (Phase 2a)

**Risk**: Low — `{{{...}}}` is a clean delimiter with no ambiguity (no nesting, no escape). Model is `parse_cstyle_comment`.

#### 8.3.2 checkbox/radiobutton fix (Phase 2b)

**Risk**: Low — straightforward catalog/snippet/completion form updates.

#### 8.3.3 Heading parser (Phase 3)

**Risk**: Low — `!` at line start is unambiguous. Content is rest of line, recursively parsed.

---

## 9. Open Questions

> Questions are resolved as the user answers them. Resolved questions are marked `[RESOLVED]` with the answer.

### Q1 — Heading macro behavior `[RESOLVED]`

**Question**: Does SugarCube execute macros inside headings?

**Answer**: YES. Per §3.5, the `heading` parser calls `w.subWikify(<hN>, '\n')`, which recursively runs the full Wikifier. `<<set>>` inside a heading executes silently.

**Decision**: Heading content is RECURSIVELY parsed (Option B2 from the original plan). `AstNode::Heading` has `children: Vec<AstNode>`, not `content: String`.

### Q2 — Token granularity `[RESOLVED]`

**Question**: Single token per construct, or split (marker + content)?

**Answer**: Split. For `! Some <<set>> heading`, the `!` gets a `Heading` token, `Some` and `heading` get prose tokens, `<<set>>` gets a macro token.

**Decision**: Marker gets its own token type; content is recursively tokenized. See AD-5.

### Q3 — Code block highlighting `[RESOLVED]`

**Question**: Single `codeBlock` token, or syntax highlighting inside?

**Answer**: Single token. Code blocks are non-executable display elements. A new token type (`CodeBlock` / `InlineCode`) is worth it (not prose).

**Decision**: Single token over full span. New token types `CodeBlock` and `InlineCode`. See AD-5.

### Q4 — List syntax verification `[RESOLVED]`

**Question**: SugarCube-native (`*`/`**`/`#`/`##`) or standard Markdown, or both?

**Answer**: SugarCube-native ONLY. Per §3.7, SugarCube does NOT support Markdown list syntax. Lists are `*`/`**`/`***` for ul, `#`/`##`/`###` for ol. NO mixed markers (`*#`). NO leading whitespace.

**Decision**: Implement SugarCube-native list syntax only. See AD-2, AD-9.

### Q5 — Phase ordering `[RESOLVED]`

**Question**: One big PR or phased?

**Answer**: Phased. Each phase documented in detail in this markdown file. Read this file at the start of each session. Append worklog and status at the end of each phase.

**Decision**: Seven phases per Section 6. This document is the single source of truth. See AD-11.

### Q6 — `<<code>>` macro `[RESOLVED]`

**Question**: Did the user mean `<<code>>` macro or `{{{...}}}` code block?

**Answer**: `{{{...}}}` code block. There is NO `<<code>>` macro in SugarCube (confirmed by research — see §3.12). The bug is that `{{{...}}}` is not parsed as a raw code block, so macros inside it execute.

**Decision**: Implement `{{{...}}}` code block parsing (Phase 2a). Do NOT add a `<<code>>` macro.

### Q7 — Scope of macro fixes `[RESOLVED]`

**Question**: Fix all ~20 macros or just checkbox/radiobutton?

**Answer**: Fix all, but in phases. Phase 2b fixes checkbox/radiobutton (critical). Phase 7 fixes the rest.

**Decision**: Phase 2b = checkbox/radiobutton. Phase 7 = all other macros. See Section 6.

### Q8 — SugarCube version targeting `[RESOLVED]`

**Question**: The research targeted v2.37.3 (latest stable). Should Knot target v2.37.x specifically, or maintain backward compatibility with older versions?

**Answer**: Maintain backward compatibility. Mark deprecated macros/functions but keep them active (with warnings). Don't remove support for older SugarCube versions.

**Decision**: Keep all existing macros (including `<<silently>>`, `<<click>>`, `<<display>>`, etc.) but mark deprecated ones with `deprecated: true` and `deprecation_message`. Add the new v2.37.0 macros (`<<silent>>`, `<<do>>`, `<<redo>>`). See Phase 7b.

### Q9 — `<<silently>>` deprecation handling `[RESOLVED]`

**Question**: `<<silently>>` is deprecated v2.37.0 (replaced by `<<silent>>`). Should Knot mark it as deprecated in the catalog (with `deprecated: true` and `deprecation_message`), or remove it?

**Answer**: Mark as deprecated (don't remove — users may still have it in their stories). Add `<<silent>>` as the replacement.

**Decision**: `<<silently>>` stays in catalog with `deprecated: true`. `<<silent>>` added as new entry. See Phase 7b.

### Q10 — Verbatim text and HTML `[RESOLVED]`

**Question**: Should `"""..."""` (verbatim text) and `<html>...</html>` (verbatim HTML) be added to this plan, or deferred?

**Answer**: `"""..."""` verbatim text is a lexical concern — DO NOW (add to this plan). `<html>...</html>` verbatim HTML is an embedded language concern — DEFER.

**Decision**: Add `"""..."""` verbatim text support (raw zone, like code blocks) to the plan. Deferred `<html>...</html>` to a future plan. The verbatim text work will be added as Phase 4.5 (after blockquotes, before lists) or folded into Phase 2 alongside code blocks (since both are raw zones). Final placement TBD during Phase 2 planning.

---

## 10. Phase Status

> Update this section at the end of each phase. Mark each phase as `NOT_STARTED`, `IN_PROGRESS`, `COMPLETED`, or `BLOCKED`.

| Phase | Description | Status | Completed Date | Notes |
|---|---|---|---|---|
| Planning | Research + plan document | COMPLETED | 2026-06-23 | User signed off; Q8/Q9/Q10 resolved |
| 1 | Foundation: line-start tracking + AST variants + token types | COMPLETED | 2026-06-23 | 794 tests pass (637 formats + 77 core + 80 server). +7 new Phase 1 tests. No behavior change. |
| 2a | Code blocks (`{{{...}}}`) | COMPLETED | 2026-06-23 | Critical bug fixed: macros inside `{{{...}}}` no longer execute. +11 new Phase 2a tests. |
| 2b | checkbox/radiobutton catalog fix | COMPLETED | 2026-06-23 | Arg schemas corrected; snippet/completion form value order fixed; classifier now recognizes quoted variable names. +6 new Phase 2b tests. |
| 3 | Headings + inline code | COMPLETED | 2026-06-23 | Headings (`!`-`!!!!!!`) with recursive content (macros execute inside). +14 new Phase 3 tests. Inline code already done in Phase 2a. |
| 4 | Horizontal rule + blockquotes | COMPLETED | 2026-06-23 | HR (`----`), line-style blockquote (`>`/`>>`), block-style blockquote (`<<<...<<<` with `<<` disambiguation). +21 new Phase 4 tests. |
| 5 | Lists | COMPLETED | 2026-06-23 | `*` (ul) / `#` (ol) with depth-based nesting. NO mixed markers. +17 new Phase 5 tests. |
| 6 | Tables | COMPLETED | 2026-06-23 | TiddlyWiki tables with row-type suffixes (h/f/c/k), header cells (!), colspan (>), rowspan (~). +18 new Phase 6 tests. All block-level markup phases complete. |
| 7a | Generalize raw-body mechanism | COMPLETED | 2026-06-23 | `body_is_raw` field added to MacroDef (catalog-driven). `<<style>>`/`<<css>>` removed from catalog. Hardcoded check replaced with catalog lookup. +5 new Phase 7a tests. |
| 7b | Add missing macros (`silent`, `do`, `redo`, `choice`) | COMPLETED | 2026-06-23 | Added `<<choice>>`, `<<setplaylist>>`, `<<stopallaudio>>`. Verified `<<silent>>`/`<<do>>`/`<<redo>>` already present. All removed macros marked deprecated. +13 new Phase 7b tests. |
| 7c | New `MacroArgKind` variants | COMPLETED | 2026-06-23 | Added `Keyword`, `Link`, `Image`, `Number` to `MacroArgKind` + corresponding `ParsedArgKind` variants. Updated classifier + token builder. |
| 7d | Fix arg schemas (batch) | COMPLETED | 2026-06-23 | Fixed `textbox`, `numberbox`, `textarea`, `option`, `include`, `widget`, `script`, `cacheaudio` arg schemas. +15 new Phase 7c/7d tests. |
| 7e | Update snippets and completion forms | COMPLETED | 2026-06-23 | Updated snippets for all modified macros. All snippets match corrected schemas. |

---

## 11. Worklog

> Append a new entry at the end of each session. Format:
> ```
> ### YYYY-MM-DD HH:MM UTC — Session: <brief description>
> **Agent**: <agent name>
> **Phase(s) worked on**: <phase numbers>
> 
> **What was done**:
> - <concrete step 1>
> - <concrete step 2>
> 
> **Files modified**:
> - <path 1>
> - <path 2>
> 
> **Decisions made**:
> - <decision 1>
> 
> **Blockers / open issues**:
> - <issue 1>
> 
> **Next session should**:
> - <next step 1>
> ```

### 2026-06-23 — Session: Planning and research

**Agent**: Super Z (main)
**Phase(s) worked on**: Planning

**What was done**:
- Cloned Knot repo (branch `ver_3`) to `/home/z/my-project/Knot`.
- Dispatched 3 parallel investigation agents to map:
  1. Parser core dispatch, AST variants, text flushing, comment parsing model, line-start awareness, inline formatting.
  2. Semantic tokens pipeline, TextMate grammar, semanticTokenScopes mapping.
  3. Macro catalog, arg types, checkbox/radiobutton/code macro state, raw/passthrough mechanism.
- Dispatched 3 parallel research agents to verify against authoritative sources:
  1. SugarCube block-level markup syntax (lists, headings, hr, blockquotes, tables, code blocks).
  2. Complete SugarCube builtin macro catalog with exact arg signatures.
  3. SugarCube heading/code-block macro execution semantics.
- Synthesized all findings into this `plan.md` document.

**Key research findings**:
- SugarCube does NOT use Marked.js — it uses a TiddlyWiki-derived Wikifier.
- Lists are `*`/`**`/`#`/`##` ONLY (no Markdown syntax, no mixed markers, no leading whitespace).
- Headings DO process macros recursively (Claim B was correct).
- Code blocks (`{{{...}}}`) are raw zones — macros do NOT execute.
- Block code vs inline code is disambiguated by position (block = `{{{`+newline at col 0; inline = `{{{` mid-line).
- Tables use TiddlyWiki syntax (undocumented in official docs but in source).
- Blockquotes have two forms: `>` (line, documented) and `<<<...<<<` (block, undocumented).
- `<<style>>`, `<<css>>`, `<<code>>`, `<<verbatim>>`, `<<html>>` are NOT macros.
- 59 builtin macros + 8 sub-macros; 6 removed in v2.37.0; 3 new in v2.37.0.

**Files modified**:
- `/home/z/my-project/Knot/plan.md` (created — this document)

**Decisions made**:
- All architectural decisions in Section 5 (AD-1 through AD-11).
- Seven-phase implementation plan (Section 6).
- TextMate grammar stays minimal (no new patterns) per user Q1.
- New token types: `Heading`, `HorizontalRule`, `ListMarker`, `Blockquote`, `BlockquoteBlock`, `Table`, `CodeBlock`, `InlineCode`.

**Blockers / open issues**:
- Q8, Q9, Q10 need user confirmation (SugarCube version targeting, `<<silently>>` deprecation handling, verbatim text/HTML scope).
- Awaiting user sign-off on the plan before starting Phase 1.

**Next session should**:
- Read this document in full (especially Section 10 Phase Status and Section 11 Worklog).
- Confirm Q8, Q9, Q10 with user if not already resolved.
- Begin Phase 1 (Foundation) per Section 7.1.
- After Phase 1: run `cargo build --release` and `cargo test`, append worklog entry, zip modified files to `/home/z/my-project/download/`.

---

### 2026-06-23 (later) — Session: Phase 1 implementation

**Agent**: Super Z (main)
**Phase(s) worked on**: 1 (Foundation)

**What was done**:
- Installed Rust toolchain (rustup stable, cargo 1.96.0) — was not pre-installed.
- Established baseline: 630 tests pass in `knot-formats`.
- Implemented AD-1 (line-start tracking via `ParseCtx` struct) in `core.rs`:
  - Added `ParseCtx { offset, col }` struct.
  - Refactored `parse_body` into `parse_body` (public wrapper) + `parse_body_with_ctx` (workhorse).
  - Added `resync_col_after_advance` helper to recompute `ctx.col` after sub-parsers advance `i`.
  - Added `compute_initial_col` helper for recursive `parse_body` calls from `parse_inline_style`.
  - Updated the `//` heuristic to use `ctx.col == 0` (with `bytes[i-1] == b'\n'` fallback for backward compat).
  - Threaded `ctx.col` updates through all 15 dispatch arms + the catch-all.
  - Added explicit `b'\n'` arm to reset `ctx.col = 0`.
- Added 10 new `AstNode` variants to `ast.rs` (per AD-3):
  - `Heading`, `HorizontalRule`, `ListItem`, `Blockquote`, `BlockquoteBlock`, `Table`, `CodeBlock`, `InlineCode`.
  - Plus supporting types: `TableRow`, `TableCell`, `TableRowType`.
- Added 8 new `SemanticTokenType` variants to `plugin.rs` (per AD-4):
  - `Heading`, `HorizontalRule`, `ListMarker`, `Blockquote`, `BlockquoteBlock`, `Table`, `CodeBlock`, `InlineCode`.
  - Updated `all_types()` (appended at indices 22-29 to preserve 0-21) and `lsp_name()`.
- Added placeholder match arms in 3 files to satisfy exhaustiveness:
  - `token_builder.rs` (in `build_semantic_tokens_at_depth`) — no-op arms for all 8 new variants.
  - `passage_build.rs` (in `build_body_blocks`) — no-op arms.
  - `registry_populate.rs` (in `extract_widget_arg_count`'s `scan_node`) — no-op arms.
- Added `semanticTokenScopes` entries to all 5 language blocks in `package.json`:
  - `twee`, `twee-sugarcube`, `twee-harlowe`, `twee-chapbook`, `twee-snowman`.
  - Each block got 8 new scope mappings (heading, horizontalRule, listMarker, blockquote, blockquoteBlock, table, codeBlock, inlineCode).
- Added 7 new Phase 1 tests in `core.rs` test module:
  - `phase1_line_start_comment_at_column_zero` — verifies `ctx.col == 0` branch in `//` heuristic.
  - `phase1_mid_line_double_slash_not_a_comment` — verifies mid-line `//` is not a comment.
  - `phase1_comment_after_newline_at_column_zero` — verifies column reset on `\n`.
  - `phase1_multiline_text_preserves_column_tracking` — verifies column tracking across multiple lines.
  - `phase1_new_ast_variants_are_constructible` — compile-time check that new variants exist with correct fields.
  - `phase1_new_semantic_token_types_exist` — verifies wire names and legend indices (22-29).
  - `phase1_inline_style_recursive_parse_preserves_behavior` — regression test for `parse_inline_style` refactor.
- Resolved Q8, Q9, Q10 in Section 9.

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — line-start tracking refactor + new tests.
- `crates/formats/src/sugarcube/ast.rs` — 10 new AstNode variants + `TableRow`/`TableCell`/`TableRowType`.
- `crates/formats/src/plugin.rs` — 8 new `SemanticTokenType` variants + `all_types()` + `lsp_name()`.
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — placeholder match arms for new variants.
- `crates/formats/src/sugarcube/graph/passage_build.rs` — placeholder match arms.
- `crates/formats/src/sugarcube/registries/registry_populate.rs` — placeholder match arms.
- `extensions/vscode/package.json` — 8 new scope mappings × 5 language blocks (40 entries total).
- `plan.md` — Phase Status updated, Q8/Q9/Q10 resolved, this worklog entry.

**Verification**:
- `cargo build --manifest-path crates/formats/Cargo.toml` — succeeds.
- `cargo build --release --manifest-path crates/server/Cargo.toml` — succeeds.
- `cargo test` (full workspace) — 794 tests pass, 0 failures:
  - core: 77 passed
  - formats: 637 passed (was 630, +7 new Phase 1 tests)
  - server: 80 passed
- No behavior change — all new AST variants are unused (parser doesn't emit them yet).
- `package.json` validated as valid JSON.
- All 5 language blocks verified to have all 8 new scope entries.

**Decisions made**:
- Used `ParseCtx` struct (AD-1 option B) for extensibility.
- Kept the `bytes[i-1] == b'\n'` peek as a fallback in the `//` heuristic for backward compatibility during the refactor — can be removed in a later phase once confidence is established.
- Used `resync_col_after_advance` helper rather than threading `ctx` into every sub-parser — minimizes the refactor surface.
- Added explicit `b'\n'` arm in the main match (instead of relying on the catch-all) for clarity and to ensure `ctx.col` resets correctly.
- Appended new `SemanticTokenType` variants at indices 22-29 to preserve existing legend indices 0-21 (per AD-4 and the §4.3.3 quirk).

**Blockers / open issues**:
- None. Phase 1 is complete and ready for Phase 2.

**Next session should**:
- Read this document (especially Section 10 Phase Status and Section 11 Worklog).
- Begin Phase 2a (Code blocks `{{{...}}}`) per Section 7.2.
- The `b'{'` arm in `core.rs` should use `ctx.col == 0` to disambiguate block vs inline code (per AD-10).
- After Phase 2a: run tests, append worklog entry, zip modified files to `/home/z/my-project/download/`.
- Then proceed to Phase 2b (checkbox/radiobutton catalog fix).

---

### 2026-06-23 (later still) — Session: Phase 2 implementation (2a + 2b)

**Agent**: Super Z (main)
**Phase(s) worked on**: 2a (Code blocks), 2b (checkbox/radiobutton catalog fix)

**What was done**:

#### Phase 2a — Code blocks (`{{{...}}}`)

- Added `b'{'` arm to `parse_body_with_ctx` in `core.rs` (before the `b'\n'` arm).
  - Detects `{{{` (3-byte lookahead).
  - Disambiguates block vs inline per AD-10: block = `ctx.col == 0` BEFORE consuming `{{{` AND next byte is `\n`; inline = otherwise.
  - Captured `at_col_zero` BEFORE incrementing `ctx.col` (initial attempt used `ctx.col == 3` which was wrong — fixed).
- Added `parse_code_block` helper:
  - Scans line-by-line for `^}}}$` (alone on its own line at column 0).
  - Handles `\r\n` and bare `\n` line endings.
  - On unclosed, consumes rest of text.
  - Emits `AstNode::CodeBlock { content, span }`.
- Added `parse_inline_code` helper:
  - Non-greedy scan for first `}}}` (matches SugarCube's `formatByChar` regex).
  - On unclosed, consumes rest of text.
  - Emits `AstNode::InlineCode { content, span }`.
- Updated `token_builder.rs`:
  - Replaced placeholder arms for `CodeBlock` and `InlineCode` with real token emission (single token over full span per AD-5).
  - Kept placeholder arms for `Heading`/`HorizontalRule`/`ListItem`/`Blockquote`/`BlockquoteBlock`/`Table` (those land in Phases 3-6).
- Added 11 Phase 2a tests in `core.rs`:
  - `phase2a_inline_code_does_not_execute_macros` — the critical bug fix test.
  - `phase2a_block_code_multiline_does_not_execute_macros` — block form.
  - `phase2a_inline_code_with_variables_not_interpolated` — vars are literal.
  - `phase2a_inline_code_with_links_not_processed` — links are literal.
  - `phase2a_block_code_disambiguation_requires_newline` — `{{{` at col 0 without `\n` is inline.
  - `phase2a_block_code_mid_line_is_inline` — `{{{` not at col 0 is inline.
  - `phase2a_unclosed_inline_code_consumes_to_end` — unclosed handling.
  - `phase2a_unclosed_block_code_consumes_to_end` — unclosed handling.
  - `phase2a_inline_code_emits_codeblock_token` — token builder emits InlineCode token.
  - `phase2a_block_code_emits_codeblock_token` — token builder emits CodeBlock token.
  - `phase2a_inline_code_followed_by_macro` — macro outside code block still executes.
- Fixed format string errors in test assertions (Rust format strings interpret `{{{` and `}}}` as placeholders — rephrased to avoid braces in message strings).

#### Phase 2b — checkbox/radiobutton catalog fix

- Rewrote `<<checkbox>>` arg schema in `catalog.rs`:
  - Position 0: `receiverName` (Variable, `is_variable: true`, `is_required: true`).
  - Position 1: `uncheckedValue` (String, `is_required: true`).
  - Position 2: `checkedValue` (String, `is_required: true`).
  - Dropped the spurious 4th "unchecked" arg (was wrong — SugarCube has only 3 positional value args + optional keywords).
  - `autocheck`/`checked` keywords deferred to Phase 7c (needs `MacroArgKind::Keyword`).
- Rewrote `<<radiobutton>>` arg schema:
  - Position 0: `receiverName` (Variable, `is_variable: true`, `is_required: true`).
  - Position 1: `checkedValue` (String, `is_required: true`).
  - Dropped the spurious 3rd arg (radiobutton has only 2 positional value args + optional keywords).
- Fixed `checkbox` snippet in `snippets.rs`: swapped `"${2:checked}" "${3:unchecked}"` → `"${2:unchecked}" "${3:checked}"`.
- Fixed `CHECKBOX_FORMS` in `completion_forms.rs`: swapped checked/unchecked in label, detail, and snippet.
- **Discovered and fixed a classifier bug** in `macro_parser.rs`:
  - SugarCube form macros take the receiver variable as a **quoted** string (e.g., `"$color"`), not an unquoted `$color`.
  - The existing classifier only recognized unquoted `$var` tokens as `VariableRef`. Quoted `"$var"` was classified as `Expression` or `String`, breaking variable-write tracking.
  - Added `ArgToken::is_quoted_variable_name()` helper: returns true for quoted strings whose content starts with `$` or `_` followed by an identifier-start char.
  - Updated `parse_structured_args` to check `token.is_variable_ref() || token.is_quoted_variable_name()` in both the `def.is_variable` path and the `MacroArgKind::Variable` path.
- Deferred `body_is_raw` field to Phase 7a (per plan option b) to keep Phase 2b focused.
- Added 6 Phase 2b tests in `core.rs`:
  - `phase2b_checkbox_variable_at_position_zero` — verifies arg 0 is `VariableRef`.
  - `phase2b_radiobutton_variable_at_position_zero` — verifies arg 0 is `VariableRef`.
  - `phase2b_checkbox_var_refs_extracted_from_receiver_arg` — verifies `"$color"` produces a var_ref.
  - `phase2b_radiobutton_var_refs_extracted_from_receiver_arg` — same for radiobutton.
  - `phase2b_checkbox_snippet_has_correct_value_order` — verifies snippet has unchecked THEN checked.
  - `phase2b_checkbox_completion_form_has_correct_value_order` — verifies completion form.

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — `b'{'` arm, `parse_code_block`, `parse_inline_code`, +17 Phase 2 tests (11 for 2a, 6 for 2b).
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — real token emission for `CodeBlock`/`InlineCode`.
- `crates/formats/src/sugarcube/macros/catalog.rs` — fixed `checkbox`/`radiobutton` arg schemas.
- `crates/formats/src/sugarcube/macros/snippets.rs` — fixed checkbox snippet value order.
- `crates/formats/src/sugarcube/macros/completion_forms.rs` — fixed CHECKBOX_FORMS value order.
- `crates/formats/src/sugarcube/parser/macro_parser.rs` — added `is_quoted_variable_name` helper + classifier fix.
- `plan.md` — Phase Status updated, this worklog entry.

**Verification**:
- `cargo build --manifest-path crates/formats/Cargo.toml` — succeeds.
- `cargo build --release --manifest-path crates/server/Cargo.toml` — succeeds.
- `cargo test` (full workspace) — **811 tests pass, 0 failures**:
  - core: 77 passed
  - formats: 654 passed (was 648 after Phase 2a, +6 new Phase 2b tests; was 637 before Phase 2, +17 total new Phase 2 tests)
  - server: 80 passed

**Decisions made**:
- Captured `at_col_zero = ctx.col == 0` BEFORE incrementing `ctx.col` by 3, then checked `at_col_zero && bytes[i] == b'\n'` for block disambiguation. (Initial attempt used `ctx.col == 3` post-increment, which only worked if the `{{{` was at column 0 — failed for col-0 case where col went 0→3. Fixed by capturing the pre-increment value.)
- Deferred `body_is_raw` field to Phase 7a to keep Phase 2b focused on the checkbox/radiobutton fix.
- Added `is_quoted_variable_name` helper rather than changing `scan_arg_tokens` to produce `VariableRef` tokens for quoted variable names — the latter would have affected all macros and risked regressions. The helper is only consulted when `def.is_variable` is true or `def.kind == Variable`.
- Kept the `autocheck`/`checked` optional keywords out of the catalog for now (deferred to Phase 7c when `MacroArgKind::Keyword` exists). The catalog declares only the 3 required positional args for checkbox and 2 for radiobutton.

**Blockers / open issues**:
- None. Phase 2 is complete and ready for Phase 3.

**Next session should**:
- Read this document (especially Section 10 Phase Status and Section 11 Worklog).
- Begin Phase 3 (Headings + inline code) per Section 7.3.
- Phase 3 adds the `b'!'` arm for headings (`!` through `!!!!!!`) at line start (`ctx.col == 0`), with recursive content parsing (macros execute inside headings per §3.5).
- The `b'{'` arm for inline code is already done (Phase 2a), so Phase 3 only needs headings.
- After Phase 3: run tests, append worklog entry, zip modified files to `/home/z/my-project/download/`.

---

### 2026-06-23 (audit) — Session: Architecture audit (post-Phase 2 sanity check)

**Agent**: Super Z (main)
**Phase(s) worked on**: Audit (no phase progression)

**What was done**:
- User requested an architecture sanity check: verify the parser produces a proper AST and that ALL downstream consumers (tokens, links, var_ops, diagnostics) walk the AST — NOT the raw body text via regex.
- Audited every AST consumer in the codebase:
  - `token_builder.rs` (`build_semantic_tokens_at_depth`, `build_diagnostics`) — both walk the AST via `match node` / `if let`. Clean.
  - `extraction.rs` (`extract_links_recursive`, `extract_var_ops_recursive`) — both walk the AST with `_ => {}` catch-alls. `CodeBlock`/`InlineCode` are correctly skipped (links/vars inside code blocks are literal). Clean.
  - `js_annotate.rs` (`annotate_inline_js`) — walks the AST with `_ => {}` catch-all. Skips code blocks. Clean.
  - `registry_populate.rs` (`extract_widget_arg_count`) — walks the AST. Placeholder arms added in Phase 1. Clean.
  - `passage_build.rs` (`build_body_blocks`) — walks the AST. Placeholder arms. Clean.
  - `macro_parser.rs` (`parse_structured_args`) — operates on the args string already isolated by the parser (NOT raw body text). The `is_quoted_variable_name` helper added in Phase 2b works on `ArgToken`s from this isolated args string. Clean.
- **Found one gap**: `extract_data_passage_refs` in `extraction.rs:453` scans the RAW body text for `data-passage="..."` after `strip_comments`. It does NOT walk the AST, so a `data-passage` inside a code block would be incorrectly extracted. This is a PRE-EXISTING limitation (not a Phase 2 regression). Deferred to a future phase.
- Added 3 **architecture invariant tests** in `core.rs` that verify `ast.links` and `ast.var_ops` are empty when a link/variable appears inside a code block. These serve as regression guards: if any future change makes a consumer scan raw text instead of the AST, these tests will fail.
- Updated the stale comment in `passage_build.rs` (Phase 1 scaffolding comment said "not yet emitted" — Phase 2a made CodeBlock/InlineCode emitted).
- Documented the full audit findings in plan.md §4.7 (new section).

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — +3 architecture invariant tests.
- `crates/formats/src/sugarcube/graph/passage_build.rs` — updated stale comment.
- `plan.md` — added §4.7 (audit findings), this worklog entry.

**Verification**:
- `cargo test` (full workspace) — **814 tests pass, 0 failures** (was 811, +3 new arch tests).

**Decisions made**:
- Deferred the `extract_data_passage_refs` fix to a future phase. It's a pre-existing limitation, not a regression, and the fix requires threading "am I inside a raw zone?" context through an AST walk — a non-trivial refactor.
- Added architecture invariant tests rather than fixing the gap now — the tests document the intended behavior and guard against future regressions.

**Blockers / open issues**:
- None. Architecture is clean. Phase 3 can proceed.

**Next session should**:
- Read this document (especially §4.7 audit findings and Section 10 Phase Status).
- Begin Phase 3 (Headings) per Section 7.3.
- The `extract_data_passage_refs` gap is documented but NOT blocking — it can be addressed in a future phase alongside the other raw-zone constructs (verbatim text `"""..."""`, etc.).

---

### 2026-06-23 (Phase 3) — Session: Headings (`!` through `!!!!!!`)

**Agent**: Super Z (main)
**Phase(s) worked on**: 3 (Headings)

**What was done**:
- Added `b'!'` arm to `parse_body_with_ctx` in `core.rs` (before the `b'\n'` arm).
  - Guard: `ctx.col == 0` — column-0 anchored per §3.5 (no leading whitespace allowed).
  - A `!` mid-line falls through to the catch-all and becomes plain text.
- Added `parse_heading` helper function:
  - Scans 1-6 `!` characters (level = count). A 7th `!` is left for content.
  - Finds end of line (next `\n` or end of text).
  - Extracts content substring (after `!` run, up to end of line).
  - Recursively parses content via `parse_body_with_ctx` with a child `ParseCtx`:
    - `offset = offset + content_start`
    - `col = level` (content starts at that column in the original text)
  - Advances `*i` to end of line (NOT past `\n` — main loop handles it).
  - Span covers `!` run through end of line (exclusive of `\n`).
  - Returns `AstNode::Heading { level, children, span }`.
- Updated `token_builder.rs` `Heading` arm:
  - Emits a `Heading` token for the `!` run (length = `level`).
  - Recurses into `children` via `build_semantic_tokens_at_depth` (preserving surrounding depth, per §4.3.4 recommendation — avoids the `InlineStyle` depth-reset behavior).
  - This produces: `Heading` token on `!`, then prose/macro/variable/link tokens for content.
- Added 14 Phase 3 tests in `core.rs`:
  - `phase3_heading_level_1` — single `!` → level 1.
  - `phase3_heading_level_3` — `!!!` → level 3.
  - `phase3_heading_level_6_max` — `!!!!!!` → level 6 (max).
  - `phase3_heading_seventh_bang_becomes_content` — 7th `!` becomes content.
  - `phase3_heading_with_space_after_bangs` — space after `!` is part of content.
  - `phase3_heading_with_leading_whitespace_not_a_heading` — ` !` is NOT a heading.
  - `phase3_heading_mid_line_not_a_heading` — mid-line `!` is NOT a heading.
  - `phase3_heading_macros_execute_inside` — CRITICAL: `<<set>>` inside heading is a real Macro node.
  - `phase3_heading_with_variable_reference` — `$name` inside heading is interpolated (in var_refs).
  - `phase3_heading_with_link` — `[[Forest]]` inside heading is a real Link node.
  - `phase3_heading_followed_by_text_on_next_line` — heading + prose on next line parses correctly.
  - `phase3_heading_no_trailing_newline` — heading at end of text (no `\n`).
  - `phase3_heading_emits_heading_token_and_content_tokens` — token builder emits Heading + Prose tokens.
  - `phase3_heading_with_macro_emits_macro_token` — macros inside headings get their own token (recursive tokenization).

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — `b'!'` arm, `parse_heading` helper, +14 Phase 3 tests.
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — `Heading` arm with marker token + recursive child tokenization.
- `plan.md` — Phase Status updated, this worklog entry.

**Verification**:
- `cargo build --manifest-path crates/formats/Cargo.toml` — succeeds.
- `cargo build --release --manifest-path crates/server/Cargo.toml` — succeeds.
- `cargo test` (full workspace) — **828 tests pass, 0 failures**:
  - core: 77 passed
  - formats: 671 passed (was 657, +14 new Phase 3 tests)
  - server: 80 passed

**Decisions made**:
- Heading span covers `!` run through end of line (exclusive of `\n`). The `\n` is consumed by the main loop's `b'\n'` arm, which resets `ctx.col` and merges with the next line's text. This matches SugarCube's `heading` parser (terminator = `\n`).
- Heading content is recursively parsed via `parse_body_with_ctx` with `col = level` (the content starts at column `level` in the original text). This ensures any block-level constructs inside heading content (rare but possible) are correctly disambiguated.
- Token builder recurses with `build_semantic_tokens_at_depth` (NOT `build_semantic_tokens`) to preserve the surrounding depth — macros inside headings get the correct `BlockDepthN` modifier. This follows the §4.3.4 recommendation.
- The `Heading` token covers ONLY the `!` run (length = `level`), NOT the full span. Content tokens are emitted by the recursive call. This matches the user's Q2 answer (split spans: marker gets its own token, content gets normal prose/macro tokens).

**Blockers / open issues**:
- None. Phase 3 is complete and ready for Phase 4.

**Next session should**:
- Read this document (especially Section 10 Phase Status and Section 11 Worklog).
- Begin Phase 4 (Horizontal rule + blockquotes) per Section 7.3.
- Phase 4 adds:
  - `b'-'` arm at line start: check if rest of line matches `^----+\s*$` (4+ dashes alone on a line).
  - `b'>'` arm at line start: scan `>` run, content = rest of line (recursive).
  - `<<<` block form: special disambiguation in the `b'<'` arm (before the `<<` macro check) — see §8.1.2.
- After Phase 4: run tests, append worklog entry, zip modified files to `/home/z/my-project/download/`.

---

### 2026-06-23 (Phase 4) — Session: Horizontal rule + blockquotes

**Agent**: Super Z (main)
**Phase(s) worked on**: 4 (Horizontal rule + blockquotes)

**What was done**:

#### Horizontal rule (`----`)

- Added `b'-'` arm to `parse_body_with_ctx` — guard: `ctx.col == 0 && is_horizontal_rule_line(&text[i..])`.
- Added `is_horizontal_rule_line` helper — checks for 4+ dashes, then optional trailing whitespace, then end-of-line. Returns `false` for `---` (3 dashes), `--` (emdash), or `---- text` (trailing non-whitespace).
- Added `parse_horizontal_rule` helper — consumes the dash run, skips trailing whitespace (NOT part of span), leaves `\n` for the main loop. Span covers the dash run only.
- Updated `token_builder.rs` `HorizontalRule` arm — single token over the dash run span, no recursion (void element).

#### Line-style blockquote (`>`, `>>`, etc.)

- Added `b'>'` arm to `parse_body_with_ctx` — guard: `ctx.col == 0`.
- Added `parse_blockquote_line` helper — scans 1+ `>` chars (depth = count, capped at 255 for u8 safety), finds end of line, recursively parses content via `parse_body_with_ctx` with `col = depth`. Span covers `>` run through end of line (exclusive of `\n`).
- Updated `token_builder.rs` `Blockquote` arm — emits `Blockquote` token for the `>` run (length = depth), then recurses into children via `build_semantic_tokens_at_depth` (preserving depth).

#### Block-style blockquote (`<<<...<<<`) — undocumented but in SugarCube source

- **Critical disambiguation** in the `b'<'` arm (per §8.1.2): BEFORE the `<<` macro check, test if `ctx.col == 0 && bytes[i+2] == b'<' && (i+3 == len || bytes[i+3] == b'\n')`. If so, dispatch to `parse_blockquote_block`. Otherwise, fall through to the `<<` macro parser.
- Added `parse_blockquote_block` helper:
  - Consumes opening `<<<`, content starts after the following `\n`.
  - Scans line-by-line for closing `<<<` (alone on its own line at column 0).
  - Recursively parses content via `parse_body_with_ctx` with `col = 0` (content starts on its own line).
  - On unclosed, consumes to end of text with `close_span = None`.
  - Span covers opening `<<<` through end of closing `<<<` line.
- Updated `token_builder.rs` `BlockquoteBlock` arm — emits `BlockquoteBlock` token for opening `<<<`, recurses into children, emits `BlockquoteBlock` token for closing `<<<` (if present).

#### Tests

Added 21 Phase 4 tests in `core.rs`:
- **Horizontal rule (7 tests)**: basic `----`, 5 dashes, trailing whitespace, 3 dashes NOT a HR, trailing text NOT a HR, leading space NOT a HR, token emission.
- **Line-style blockquote (7 tests)**: depth 1, depth 2, space after marker, leading space rejection, macros execute inside, links processed, token + content emission.
- **Block-style blockquote (7 tests)**: basic `<<<...<<<`, macros inside, unclosed handling, disambiguation from `<<` macro, `<<<` not at col 0 is NOT a blockquote block, token emission (2 tokens: open + close), multi-paragraph content.

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — `b'<'` disambiguation, `b'-'` arm, `b'>'` arm, `is_horizontal_rule_line`, `parse_horizontal_rule`, `parse_blockquote_line`, `parse_blockquote_block` helpers, +21 Phase 4 tests.
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — `HorizontalRule`, `Blockquote`, `BlockquoteBlock` arms with marker tokens + recursive child tokenization.
- `plan.md` — Phase Status updated, this worklog entry.

**Verification**:
- `cargo build --manifest-path crates/formats/Cargo.toml` — succeeds.
- `cargo build --release --manifest-path crates/server/Cargo.toml` — succeeds.
- `cargo test` (full workspace) — **849 tests pass, 0 failures**:
  - core: 77 passed
  - formats: 692 passed (was 671, +21 new Phase 4 tests)
  - server: 80 passed

**Decisions made**:
- **`<<<` disambiguation**: placed the check INSIDE the existing `b'<'` arm (before the `<<` macro logic), rather than as a separate arm. This is because `<<<` starts with `<<`, so the `b'<'` arm with `bytes[i+1] == b'<'` guard catches it first. The check must happen BEFORE macro parsing to avoid misinterpreting `<<<` as `<<` + `<`. The guard requires `ctx.col == 0` (column-0 anchored) AND `bytes[i+2] == b'<'` (third `<`) AND followed by `\n` or end-of-text (per SugarCube's `^<<<\n` regex).
- **Horizontal rule span**: covers ONLY the dash run, NOT trailing whitespace. This matches SugarCube's behavior — the `<hr>` element has no content. Trailing whitespace is consumed by the parser (to advance past it) but not included in the span.
- **Blockquote line depth**: capped at 255 (u8 max) for safety. SugarCube has no limit, but 256+ `>` characters is extremely unlikely in practice. If it happens, depth wraps at 255, which still renders as deeply nested blockquotes.
- **Blockquote block close_span**: `None` when unclosed. The token builder checks for `Some` before emitting the closing delimiter token, so unclosed blocks only get an opening token.
- **Token builder recursion for blockquotes**: used `build_semantic_tokens_at_depth` (NOT `build_semantic_tokens`) to preserve the surrounding depth, per §4.3.4. For line-style blockquotes, the recursive depth is `*depth as usize` (the `>` count). For block-style blockquotes, the recursive depth is the surrounding `depth` (block content doesn't add nesting level).

**Blockers / open issues**:
- None. Phase 4 is complete and ready for Phase 5.

**Next session should**:
- Read this document (especially Section 10 Phase Status and Section 11 Worklog).
- Begin Phase 5 (Lists) per Section 7.3.
- Phase 5 adds `b'*'` and `b'#'` arms at line start (`ctx.col == 0`):
  - Scan marker run (`*+` or `#+` — all-`*` or all-`#`, NO mixed markers like `*#`).
  - Depth = marker char count.
  - Content = rest of line (recursive).
  - Emit `ListItem` node with `depth`, `ordered`, `marker`, `children`, `span`.
  - Token: `ListMarker` on the marker run, then recurse into children.
- After Phase 5: run tests, append worklog entry, zip modified files to `/home/z/my-project/download/`.

---

### 2026-06-23 (Phase 5) — Session: Lists (`*` / `#`)

**Agent**: Super Z (main)
**Phase(s) worked on**: 5 (Lists)

**What was done**:
- Added `b'*'` arm to `parse_body_with_ctx` — guard: `ctx.col == 0`. Dispatches to `parse_list_item(text, &mut i, ctx, start, false)` (unordered).
- Added `b'#'` arm to `parse_body_with_ctx` — guard: `ctx.col == 0`. Dispatches to `parse_list_item(text, &mut i, ctx, start, true)` (ordered).
- Added `parse_list_item` helper:
  - Scans a run of identical marker characters (`*+` or `#+`).
  - Depth = marker char count (capped at 255 for u8 safety).
  - Mixed markers (`*#`) stop at the first non-matching char — the `#` becomes literal content.
  - Content = rest of line (up to `\n`), recursively parsed via `parse_body_with_ctx` with `col = depth`.
  - Span covers marker run through end of line (exclusive of `\n`).
  - Returns `AstNode::ListItem { depth, ordered, marker, children, span }`.
- Updated `token_builder.rs` `ListItem` arm:
  - Emits a `ListMarker` token for the marker run (length = `marker.len()`).
  - Recurses into children via `build_semantic_tokens_at_depth` (preserving surrounding depth, per §4.3.4).
- Added 17 Phase 5 tests in `core.rs`:
  - `phase5_unordered_list_depth_1` — `*item` → depth 1, unordered.
  - `phase5_ordered_list_depth_1` — `#item` → depth 1, ordered.
  - `phase5_unordered_list_depth_2` — `**nested` → depth 2.
  - `phase5_ordered_list_depth_3` — `###deep` → depth 3.
  - `phase5_list_with_space_after_marker` — space becomes content.
  - `phase5_list_with_leading_space_not_a_list` — leading space rejects.
  - `phase5_mixed_markers_not_supported` — `*#item` → only `*` matches, `#` is content.
  - `phase5_list_macros_execute_inside` — `<<set>>` inside list item is a real Macro.
  - `phase5_list_with_variable_reference` — `$name` inside list item is interpolated.
  - `phase5_list_with_link` — `[[Forest]]` inside list item is a real Link.
  - `phase5_multiple_list_items` — 3 consecutive list items.
  - `phase5_nested_list_items` — depths 1, 2, 3 on consecutive lines.
  - `phase5_mixed_ul_and_ol` — `*` then `##` (same-depth type switching).
  - `phase5_list_mid_line_asterisk_not_a_list` — mid-line `*` is NOT a list.
  - `phase5_list_emits_listmarker_token_and_content` — token builder emits ListMarker + Prose.
  - `phase5_ordered_list_emits_listmarker_token` — `##` marker token.
  - `phase5_list_with_macro_emits_macro_token` — recursive tokenization of macros inside list items.

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — `b'*'` arm, `b'#'` arm, `parse_list_item` helper, +17 Phase 5 tests.
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — `ListItem` arm with `ListMarker` token + recursive child tokenization.
- `plan.md` — Phase Status updated, this worklog entry.

**Verification**:
- `cargo build --manifest-path crates/formats/Cargo.toml` — succeeds.
- `cargo build --release --manifest-path crates/server/Cargo.toml` — succeeds.
- `cargo test` (full workspace) — **866 tests pass, 0 failures**:
  - core: 77 passed
  - formats: 709 passed (was 692, +17 new Phase 5 tests)
  - server: 80 passed

**Decisions made**:
- **Mixed markers**: The parser scans a run of identical marker characters only. `*#item` produces a depth-1 unordered list item with content `#item` — the `#` becomes literal text. This matches SugarCube's regex `^(?:(\*+)|(#+))` which matches all-`*` or all-`#` but NOT mixed.
- **Depth cap at 255**: SugarCube has no limit, but 256+ marker characters is extremely unlikely. The `u8` type wraps at 255, which still renders as deeply nested lists. This matches the same decision made for `Blockquote::depth` in Phase 4.
- **Token builder recursion**: used `build_semantic_tokens_at_depth` (NOT `build_semantic_tokens`) to preserve the surrounding depth, per §4.3.4. Macros inside list items get the correct `BlockDepthN` modifier.
- **`ListMarker` token length**: `marker.len()` — covers exactly the `*`/`#` run, NOT the item content. Content tokens are emitted by the recursive call. This matches the user's Q2 answer (split spans: marker gets its own token, content gets normal prose/macro tokens).

**Blockers / open issues**:
- None. Phase 5 is complete and ready for Phase 6.

**Next session should**:
- Read this document (especially Section 10 Phase Status and Section 11 Worklog).
- Begin Phase 6 (Tables) per Section 7.3.
- Phase 6 adds the `b'|'` arm at line start (`ctx.col == 0`):
  - Parse TiddlyWiki-style table rows: `|cell|cell|...|[fhck]?$`.
  - Row-type suffix: `h` (header), `f` (footer), `c` (caption), `k` (class).
  - Cell content beginning with `!` → header cell (`<th>`).
  - Cell content `>` only → colspan. `~` only → rowspan.
  - Group consecutive rows into a `Table` node.
  - Cell content recursively parsed (macros execute per §3.9).
  - Token: `Table` on `|` delimiters + row-type suffix, then recurse into cells.
- After Phase 6: run tests, append worklog entry, zip modified files to `/home/z/my-project/download/`.

---

### 2026-06-23 (Phase 6) — Session: Tables (TiddlyWiki syntax)

**Agent**: Super Z (main)
**Phase(s) worked on**: 6 (Tables) — **ALL BLOCK-LEVEL MARKUP PHASES COMPLETE**

**What was done**:
- Added `b'|'` arm to `parse_body_with_ctx` — guard: `ctx.col == 0 && is_table_row_line(&text[i..])`.
- Added 4 helper functions:
  - `is_table_row_line` — checks if a line matches the table row pattern (starts with `|`, ends with `|` or `|` + suffix `[fhck]`).
  - `parse_table_row_suffix` — determines `TableRowType` and closing-`|` position from the line suffix.
  - `parse_table` — scans consecutive table-row lines, groups into a single `AstNode::Table` node. Classifies rows by type:
    - `h` → `header` (first `h` row) + stored in `rows`.
    - `f` → `footer` (first `f` row) + stored in `rows`.
    - `c` → `caption` (cell content extracted as string).
    - `k` → `class` (cell content extracted as string).
    - Body → `rows`.
    - ALL rows (including `h`/`f`) stored in `rows` in document order for token-builder convenience.
  - `parse_table_cells` — splits cell text by `|`, classifies each cell:
    - `!` prefix → header cell (`is_header = true`, `!` stripped from content).
    - `>` only (trimmed) → colspan cell.
    - `~` only (trimmed) → rowspan cell.
    - Recursively parses cell content via `parse_body_with_ctx`.
- Updated `token_builder.rs` `Table` arm:
  - Walks `rows` (all rows in document order).
  - Emits a `Table` token at `row.span.start` (the opening `|`), length 1.
  - Recurses into each cell's `children` via `build_semantic_tokens_at_depth`.
  - Internal `|` delimiters and closing `|` + suffix are NOT tokenized (simplification — they render as plain text between cell content tokens).
- Added 18 Phase 6 tests:
  - Basic body row, multiple rows, header row (`h` suffix + `!` cells), footer row (`f`), caption row (`c`), class row (`k`).
  - Colspan (`>`) and rowspan (`~`) cells.
  - Negative tests: no closing `|`, invalid suffix, leading space.
  - Macros/variables/links inside cells are processed.
  - Table followed by text, empty cells.
  - Token emission (Table token + Prose/Macro content tokens).

**Files modified**:
- `crates/formats/src/sugarcube/parser/core.rs` — `b'|'` arm, `is_table_row_line`, `parse_table_row_suffix`, `parse_table`, `parse_table_cells` helpers, +18 Phase 6 tests.
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — `Table` arm with row walk + cell recursion.
- `plan.md` — Phase Status updated, this worklog entry.

**Verification**:
- `cargo build --release` (server) — succeeds.
- `cargo test` (full workspace) — **884 tests pass, 0 failures**:
  - core: 77 passed
  - formats: 727 passed (was 709, +18 new Phase 6 tests)
  - server: 80 passed

**Decisions made**:
- **Offset convention**: Changed `parse_table` to take `offset` (body offset, 0 for top-level) instead of `span_start` (body-relative position of first `|`). This matches the convention used by all other helpers and simplifies span computation to `offset + text_relative_pos`.
- **Row storage**: ALL rows (body, header, footer) stored in `rows: Vec<TableRow>` in document order. `header`/`footer` are additional cloned references to the first `h`/`f` row for consumer convenience. Caption and class rows are NOT stored in `rows` — their content is extracted into `caption`/`class` strings. This ensures no data loss while providing convenient access patterns.
- **Token emission simplification**: Only the opening `|` of each row gets a `Table` token. Internal `|` delimiters and the closing `|` + suffix are NOT tokenized — they fall in gaps between cell content and render as plain text. This is a pragmatic simplification; a future pass could emit `Table` tokens for every `|` delimiter if finer highlighting is desired.
- **Cells cannot contain `|`**: TiddlyWiki table syntax splits cells on `|` with no escape mechanism. Links with pipe syntax (`[[display|target]]`) do NOT work inside table cells — users must use arrow syntax (`[[display->target]]`). This matches SugarCube's source behavior.

**Blockers / open issues**:
- None. Phase 6 is complete. ALL block-level markup phases (1-6) are done.

**Next session should**:
- Read this document (especially Section 10 Phase Status and Section 11 Worklog).
- Begin Phase 7a (Generalize raw-body mechanism) per Section 6.
- Phase 7a:
  - Add `body_is_raw: bool` field to `MacroDef` in `types.rs`.
  - Set `body_is_raw: true` for `script` only.
  - Remove `<<style>>` and `<<css>>` from the catalog (they don't exist in SugarCube).
  - Remove the hardcoded `script`/`style`/`css` check in `macro_parser.rs:154`.
  - Update `tree_builder.rs` `is_prose_rendering_macro` (remove `style`/`css`).
- After Phase 7a: run tests, append worklog entry, zip modified files to `/home/z/my-project/download/`.

---

### 2026-06-23 (Phase 7a) — Session: Generalize raw-body mechanism

**Agent**: Super Z (main)
**Phase(s) worked on**: 7a (Generalize raw-body mechanism)

**What was done**:
- Added `body_is_raw: bool` field to `MacroDef` struct in `types.rs` (with doc comment explaining the catalog-driven approach).
- Used a Python script (`/home/z/my-project/scripts/add_body_is_raw.py`) to mechanically add `body_is_raw: false` to all 70 catalog entries and `body_is_raw: true` to the `script` entry. The script also removed the `css` catalog entry.
- Removed the `<<css>>` `MacroDef` entry from `catalog.rs` (it doesn't exist in SugarCube per §3.12).
- Updated `macro_parser.rs` — replaced the hardcoded `name.eq_ignore_ascii_case("script") || name.eq_ignore_ascii_case("style") || name.eq_ignore_ascii_case("css")` check with a catalog-driven lookup: `find_macro(&name).map(|def| def.body_is_raw).unwrap_or(false)`.
- Updated `tree_builder.rs` `is_prose_rendering_macro` — removed `style`/`css` from the non-rendering list, added `silent` (forward-looking for Phase 7b). Updated the doc comment.
- Updated `test_body_macros` test in `mod.rs` — changed `assert!(blocks.contains("css"))` to `assert!(!blocks.contains("css"))` and added `assert!(!blocks.contains("style"))`.
- Added 5 Phase 7a tests:
  - `phase7a_script_still_has_raw_body` — verifies `<<script>>` still captures body as raw text (macros inside don't execute).
  - `phase7a_css_not_a_macro` — verifies `<<css>>` is treated as an unknown macro (no raw body).
  - `phase7a_style_not_in_catalog` — verifies `find_macro("style")` and `find_macro("css")` return `None`.
  - `phase7a_script_body_is_raw_in_catalog` — verifies `script` has `body_is_raw: true` and other macros have `false`.
  - `phase7a_script_with_macros_outside_still_works` — verifies macros outside `<<script>>` still execute.

**Files modified**:
- `crates/formats/src/types.rs` — added `body_is_raw: bool` field to `MacroDef`.
- `crates/formats/src/sugarcube/macros/catalog.rs` — removed `css` entry, added `body_is_raw` to all 70 entries (script=true, others=false).
- `crates/formats/src/sugarcube/parser/macro_parser.rs` — replaced hardcoded name check with catalog-driven `body_is_raw` lookup.
- `crates/formats/src/sugarcube/parser/tree_builder.rs` — updated `is_prose_rendering_macro` (removed style/css, added silent).
- `crates/formats/src/sugarcube/macros/mod.rs` — updated `test_body_macros` test.
- `crates/formats/src/sugarcube/parser/core.rs` — +5 Phase 7a tests.
- `plan.md` — Phase Status updated, this worklog entry.

**Verification**:
- `cargo build --release` (server) — succeeds.
- `cargo test` (full workspace) — **889 tests pass, 0 failures**:
  - core: 77 passed
  - formats: 732 passed (was 727, +5 new Phase 7a tests)
  - server: 80 passed

**Decisions made**:
- **Catalog-driven raw-body**: The raw-body check is now `find_macro(&name).map(|def| def.body_is_raw).unwrap_or(false)` instead of hardcoded name comparison. This means any future macro can declare `body_is_raw: true` in the catalog without parser changes.
- **`<<style>>` was never in the catalog**: The parser's hardcoded check handled `style`, but there was never a `MacroDef` entry for it. Now that the check is catalog-driven, `<<style>>` falls through to unknown-macro handling (the parser still parses it, but without raw-body treatment). This is correct — `<<style>>` doesn't exist in SugarCube.
- **`<<css>>` removed from catalog**: It was in the catalog but doesn't exist in SugarCube. Removed it. The parser still parses `<<css>>` as an unknown macro (the tree builder pairs it with `<</css>>` if present), but without raw-body treatment.
- **Added `silent` to `is_prose_rendering_macro`**: Forward-looking for Phase 7b when `<<silent>>` is added. `<<silent>>` discards its body output (like `<<silently>>`), so its content is NOT prose.

**Blockers / open issues**:
- None. Phase 7a is complete and ready for Phase 7b.

**Next session should**:
- Read this document (especially Section 10 Phase Status and Section 11 Worklog).
- Begin Phase 7b (Add missing macros) per Section 6.
- Phase 7b:
  - Add `<<silent>>` (NEW v2.37.0, replacement for `<<silently>>`).
  - Add `<<do>>` / `<<redo>>` (NEW v2.37.0).
  - Add `<<choice>>` (deprecated v2.37.0 but present).
  - Verify and mark deprecated any removed macros still in catalog (`click`, `display`, `forget`, `remember`, `setplaylist`, `stopallaudio`) — per Q8, keep them but mark deprecated.
- After Phase 7b: run tests, append worklog entry, zip modified files to `/home/z/my-project/download/`.

---

### 2026-06-23 (Phase 7b) — Session: Add missing macros

**Agent**: Super Z (main)
**Phase(s) worked on**: 7b (Add missing macros)

**What was done**:
- Discovered `<<silent>>`, `<<do>>`, `<<redo>>` were already in the catalog (likely added by earlier work). Verified their entries are correct (body, kind, deprecated status).
- Added `<<choice>>` to catalog — deprecated v2.37.0, inline, `passageName` (passage ref) + optional `linkText`.
- Added `<<setplaylist>>` to catalog — removed v2.37.0, kept deprecated per Q8 (backward compat), `list_id` arg.
- Added `<<stopallaudio>>` to catalog — removed v2.37.0, kept deprecated per Q8, no args.
- Added snippets for `click`, `choice`, `setplaylist`, `stopallaudio` in `snippets.rs` (needed to pass the `test_phase3_all_catalog_macros_have_snippets_or_forms` test).
- Verified all 9 deprecated/removed macros (`click`, `display`, `forget`, `remember`, `setplaylist`, `stopallaudio`, `silently`, `choice`, `actions`) are marked `deprecated: true` with deprecation messages.
- Added 13 Phase 7b tests:
  - Catalog presence + deprecated status for `silent`, `do`, `redo`, `choice`, `setplaylist`, `stopallaudio`, `silently`.
  - `phase7b_all_removed_macros_are_deprecated` — verifies all 9 deprecated macros.
  - `phase7b_new_macros_have_snippets` — verifies all new macros have snippets.
  - Parser tests: `<<silent>>` parses as block macro (children parsed), `<<do>>` as block, `<<redo>>` as inline (no children), `<<choice>>` as inline.

**Files modified**:
- `crates/formats/src/sugarcube/macros/catalog.rs` — added `choice`, `setplaylist`, `stopallaudio` entries.
- `crates/formats/src/sugarcube/macros/snippets.rs` — added snippets for `click`, `choice`, `setplaylist`, `stopallaudio`.
- `crates/formats/src/sugarcube/parser/core.rs` — +13 Phase 7b tests.
- `plan.md` — Phase Status updated, this worklog entry.

**Verification**:
- `cargo build --release` (server) — succeeds.
- `cargo test` (full workspace) — **902 tests pass, 0 failures**:
  - core: 77 passed
  - formats: 745 passed (was 732, +13 new Phase 7b tests)
  - server: 80 passed

**Decisions made**:
- **`<<silent>>`/`<<do>>`/`<<redo>>` already present**: These were in the catalog from earlier work. No changes needed — just verified correctness.
- **`<<choice>>` arg schema**: `passageName` (passage ref, required) + optional `linkText`. The `linkMarkup`/`imageMarkup` forms aren't modeled as separate arg types yet (deferred to Phase 7c when `MacroArgKind::Link`/`Image` are added).
- **`<<do>>`/`<<redo>>` args**: Both have `args: None` for now. Per §3.12, `<<do [tag tags] [element tag]>>` and `<<redo [tags]>>` have optional keyword args. These will be added in Phase 7d (arg schema fixes) or 7c (new `MacroArgKind::Keyword`).
- **`<<setplaylist>>`/`<<stopallaudio>>` kept deprecated**: Per Q8 (backward compatibility), removed macros are kept in the catalog but marked deprecated with deprecation messages. This allows users with older stories to get warnings rather than errors.

**Blockers / open issues**:
- None. Phase 7b is complete and ready for Phase 7c.

**Next session should**:
- Read this document (especially Section 10 Phase Status and Section 11 Worklog).
- Begin Phase 7c (New `MacroArgKind` variants) per Section 6.
- Phase 7c:
  - Add `MacroArgKind::Keyword` (for `autofocus`, `selected`, `keep`, `container`, `autocheck`, `checked`, `once`, `autoselect`, etc.).
  - Add `MacroArgKind::Link` (for `[[...]]` link markup args).
  - Add `MacroArgKind::Image` (for `[img[...]]` image markup args).
  - Add `MacroArgKind::Number` (for numeric args like `<<numberbox>>` default, `<<audio>>` volume).
  - Update `parse_structured_args` to handle new kinds.
- After Phase 7c: run tests, append worklog entry, zip modified files to `/home/z/my-project/download/`.

---

### 2026-06-23 (Phase 7c/7d/7e) — Session: New MacroArgKind variants + arg schema fixes + snippet updates

**Agent**: Super Z (main)
**Phase(s) worked on**: 7c, 7d, 7e — **ALL PHASES COMPLETE**

**What was done**:

#### Phase 7c — New `MacroArgKind` variants

- Added 4 new variants to `MacroArgKind` enum in `types.rs`: `Keyword`, `Link`, `Image`, `Number`.
- Added 4 corresponding `ParsedArgKind` variants in `ast.rs`: `Keyword`, `LinkMarkup`, `ImageMarkup`, `Number`.
- Added detection helpers to `ArgToken` in `macro_parser.rs`:
  - `is_number_literal()` — checks if a BareName token is a numeric literal (digits + at most one dot).
  - `is_keyword_token()` — checks if a token is a BareName (bareword keyword).
  - `is_link_markup()` / `is_image_markup()` — stubs returning `false` (scanner currently skips bracketed content; link/image markup detection deferred to a future scanner enhancement).
- Updated `parse_structured_args` classifier to handle the new `MacroArgKind` variants:
  - `Keyword` → `ParsedArgKind::Keyword` (if token is a bareword).
  - `Link` → `ParsedArgKind::LinkMarkup` (if token is link markup — currently always falls through to Expression).
  - `Image` → `ParsedArgKind::ImageMarkup` (same caveat).
  - `Number` → `ParsedArgKind::Number` (if token is a numeric literal).
- Updated `emit_structured_arg_tokens` in `token_builder.rs` to emit tokens for the new `ParsedArgKind` variants:
  - `Keyword` → `SemanticTokenType::Keyword`
  - `LinkMarkup` → `SemanticTokenType::Link`
  - `ImageMarkup` → `SemanticTokenType::Link`
  - `Number` → `SemanticTokenType::Number`
- Updated `scan_arg_tokens` to scan numeric literals (digits + decimal point) as BareName tokens.
- Updated `is_bare_passage_name_candidate` to accept tokens starting with a digit (for numeric literals).
- Fixed non-exhaustive matches in server crate (`completion.rs`, `hover.rs`) for the new `MacroArgKind` variants.

#### Phase 7d — Fix arg schemas (batch)

Fixed arg schemas for 8 macros per §3.12:
- **`textbox`**: `receiverName` (Variable) + `defaultValue` (String) + `passage` (String, passage_ref) + `autofocus` (Keyword). Was: 2 args (variable + placeholder).
- **`numberbox`**: Same as textbox but `defaultValue` is `Number` kind. Was: 2 args (variable + expression).
- **`textarea`**: `receiverName` (Variable) + `defaultValue` (String) + `autofocus` (Keyword). NO passage arg (unlike textbox/numberbox). Was: 2 args (variable + placeholder).
- **`option`**: `label` (String) + `value` (String) + `selected` (Keyword). Was: 2 args (display + value), missing `selected`.
- **`include`**: `passageName` (String, passage_ref) + `elementName` (String). Was: 1 arg (passage), missing `elementName`.
- **`widget`**: `widgetName` (String) + `container` (Keyword). Was: 1 arg (name), missing `container`.
- **`script`**: `language` (Keyword). Was: `args: None`, missing `language`.
- **`cacheaudio`**: `trackId` (String) + `sourceList` (String). Was: `args: None`, missing both args.

#### Phase 7e — Update snippets and completion forms

Updated snippets in `snippets.rs` for all modified macros:
- `textbox`: added `passage` and `autofocus` placeholders.
- `numberbox`: added `passage` and `autofocus` placeholders.
- `textarea`: added `autofocus` placeholder (no passage).
- `option`: changed `display` → `label`, added `selected` placeholder.
- `include`: added `element` placeholder.
- `widget`: added `container` placeholder.
- `script`: added `language` placeholder.
- `listbox`/`cycle`: updated `option` call to use `label`/`value`/`selected`.

#### Tests

Added 15 Phase 7c/7d tests:
- Catalog schema verification for all 8 fixed macros.
- `MacroArgKind` and `ParsedArgKind` variant existence checks.
- Parser classification tests: `numberbox` number arg → `Number`, `textarea` autofocus → `Keyword`, `widget` container → `Keyword`, `option` selected → `Keyword`, `include` elementName → `String`.

**Files modified**:
- `crates/formats/src/types.rs` — added 4 `MacroArgKind` variants.
- `crates/formats/src/sugarcube/ast.rs` — added 4 `ParsedArgKind` variants.
- `crates/formats/src/sugarcube/parser/macro_parser.rs` — `ArgToken` helpers, classifier updates, numeric literal scanning, `is_bare_passage_name_candidate` fix.
- `crates/formats/src/sugarcube/lsp/token_builder.rs` — `emit_structured_arg_tokens` new variant handling.
- `crates/formats/src/sugarcube/macros/catalog.rs` — 8 macro arg schema fixes.
- `crates/formats/src/sugarcube/macros/snippets.rs` — snippet updates for all modified macros.
- `crates/server/src/handlers/completion.rs` — non-exhaustive match fix for `MacroArgKind`.
- `crates/server/src/handlers/hover.rs` — non-exhaustive match fix for `MacroArgKind`.
- `crates/formats/src/sugarcube/parser/core.rs` — +15 Phase 7c/7d tests.
- `plan.md` — Phase Status updated (ALL COMPLETE), this worklog entry.

**Verification**:
- `cargo build --release` (server) — succeeds.
- `cargo test` (full workspace) — **917 tests pass, 0 failures**:
  - core: 77 passed
  - formats: 760 passed (was 745, +15 new Phase 7c/7d tests)
  - server: 80 passed

**Decisions made**:
- **Link/Image markup detection deferred**: The `scan_arg_tokens` scanner currently skips bracketed content (`[...]`), which means `[[...]]` link markup and `[img[...]]` image markup are NOT tokenized. They fall through to the Expression path. Teaching the scanner to recognize `[[` as a link-start (rather than a bracket-skip) is a non-trivial enhancement deferred to future work. The `MacroArgKind::Link`/`Image` variants and `ParsedArgKind::LinkMarkup`/`ImageMarkup` variants exist in the type system but are not yet produced by the classifier.
- **Numeric literal scanning**: Added a dedicated numeric-literal scanning branch in `scan_arg_tokens` that handles integers (`100`) and floats (`0.5`) with at most one decimal point. This enables `MacroArgKind::Number` classification for `<<numberbox>>`'s `defaultValue`.
- **`is_bare_passage_name_candidate` fix**: Previously rejected tokens starting with a digit. Now accepts them so numeric literals pass through the filter and reach the classifier.
- **Remaining macros**: `type`, `for`, `case`, `audio`/`masteraudio`/`playlist`/`track`, `link`/`button`, `goto`/`back`/`return`, `actions`, `do`/`redo` still have incomplete arg schemas (mostly `args: None` or simplified schemas). These can be addressed in future work — the most impactful fixes (form macros, `include`, `widget`, `script`, `cacheaudio`, `option`) are done.

**Blockers / open issues**:
- None. ALL phases (1-7) are complete.

**ALL PHASES COMPLETE.** The block-level markup overhaul and macro catalog fix are done.

---

### 2026-06-23 (Phase 7d completion) — Session: Complete remaining macro schemas

**Agent**: Super Z (main)
**Phase(s) worked on**: 7d completion (remaining macro arg schemas)

**What was done**:
Fixed arg schemas for ALL remaining macros that had incomplete or missing schemas:
- **`case`**: Added `valueList` (Expression, variadic). Was `args: None`.
- **`do`**: Added `tag` (Keyword) + `element` (Keyword). Was `args: None`.
- **`redo`**: Added `tags` (Keyword). Was `args: None`.
- **`masteraudio`**: Added `actionList` (Keyword). Was `args: None`.
- **`playlist`**: Added `listId` (String) + `actionList` (Keyword). Was `args: None`.
- **`type`**: Added full 12-arg signature: `speed` (String, required) + `start`/`delay`/`class`/`classes`/`element`/`tag`/`id`/`ID`/`keep|none`/`skipkey`/`key`. Was only 1 arg (speed).
- **`audio`**: Added `trackIdList` (String) + `actionList` (Keyword). Was only 1 arg.
- **`track`**: Added optional `actionList` (Keyword) at position 1. Was only 1 arg.
- **`cycle`**: Added `once` (Keyword) + `autoselect` (Keyword). Was only 1 arg (variable).
- **`listbox`**: Added `autoselect` (Keyword). Was only 1 arg (variable).
- **`link`/`button`**: Renamed arg 0 label from `label` to `linkText` (per SugarCube docs).
- **`back`/`return`**: Renamed arg 0 label from `display_text` to `linkText`.

Macros that correctly remain `args: None` (JS-expression macros whose args go to oxc):
`if`, `elseif`, `else`, `for`, `break`, `continue`, `switch`, `set`, `run`, `print`, `=`, `-`, `silent`, `silently`, `next`, `createaudiogroup`, `createplaylist`, `stop`, `default`, `unset`, `capture`, `waitforaudio`, `nobr`, `done`.

Used a Python script (`/home/z/my-project/scripts/fix_remaining_schemas.py`) for the batch catalog edits.

Added 7 verification tests:
- `phase7d_all_non_expression_macros_have_args` — comprehensive check that ALL non-JS-expression macros have `args: Some`.
- `phase7d_case_has_variadic_expression` — verifies `case` has `valueList` (Expression).
- `phase7d_type_has_full_signature` — verifies `type` has ≥7 args with `speed` required.
- `phase7d_cycle_has_once_and_autoselect` — verifies `cycle` has `once`/`autoselect` keywords.
- `phase7d_listbox_has_autoselect` — verifies `listbox` has `autoselect` keyword.
- `phase7d_link_button_use_linktext_label` — verifies `link`/`button` use `linkText` label.
- `phase7d_audio_has_trackidlist_and_actionlist` — verifies `audio` has both args.

**Files modified**:
- `crates/formats/src/sugarcube/macros/catalog.rs` — 10 macro schema fixes + 4 label renames.
- `crates/formats/src/sugarcube/parser/core.rs` — +7 completion tests.

**Verification**:
- `cargo build --release` (server) — succeeds.
- `cargo test` (full workspace) — **924 tests pass, 0 failures**:
  - core: 77 passed
  - formats: 767 passed (was 760, +7 new completion tests)
  - server: 80 passed

**ALL MACRO SCHEMAS COMPLETE.** Every non-JS-expression macro in the catalog now has a declared arg schema matching SugarCube v2.37.3 documentation. The only macros with `args: None` are those whose args are raw JS expressions handled by oxc (if, set, for, switch, etc.) or macros that genuinely take no args (silent, nobr, done, etc.).
