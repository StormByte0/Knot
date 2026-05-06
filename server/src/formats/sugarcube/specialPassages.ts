/**
 * Knot v2 — SugarCube 2 Special Passages
 *
 * Declarative definitions for SugarCube's named and tag-based
 * special passages.
 */

import type { SpecialPassageDef } from '../_types';
import { PassageKind } from '../../hooks/hookTypes';

export const SPECIAL_PASSAGES: SpecialPassageDef[] = [
  // ── Named special passages ────────────────────────────────────
  { name: 'StoryInit',         kind: PassageKind.Special, description: 'Runs code before every passage render',      priority: 0, tag: 'init', typeId: 'init' },
  { name: 'StoryInterface',    kind: PassageKind.Special, description: 'Defines the story\'s custom HTML interface structure', priority: 0, typeId: 'interface' },
  { name: 'PassageHeader',     kind: PassageKind.Special, description: 'Rendered before every passage content',     priority: 2, typeId: 'header' },
  { name: 'PassageFooter',     kind: PassageKind.Special, description: 'Rendered after every passage content',      priority: 2, typeId: 'footer' },
  { name: 'PassageReady',      kind: PassageKind.Special, description: 'Runs code after every passage renders',     priority: 2, typeId: 'post-render' },
  { name: 'PassageDone',       kind: PassageKind.Special, description: 'Runs code after every passage transition',  priority: 2, typeId: 'post-transition' },
  { name: 'StoryMenu',         kind: PassageKind.Special, description: 'Items for the story menu',                  priority: 2, typeId: 'menu' },
  { name: 'StoryAuthor',       kind: PassageKind.Special, description: 'Author name display',                       priority: 2, typeId: 'author' },
  { name: 'StoryCaption',      kind: PassageKind.Special, description: 'Sidebar caption',                           priority: 2, typeId: 'caption' },
  { name: 'StoryDisplayTitle', kind: PassageKind.Special, description: 'Custom story title display',                priority: 2, typeId: 'display-title' },
  { name: 'StorySubtitle',     kind: PassageKind.Special, description: 'Story subtitle',                            priority: 2, typeId: 'subtitle' },

  // ── Debug passages (active when Config.debug is true) ─────────
  { name: 'DebugView',         kind: PassageKind.Special, description: 'Customize debug view output',               priority: 3 },
  { name: 'DebugHeader',       kind: PassageKind.Special, description: 'Rendered at top of debug view',             priority: 3 },
  { name: 'DebugFooter',       kind: PassageKind.Special, description: 'Rendered at bottom of debug view',          priority: 3 },

  // ── Tag-based special passages ────────────────────────────────
  { name: '',                   kind: PassageKind.Special, description: 'Widget definition passage',                tag: 'widget', typeId: 'widget' },
];
