/**
 * Knot v2 — Snowman 2 Snippet Templates
 *
 * VS Code snippet definitions for Snowman template blocks
 * and common patterns.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { SnippetDef } from '../_types';

export const SNIPPET_TEMPLATES: readonly SnippetDef[] = [
  // ── Template Blocks ────────────────────────────────────────────
  {
    key: 'template-block',
    prefix: '<%',
    description: '<% %> JavaScript execution block',
    body: ['<% ${1:// code} %>'],
    category: 'template',
  },
  {
    key: 'template-expression',
    prefix: '<%=',
    description: '<%= %> JavaScript expression output block',
    body: ['<%= ${1:expression} %>'],
    category: 'template',
  },
  {
    key: 'template-if',
    prefix: 'if',
    description: '<% if %> conditional block',
    body: ['<% if (${1:condition}) { %>', '\t$0', '<% } %>'],
    category: 'control',
  },
  {
    key: 'template-if-else',
    prefix: 'if-else',
    description: '<% if/else %> conditional block',
    body: ['<% if (${1:condition}) { %>', '\t$2', '<% } else { %>', '\t$0', '<% } %>'],
    category: 'control',
  },
  {
    key: 'template-for',
    prefix: 'for',
    description: '<% for %> loop block',
    body: ['<% for (${1:var i = 0}; ${2:i < length}; ${3:i++}) { %>', '\t$0', '<% } %>'],
    category: 'control',
  },
  {
    key: 'template-while',
    prefix: 'while',
    description: '<% while %> loop block',
    body: ['<% while (${1:condition}) { %>', '\t$0', '<% } %>'],
    category: 'control',
  },

  // ── Navigation ─────────────────────────────────────────────────
  {
    key: 'story-show',
    prefix: 'story.show',
    description: 'story.show() — navigate to a passage',
    body: ['story.show("${1:PassageName}")'],
    category: 'navigation',
  },

  // ── Variables ──────────────────────────────────────────────────
  {
    key: 'story-variable',
    prefix: 's.',
    description: 's.variable — Snowman story variable (persists)',
    body: ['s.${1:variableName}'],
    category: 'variable',
  },
  {
    key: 'temp-variable',
    prefix: 't.',
    description: 't.variable — Snowman temp variable (passage-scoped)',
    body: ['t.${1:variableName}'],
    category: 'variable',
  },
  {
    key: 'set-story-variable',
    prefix: 's-set',
    description: 'Set a Snowman story variable',
    body: ['<% s.${1:variableName} = ${2:value}; %>'],
    category: 'variable',
  },
  {
    key: 'set-temp-variable',
    prefix: 't-set',
    description: 'Set a Snowman temp variable',
    body: ['<% t.${1:variableName} = ${2:value}; %>'],
    category: 'variable',
  },
  {
    key: 'print-variable',
    prefix: 's-print',
    description: 'Print a Snowman story variable',
    body: ['<%= s.${1:variableName} %>'],
    category: 'output',
  },
  {
    key: 'print-temp-variable',
    prefix: 't-print',
    description: 'Print a Snowman temp variable',
    body: ['<%= t.${1:variableName} %>'],
    category: 'output',
  },

  // ── Links ──────────────────────────────────────────────────────
  {
    key: 'link',
    prefix: 'link',
    description: '[[ ]] passage link',
    body: ['[[${1:PassageName}]]'],
    category: 'navigation',
  },
  {
    key: 'link-arrow',
    prefix: 'link-arrow',
    description: '[[Text->Target]] passage link with display text',
    body: ['[[${1:Text}->${2:PassageName}]]'],
    category: 'navigation',
  },
];
