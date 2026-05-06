/**
 * Knot v2 — SugarCube 2 Timed / Live Macros
 *
 * Delayed rendering, repeating content, and timer control.
 * repeat / stop / timed / next / done
 */

import type { MacroDef } from '../_types';
import { mc, sig, arg, MacroKind } from './macros-helpers';

export function getTimedMacros(): MacroDef[] {
  return [
    mc('repeat', 'live', MacroKind.Command,
      'Repeat content on a timer',
      [sig([arg('delay', 'expression', true, { description: 'Delay in ms' }), arg('unit', 'string', false, { description: 'Time unit' })])],
      { hasBody: true },
    ),
    mc('stop', 'live', MacroKind.Command,
      'Stop a <<repeat>> timer',
      [sig([])],
    ),
    mc('timed', 'live', MacroKind.Command,
      'Delay content rendering',
      [sig([arg('delay', 'expression', true, { description: 'Delay in ms' })])],
      { hasBody: true },
    ),
    mc('next', 'live', MacroKind.Command,
      'Next step in a <<timed>> sequence',
      [sig([arg('delay', 'expression', false, { description: 'Delay in ms before this step renders' })])],
      { parents: ['timed'] },
    ),
    mc('done', 'live', MacroKind.Changer,
      'Final section that runs when <<timed>> completes or <<repeat>> is stopped',
      [sig([])],
      { hasBody: true, parents: ['timed', 'repeat'] },
    ),
  ];
}
