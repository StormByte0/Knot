/**
 * Knot v2 — Harlowe 3 Basics Macros
 *
 * Variable assignment, output, branching (conditional), and iteration macros.
 */

import type { MacroDef } from '../_types';
import { m, mc, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getBasicsMacros(): MacroDef[] {
  return [

    // ─── VARIABLE ASSIGNMENT ───────────────────────────────────────
    m('set:', MacroCategory.Variable, MacroKind.Instant,
      'Stores a value in a variable, or modifies an existing value.',
      [
        sig([arg('assignment', 'Lambda', true, { variadic: true, description: 'One or more "variable to value" expressions' })], 'Instant', 'Sets variables without producing visible output.'),
      ],
      { isAssignment: true }),
    m('put:', MacroCategory.Variable, MacroKind.Instant,
      'Stores a value in a variable (alternative syntax to set:).',
      [
        sig([arg('assignment', 'Lambda', true, { variadic: true, description: 'One or more "variable into value" expressions' })], 'Instant'),
      ],
      { isAssignment: true }),
    m('move:', MacroCategory.Variable, MacroKind.Instant,
      'Moves a value from one variable or data structure position into another, removing the original.',
      [
        sig([arg('assignment', 'Lambda', true, { variadic: true, description: '"variable into expression" or "data position into variable" expressions' })], 'Instant'),
      ],
      { isAssignment: true }),

    // ─── OUTPUT ────────────────────────────────────────────────────
    m('print:', MacroCategory.Output, MacroKind.Command,
      'Prints a value into the passage, converting it to text.',
      [
        sig([arg('value', 'any', true, { description: 'The value to print' })], 'Command', 'Outputs the value as text.'),
      ]),
    m('display:', MacroCategory.Output, MacroKind.Command,
      'Renders the contents of another passage at this location.',
      [
        sig([arg('passageName', 'string', true, { description: 'Name of the passage to display' })], 'Command', 'Renders the passage content inline.'),
      ],
      { isInclude: true, passageArgPosition: 0 }),

    // ─── BRANCHING ─────────────────────────────────────────────────
    m('if:', MacroCategory.Control, MacroKind.Changer,
      'Conditionally renders the attached hook when the condition is true.',
      [
        sig([arg('condition', 'boolean', true, { description: 'The condition to check' })], 'Changer', 'Attaches to a hook that is shown only when the condition is true.'),
      ],
      { children: ['else-if:', 'else:'], isConditional: true }),
    m('unless:', MacroCategory.Control, MacroKind.Changer,
      'Conditionally renders the attached hook when the condition is false (inverse of if:).',
      [
        sig([arg('condition', 'boolean', true, { description: 'The condition to check (hook shown when false)' })], 'Changer'),
      ],
      { children: ['else-if:', 'else:'], isConditional: true }),
    m('else-if:', MacroCategory.Control, MacroKind.Changer,
      'An additional condition after if: or unless:.',
      [
        sig([arg('condition', 'boolean', true, { description: 'The additional condition to check' })], 'Changer'),
      ],
      { parents: ['if:', 'unless:'], children: ['else-if:', 'else:'], isConditional: true }),
    m('else:', MacroCategory.Control, MacroKind.Changer,
      'Renders the attached hook when all preceding if:/unless:/else-if: conditions were false.',
      [
        sig([], 'Changer', 'No arguments — simply pairs with a preceding if:/unless:.'),
      ],
      { parents: ['if:', 'unless:', 'else-if:'] }),

    // ─── ITERATION ─────────────────────────────────────────────────
    mc('for:', 'iteration', MacroKind.Changer,
      'Iterates over a range, array, or string, rendering the attached hook for each element.',
      [
        sig([arg('variableName', 'var', true, { description: 'The variable that will hold each value' }), arg('range', 'range|array|string|dataset', true, { description: 'The collection to iterate over' })], 'Changer'),
        sig([arg('variableName', 'var', true), arg('from', 'number', true, { description: 'Start value' }), arg('to', 'number', true, { description: 'End value' })], 'Changer'),
      ],
      { aliases: ['loop:'] }),
    m('either:', MacroCategory.Utility, MacroKind.Instant,
      'Randomly selects one of the given values.',
      [
        sig([arg('values', 'any', true, { variadic: true, description: 'Two or more values to choose from' })], 'any', 'Returns one of the provided values at random.'),
      ]),
    m('cond:', MacroCategory.Utility, MacroKind.Instant,
      'Selects the first value whose matching condition is true (like a multi-branch if).',
      [
        sig([arg('condition', 'boolean', true, { description: 'Condition to check' }), arg('value', 'any', true, { description: 'Value to return if condition is true' })], 'any', 'Pairs of condition/value arguments.'),
      ]),
    m('nth:', MacroCategory.Utility, MacroKind.Instant,
      'Selects the value at the given 1-based index from the provided values.',
      [
        sig([arg('index', 'number', true, { description: '1-based index' }), arg('values', 'any', true, { variadic: true, description: 'Values to select from' })], 'any'),
      ]),
  ];
}
