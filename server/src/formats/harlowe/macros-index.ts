/**
 * Knot v2 — Harlowe 3 Macro Index
 *
 * Assembles the complete HARLOWE_MACROS array from all category
 * files and builds the ALIAS_MAP for alias → canonical name lookup.
 */

import type { MacroDef } from '../_types';

import { getBasicsMacros } from './macros-basics';
import { getDataMacros } from './macros-data';
import { getDisplayMacros } from './macros-display';
import { getInteractiveMacros } from './macros-interactive';
import { getNavigationMacros } from './macros-navigation';
import { getAdvancedMacros } from './macros-advanced';

// ─── Complete macro catalog ─────────────────────────────────────

export const HARLOWE_MACROS: MacroDef[] = [
  ...getBasicsMacros(),
  ...getDataMacros(),
  ...getDisplayMacros(),
  ...getInteractiveMacros(),
  ...getNavigationMacros(),
  ...getAdvancedMacros(),
];

// ─── Alias map ──────────────────────────────────────────────────

export const ALIAS_MAP = new Map<string, string>();
for (const macro of HARLOWE_MACROS) {
  if (macro.aliases) {
    for (const alias of macro.aliases) {
      ALIAS_MAP.set(alias, macro.name);
    }
  }
}
