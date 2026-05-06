/**
 * Knot v2 — Chapbook 2 Runtime Globals & Virtual Prelude
 *
 * Defines the runtime objects available in Chapbook's JS scope
 * (window.CS, etc.) and a virtual prelude for static analysis.
 * Also includes `var` and `temp` pseudo-globals for hover support.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { RuntimeGlobalDef } from '../_types';

export const RUNTIME_GLOBALS: readonly RuntimeGlobalDef[] = [
  {
    name: 'CS',
    description: 'Chapbook core runtime object — provides engine state, passage navigation, and variable access',
    hasMembers: true,
    members: [
      { name: 'Engine',      description: 'Engine controller — passage navigation, restart, undo',  type: 'object' },
      { name: 'State',       description: 'State manager — story and temporary variables',          type: 'object' },
      { name: 'Passage',     description: 'Current passage object — name, tags, text',              type: 'object' },
      { name: 'Story',       description: 'Story metadata — title, author, if/settings',            type: 'object' },
      { name: 'Config',      description: 'Chapbook configuration object',                          type: 'object' },
    ],
  },
  {
    name: 'var',
    description: 'Chapbook story variable namespace — persists across passages (e.g. var.health, var.name)',
    hasMembers: false,
  },
  {
    name: 'temp',
    description: 'Chapbook temporary variable namespace — scoped to current passage (e.g. temp.visited)',
    hasMembers: false,
  },
];

/** JavaScript code that sets up a virtual runtime for static analysis */
export const VIRTUAL_RUNTIME_PRELUDE = `
// Chapbook 2 Virtual Runtime Prelude
// Simulates the Chapbook runtime environment for static analysis
var CS = { Engine: {}, State: {}, Passage: { name: '', tags: [], text: '' }, Story: {}, Config: {} };
// Note: Chapbook's var.* and temp.* are pseudo-namespaces in the template
// language, not actual JS globals. They are represented in RUNTIME_GLOBALS
// for hover/completion support but cannot be declared as JS variables
// since 'var' is a reserved keyword.
`.trim();
