import type { CompletionItem, Diagnostic } from 'vscode-languageserver/node';
import { CompletionItemKind, InsertTextFormat } from 'vscode-languageserver/node';
import type {
  StoryFormatAdapter,
  FormatContext,
  AdapterCompletionRequest,
  AdapterHoverRequest,
  AdapterDiagnosticRequest,
} from '../types';
import { BUILTINS, BUILTIN_MAP, BLOCK_MACRO_NAMES } from './macros';

// ---------------------------------------------------------------------------
// Per-macro snippet overrides
//
// Keys are macro names. Values are the snippet body inserted AFTER the opening
// << and BEFORE the auto-closed >>. The completion handler wraps everything so:
//   - The user has already typed <<
//   - >> is auto-inserted by the editor
//   - We insert: <name> <args_snippet>>\n<body>\n<</name>  (for block macros)
//
// Tabstop conventions:
//   $1, $2 … — positional tab stops
//   ${1:placeholder} — tab stop with placeholder text
//   $0 — final cursor position
// ---------------------------------------------------------------------------

const MACRO_SNIPPETS: Record<string, string> = {
  // ── Variables ─────────────────────────────────────────────────────────────
  'set':    'set ${1:\\$var} to ${2:value}',
  'unset':  'unset ${1:\\$var}',
  'run':    'run ${1:expression}',
  'capture': 'capture ${1:\\$var}>>\n$2\n<</capture',

  // ── Output ────────────────────────────────────────────────────────────────
  'print':    'print ${1:expression}',
  '=':        '= ${1:expression}',
  '-':        '- ${1:expression}',
  'type':     'type ${1:speed}>>\n$2\n<</type',
  'nobr':     'nobr>>\n$2\n<</nobr',
  'silently': 'silently>>\n$2\n<</silently',

  // ── Control flow ──────────────────────────────────────────────────────────
  'if':      'if ${1:condition}>>\n$2\n<</if',
  'elseif':  'elseif ${1:condition}',
  'for':     'for ${1:_i}, ${2:\\$array}>>\n$3\n<</for',
  'switch':  'switch ${1:\\$var}>>\n<<case ${2:value}>>\n$3\n<</switch',

  // ── Links / interaction ───────────────────────────────────────────────────
  'link':        'link "${1:label}" "${2:passage}">>\n$3\n<</link',
  'button':      'button "${1:label}" "${2:passage}">>\n$3\n<</button',
  'linkappend':  'linkappend "${1:label}">>\n$2\n<</linkappend',
  'linkprepend': 'linkprepend "${1:label}">>\n$2\n<</linkprepend',
  'linkreplace': 'linkreplace "${1:label}">>\n$2\n<</linkreplace',

  // ── Navigation ────────────────────────────────────────────────────────────
  'goto':    'goto "${1:passage}"',
  'include': 'include "${1:passage}"',
  'back':    'back',
  'return':  'return',

  // ── DOM ───────────────────────────────────────────────────────────────────
  'append':      'append "${1:#selector}">>\n$2\n<</append',
  'prepend':     'prepend "${1:#selector}">>\n$2\n<</prepend',
  'replace':     'replace "${1:#selector}">>\n$2\n<</replace',
  'remove':      'remove "${1:#selector}"',
  'addclass':    'addclass "${1:#selector}" "${2:class}"',
  'removeclass': 'removeclass "${1:#selector}" "${2:class}"',
  'toggleclass': 'toggleclass "${1:#selector}" "${2:class}"',

  // ── Widgets / scripting ───────────────────────────────────────────────────
  'widget': 'widget "${1:name}">>\n$2\n<</widget',
  'script': 'script>>\n$1\n<</script',
  'done':   'done>>\n$1\n<</done',

  // ── Timing ────────────────────────────────────────────────────────────────
  'timed':  'timed ${1:2s}>>\n$2\n<</timed',
  'repeat': 'repeat ${1:2s}>>\n$2\n<</repeat',

  // ── Forms ─────────────────────────────────────────────────────────────────
  'checkbox':    'checkbox "${1:label}" ${2:\\$var} "${3:checked}" "${4:unchecked}"',
  'radiobutton': 'radiobutton "${1:label}" ${2:\\$var} "${3:value}"',
  'textbox':     'textbox ${1:\\$var} "${2:placeholder}"',
  'textarea':    'textarea ${1:\\$var} "${2:placeholder}"',
  'numberbox':   'numberbox ${1:\\$var} ${2:0}',
};

