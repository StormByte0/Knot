/**
 * Knot v2 — Chapbook 2 Control Inserts
 *
 * Inserts for control flow: if, unless, else, section, section-end.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { MacroDef } from '../_types';
import { insert, MacroCategory, MacroKind, arg, sig } from './inserts-helpers';

export const CONTROL_INSERTS: MacroDef[] = [

  // ── Conditional Display ───────────────────────────────────────────

  insert('if', MacroCategory.Control, MacroKind.Changer,
    'Conditional display — renders content when condition is truthy',
    [
      sig([arg('condition', 'expression', true, { description: 'Variable expression to evaluate' })], 'Changer', 'Renders the following content only when the condition is truthy.'),
    ],
    { children: ['else'], hasBody: true, isConditional: true },
  ),

  insert('unless', MacroCategory.Control, MacroKind.Changer,
    'Inverse conditional — renders content when condition is falsy',
    [
      sig([arg('condition', 'expression', true, { description: 'Variable expression to evaluate' })], 'Changer', 'Renders the following content only when the condition is falsy.'),
    ],
    { children: ['else'], hasBody: true, isConditional: true },
  ),

  insert('else', MacroCategory.Control, MacroKind.Changer,
    'Else clause for {if} or {unless}',
    [
      sig([], 'Changer', 'Renders content when the preceding {if} or {unless} condition was not met.'),
    ],
    { parents: ['if', 'unless'], hasBody: true },
  ),

  // ── Section ───────────────────────────────────────────────────────

  insert('section', MacroCategory.Control, MacroKind.Changer,
    'Begin a content section with scoped variables',
    [
      sig([], 'Changer', 'Begins a new content section. Sections create variable scopes and can be used with modifiers.'),
    ],
    { children: ['section-end'], hasBody: true, categoryDetail: 'section' },
  ),

  insert('section-end', MacroCategory.Control, MacroKind.Changer,
    'End a content section',
    [
      sig([], 'Changer', 'Ends the most recently opened content section.'),
    ],
    { parents: ['section'], categoryDetail: 'section', aliases: ['end section'] },
  ),
];
