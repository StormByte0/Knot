/**
 * Knot v2 — SugarCube 2 Variable Macros
 *
 * Variable assignment, deletion, and persistence.
 * set / unset / remember / forget
 */

import type { MacroDef } from '../_types';
import { m, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getVariableMacros(): MacroDef[] {
  return [
    m('set', MacroCategory.Variable, MacroKind.Command,
      'Set one or more story or temporary variables',
      [sig([arg('expression', 'expression', true, { variadic: true, description: 'Assignment expression (e.g., $hp to 10)' })])],
      { isAssignment: true },
    ),
    m('unset', MacroCategory.Variable, MacroKind.Command,
      'Delete one or more story or temporary variables',
      [sig([arg('variable', 'variable', true, { variadic: true, description: 'Variable reference to delete ($var or _var)' })])],
    ),
    m('remember', MacroCategory.Variable, MacroKind.Command,
      'Set a story variable and persist it to local storage across sessions',
      [sig([arg('expression', 'expression', true, { description: 'Assignment expression (e.g., $name to "Alice")' })])],
      { isAssignment: true },
    ),
    m('forget', MacroCategory.Variable, MacroKind.Command,
      'Remove a remembered variable from local storage',
      [sig([arg('expression', 'expression', true, { description: 'Variable reference' })])],
    ),
  ];
}
