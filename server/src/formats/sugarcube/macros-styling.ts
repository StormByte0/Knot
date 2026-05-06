/**
 * Knot v2 — SugarCube 2 Styling Macros
 *
 * CSS class and inline style application.
 * span / div / class / id / style
 */

import type { MacroDef } from '../_types';
import { m, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getStylingMacros(): MacroDef[] {
  return [
    m('span', MacroCategory.Styling, MacroKind.Command,
      'Apply a CSS class to inline content',
      [sig([arg('class', 'string', true, { description: 'CSS class name(s)' })])],
      { hasBody: true },
    ),
    m('div', MacroCategory.Styling, MacroKind.Command,
      'Apply a CSS class to block content',
      [sig([arg('class', 'string', true, { description: 'CSS class name(s)' })])],
      { hasBody: true },
    ),
    m('class', MacroCategory.Styling, MacroKind.Command,
      'Apply a CSS class (block-level)',
      [sig([arg('class', 'string', true, { description: 'CSS class name(s)' })])],
      { hasBody: true },
    ),
    m('id', MacroCategory.Styling, MacroKind.Command,
      'Apply an ID to block content',
      [sig([arg('id', 'string', true, { description: 'Element ID' })])],
      { hasBody: true },
    ),
    m('style', MacroCategory.Styling, MacroKind.Command,
      'Apply inline CSS styles',
      [sig([arg('style', 'string', true, { description: 'CSS style string' })])],
      { hasBody: true },
    ),
  ];
}
