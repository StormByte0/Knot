/**
 * Knot v2 — Chapbook 2 Navigation Inserts
 *
 * Inserts for navigation: back link, restart link, undo link,
 * link to, redirect to.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { MacroDef } from '../_types';
import { insert, MacroCategory, MacroKind, arg, sig } from './inserts-helpers';

export const NAVIGATION_INSERTS: MacroDef[] = [

  insert('back link', MacroCategory.Navigation, MacroKind.Command,
    'Link to go back in history',
    [
      sig([], 'Command', 'Renders a link that navigates to the previous passage in the history.'),
    ],
    { isNavigation: true },
  ),

  insert('restart link', MacroCategory.Navigation, MacroKind.Command,
    'Link to restart the story',
    [
      sig([arg('label', 'string', false, { description: 'Optional label text for the link (default: "Restart")' })], 'Command', 'Renders a link that restarts the story from the beginning.'),
    ],
    { isNavigation: true },
  ),

  insert('undo link', MacroCategory.Navigation, MacroKind.Command,
    'Link to undo last action',
    [
      sig([], 'Command', 'Renders a link that undoes the last navigation action.'),
    ],
    { isNavigation: true },
  ),

  insert('link to', MacroCategory.Navigation, MacroKind.Command,
    'External link',
    [
      sig([arg('url', 'string', true, { description: 'The URL to link to' })], 'Command', 'Renders a link to an external URL.'),
    ],
  ),

  insert('redirect to', MacroCategory.Navigation, MacroKind.Command,
    'Redirect to another passage immediately',
    [
      sig([arg('passage', 'string', true, { description: 'Name of the passage to redirect to' })], 'Command', 'Immediately navigates to the named passage without rendering the current passage content.'),
    ],
    { isNavigation: true, passageArgPosition: 0 },
  ),
];
