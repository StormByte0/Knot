/**
 * Knot v2 — Harlowe 3 Navigation Macros
 *
 * Navigation commands, history, passage/story queries, storylets,
 * save/load, URL/scroll, and click-goto/click-undo navigation.
 */

import type { MacroDef } from '../_types';
import { m, mc, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getNavigationMacros(): MacroDef[] {
  return [

    // ─── NAVIGATION ─────────────────────────────────────────────────
    m('go-to:', MacroCategory.Navigation, MacroKind.Command,
      'Immediately navigates to the given passage.',
      [
        sig([arg('passageName', 'string', true, { description: 'Target passage name' })], 'Command'),
      ],
      { isNavigation: true, passageArgPosition: 0 }),
    m('redirect:', MacroCategory.Navigation, MacroKind.Command,
      'Redirects the player to a passage, similar to go-to: but intended for passage-level redirection.',
      [
        sig([arg('passageName', 'string', true, { description: 'Target passage name' })], 'Command'),
      ],
      { isNavigation: true, passageArgPosition: 0 }),
    m('undo:', MacroCategory.Navigation, MacroKind.Command,
      'Undoes the last turn, returning to the previous passage.',
      [
        sig([], 'Command'),
      ]),
    m('restart:', MacroCategory.Navigation, MacroKind.Command,
      'Restarts the entire story from the beginning.',
      [
        sig([], 'Command'),
      ],
      { aliases: ['reload:'] }),

    // ─── CLICK-NAV (navigation via click) ───────────────────────────
    m('click-goto:', MacroCategory.Navigation, MacroKind.Command,
      'Creates a clickable element that navigates to a passage.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Hook to make clickable' }), arg('passageName', 'string', true, { description: 'Target passage name' })], 'Command'),
      ],
      { isNavigation: true, passageArgPosition: 1 }),
    m('click-undo:', MacroCategory.Navigation, MacroKind.Command,
      'Creates a clickable element that undoes the last turn.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Hook to make clickable' })], 'Command'),
      ]),

    // ─── MOUSE-NAV ──────────────────────────────────────────────────
    m('mouseover-goto:', MacroCategory.Navigation, MacroKind.Command,
      'Navigates to a passage when the mouse enters a named hook.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Hook to watch' }), arg('passageName', 'string', true, { description: 'Target passage' })], 'Command'),
      ],
      { isNavigation: true, passageArgPosition: 1 }),
    m('mouseout-goto:', MacroCategory.Navigation, MacroKind.Command,
      'Navigates to a passage when the mouse leaves a named hook.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Hook to watch' }), arg('passageName', 'string', true, { description: 'Target passage' })], 'Command'),
      ],
      { isNavigation: true, passageArgPosition: 1 }),
    m('mouseover-undo:', MacroCategory.Navigation, MacroKind.Command,
      'Undoes the last turn when the mouse enters a named hook.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Hook to watch' })], 'Command'),
      ]),
    m('mouseout-undo:', MacroCategory.Navigation, MacroKind.Command,
      'Undoes the last turn when the mouse leaves a named hook.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Hook to watch' })], 'Command'),
      ]),

    // ─── HISTORY ────────────────────────────────────────────────────
    m('history:', MacroCategory.Navigation, MacroKind.Instant,
      'Returns an array of passage names visited in order during this session.',
      [
        sig([], 'Array<string>'),
      ]),
    m('visited:', MacroCategory.Navigation, MacroKind.Instant,
      'Returns the number of times the given passage has been visited.',
      [
        sig([arg('passageName', 'string', false, { description: 'Passage name (default: current passage)' })], 'number'),
      ]),
    m('passage:', MacroCategory.Navigation, MacroKind.Instant,
      'Returns the datamap of the given passage\'s metadata, or the current passage.',
      [
        sig([], 'Datamap', 'Returns the current passage\'s data.'),
        sig([arg('passageName', 'string', true, { description: 'The passage name' })], 'Datamap'),
      ]),
    m('passages:', MacroCategory.Navigation, MacroKind.Instant,
      'Returns an array of datamaps for every passage in the story.',
      [
        sig([], 'Array<Datamap>'),
      ]),
    m('forget-visits:', MacroCategory.Navigation, MacroKind.Instant,
      'Removes visit records for the given passage names.',
      [
        sig([arg('passageNames', 'string', true, { variadic: true, description: 'Passage names to forget visits for' })], 'Instant'),
      ]),
    m('forget-undos:', MacroCategory.Navigation, MacroKind.Instant,
      'Removes the ability to undo past the current turn.',
      [
        sig([], 'Instant'),
      ]),
    m('metadata:', MacroCategory.Navigation, MacroKind.Instant,
      'Returns the metadata datamap of the given passage.',
      [
        sig([arg('passageName', 'string', false, { description: 'Passage name (default: current passage)' })], 'Datamap'),
      ]),
    m('seed:', MacroCategory.System, MacroKind.Instant,
      'Sets the random number generator seed, making random: and either: deterministic.',
      [
        sig([arg('seed', 'string|number', true, { description: 'The RNG seed value' })], 'Instant'),
      ]),

    // ─── STORYLETS ──────────────────────────────────────────────────
    mc('storylet:', 'interactive', MacroKind.Instant,
      'Declares a passage as a storylet with an availability condition.',
      [
        sig([arg('when', 'Lambda', true, { description: '"when" lambda determining availability' })], 'Storylet'),
        sig([arg('when', 'Lambda', true), arg('exclusivity', 'number|Lambda', false, { description: 'Exclusivity level' })], 'Storylet'),
      ]),
    mc('open-storylets:', 'interactive', MacroKind.Instant,
      'Returns an array of currently available storylets.',
      [
        sig([], 'Array<Storylet>'),
        sig([arg('lambda', 'Lambda', false, { description: 'Lambda to filter/select storylets' })], 'Array<Storylet>'),
      ]),
    mc('exclusivity:', 'interactive', MacroKind.Instant,
      'Sets the exclusivity level for a storylet passage (higher = more exclusive).',
      [
        sig([arg('level', 'number', true, { description: 'Exclusivity level' })], 'Changer'),
        sig([arg('lambda', 'Lambda', true, { description: 'Lambda producing exclusivity level' })], 'Changer'),
      ]),
    mc('urgency:', 'interactive', MacroKind.Instant,
      'Sets the urgency level for a storylet passage (higher = more urgent).',
      [
        sig([arg('level', 'number', true, { description: 'Urgency level' })], 'Changer'),
        sig([arg('lambda', 'Lambda', true, { description: 'Lambda producing urgency level' })], 'Changer'),
      ]),

    // ─── SAVE/LOAD ──────────────────────────────────────────────────
    mc('load-game:', 'save', MacroKind.Command,
      'Loads a previously saved game from the given save slot.',
      [
        sig([arg('slotName', 'string', true, { description: 'The save slot name' })], 'Command'),
      ]),
    mc('save-game:', 'save', MacroKind.Command,
      'Saves the current game state to the given save slot.',
      [
        sig([arg('slotName', 'string', true, { description: 'The save slot name' })], 'boolean', 'Returns true if save succeeded.'),
        sig([arg('slotName', 'string', true), arg('metadata', 'Datamap', false, { description: 'Metadata to store with the save' })], 'boolean'),
      ]),
    mc('saved-games:', 'save', MacroKind.Instant,
      'Returns a datamap of all saved games, mapping slot names to their metadata.',
      [
        sig([], 'Datamap'),
      ]),

    // ─── URL/SCROLL ─────────────────────────────────────────────────
    m('goto-url:', MacroCategory.Navigation, MacroKind.Command,
      'Navigates the browser to an external URL.',
      [
        sig([arg('url', 'string', true, { description: 'The URL to navigate to' })], 'Command'),
      ],
      { isNavigation: true }),
    m('open-url:', MacroCategory.Navigation, MacroKind.Command,
      'Opens an external URL in a new browser tab.',
      [
        sig([arg('url', 'string', true, { description: 'The URL to open' })], 'Command'),
      ],
      { isNavigation: true }),
    m('page-url:', MacroCategory.Utility, MacroKind.Instant,
      'Returns the current page URL as a string.',
      [
        sig([], 'string'),
      ]),
    m('scroll:', MacroCategory.Navigation, MacroKind.Command,
      'Scrolls to a named hook or to the top of the page.',
      [
        sig([], 'Command', 'Scrolls to the top.'),
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to scroll to' })], 'Command'),
      ]),
  ];
}
