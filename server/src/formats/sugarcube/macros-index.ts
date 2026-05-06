/**
 * Knot v2 — SugarCube 2 Macro Index
 *
 * Assembles the complete SUGARCUBE_MACROS array from all category
 * files and builds the ALIAS_MAP for alias → canonical name lookup.
 */

import type { MacroDef } from '../_types';

import { getControlMacros } from './macros-control';
import { getNavigationMacros } from './macros-navigation';
import { getOutputMacros } from './macros-output';
import { getVariableMacros } from './macros-variable';
import { getStylingMacros } from './macros-styling';
import { getRevisionMacros } from './macros-revision';
import { getAudioMacros } from './macros-audio';
import { getTimedMacros } from './macros-timed';
import { getFormMacros } from './macros-form';
import { getMiscMacros } from './macros-misc';

// ─── Complete macro catalog ─────────────────────────────────────

export const SUGARCUBE_MACROS: MacroDef[] = [
  ...getControlMacros(),
  ...getNavigationMacros(),
  ...getOutputMacros(),
  ...getVariableMacros(),
  ...getStylingMacros(),
  ...getRevisionMacros(),
  ...getAudioMacros(),
  ...getTimedMacros(),
  ...getFormMacros(),
  ...getMiscMacros(),
];

// ─── Alias map ──────────────────────────────────────────────────

export const ALIAS_MAP = new Map<string, string>();
for (const macro of SUGARCUBE_MACROS) {
  if (macro.aliases) {
    for (const alias of macro.aliases) {
      ALIAS_MAP.set(alias, macro.name);
    }
  }
}
