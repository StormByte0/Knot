/**
 * Knot v2 — Harlowe 3 Advanced Macros
 *
 * Custom macros, datatype helpers, debugging, math, date/time,
 * lambda keywords, and UI icon macros.
 */

import type { MacroDef } from '../_types';
import { m, mc, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getAdvancedMacros(): MacroDef[] {
  return [

    // ─── CUSTOM MACROS ──────────────────────────────────────────────
    mc('macro:', 'customMacro', MacroKind.Instant,
      'Defines a custom macro from a lambda that can be called later.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'A lambda defining the custom macro\'s parameters and body' })], 'CustomMacro'),
      ]),
    mc('output:', 'customMacro', MacroKind.Command,
      'Inside a custom macro definition, produces visible output.',
      [
        sig([arg('value', 'any', true, { description: 'The value to output' })], 'Command'),
      ],
      { aliases: ['out:'] }),
    mc('output-data:', 'customMacro', MacroKind.Instant,
      'Inside a custom macro definition, returns data without producing visible output.',
      [
        sig([arg('value', 'any', true, { description: 'The data to return' })], 'any'),
      ],
      { aliases: ['out-data:'] }),
    mc('error:', 'customMacro', MacroKind.Command,
      'Produces an error message, typically used inside custom macro definitions.',
      [
        sig([arg('message', 'string', true, { description: 'The error message to display' })], 'Command'),
      ]),

    // ─── DATATYPE HELPERS ───────────────────────────────────────────
    m('datatype:', MacroCategory.Utility, MacroKind.Instant,
      'Creates a datatype value for use in custom macro type signatures.',
      [
        sig([arg('name', 'string', true, { description: 'Name of the datatype' })], 'Datatype'),
        sig([], 'Datatype', 'With no arguments, produces a generic datatype matcher.'),
      ]),
    m('datapattern:', MacroCategory.Utility, MacroKind.Instant,
      'Creates a data pattern for matching complex data structures.',
      [
        sig([arg('pattern', 'any', true, { variadic: true, description: 'Pattern components' })], 'Datapattern'),
      ]),
    m('partial:', MacroCategory.Utility, MacroKind.Instant,
      'Creates a partial application of a macro or lambda.',
      [
        sig([arg('macro', 'macro|Lambda', true, { description: 'The macro or lambda to partially apply' }), arg('args', 'any', true, { variadic: true, description: 'Arguments to pre-fill' })], 'partial'),
      ]),
    mc('num-type:', 'datatype', MacroKind.Instant,
      'Produces the datatype matching any number value, for use in custom macro type signatures.',
      [
        sig([], 'Datatype', 'The number datatype, equivalent to datatype "number".'),
      ]),
    mc('str-type:', 'datatype', MacroKind.Instant,
      'Produces the datatype matching any string value, for use in custom macro type signatures.',
      [
        sig([], 'Datatype', 'The string datatype, equivalent to datatype "string".'),
      ]),
    mc('bool-type:', 'datatype', MacroKind.Instant,
      'Produces the datatype matching any boolean value, for use in custom macro type signatures.',
      [
        sig([], 'Datatype', 'The boolean datatype, equivalent to datatype "boolean".'),
      ]),
    mc('array-type:', 'datatype', MacroKind.Instant,
      'Produces the datatype matching any array value, for use in custom macro type signatures.',
      [
        sig([], 'Datatype', 'The array datatype.'),
      ]),
    mc('dm-type:', 'datatype', MacroKind.Instant,
      'Produces the datatype matching any datamap value, for use in custom macro type signatures.',
      [
        sig([], 'Datatype', 'The datamap datatype.'),
      ]),
    mc('ds-type:', 'datatype', MacroKind.Instant,
      'Produces the datatype matching any dataset value, for use in custom macro type signatures.',
      [
        sig([], 'Datatype', 'The dataset datatype.'),
      ]),
    mc('any-type:', 'datatype', MacroKind.Instant,
      'Produces the datatype matching any value at all, for use in custom macro type signatures.',
      [
        sig([], 'Datatype', 'The any datatype — matches everything.'),
      ]),
    mc('command-type:', 'datatype', MacroKind.Instant,
      'Produces the datatype matching any command value, for use in custom macro type signatures.',
      [
        sig([], 'Datatype', 'The command datatype.'),
      ]),
    mc('changer-type:', 'datatype', MacroKind.Instant,
      'Produces the datatype matching any changer value, for use in custom macro type signatures.',
      [
        sig([], 'Datatype', 'The changer datatype.'),
      ]),

    // ─── LAMBDA KEYWORDS (documented as macros for LSP support) ─────
    // These are not true macros but keywords used in lambda expressions.
    // Documenting them allows the LSP to provide hover/completion.
    mc('where:', 'lambda', MacroKind.Instant,
      'Keyword used in lambda expressions to filter elements by a boolean condition. ' +
      'Written after the lambda variable: e.g. _item where _item > 5.',
      [
        sig([arg('condition', 'boolean', true, { description: 'The filter condition' })], 'Lambda'),
      ]),
    mc('via:', 'lambda', MacroKind.Instant,
      'Keyword used in lambda expressions to compute a value for each element. ' +
      'Written after the lambda variable: e.g. _item via _item * 2.',
      [
        sig([arg('expression', 'any', true, { description: 'The expression to compute' })], 'Lambda'),
      ]),
    mc('making:', 'lambda', MacroKind.Instant,
      'Keyword used in lambda expressions to name an accumulator variable in fold operations. ' +
      'Written alongside the element variable: e.g. _item making _total via _total + _item.',
      [
        sig([arg('accumulatorName', 'var', true, { description: 'Name for the accumulator variable' })], 'Lambda'),
      ]),

    // ─── DEBUGGING ──────────────────────────────────────────────────
    mc('ignore:', 'debugging', MacroKind.Instant,
      'Evaluates its argument but discards the result, producing no output.',
      [
        sig([arg('value', 'any', true, { variadic: true, description: 'Values to ignore' })], 'Instant'),
      ]),
    mc('test-true:', 'debugging', MacroKind.Instant,
      'Asserts that the given value is true; produces a visible error otherwise.',
      [
        sig([arg('condition', 'boolean', true, { description: 'Condition that must be true' })], 'Instant'),
      ]),
    mc('test-false:', 'debugging', MacroKind.Instant,
      'Asserts that the given value is false; produces a visible error otherwise.',
      [
        sig([arg('condition', 'boolean', true, { description: 'Condition that must be false' })], 'Instant'),
      ]),
    mc('assert:', 'debugging', MacroKind.Instant,
      'Asserts that a condition is true; produces a visible error if false.',
      [
        sig([arg('condition', 'boolean', true, { description: 'The assertion condition' })], 'Instant'),
      ]),
    mc('assert-exists:', 'debugging', MacroKind.Instant,
      'Asserts that the given value is not nothing/undefined.',
      [
        sig([arg('value', 'any', true, { description: 'Value that must exist' })], 'Instant'),
      ]),
    mc('debug:', 'debugging', MacroKind.Command,
      'Outputs debug information about its argument to the debug console.',
      [
        sig([arg('value', 'any', true, { description: 'The value to debug' })], 'Command'),
      ]),
    mc('mock-turns:', 'debugging', MacroKind.Instant,
      'Sets a mock number of turns (for testing visited:/history: in preview).',
      [
        sig([arg('turns', 'number', true, { description: 'Mock turn count' })], 'Instant'),
      ]),
    mc('mock-visits:', 'debugging', MacroKind.Instant,
      'Sets mock passage visit counts (for testing visited: in preview).',
      [
        sig([arg('pairs', 'string|number', true, { variadic: true, description: 'Alternating passage name and visit count' })], 'Instant'),
      ]),

    // ─── MATH ───────────────────────────────────────────────────────
    mc('abs:', 'math', MacroKind.Instant,
      'Returns the absolute value of a number.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
      ]),
    mc('cos:', 'math', MacroKind.Instant,
      'Returns the cosine of an angle in radians.',
      [
        sig([arg('radians', 'number', true, { description: 'Angle in radians' })], 'number'),
      ]),
    mc('exp:', 'math', MacroKind.Instant,
      'Returns e raised to the given power.',
      [
        sig([arg('power', 'number', true, { description: 'The exponent' })], 'number'),
      ]),
    mc('log:', 'math', MacroKind.Instant,
      'Returns the natural logarithm of a number.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
      ]),
    mc('log10:', 'math', MacroKind.Instant,
      'Returns the base-10 logarithm of a number.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
      ]),
    mc('log2:', 'math', MacroKind.Instant,
      'Returns the base-2 logarithm of a number.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
      ]),
    mc('max:', 'math', MacroKind.Instant,
      'Returns the larger of two numbers.',
      [
        sig([arg('a', 'number', true), arg('b', 'number', true)], 'number'),
      ]),
    mc('min:', 'math', MacroKind.Instant,
      'Returns the smaller of two numbers.',
      [
        sig([arg('a', 'number', true), arg('b', 'number', true)], 'number'),
      ]),
    mc('pow:', 'math', MacroKind.Instant,
      'Returns base raised to the exponent power.',
      [
        sig([arg('base', 'number', true, { description: 'The base' }), arg('exponent', 'number', true, { description: 'The exponent' })], 'number'),
      ]),
    mc('sign:', 'math', MacroKind.Instant,
      'Returns the sign of a number: -1, 0, or 1.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
      ]),
    mc('sin:', 'math', MacroKind.Instant,
      'Returns the sine of an angle in radians.',
      [
        sig([arg('radians', 'number', true, { description: 'Angle in radians' })], 'number'),
      ]),
    mc('sqrt:', 'math', MacroKind.Instant,
      'Returns the square root of a number.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
      ]),
    mc('tan:', 'math', MacroKind.Instant,
      'Returns the tangent of an angle in radians.',
      [
        sig([arg('radians', 'number', true, { description: 'Angle in radians' })], 'number'),
      ]),
    mc('ceil:', 'math', MacroKind.Instant,
      'Rounds a number up to the nearest integer.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
      ]),
    mc('floor:', 'math', MacroKind.Instant,
      'Rounds a number down to the nearest integer.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
      ]),
    mc('num:', 'math', MacroKind.Instant,
      'Converts a string to a number.',
      [
        sig([arg('string', 'string', true, { description: 'The string to convert' })], 'number'),
      ],
      { aliases: ['number:'] }),
    mc('random:', 'math', MacroKind.Instant,
      'Returns a random integer between min and max inclusive.',
      [
        sig([arg('max', 'number', true, { description: 'Maximum value' })], 'number'),
        sig([arg('min', 'number', true, { description: 'Minimum value' }), arg('max', 'number', true, { description: 'Maximum value' })], 'number'),
      ]),
    mc('round:', 'math', MacroKind.Instant,
      'Rounds a number to the nearest integer, or to the given decimal places.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
        sig([arg('number', 'number', true), arg('places', 'number', true, { description: 'Decimal places' })], 'number'),
      ]),
    mc('trunc:', 'math', MacroKind.Instant,
      'Truncates a number to an integer by removing the fractional part.',
      [
        sig([arg('number', 'number', true, { description: 'The number' })], 'number'),
      ]),
    mc('dice:', 'math', MacroKind.Instant,
      'Simulates rolling dice and returns an array of the individual roll results.',
      [
        sig([arg('sides', 'number', true, { description: 'Number of sides per die' })], 'Array<number>'),
        sig([arg('count', 'number', true, { description: 'Number of dice to roll' }), arg('sides', 'number', true, { description: 'Number of sides per die' })], 'Array<number>'),
      ]),

    // ─── DATE/TIME ──────────────────────────────────────────────────
    mc('current-date:', 'dateTime', MacroKind.Instant,
      'Returns the current date as a string (e.g. "Thu Jan 1 1970").',
      [
        sig([], 'string'),
      ]),
    mc('current-time:', 'dateTime', MacroKind.Instant,
      'Returns the current time as a string (e.g. "12:00 AM").',
      [
        sig([], 'string'),
      ]),
    mc('monthday:', 'dateTime', MacroKind.Instant,
      'Returns the current day of the month (1-31).',
      [
        sig([], 'number'),
      ]),
    mc('weekday:', 'dateTime', MacroKind.Instant,
      'Returns the current day of the week as a string (e.g. "Monday").',
      [
        sig([], 'string'),
      ]),

    // ─── UI ICONS ───────────────────────────────────────────────────
    m('icon-undo:', MacroCategory.System, MacroKind.Command,
      'Renders an undo icon button.',
      [
        sig([], 'Command'),
      ]),
    m('icon-redo:', MacroCategory.System, MacroKind.Command,
      'Renders a redo icon button.',
      [
        sig([], 'Command'),
      ]),
    m('icon-fullscreen:', MacroCategory.System, MacroKind.Command,
      'Renders a fullscreen toggle icon button.',
      [
        sig([], 'Command'),
      ]),
    m('icon-restart:', MacroCategory.System, MacroKind.Command,
      'Renders a restart icon button.',
      [
        sig([], 'Command'),
      ]),
    m('icon-counter:', MacroCategory.System, MacroKind.Command,
      'Renders a turn counter display.',
      [
        sig([], 'Command'),
      ]),
  ];
}
