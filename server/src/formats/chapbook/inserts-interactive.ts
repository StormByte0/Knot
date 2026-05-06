/**
 * Knot v2 — Chapbook 2 Interactive Inserts
 *
 * Inserts for interactive elements: reveal link, insert link,
 * replace link, cycling link, cycle, select link, dropdown menu,
 * text input, meter, progress bar, toggle, tooltip.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { MacroDef } from '../_types';
import { insert, MacroCategory, MacroKind, arg, sig } from './inserts-helpers';

export const INTERACTIVE_INSERTS: MacroDef[] = [

  // ── Revealing ─────────────────────────────────────────────────────

  insert('reveal link', MacroCategory.Custom, MacroKind.Command,
    'Reveal hidden content on click',
    [
      sig([arg('link', 'string', true, { description: 'The link text to click' })], 'Command', 'Renders a link; when clicked, reveals the content that follows.'),
      sig([arg('link', 'string', true, { description: 'The link text to click' }), arg('passage', 'string', true, { description: 'Passage to navigate to after revealing' })], 'Command', 'Renders a link; when clicked, reveals content then navigates to the named passage.'),
    ],
    { categoryDetail: 'revealing', isNavigation: false, passageArgPosition: 1 },
  ),

  insert('insert link', MacroCategory.Custom, MacroKind.Command,
    'Insert content from another passage on click',
    [
      sig([arg('link', 'string', true, { description: 'The link text to click' }), arg('passage', 'string', false, { description: 'Passage whose content to insert' })], 'Command', 'Renders a link; when clicked, inserts the content of the named passage.'),
    ],
    { categoryDetail: 'revealing', isInclude: true, passageArgPosition: 1 },
  ),

  insert('replace link', MacroCategory.Custom, MacroKind.Command,
    'Replace content with passage content on click',
    [
      sig([arg('link', 'string', true, { description: 'The link text to click' }), arg('passage', 'string', true, { description: 'Passage whose content replaces the current content' })], 'Command', 'Renders a link; when clicked, replaces surrounding content with the named passage\'s content.'),
    ],
    { categoryDetail: 'revealing', isInclude: true, passageArgPosition: 1 },
  ),

  // ── Cycling ───────────────────────────────────────────────────────

  insert('cycling link', MacroCategory.Custom, MacroKind.Command,
    'Cycle through values on click',
    [
      sig([arg('values', 'string', true, { variadic: true, description: 'Values to cycle through' })], 'Command', 'Renders a link that cycles through the given values each time it is clicked.'),
    ],
    { categoryDetail: 'cycling' },
  ),

  insert('cycle', MacroCategory.Variable, MacroKind.Command,
    'Cycle through a list of values for a variable',
    [
      sig([arg('variable', 'string', true, { description: 'Variable to cycle (e.g. var.choice)' }), arg('options', 'string', true, { variadic: true, description: 'Options to cycle through' })], 'Command', 'Renders a cycling selector that rotates through the given options, updating the variable each time.'),
    ],
    { categoryDetail: 'cycling', isAssignment: true },
  ),

  insert('select link', MacroCategory.Custom, MacroKind.Command,
    'Select from options via link clicks',
    [
      sig([arg('variable', 'string', true, { description: 'Variable to set (e.g. var.choice)' }), arg('options', 'string', true, { variadic: true, description: 'Options to select from' })], 'Command', 'Renders a series of links representing options; clicking one sets the variable to that value.'),
    ],
    { categoryDetail: 'cycling', isAssignment: true },
  ),

  // ── Input ─────────────────────────────────────────────────────────

  insert('dropdown menu', MacroCategory.Custom, MacroKind.Command,
    'Dropdown selector',
    [
      sig([arg('options', 'string', true, { variadic: true, description: 'Dropdown options' })], 'Command', 'Renders a dropdown menu with the given options.'),
    ],
    { categoryDetail: 'input' },
  ),

  insert('text input', MacroCategory.Custom, MacroKind.Command,
    'Text input field',
    [
      sig([arg('placeholder', 'string', false, { description: 'Placeholder text for the input' })], 'Command', 'Renders a text input field.'),
    ],
    { categoryDetail: 'input' },
  ),

  // ── Visual ────────────────────────────────────────────────────────

  insert('meter', MacroCategory.Styling, MacroKind.Command,
    'Visual progress bar',
    [
      sig([arg('value', 'number', true, { description: 'Current value (0-1)' })], 'Command', 'Renders a visual meter/progress bar at the given fraction.'),
    ],
  ),

  insert('progress bar', MacroCategory.Styling, MacroKind.Command,
    'Progress bar',
    [
      sig([arg('value', 'number', true, { description: 'Current value (0-1)' })], 'Command', 'Renders a progress bar at the given fraction.'),
    ],
  ),

  // ── Variable Interaction ──────────────────────────────────────────

  insert('toggle', MacroCategory.Variable, MacroKind.Command,
    'Toggle a boolean variable',
    [
      sig([arg('variable', 'string', true, { description: 'Variable name to toggle (e.g. var.flag)' })], 'Command', 'Renders a toggle that flips a boolean variable between true and false.'),
    ],
    { isAssignment: true },
  ),

  insert('tooltip', MacroCategory.Styling, MacroKind.Command,
    'Tooltip on hover',
    [
      sig([arg('text', 'string', true, { description: 'The visible text' }), arg('tip', 'string', true, { description: 'The tooltip text shown on hover' })], 'Command'),
    ],
  ),
];
