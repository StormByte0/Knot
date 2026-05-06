/**
 * Knot v2 — Chapbook 2 Insert Index
 *
 * Assembles the complete CHAPBOOK_INSERTS array from all category
 * files and builds the ALIAS_MAP, KNOWN_INSERT_NAMES, and
 * KNOWN_MODIFIER_NAMES for lookup and diagnostics.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { MacroDef } from '../_types';

import { NAVIGATION_INSERTS } from './inserts-navigation';
import { OUTPUT_INSERTS } from './inserts-output';
import { INTERACTIVE_INSERTS } from './inserts-interactive';
import { CONTROL_INSERTS } from './inserts-control';
import { MODIFIER_INSERTS } from './inserts-modifiers';
import { DEBUG_INSERTS } from './inserts-debug';

// ─── Complete insert catalog ────────────────────────────────────

export const CHAPBOOK_INSERTS: MacroDef[] = [
  ...NAVIGATION_INSERTS,
  ...OUTPUT_INSERTS,
  ...INTERACTIVE_INSERTS,
  ...CONTROL_INSERTS,
  ...MODIFIER_INSERTS,
  ...DEBUG_INSERTS,
];

// ─── Alias map ──────────────────────────────────────────────────

export const ALIAS_MAP = new Map<string, string>();
for (const ins of CHAPBOOK_INSERTS) {
  if (ins.aliases) {
    for (const alias of ins.aliases) {
      ALIAS_MAP.set(alias, ins.name);
    }
  }
}

// ─── Known insert names (including aliases) ─────────────────────

export const KNOWN_INSERT_NAMES = new Set<string>();
for (const ins of CHAPBOOK_INSERTS) {
  KNOWN_INSERT_NAMES.add(ins.name);
  if (ins.aliases) {
    for (const alias of ins.aliases) {
      KNOWN_INSERT_NAMES.add(alias);
    }
  }
}

// ─── Known modifier names ───────────────────────────────────────

export const KNOWN_MODIFIER_NAMES = new Set<string>();
for (const ins of CHAPBOOK_INSERTS) {
  if (ins.categoryDetail === 'modifier') {
    KNOWN_MODIFIER_NAMES.add(ins.name);
  }
}
