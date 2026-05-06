/**
 * Knot v2 — SugarCube 2 Snippet Templates
 *
 * VS Code snippet definitions for SugarCube macros and common patterns.
 */

import type { SnippetDef } from '../_types';

export const SNIPPET_TEMPLATES: SnippetDef[] = [
  // ── Control Flow ──────────────────────────────────────────────
  {
    key: 'if',
    prefix: 'if',
    description: '<<if>> conditional block',
    body: ['<<if ${1:condition}>>', '\t$0', '<</if>>'],
    category: 'control',
  },
  {
    key: 'if-else',
    prefix: 'if-else',
    description: '<<if>> / <<else>> conditional block',
    body: ['<<if ${1:condition}>>', '\t$2', '<<else>>', '\t$0', '<</if>>'],
    category: 'control',
  },
  {
    key: 'for',
    prefix: 'for',
    description: '<<for>> loop with range',
    body: ['<<for ${1:$var} to ${2:range}>>', '\t$0', '<</for>>'],
    category: 'control',
  },
  {
    key: 'switch',
    prefix: 'switch',
    description: '<<switch>> / <<case>> block',
    body: ['<<switch ${1:expression}>>', '<<case ${2:value}>>', '\t$0', '<</switch>>'],
    category: 'control',
  },
  {
    key: 'while',
    prefix: 'while',
    description: '<<while>> loop',
    body: ['<<while ${1:condition}>>', '\t$0', '<</while>>'],
    category: 'control',
  },

  // ── Variable Assignment ───────────────────────────────────────
  {
    key: 'set',
    prefix: 'set',
    description: '<<set>> variable assignment',
    body: ['<<set ${1:$var} to ${2:value}>>'],
    category: 'variable',
  },
  {
    key: 'print',
    prefix: 'print',
    description: '<<print>> expression output',
    body: ['<<print ${1:expression}>>'],
    category: 'output',
  },

  // ── Navigation ────────────────────────────────────────────────
  {
    key: 'link',
    prefix: 'link',
    description: '<<link>> interactive link with body',
    body: ['<<link "${1:text}">', '\t$0', '<</link>>'],
    category: 'navigation',
  },
  {
    key: 'goto',
    prefix: 'goto',
    description: '<<goto>> navigate to passage',
    body: ['<<goto "${1:PassageName}">>'],
    category: 'navigation',
  },
  {
    key: 'include',
    prefix: 'include',
    description: '<<include>> transclude passage content',
    body: ['<<include "${1:PassageName}">>'],
    category: 'output',
  },

  // ── Widget ────────────────────────────────────────────────────
  {
    key: 'widget',
    prefix: 'widget',
    description: '<<widget>> define a custom widget',
    body: ['<<widget "${1:name}">', '\t$0', '<</widget>>'],
    category: 'widget',
  },

  // ── Output Control ────────────────────────────────────────────
  {
    key: 'nobr',
    prefix: 'nobr',
    description: '<<nobr>> suppress line breaks',
    body: ['<<nobr>>', '\t$0', '<</nobr>>'],
    category: 'output',
  },
  {
    key: 'silently',
    prefix: 'silently',
    description: '<<silently>> suppress all output',
    body: ['<<silently>>', '\t$0', '<</silently>>'],
    category: 'output',
  },
  {
    key: 'capture',
    prefix: 'capture',
    description: '<<capture>> capture output into variable',
    body: ['<<capture ${1:$var}>>', '\t$0', '<</capture>>'],
    category: 'variable',
  },
  {
    key: 'script',
    prefix: 'script',
    description: '<<script>> inline JavaScript',
    body: ['<<script>>', '\t$0', '<</script>>'],
    category: 'system',
  },

  // ── Interactive ───────────────────────────────────────────────
  {
    key: 'button',
    prefix: 'button',
    description: '<<button>> interactive button',
    body: ['<<button "${1:text}">', '\t$0', '<</button>>'],
    category: 'interactive',
  },

  // ── Timed / Live ──────────────────────────────────────────────
  {
    key: 'timed',
    prefix: 'timed',
    description: '<<timed>> delayed content',
    body: ['<<timed ${1:delay}ms>>', '\t$0', '<</timed>>'],
    category: 'live',
  },
  {
    key: 'repeat',
    prefix: 'repeat',
    description: '<<repeat>> repeating content',
    body: ['<<repeat ${1:delay}ms>>', '\t$0', '<</repeat>>'],
    category: 'live',
  },
  {
    key: 'type',
    prefix: 'type',
    description: '<<type>> type out text',
    body: ['<<type ${1:text} ${2:speed}ms>>'],
    category: 'output',
  },

  // ── Audio ─────────────────────────────────────────────────────
  {
    key: 'audio',
    prefix: 'audio',
    description: '<<audio>> control audio playback',
    body: ['<<audio "${1:id}" ${2:action}>>'],
    category: 'audio',
  },

  // ── Form Inputs ───────────────────────────────────────────────
  {
    key: 'checkbox',
    prefix: 'checkbox',
    description: '<<checkbox>> form input',
    body: ['<<checkbox "${1:$var}" "${2:value}" ${3:checked}>>'],
    category: 'form',
  },
  {
    key: 'radiobutton',
    prefix: 'radiobutton',
    description: '<<radiobutton>> form input',
    body: ['<<radiobutton "${1:$var}" "${2:value}">>'],
    category: 'form',
  },
  {
    key: 'textbox',
    prefix: 'textbox',
    description: '<<textbox>> form input',
    body: ['<<textbox "${1:$var}" "${2:default}">>'],
    category: 'form',
  },
  {
    key: 'textarea',
    prefix: 'textarea',
    description: '<<textarea>> form input',
    body: ['<<textarea "${1:$var}" "${2:default}">>'],
    category: 'form',
  },
  {
    key: 'numberbox',
    prefix: 'numberbox',
    description: '<<numberbox>> form input',
    body: ['<<numberbox "${1:$var}" "${2:default}">>'],
    category: 'form',
  },
  {
    key: 'listbox',
    prefix: 'listbox',
    description: '<<listbox>> form input',
    body: ['<<listbox "${1:$var}">', '\t<<option "${2:value}">', '<</listbox>>'],
    category: 'form',
  },
  {
    key: 'dropdown',
    prefix: 'dropdown',
    description: '<<dropdown>> form input',
    body: ['<<dropdown "${1:$var}" "${2:options}">>'],
    category: 'form',
  },
  {
    key: 'input',
    prefix: 'input',
    description: '<<input>> HTML form input',
    body: ['<<input "${1:type}" "${2:name}" ${3:value}>>'],
    category: 'form',
  },
];
