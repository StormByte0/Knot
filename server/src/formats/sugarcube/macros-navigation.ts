/**
 * Knot v2 — SugarCube 2 Navigation Macros
 *
 * Passage navigation, links, and interactive link variants.
 * goto / back / return / button / link / linkappend /
 * linkprepend / linkreplace / actions / choice
 */

import type { MacroDef } from '../_types';
import { m, mc, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getNavigationMacros(): MacroDef[] {
  return [
    // ── Direct Navigation ────────────────────────────────────────
    m('goto', MacroCategory.Navigation, MacroKind.Command,
      'Navigate to a passage immediately',
      [sig([arg('passage', 'string', true, { description: 'Passage name or expression' })])],
      { isNavigation: true, passageArgPosition: 0 },
    ),
    m('back', MacroCategory.Navigation, MacroKind.Command,
      'Navigate to the previous passage',
      [sig([]), sig([arg('label', 'string', false, { description: 'Button label' })])],
      { isNavigation: true },
    ),
    m('return', MacroCategory.Navigation, MacroKind.Command,
      'Return to a prior passage in the history',
      [sig([]), sig([arg('passage', 'string', false, { description: 'Passage to return to' })])],
      { isNavigation: true },
    ),

    // ── Interactive Links ────────────────────────────────────────
    mc('button', 'interactive', MacroKind.Changer,
      'Interactive button that navigates or runs code',
      [
        sig([arg('passage', 'string', true, { description: 'Passage name or URL' })]),
        sig([arg('passage', 'string', true, { description: 'Passage name or URL' }), arg('text', 'string', false, { description: 'Button label' })]),
      ],
      { hasBody: true, isNavigation: true, passageArgPosition: 0 },
    ),
    mc('link', 'interactive', MacroKind.Changer,
      'Interactive link with body content',
      [
        sig([arg('passage', 'string', true, { description: 'Passage name' })]),
        sig([arg('text', 'string', true, { description: 'Link text' }), arg('passage', 'string', false, { description: 'Passage name' })]),
      ],
      { hasBody: true, isNavigation: true, passageArgPosition: 0 },
    ),
    mc('linkappend', 'interactive', MacroKind.Command,
      'Append content when link is clicked',
      [sig([arg('text', 'string', true, { description: 'Link text' })])],
    ),
    mc('linkprepend', 'interactive', MacroKind.Command,
      'Prepend content when link is clicked',
      [sig([arg('text', 'string', true, { description: 'Link text' })])],
    ),
    mc('linkreplace', 'interactive', MacroKind.Command,
      'Replace content when link is clicked',
      [sig([arg('text', 'string', true, { description: 'Link text' })])],
    ),
    mc('actions', 'interactive', MacroKind.Command,
      'Render a list of passage links that disappear after visiting',
      [sig([arg('passages', 'string', true, { description: 'Comma-separated passage names' })])],
      { isNavigation: true },
    ),
    mc('choice', 'interactive', MacroKind.Command,
      'Render a passage choice link',
      [
        sig([arg('passage', 'string', true, { description: 'Passage name' })]),
        sig([arg('text', 'string', true, { description: 'Link text' }), arg('passage', 'string', false, { description: 'Passage name' })]),
      ],
      { isNavigation: true },
    ),
  ];
}