export class SugarCubeAdapter implements StoryFormatAdapter {
  readonly id          = 'sugarcube-2';
  readonly displayName = 'SugarCube 2';

  // ── Completion ─────────────────────────────────────────────────────────────

  provideFormatCompletions(req: AdapterCompletionRequest, _ctx: FormatContext): CompletionItem[] {
    const { text, offset } = req;

    // Close-tag context: <</ ...
    const closeCtx = this.extractMacroCloseContext(text, offset);
    if (closeCtx !== null) {
      return this.buildCloseTagCompletions(text, offset, closeCtx);
    }

    // Only provide builtins when inside a macro-open context (<<)
    if (this.extractMacroOpenContext(text, offset) === null) {
      return [];
    }

    const items: CompletionItem[] = [];
    for (const m of BUILTINS) {
      items.push(this.buildBuiltinItem(m.name, m.description, m.hasBody));
    }
    return items;
  }

  /**
   * Returns the snippet body (without the leading <<).
   * Uses per-macro overrides from MACRO_SNIPPETS first, then falls back to
   * the generic block/inline pattern.
   */
  buildMacroSnippet(name: string, hasBody: boolean): string | null {
    // Per-macro override takes priority
    const custom = MACRO_SNIPPETS[name];
    if (custom !== undefined) return custom;

    // Generic fallback
    const isBlock = hasBody || BLOCK_MACRO_NAMES.has(name);
    if (isBlock) {
      return `${name} $1>>\n$2\n<</${name}`;
    }
    return `${name} $1`;
  }

  getBlockMacroNames(): ReadonlySet<string> {
    return BLOCK_MACRO_NAMES;
  }

  // ── Hover ──────────────────────────────────────────────────────────────────

  provideBuiltinHover(req: AdapterHoverRequest, _ctx: FormatContext): string | null {
    const { tokenType, rawName } = req;
    if (tokenType === 'macro') {
      const m = BUILTIN_MAP.get(rawName);
      if (m) return `**Macro** \`<<${m.name}>>\`\n\n${m.description}`;
    }
    if (tokenType === 'variable' || tokenType === 'function') {
      return this.hoverForGlobal(rawName);
    }
    return null;
  }

  describeVariableSigil(sigil: string): string | null {
    if (sigil === '$') return 'SugarCube story variable — persists across passages';
    if (sigil === '_') return 'SugarCube temporary variable — scoped to the current passage';
    return null;
  }

  // ── Diagnostics ────────────────────────────────────────────────────────────

  provideDiagnostics(_req: AdapterDiagnosticRequest, _ctx: FormatContext): Diagnostic[] {
    return [];
  }

  // ── Virtual runtime prelude ────────────────────────────────────────────────

  getVirtualRuntimePrelude(): string {
    return `
declare const State: {
  variables:  Record<string, unknown>;
  temporary:  Record<string, unknown>;
  turns:      number;
  passage:    string;
  active:     { title: string; tags: string[] };
  top:        { title: string; tags: string[] };
  history:    { title: string; tags: string[] }[];
  peek(offset?: number): { title: string; tags: string[] };
  has(passageTitle: string): boolean;
  hasTag(tag: string): boolean;
  index:      number;
  size:       number;
};
declare const Engine: {
  play(passageTitle: string, noHistory?: boolean): void;
  forward(): void;
  backward(): void;
  goto(passageTitle: string): void;
  isIdle():    boolean;
  isPlaying(): boolean;
};
declare const Story: {
  title: string;
  has(passageTitle: string): boolean;
  get(passageTitle: string): { title: string; tags: string[]; text: string };
  filter(predicate: (p: { title: string; tags: string[] }) => boolean): { title: string; tags: string[] }[];
};
declare const SugarCube: { version: { title: string; major: number; minor: number; patch: number } };
declare const setup:    Record<string, unknown>;
declare const passage:  string;
declare const tags:     string[];
declare const visited:  (...passages: string[]) => number;
declare const visitedTags: (...tags: string[]) => number;
declare const turns:    number;
declare const time:     number;
declare const $args:    unknown[];
`;
  }

  // ── Private ────────────────────────────────────────────────────────────────

