/**
 * Knot v2 — Snowman 2 Runtime Globals & Virtual Prelude
 *
 * Defines the runtime objects available in Snowman's JS scope
 * (s, t, story, window.story) and a virtual prelude for
 * static analysis.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { RuntimeGlobalDef } from '../_types';

export const RUNTIME_GLOBALS: readonly RuntimeGlobalDef[] = [
  {
    name: 's',
    description: 'Snowman story variable object (s.name) — window.story, persists across passages',
    hasMembers: true,
    members: [
      { name: 'show',      description: 'Navigate to a passage by name',           type: 'function' },
      { name: 'passage',   description: 'Get a Passage object by name',            type: 'function' },
      { name: 'name',      description: 'Name of the current passage',             type: 'string' },
      { name: 'passages',  description: 'Map of all Passage objects',              type: 'object' },
      { name: 'title',     description: 'Story title',                             type: 'string' },
      { name: 'author',    description: 'Story author',                            type: 'string' },
      { name: 'tags',      description: 'All tags used across passages',           type: 'array' },
    ],
  },
  {
    name: 't',
    description: 'Snowman temp variable object (t.name) — window.passage, scoped to current passage',
    hasMembers: true,
    members: [
      { name: 'name',   description: 'Name of the current passage',    type: 'string' },
      { name: 'source', description: 'Raw source text of the passage', type: 'string' },
      { name: 'tags',   description: 'Tags on the current passage',    type: 'array' },
    ],
  },
  {
    name: 'story',
    description: 'Snowman story object — same as window.story, persists across passages',
    hasMembers: true,
    members: [
      { name: 'show',      description: 'Navigate to a passage by name',           type: 'function' },
      { name: 'passage',   description: 'Get a Passage object by name',            type: 'function' },
      { name: 'name',      description: 'Name of the current passage',             type: 'string' },
      { name: 'passages',  description: 'Map of all Passage objects',              type: 'object' },
      { name: 'title',     description: 'Story title',                             type: 'string' },
      { name: 'author',    description: 'Story author',                            type: 'string' },
      { name: 'tags',      description: 'All tags used across passages',           type: 'array' },
    ],
  },
  {
    name: 'window',
    description: 'Browser window object — provides window.story and window.passage aliases',
    hasMembers: true,
    members: [
      { name: 'story',   description: 'Alias for the story object',         type: 'object' },
      { name: 'passage', description: 'Alias for the current passage object', type: 'object' },
    ],
  },
];

/** JavaScript code that sets up a virtual runtime for static analysis */
export const VIRTUAL_RUNTIME_PRELUDE: string = `
// Snowman 2 Virtual Runtime Prelude
// Simulates the Snowman runtime environment for static analysis
var s = { show: function(){}, passage: function(){ return { name:'', source:'', tags:[] }; }, name: '', passages: {}, title: '', author: '', tags: [] };
var t = { name: '', source: '', tags: [] };
var story = { show: function(){}, passage: function(){ return { name:'', source:'', tags:[] }; }, name: '', passages: {}, title: '', author: '', tags: [] };
var window = { story: story, passage: t };
`.trim();
