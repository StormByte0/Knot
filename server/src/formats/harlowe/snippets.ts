/**
 * Knot v2 — Harlowe 3 Snippet Templates
 *
 * VS Code snippet templates for Harlowe 3 macros and constructs.
 * Uses VS Code snippet syntax for tab stops and placeholders.
 */

import type { SnippetDef } from '../_types';

export const SNIPPET_TEMPLATES: readonly SnippetDef[] = [
  // ── Control Flow ─────────────────────────────────────────────
  {
    key: 'if',
    prefix: 'if',
    description: 'Harlowe (if:) conditional with hook',
    body: ['(if: ${1:condition})[', '\t$0', ']'],
    category: 'control',
  },
  {
    key: 'if-else',
    prefix: 'ifelse',
    description: 'Harlowe (if:)/(else:) conditional',
    body: ['(if: ${1:condition})[', '\t${2:then}', '](else:)[', '\t${3:else}', ']'],
    category: 'control',
  },
  {
    key: 'if-elseif-else',
    prefix: 'ifelseif',
    description: 'Harlowe (if:)/(else-if:)/(else:) conditional',
    body: ['(if: ${1:condition})[', '\t${2:then}', '](else-if: ${3:otherCondition})[', '\t${4:or}', '](else:)[', '\t${5:else}', ']'],
    category: 'control',
  },
  {
    key: 'unless',
    prefix: 'unless',
    description: 'Harlowe (unless:) inverse conditional with hook',
    body: ['(unless: ${1:condition})[', '\t$0', ']'],
    category: 'control',
  },
  {
    key: 'for-loop',
    prefix: 'for',
    description: 'Harlowe (for:) loop',
    body: ['(for: ${1:_variable} => ...${2:range})[', '\t$0', ']'],
    category: 'control',
  },

  // ── Variable Assignment ──────────────────────────────────────
  {
    key: 'set',
    prefix: 'set',
    description: 'Harlowe (set:) variable assignment',
    body: ['(set: $${1:variable} to ${0:value})'],
    category: 'variable',
  },
  {
    key: 'put',
    prefix: 'put',
    description: 'Harlowe (put:) variable assignment',
    body: ['(put: ${0:value} into $${1:variable})'],
    category: 'variable',
  },
  {
    key: 'move',
    prefix: 'move',
    description: 'Harlowe (move:) variable move',
    body: ['(move: $${1:source} into $${0:destination})'],
    category: 'variable',
  },

  // ── Output ───────────────────────────────────────────────────
  {
    key: 'print',
    prefix: 'print',
    description: 'Harlowe (print:) expression output',
    body: ['(print: ${0:expression})'],
    category: 'output',
  },
  {
    key: 'display',
    prefix: 'display',
    description: 'Harlowe (display:) passage inclusion',
    body: ['(display: "${0:PassageName}")'],
    category: 'output',
  },

  // ── Navigation ───────────────────────────────────────────────
  {
    key: 'goto',
    prefix: 'goto',
    description: 'Harlowe (go-to:) navigation',
    body: ['(go-to: "${0:PassageName}")'],
    category: 'navigation',
  },
  {
    key: 'link-goto',
    prefix: 'linkgoto',
    description: 'Harlowe (link-goto:) passage link',
    body: ['(link-goto: "${1:DisplayText}", "${0:PassageName}")'],
    category: 'navigation',
  },
  {
    key: 'link-reveal',
    prefix: 'linkreveal',
    description: 'Harlowe (link-reveal:) revealing link',
    body: ['(link-reveal: "${1:Click me}")[', '\t$0', ']'],
    category: 'navigation',
  },
  {
    key: 'link-reveal-goto',
    prefix: 'linkrevealgoto',
    description: 'Harlowe (link-reveal-goto:) revealing navigation link',
    body: ['(link-reveal-goto: "${1:DisplayText}", "${0:PassageName}")[', '\t${2:Revealed content}', ']'],
    category: 'navigation',
  },
  {
    key: 'undo',
    prefix: 'undo',
    description: 'Harlowe (undo:) navigation undo',
    body: ['(undo:)'],
    category: 'navigation',
  },

  // ── Revision ─────────────────────────────────────────────────
  {
    key: 'replace',
    prefix: 'replace',
    description: 'Harlowe (replace:) hook content replacement',
    body: ['(replace: ?${1:hookName})[', '\t$0', ']'],
    category: 'revision',
  },
  {
    key: 'append',
    prefix: 'append',
    description: 'Harlowe (append:) hook content append',
    body: ['(append: ?${1:hookName})[', '\t$0', ']'],
    category: 'revision',
  },
  {
    key: 'prepend',
    prefix: 'prepend',
    description: 'Harlowe (prepend:) hook content prepend',
    body: ['(prepend: ?${1:hookName})[', '\t$0', ']'],
    category: 'revision',
  },

  // ── Live / Timed ─────────────────────────────────────────────
  {
    key: 'live',
    prefix: 'live',
    description: 'Harlowe (live:) live updating',
    body: ['(live: ${1:1s})[', '\t$0', ']'],
    category: 'timed',
  },
  {
    key: 'after',
    prefix: 'after',
    description: 'Harlowe (after:) delayed execution',
    body: ['(after: ${1:1s})[', '\t$0', ']'],
    category: 'timed',
  },

  // ── Styling ──────────────────────────────────────────────────
  {
    key: 'align',
    prefix: 'align',
    description: 'Harlowe (align:) text alignment',
    body: ['(align: "${1:==>}")[', '\t$0', ']'],
    category: 'styling',
  },
  {
    key: 'text-style',
    prefix: 'textstyle',
    description: 'Harlowe (text-style:) text styling',
    body: ['(text-style: "${0:bold}")'],
    category: 'styling',
  },
  {
    key: 'text-colour',
    prefix: 'textcolour',
    description: 'Harlowe (text-colour:) text color',
    body: ['(text-colour: ${0:red})'],
    category: 'styling',
  },

  // ── Data Structures ──────────────────────────────────────────
  {
    key: 'datamap',
    prefix: 'dm',
    description: 'Harlowe (dm:) datamap literal',
    body: ['(dm: "${1:key}", ${0:value})'],
    category: 'data',
  },
  {
    key: 'array',
    prefix: 'a',
    description: 'Harlowe (a:) array literal',
    body: ['(a: ${0:values})'],
    category: 'data',
  },
  {
    key: 'dataset',
    prefix: 'ds',
    description: 'Harlowe (ds:) dataset literal',
    body: ['(ds: ${0:values})'],
    category: 'data',
  },

  // ── Advanced ─────────────────────────────────────────────────
  {
    key: 'macro-def',
    prefix: 'macro',
    description: 'Harlowe (macro:) custom macro definition',
    body: ['(macro: ${1:args}, [', '\t$0', '])'],
    category: 'advanced',
  },
];