  private extractMacroCloseContext(text: string, offset: number): string | null {
    const m = text.slice(0, offset).match(/<{2}\/([\w-]*)$/);
    return m ? m[1]! : null;
  }

  private extractMacroOpenContext(text: string, offset: number): string | null {
    const before = text.slice(0, offset);
    const m = before.match(/<<([A-Za-z_=\-][\w-]*)?\s*$/);
    if (!m) return null;
    if (!before.endsWith('<<') && !/<<[\w-]*$/.test(before)) return null;
    return m[1] ?? '';
  }

  private buildBuiltinItem(name: string, detail: string, hasBody: boolean): CompletionItem {
    const snippet = this.buildMacroSnippet(name, hasBody)!;
    return {
      label:            `<<${name}>>`,
      filterText:       name,
      insertText:       snippet,
      insertTextFormat: InsertTextFormat.Snippet,
      kind:             CompletionItemKind.Function,
      detail,
      sortText:         `1_${name}`,
    };
  }

  private buildCloseTagCompletions(text: string, offset: number, partial: string): CompletionItem[] {
    const before = text.slice(0, offset);
    type Ev = { pos: number; kind: 'open' | 'close'; name: string };
    const events: Ev[] = [];

    const openRe  = /<<([A-Za-z_][A-Za-z0-9_-]*)(?:\s[^>]*)?>>/g;
    const closeRe = /<<\/([A-Za-z_][A-Za-z0-9_-]*)>>/g;
    let m: RegExpExecArray | null;

    openRe.lastIndex = 0;
    while ((m = openRe.exec(before)) !== null) {
      if (BLOCK_MACRO_NAMES.has(m[1]!)) events.push({ pos: m.index, kind: 'open', name: m[1]! });
    }
    closeRe.lastIndex = 0;
    while ((m = closeRe.exec(before)) !== null) {
      events.push({ pos: m.index, kind: 'close', name: m[1]! });
    }

    events.sort((a, b) => a.pos - b.pos);
    const openStack: string[] = [];
    for (const ev of events) {
      if (ev.kind === 'open') {
        openStack.push(ev.name);
      } else {
        for (let i = openStack.length - 1; i >= 0; i--) {
          if (openStack[i] === ev.name) { openStack.splice(i, 1); break; }
        }
      }
    }

    const seen = new Set<string>();
    const items: CompletionItem[] = [];

    for (let i = openStack.length - 1; i >= 0; i--) {
      const name = openStack[i]!;
      if (seen.has(name) || (partial && !name.startsWith(partial))) continue;
      seen.add(name);
      items.push({
        label:      `</${name}>>`,
        filterText: name,
        insertText: name,
        insertTextFormat: InsertTextFormat.PlainText,
        kind:       CompletionItemKind.Function,
        detail:     `Close <<${name}>>`,
        sortText:   `0_${String(openStack.length - i).padStart(4, '0')}_${name}`,
      });
    }

    if (items.length === 0) {
      for (const name of BLOCK_MACRO_NAMES) {
        if (partial && !name.startsWith(partial)) continue;
        items.push({
          label:      `</${name}>>`,
          filterText: name,
          insertText: name,
          insertTextFormat: InsertTextFormat.PlainText,
          kind:       CompletionItemKind.Function,
          detail:     `Close <<${name}>>`,
          sortText:   `1_${name}`,
        });
      }
    }

    return items;
  }

  private hoverForGlobal(name: string): string | null {
    const globals: Record<string, string> = {
      State:     '**SugarCube** `State` — the story history and variable store.',
      Engine:    '**SugarCube** `Engine` — controls passage navigation.',
      Story:     '**SugarCube** `Story` — passage access and metadata.',
      SugarCube: '**SugarCube** version metadata object.',
      setup:     '**SugarCube** `setup` — author-defined initialisation object.',
      passage:   '**SugarCube** `passage` — title of the current passage.',
      tags:      '**SugarCube** `tags` — tag array of the current passage.',
      visited:   '**SugarCube** `visited(...passages)` — times any listed passage was visited.',
      turns:     '**SugarCube** `turns` — number of turns elapsed.',
      time:      '**SugarCube** `time` — milliseconds since last `<<timed>>` or `<<repeat>>`.',
      $args:     '**SugarCube** `$args` — arguments passed to the current `<<widget>>`.',
    };
    return globals[name] ?? null;
  }
}