/**
 * Knot v2 — Chapbook 2 Debug Inserts
 *
 * Debug inserts: note, debug.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { MacroDef } from '../_types';
import { insert, MacroCategory, MacroKind, arg, sig } from './inserts-helpers';

export const DEBUG_INSERTS: MacroDef[] = [

  insert('note', MacroCategory.System, MacroKind.Command,
    'Author\'s note visible only in debug mode',
    [
      sig([arg('text', 'string', true, { description: 'The note text to display in debug mode' })], 'Command', 'Displays a note that is only visible when running in debug/test mode. Not shown in published stories.'),
    ],
    { categoryDetail: 'debug' },
  ),

  insert('debug', MacroCategory.System, MacroKind.Command,
    'Debug output of an expression',
    [
      sig([arg('expression', 'expression', true, { description: 'Expression to evaluate and display' })], 'Command', 'Evaluates the expression and displays the result. Only active in debug mode.'),
    ],
    { categoryDetail: 'debug' },
  ),
];
