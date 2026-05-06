/**
 * Knot v2 — Harlowe 3 Interactive Macros
 *
 * Links (changer + command), clicks, interactive input, revision,
 * mouse events, live/timed, and dialogs.
 */

import type { MacroDef } from '../_types';
import { m, mc, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getInteractiveMacros(): MacroDef[] {
  return [

    // ─── INTERACTIVE INPUT ──────────────────────────────────────────
    mc('cycling-link:', 'interactive', MacroKind.Command,
      'Creates a cycling link that advances through values on each click.',
      [
        sig([arg('variable', 'var', true, { description: 'Variable to bind' }), arg('values', 'string', true, { variadic: true, description: 'Values to cycle through' })], 'Command'),
      ]),
    mc('seq-link:', 'interactive', MacroKind.Command,
      'Creates a link that, when clicked, shows the next value in a sequence and eventually disappears.',
      [
        sig([arg('variable', 'var', true, { description: 'Variable to bind' }), arg('values', 'string', true, { variadic: true, description: 'Values to show sequentially' })], 'Command'),
      ],
      { aliases: ['sequence-link:'] }),
    mc('input:', 'interactive', MacroKind.Command,
      'Creates a text input field bound to a variable.',
      [
        sig([arg('variable', 'var', true, { description: 'Variable to bind to the input' })], 'Command'),
        sig([arg('variable', 'var', true), arg('options', 'Datamap', false, { description: 'Options like "default"' })], 'Command'),
      ]),
    mc('force-input:', 'interactive', MacroKind.Command,
      'Creates a text input field that forces the bound variable to update on every keystroke.',
      [
        sig([arg('variable', 'var', true, { description: 'Variable to bind' })], 'Command'),
        sig([arg('variable', 'var', true), arg('options', 'Datamap', false)], 'Command'),
      ]),
    mc('input-box:', 'interactive', MacroKind.Command,
      'Creates a multi-line text input box bound to a variable.',
      [
        sig([arg('variable', 'var', true, { description: 'Variable to bind' })], 'Command'),
        sig([arg('variable', 'var', true), arg('options', 'Datamap', false, { description: 'Options like "default", "size"' })], 'Command'),
      ]),
    mc('force-input-box:', 'interactive', MacroKind.Command,
      'Creates a multi-line text input box that forces variable updates on every keystroke.',
      [
        sig([arg('variable', 'var', true, { description: 'Variable to bind' })], 'Command'),
        sig([arg('variable', 'var', true), arg('options', 'Datamap', false)], 'Command'),
      ]),
    mc('checkbox:', 'interactive', MacroKind.Command,
      'Creates a checkbox input bound to a variable.',
      [
        sig([arg('variable', 'var', true, { description: 'Variable to bind' }), arg('label', 'string', true, { description: 'Checkbox label' }), arg('checked', 'boolean', true, { description: 'Initial checked state' })], 'Command'),
      ]),
    mc('checkbox-fullscreen:', 'interactive', MacroKind.Command,
      'Creates a checkbox that toggles fullscreen mode.',
      [
        sig([arg('label', 'string', true, { description: 'Checkbox label' })], 'Command'),
      ]),
    mc('dropdown:', 'interactive', MacroKind.Command,
      'Creates a dropdown menu bound to a variable.',
      [
        sig([arg('variable', 'var', true, { description: 'Variable to bind' }), arg('values', 'string', true, { variadic: true, description: 'Dropdown options' })], 'Command'),
        sig([arg('variable', 'var', true), arg('options', 'Datamap', true, { description: 'Options including "default"' }), arg('values', 'string', true, { variadic: true })], 'Command'),
      ]),
    mc('meter:', 'interactive', MacroKind.Command,
      'Creates a visual meter/bar displaying a fractional value.',
      [
        sig([arg('value', 'number', true, { description: 'Current value (0-1)' }), arg('options', 'Datamap', false, { description: 'Options: "size", "colour", "border"' })], 'Command'),
      ]),

    // ─── LINK CHANGERS ──────────────────────────────────────────────
    mc('link:', 'interactive', MacroKind.Changer,
      'Creates a link that, when clicked, replaces the attached hook content.',
      [
        sig([arg('linkText', 'string', true, { description: 'The link text' })], 'Changer'),
        sig([arg('linkText', 'string', true), arg('hookValue', 'Changer|Lambda', false, { description: 'Changer or lambda to apply when clicked' })], 'Changer'),
      ],
      { aliases: ['link-replace:'] }),
    mc('link-reveal:', 'interactive', MacroKind.Changer,
      'Creates a link that, when clicked, reveals the attached hook content (appending it).',
      [
        sig([arg('linkText', 'string', true, { description: 'The link text' })], 'Changer'),
        sig([arg('linkText', 'string', true), arg('hookValue', 'Changer|Lambda', false)], 'Changer'),
      ],
      { aliases: ['link-append:'] }),
    mc('link-repeat:', 'interactive', MacroKind.Changer,
      'Creates a link that, each time it is clicked, re-runs the attached hook.',
      [
        sig([arg('linkText', 'string', true, { description: 'The link text' })], 'Changer'),
        sig([arg('linkText', 'string', true), arg('hookValue', 'Changer|Lambda', false)], 'Changer'),
      ]),
    mc('link-rerun:', 'interactive', MacroKind.Changer,
      'Creates a link that, when clicked, re-runs the attached hook from scratch.',
      [
        sig([arg('linkText', 'string', true, { description: 'The link text' })], 'Changer'),
        sig([arg('linkText', 'string', true), arg('hookValue', 'Changer|Lambda', false)], 'Changer'),
      ]),

    // ─── LINK COMMANDS ──────────────────────────────────────────────
    m('link-goto:', MacroCategory.Navigation, MacroKind.Command,
      'Creates a link to another passage.',
      [
        sig([arg('passageName', 'string', true, { description: 'The target passage name' })], 'Command'),
        sig([arg('linkText', 'string', true, { description: 'The displayed link text' }), arg('passageName', 'string', true, { description: 'The target passage name' })], 'Command'),
      ],
      { isNavigation: true, passageArgPosition: 1 }),
    m('link-reveal-goto:', MacroCategory.Navigation, MacroKind.Command,
      'Creates a link to another passage that also reveals content before transitioning.',
      [
        sig([arg('passageName', 'string', true, { description: 'Target passage name' })], 'Command'),
        sig([arg('linkText', 'string', true), arg('passageName', 'string', true)], 'Command'),
      ],
      { isNavigation: true, passageArgPosition: 1 }),
    m('link-undo:', MacroCategory.Navigation, MacroKind.Command,
      'Creates a link that undoes the last turn.',
      [
        sig([arg('linkText', 'string', true, { description: 'The link text' })], 'Command'),
      ]),
    m('link-fullscreen:', MacroCategory.Navigation, MacroKind.Command,
      'Creates a link that toggles fullscreen mode.',
      [
        sig([arg('linkText', 'string', true, { description: 'The link text' })], 'Command'),
      ]),
    mc('link-show:', 'revision', MacroKind.Command,
      'Creates a link that, when clicked, shows a named hook.',
      [
        sig([arg('linkText', 'string', true, { description: 'The link text' }), arg('hookName', 'hookName', true, { variadic: true, description: 'Names of hooks to show' })], 'Command'),
      ]),
    mc('link-storylet:', 'interactive', MacroKind.Command,
      'Creates a link to a storylet, selecting from available storylets via a lambda.',
      [
        sig([arg('linkText', 'string', true, { description: 'The link text' })], 'Command'),
        sig([arg('linkText', 'string', true), arg('lambda', 'Lambda', true, { description: 'Lambda to filter/select storylets' })], 'Command'),
      ]),

    // ─── CLICK ──────────────────────────────────────────────────────
    mc('click:', 'interactive', MacroKind.Changer,
      'Makes the attached hook respond to clicks, running its contents each time.',
      [
        sig([], 'Changer', 'Clicks on the hook itself.'),
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to click' })], 'Changer'),
      ]),
    mc('click-replace:', 'interactive', MacroKind.Changer,
      'When a named hook is clicked, replaces its content with the attached hook content.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to replace on click' })], 'Changer'),
      ]),
    mc('click-rerun:', 'interactive', MacroKind.Changer,
      'When a named hook is clicked, re-runs its content.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to re-run on click' })], 'Changer'),
      ]),
    mc('click-append:', 'interactive', MacroKind.Changer,
      'When a named hook is clicked, appends the attached hook content to it.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to append to on click' })], 'Changer'),
      ]),
    mc('click-prepend:', 'interactive', MacroKind.Changer,
      'When a named hook is clicked, prepends the attached hook content to it.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to prepend to on click' })], 'Changer'),
      ]),
    mc('action:', 'interactive', MacroKind.Changer,
      'Runs a lambda when the attached hook is clicked.',
      [
        sig([arg('lambda', 'Lambda', true, { description: 'Lambda to run on click' })], 'Changer'),
      ]),

    // ─── DOM/CONTENT (REVISION) ─────────────────────────────────────
    mc('replace:', 'revision', MacroKind.Command,
      'Replaces the content of a named hook with new content.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to replace' })], 'Command'),
      ]),
    mc('append:', 'revision', MacroKind.Command,
      'Appends content to a named hook.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to append to' })], 'Command'),
      ]),
    mc('prepend:', 'revision', MacroKind.Command,
      'Prepends content to a named hook.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to prepend to' })], 'Command'),
      ]),
    mc('replace-with:', 'revision', MacroKind.Command,
      'Immediately replaces a named hook with the given value (no attached hook needed).',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to replace' }), arg('value', 'string|HookSet', true, { description: 'New content' })], 'Command'),
      ]),
    mc('append-with:', 'revision', MacroKind.Command,
      'Immediately appends the given value to a named hook (no attached hook needed).',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to append to' }), arg('value', 'string|HookSet', true, { description: 'Content to append' })], 'Command'),
      ]),
    mc('prepend-with:', 'revision', MacroKind.Command,
      'Immediately prepends the given value to a named hook (no attached hook needed).',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to prepend to' }), arg('value', 'string|HookSet', true, { description: 'Content to prepend' })], 'Command'),
      ]),
    mc('rerun:', 'revision', MacroKind.Command,
      'Re-runs the changers and macros attached to a named hook.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to re-run' })], 'Command'),
      ]),
    mc('show:', 'revision', MacroKind.Command,
      'Reveals a previously hidden named hook.',
      [
        sig([arg('hookName', 'hookName', true, { variadic: true, description: 'Names of hooks to show' })], 'Command'),
      ]),

    // ─── LIVE/TIMED ─────────────────────────────────────────────────
    mc('live:', 'live', MacroKind.Changer,
      'Makes the attached hook re-render at a regular time interval.',
      [
        sig([arg('delay', 'number', false, { description: 'Seconds between re-renders (default: 0.2)' })], 'Changer'),
      ]),
    mc('stop:', 'live', MacroKind.Command,
      'Stops the nearest (live:) macro from continuing to re-render.',
      [
        sig([], 'Command'),
      ]),
    mc('event:', 'live', MacroKind.Command,
      'Runs the attached hook when a specific DOM event occurs.',
      [
        sig([arg('eventName', 'string', true, { description: 'DOM event name (e.g. "click")' }), arg('lambda', 'Lambda', true, { description: 'Lambda to run when event fires' })], 'Command'),
      ]),
    mc('after:', 'live', MacroKind.Command,
      'Delays the rendering of the attached hook by the given duration.',
      [
        sig([arg('delay', 'number|duration', true, { description: 'Delay in seconds or duration string' })], 'Command'),
      ]),
    mc('after-error:', 'live', MacroKind.Command,
      'Like after:, but only triggers if an error has occurred.',
      [
        sig([arg('delay', 'number|duration', true, { description: 'Delay before checking for errors' })], 'Command'),
      ]),
    mc('more:', 'live', MacroKind.Command,
      'Pauses a (live:) macro until the user clicks a "more" link.',
      [
        sig([], 'Command'),
      ]),

    // ─── DIALOGS ────────────────────────────────────────────────────
    mc('dialog:', 'dialog', MacroKind.Command,
      'Shows a dialog box with a message and optional buttons.',
      [
        sig([arg('message', 'string', true, { description: 'The dialog message' })], 'Command'),
        sig([arg('message', 'string', true), arg('buttons', 'string', true, { variadic: true, description: 'Button labels' })], 'Command'),
      ],
      { aliases: ['alert:'] }),
    mc('confirm:', 'dialog', MacroKind.Command,
      'Shows a confirmation dialog with OK and Cancel buttons.',
      [
        sig([arg('message', 'string', true, { description: 'The confirmation message' })], 'Command'),
      ]),
    mc('prompt:', 'dialog', MacroKind.Command,
      'Shows a prompt dialog asking the user for text input.',
      [
        sig([arg('message', 'string', true, { description: 'The prompt message' }), arg('defaultValue', 'string', false, { description: 'Default input value' })], 'Command'),
      ]),

    // ─── MOUSE EVENTS ───────────────────────────────────────────────
    mc('mouseover:', 'interactive', MacroKind.Changer,
      'Makes the attached hook respond when the mouse enters a named hook.',
      [
        sig([], 'Changer', 'Mouseover on the attached hook itself.'),
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to watch' })], 'Changer'),
      ]),
    mc('mouseout:', 'interactive', MacroKind.Changer,
      'Makes the attached hook respond when the mouse leaves a named hook.',
      [
        sig([], 'Changer', 'Mouseout on the attached hook itself.'),
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to watch' })], 'Changer'),
      ]),
    mc('mouseover-replace:', 'interactive', MacroKind.Changer,
      'Replaces a named hook\'s content when the mouse enters it.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to replace on mouseover' })], 'Changer'),
      ]),
    mc('mouseover-append:', 'interactive', MacroKind.Changer,
      'Appends to a named hook when the mouse enters it.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to append to on mouseover' })], 'Changer'),
      ]),
    mc('mouseover-prepend:', 'interactive', MacroKind.Changer,
      'Prepends to a named hook when the mouse enters it.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to prepend to on mouseover' })], 'Changer'),
      ]),
    mc('mouseout-replace:', 'interactive', MacroKind.Changer,
      'Replaces a named hook\'s content when the mouse leaves it.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to replace on mouseout' })], 'Changer'),
      ]),
    mc('mouseout-append:', 'interactive', MacroKind.Changer,
      'Appends to a named hook when the mouse leaves it.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to append to on mouseout' })], 'Changer'),
      ]),
    mc('mouseout-prepend:', 'interactive', MacroKind.Changer,
      'Prepends to a named hook when the mouse leaves it.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to prepend to on mouseout' })], 'Changer'),
      ]),
  ];
}
