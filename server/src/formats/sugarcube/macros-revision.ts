/**
 * Knot v2 — SugarCube 2 Revision & DOM Macros
 *
 * Content manipulation via CSS selectors — appending, replacing, removing
 * elements and toggling CSS classes.
 * append / prepend / replace / remove /
 * addclass / removeclass / toggleclass / setclass / triggerevent
 */

import type { MacroDef } from '../_types';
import { mc, m, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getRevisionMacros(): MacroDef[] {
  return [
    // ── Content Revision ─────────────────────────────────────────
    mc('append', 'revision', MacroKind.Command,
      'Append content to a selector',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' })])],
      { hasBody: true },
    ),
    mc('prepend', 'revision', MacroKind.Command,
      'Prepend content to a selector',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' })])],
      { hasBody: true },
    ),
    mc('replace', 'revision', MacroKind.Command,
      'Replace content at a selector',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' })])],
      { hasBody: true },
    ),
    mc('remove', 'revision', MacroKind.Command,
      'Remove elements matching a selector',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' })])],
    ),

    // ── DOM Class Manipulation ───────────────────────────────────
    m('addclass', MacroCategory.System, MacroKind.Command,
      'Add a CSS class to elements',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' }), arg('class', 'string', true, { description: 'CSS class' })])],
    ),
    m('removeclass', MacroCategory.System, MacroKind.Command,
      'Remove a CSS class from elements',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' }), arg('class', 'string', true, { description: 'CSS class' })])],
    ),
    m('toggleclass', MacroCategory.System, MacroKind.Command,
      'Toggle a CSS class on elements',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' }), arg('class', 'string', true, { description: 'CSS class' })])],
    ),
    m('setclass', MacroCategory.System, MacroKind.Command,
      'Set the CSS class of elements',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' }), arg('class', 'string', true, { description: 'CSS class' })])],
    ),
    m('triggerevent', MacroCategory.System, MacroKind.Command,
      'Trigger a DOM event on elements',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' }), arg('event', 'string', true, { description: 'Event name' })])],
    ),
  ];
}
