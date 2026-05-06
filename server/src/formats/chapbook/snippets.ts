/**
 * Knot v2 — Chapbook 2 Snippet Templates
 *
 * VS Code snippet definitions for Chapbook inserts and common patterns.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { SnippetDef } from '../_types';

export const SNIPPET_TEMPLATES: readonly SnippetDef[] = [
  // ── Control Flow ──────────────────────────────────────────────
  {
    key: 'if',
    prefix: 'if',
    description: '{if} conditional block',
    body: ['{if ${1:condition}}', '\t$0', '{endif}'],
    category: 'control',
  },
  {
    key: 'unless',
    prefix: 'unless',
    description: '{unless} inverse conditional block',
    body: ['{unless ${1:condition}}', '\t$0', '{endunless}'],
    category: 'control',
  },

  // ── Embedding ─────────────────────────────────────────────────
  {
    key: 'embed-passage',
    prefix: 'embed passage',
    description: '{embed passage:} transclude passage content',
    body: ['{embed passage: \'${1:PassageName}\'}'],
    category: 'output',
  },

  // ── Revealing ─────────────────────────────────────────────────
  {
    key: 'reveal-link',
    prefix: 'reveal link',
    description: '{reveal link:} reveal hidden content on click',
    body: ['{reveal link: \'${1:Click me}\', passage: \'${2:PassageName}\'}'],
    category: 'interactive',
  },

  // ── Navigation ────────────────────────────────────────────────
  {
    key: 'redirect-to',
    prefix: 'redirect to',
    description: '{redirect to:} navigate to passage immediately',
    body: ['{redirect to: \'${1:PassageName}\'}'],
    category: 'navigation',
  },

  // ── Section ───────────────────────────────────────────────────
  {
    key: 'section',
    prefix: 'section',
    description: '{section} content section with scoped variables',
    body: ['{section}', '\t$0', '{section-end}'],
    category: 'control',
  },

  // ── Cycling ───────────────────────────────────────────────────
  {
    key: 'cycling-link',
    prefix: 'cycling link',
    description: '{cycling link:} cycle through values on click',
    body: ['{cycling link: \'${1:First}\', \'${2:Second}\', \'${3:Third}\'}'],
    category: 'interactive',
  },
  {
    key: 'cycle',
    prefix: 'cycle',
    description: '{cycle} cycle through values for a variable',
    body: ['{cycle ${1:var.choice}, \'${2:First}\', \'${3:Second}\'}'],
    category: 'interactive',
  },
];
