/**
 * Knot v2 — SugarCube 2 Control Macros
 *
 * Conditional display, branching, iteration, and output control.
 * if / else / elseif / for / while / break / continue /
 * switch / case / default / capture / silently / nobr
 */

import type { MacroDef } from '../_types';
import { m, mc, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getControlMacros(): MacroDef[] {
  return [
    // ── Conditional / Branching ──────────────────────────────────
    m('if', MacroCategory.Control, MacroKind.Changer,
      'Conditional display — renders body if expression is truthy',
      [sig([arg('condition', 'expression', true, { description: 'JavaScript expression' })])],
      { children: ['else', 'elseif'], hasBody: true, isConditional: true },
    ),
    m('else', MacroCategory.Control, MacroKind.Changer,
      'Else clause for <<if>>',
      [sig([])],
      { parents: ['if'], hasBody: true },
    ),
    m('elseif', MacroCategory.Control, MacroKind.Changer,
      'Else-if clause for <<if>>',
      [sig([arg('condition', 'expression', true, { description: 'JavaScript expression' })])],
      { parents: ['if'], hasBody: true, isConditional: true },
    ),

    // ── Iteration ────────────────────────────────────────────────
    mc('for', 'iteration', MacroKind.Changer,
      'Iterate over a range or collection',
      [
        sig([arg('variable', 'variable', true, { description: 'Loop variable ($var or _var)' }), arg('range', 'expression', true, { description: 'Range expression (e.g., 1 to 10, or array)' })]),
        sig([arg('init', 'expression', true, { description: 'Initialization expression' }), arg('condition', 'expression', true, { description: 'Loop condition expression' }), arg('post', 'expression', true, { description: 'Post-iteration expression' })]),
      ],
      { hasBody: true, children: ['break', 'continue'] },
    ),
    mc('while', 'iteration', MacroKind.Changer,
      'Loop while expression is truthy',
      [sig([arg('condition', 'expression', true, { description: 'JavaScript expression evaluated each iteration' })])],
      { hasBody: true, isConditional: true, children: ['break', 'continue'] },
    ),
    m('break', MacroCategory.Control, MacroKind.Command,
      'Break out of the nearest <<for>> or <<while>> loop',
      [sig([])],
      { parents: ['for', 'while'] },
    ),
    m('continue', MacroCategory.Control, MacroKind.Command,
      'Skip to the next iteration of the nearest <<for>> or <<while>> loop',
      [sig([])],
      { parents: ['for', 'while'] },
    ),

    // ── Switch / Case ────────────────────────────────────────────
    m('switch', MacroCategory.Control, MacroKind.Changer,
      'Switch-case selection',
      [sig([arg('expression', 'expression', true, { description: 'JavaScript expression' })])],
      { children: ['case', 'default'], hasBody: true },
    ),
    m('case', MacroCategory.Control, MacroKind.Changer,
      'Case clause for <<switch>>',
      [sig([arg('value', 'expression', true, { description: 'Match value' })])],
      { parents: ['switch'], hasBody: true },
    ),
    m('default', MacroCategory.Control, MacroKind.Changer,
      'Default clause for <<switch>>',
      [sig([])],
      { parents: ['switch'], hasBody: true },
    ),

    // ── Output Control ───────────────────────────────────────────
    m('capture', MacroCategory.Variable, MacroKind.Changer,
      'Capture output into a variable',
      [sig([arg('variable', 'string', true, { description: 'Variable name ($var)' })])],
      { hasBody: true, isAssignment: true },
    ),
    m('silently', MacroCategory.Output, MacroKind.Changer,
      'Suppress all output from body',
      [sig([])],
      { hasBody: true },
    ),
    m('nobr', MacroCategory.Output, MacroKind.Changer,
      'Suppress line breaks in body',
      [sig([])],
      { hasBody: true },
    ),
  ];
}
