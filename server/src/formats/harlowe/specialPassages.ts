/**
 * Knot v2 — Harlowe 3 Special Passages
 *
 * Declarative definitions for Harlowe's format-specific special passages.
 * Core pre-classifies [script]→Script and [stylesheet]→Stylesheet per
 * the Twee 3 spec. This list adds Harlowe-specific special passages.
 */

import type { SpecialPassageDef } from '../_types';
import { PassageKind } from '../../hooks/hookTypes';

export const SPECIAL_PASSAGES: readonly SpecialPassageDef[] = [
  { name: 'Startup',      kind: PassageKind.Special, description: 'Runs once when the story begins',        priority: 0, tag: 'startup',        typeId: 'startup' },
  { name: 'Header',       kind: PassageKind.Special, description: 'Content prepended to every passage',     priority: 1, tag: 'header',         typeId: 'header' },
  { name: 'Footer',       kind: PassageKind.Special, description: 'Content appended to every passage',      priority: 1, tag: 'footer',         typeId: 'footer' },
  { name: 'DebugHeader',  kind: PassageKind.Special, description: 'Header content shown only in debug mode', tag: 'debug-header',  typeId: 'debug-header' },
  { name: 'DebugFooter',  kind: PassageKind.Special, description: 'Footer content shown only in debug mode', tag: 'debug-footer',  typeId: 'debug-footer' },
  { name: 'DebugStartup', kind: PassageKind.Special, description: 'Startup content run only in debug mode',  tag: 'debug-startup', typeId: 'debug-startup' },
];
