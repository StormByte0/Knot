/**
 * Knot v2 — Harlowe 3 Display Macros
 *
 * Styling/layout, borders, colour, text-style, transitions,
 * verbatim, and enchanting macros.
 */

import type { MacroDef } from '../_types';
import { m, mc, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getDisplayMacros(): MacroDef[] {
  return [

    // ─── VERBATIM ───────────────────────────────────────────────────
    m('verbatim:', MacroCategory.Styling, MacroKind.Changer,
      'Prevents markup parsing inside the attached hook, rendering text literally.',
      [
        sig([], 'Changer', 'No arguments — simply wraps the hook in verbatim mode.'),
      ],
      { aliases: ['v6m:'] }),
    m('verbatim-print:', MacroCategory.Output, MacroKind.Command,
      'Prints a value without processing any markup within it.',
      [
        sig([arg('value', 'any', true, { description: 'The value to print verbatim' })], 'Command'),
      ],
      { aliases: ['v6m-print:'] }),
    m('verbatim-source:', MacroCategory.Utility, MacroKind.Instant,
      'Produces the source code representation of a value, as a string.',
      [
        sig([arg('value', 'any', true, { description: 'The value whose source code to produce' })], 'string'),
      ],
      { aliases: ['v6m-source:'] }),

    // ─── ENCHANTING ─────────────────────────────────────────────────
    m('change:', MacroCategory.Styling, MacroKind.Changer,
      'Applies a changer to every named hook on the page that matches the given hook name.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'The hook name or hookset to enchant' }), arg('changer', 'Changer', true, { description: 'The changer to apply' })], 'Changer'),
      ]),
    m('enchant:', MacroCategory.Styling, MacroKind.Changer,
      'Applies a changer to all matching hooks or text on the page.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'The hook name, hookset, or selector to enchant' }), arg('changer', 'Changer', true, { description: 'The changer to apply' })], 'Changer'),
      ]),
    m('enchant-in:', MacroCategory.Styling, MacroKind.Changer,
      'Like enchant:, but only affects hooks within the attached hook.',
      [
        sig([arg('hookName', 'hookName', true, { description: 'The hook name or selector to enchant' }), arg('changer', 'Changer', true, { description: 'The changer to apply' })], 'Changer'),
      ]),
    m('hooks-named:', MacroCategory.Utility, MacroKind.Instant,
      'Returns a hookset of all hooks with the given name, for use with enchant: or change:.',
      [
        sig([arg('name', 'string', true, { description: 'The hook name' })], 'HookSet'),
      ]),

    // ─── BORDERS ────────────────────────────────────────────────────
    m('border:', MacroCategory.Styling, MacroKind.Changer,
      'Applies a border style to the attached hook.',
      [
        sig([arg('style', 'string', true, { description: 'CSS border style value' })], 'Changer'),
        sig([arg('style', 'string', true), arg('colour', 'colour', false, { description: 'Border colour' })], 'Changer'),
        sig([arg('top', 'string', true), arg('right', 'string', true), arg('bottom', 'string', true), arg('left', 'string', true)], 'Changer'),
      ],
      { aliases: ['b4r:'] }),
    m('border-colour:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the border colour of the attached hook.',
      [
        sig([arg('colour', 'colour', true, { description: 'The border colour' })], 'Changer'),
      ],
      { aliases: ['b4r-colour:', 'border-color:', 'b4r-color:'] }),
    m('border-size:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the border width of the attached hook.',
      [
        sig([arg('size', 'number', true, { description: 'Border width in pixels' })], 'Changer'),
        sig([arg('top', 'number', true), arg('right', 'number', true), arg('bottom', 'number', true), arg('left', 'number', true)], 'Changer'),
      ],
      { aliases: ['b4r-size:'] }),
    m('corner-radius:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the corner radius of the attached hook.',
      [
        sig([arg('radius', 'number', true, { description: 'Corner radius in pixels' })], 'Changer'),
      ]),

    // ─── COLOUR ─────────────────────────────────────────────────────
    mc('hsl:', 'colour', MacroKind.Instant,
      'Creates a colour value from HSL values.',
      [
        sig([arg('hue', 'number', true, { description: 'Hue (0-360)' }), arg('saturation', 'number', true, { description: 'Saturation (0-1)' }), arg('lightness', 'number', true, { description: 'Lightness (0-1)' })], 'colour'),
        sig([arg('hue', 'number', true), arg('saturation', 'number', true), arg('lightness', 'number', true), arg('alpha', 'number', true, { description: 'Alpha (0-1)' })], 'colour'),
      ],
      { aliases: ['hsla:'] }),
    mc('rgb:', 'colour', MacroKind.Instant,
      'Creates a colour value from RGB values.',
      [
        sig([arg('red', 'number', true, { description: 'Red (0-255)' }), arg('green', 'number', true, { description: 'Green (0-255)' }), arg('blue', 'number', true, { description: 'Blue (0-255)' })], 'colour'),
        sig([arg('red', 'number', true), arg('green', 'number', true), arg('blue', 'number', true), arg('alpha', 'number', true, { description: 'Alpha (0-1)' })], 'colour'),
      ],
      { aliases: ['rgba:'] }),
    mc('lch:', 'colour', MacroKind.Instant,
      'Creates a colour value from LCH values.',
      [
        sig([arg('L', 'number', true, { description: 'Lightness (0-100)' }), arg('C', 'number', true, { description: 'Chroma' }), arg('H', 'number', true, { description: 'Hue (0-360)' })], 'colour'),
        sig([arg('L', 'number', true), arg('C', 'number', true), arg('H', 'number', true), arg('alpha', 'number', true, { description: 'Alpha (0-1)' })], 'colour'),
      ],
      { aliases: ['lcha:'] }),
    mc('complement:', 'colour', MacroKind.Instant,
      'Returns the complementary colour of the given colour.',
      [
        sig([arg('colour', 'colour', true, { description: 'The colour to complement' })], 'colour'),
      ]),
    mc('palette:', 'colour', MacroKind.Instant,
      'Generates an array of colours forming a palette based on a given colour and type.',
      [
        sig([arg('type', 'string', true, { description: 'Palette type: "mono", "adjacent", "triad", or "tetrad"' }), arg('colour', 'colour', true, { description: 'Base colour' })], 'Array<colour>'),
      ]),
    mc('gradient:', 'colour', MacroKind.Instant,
      'Creates a gradient colour value with multiple colour stops.',
      [
        sig([arg('angle', 'number', true, { description: 'Gradient angle in degrees' }), arg('stops', 'colour|number', true, { variadic: true, description: 'Alternating colour and stop position pairs' })], 'gradient'),
      ]),
    mc('stripes:', 'colour', MacroKind.Instant,
      'Creates a striped pattern colour value.',
      [
        sig([arg('angle', 'number', true, { description: 'Stripe angle in degrees' }), arg('stops', 'colour|number', true, { variadic: true, description: 'Alternating colour and pixel-width pairs' })], 'stripes'),
      ]),
    mc('mix:', 'colour', MacroKind.Instant,
      'Mixes two colours together by a given amount.',
      [
        sig([arg('colour1', 'colour', true, { description: 'First colour' }), arg('colour2', 'colour', true, { description: 'Second colour' }), arg('amount', 'number', false, { description: 'Mix ratio 0-1 (default 0.5)' })], 'colour'),
      ]),

    // ─── STYLING ────────────────────────────────────────────────────
    m('align:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the text alignment of the attached hook.',
      [
        sig([arg('alignment', 'string', true, { description: 'Alignment: "==>", "===>", "==>", "====>", or "<==>"' })], 'Changer'),
      ]),
    m('bg:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the background colour or gradient of the attached hook.',
      [
        sig([arg('colour', 'colour|gradient', true, { description: 'The background colour or gradient' })], 'Changer'),
      ],
      { aliases: ['background:'] }),
    m('box:', MacroCategory.Styling, MacroKind.Changer,
      'Creates a styled box around the attached hook.',
      [
        sig([arg('options', 'Datamap', false, { description: 'Box options: "colour", "border", "shadow", etc.' })], 'Changer'),
      ]),
    m('button:', MacroCategory.Styling, MacroKind.Changer,
      'Styles the attached hook as a clickable button.',
      [
        sig([], 'Changer'),
        sig([arg('options', 'Datamap', false, { description: 'Button options' })], 'Changer'),
      ]),
    m('char-style:', MacroCategory.Styling, MacroKind.Changer,
      'Changes the default text style of individual characters in the attached hook.',
      [
        sig([arg('changer', 'Changer', true, { description: 'Changer to apply to each character' })], 'Changer'),
      ]),
    m('collapse:', MacroCategory.Styling, MacroKind.Changer,
      'Removes whitespace from the attached hook\'s rendered output.',
      [
        sig([], 'Changer'),
      ]),
    m('css:', MacroCategory.Styling, MacroKind.Changer,
      'Applies raw CSS to the attached hook.',
      [
        sig([arg('css', 'string', true, { description: 'CSS rules to apply' })], 'Changer'),
      ]),
    m('float-box:', MacroCategory.Styling, MacroKind.Changer,
      'Creates a floating box positioned relative to the page.',
      [
        sig([], 'Changer'),
        sig([arg('options', 'Datamap', false, { description: 'Box options: "position", "size"' })], 'Changer'),
      ]),
    m('font:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the font family of the attached hook.',
      [
        sig([arg('fontFamily', 'string', true, { description: 'CSS font family string' })], 'Changer'),
      ]),
    m('hook:', MacroCategory.Styling, MacroKind.Changer,
      'Gives the attached hook a name, allowing it to be targeted by enchant:, replace:, etc.',
      [
        sig([arg('name', 'string', true, { description: 'The hook name' })], 'Changer'),
      ]),
    m('hover-style:', MacroCategory.Styling, MacroKind.Changer,
      'Applies a changer when the mouse hovers over the attached hook.',
      [
        sig([arg('changer', 'Changer', true, { description: 'Changer to apply on hover' })], 'Changer'),
      ]),
    m('line-style:', MacroCategory.Styling, MacroKind.Changer,
      'Changes the default text style of entire lines in the attached hook.',
      [
        sig([arg('changer', 'Changer', true, { description: 'Changer to apply to each line' })], 'Changer'),
      ]),
    m('link-style:', MacroCategory.Styling, MacroKind.Changer,
      'Changes the default style of links within the attached hook.',
      [
        sig([arg('changer', 'Changer', true, { description: 'Changer to apply to links' })], 'Changer'),
      ]),
    m('opacity:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the opacity of the attached hook.',
      [
        sig([arg('value', 'number', true, { description: 'Opacity value (0 to 1)' })], 'Changer'),
      ]),
    m('text-colour:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the text colour of the attached hook.',
      [
        sig([arg('colour', 'colour', true, { description: 'The text colour' })], 'Changer'),
      ],
      { aliases: ['colour:', 'text-color:', 'color:'] }),
    m('text-indent:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the first-line indent of the attached hook.',
      [
        sig([arg('size', 'number', true, { description: 'Indent size in pixels or em' })], 'Changer'),
      ]),
    m('text-rotate-x:', MacroCategory.Styling, MacroKind.Changer,
      'Rotates the attached hook around the X axis by the given degrees.',
      [
        sig([arg('degrees', 'number', true, { description: 'Rotation angle in degrees' })], 'Changer'),
      ]),
    m('text-rotate-y:', MacroCategory.Styling, MacroKind.Changer,
      'Rotates the attached hook around the Y axis by the given degrees.',
      [
        sig([arg('degrees', 'number', true, { description: 'Rotation angle in degrees' })], 'Changer'),
      ]),
    m('text-rotate-z:', MacroCategory.Styling, MacroKind.Changer,
      'Rotates the attached hook around the Z axis by the given degrees.',
      [
        sig([arg('degrees', 'number', true, { description: 'Rotation angle in degrees' })], 'Changer'),
      ],
      { aliases: ['text-rotate:'] }),
    m('text-size:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the font size of the attached hook.',
      [
        sig([arg('size', 'number', true, { description: 'Font size in pixels or em' })], 'Changer'),
      ],
      { aliases: ['size:'] }),
    m('text-style:', MacroCategory.Styling, MacroKind.Changer,
      'Applies a named text style to the attached hook.',
      [
        sig([arg('style', 'string', true, { description: 'Style name: "bold", "italic", "underline", "strike", "superscript", "subscript", "mark", "condense", "expand", "outline", "shadow", "emboss", "engrave", "smear", "blur", "blurrier", "blink", "shudder", "mark", "rumble"' })], 'Changer'),
      ]),
    m('hidden:', MacroCategory.Styling, MacroKind.Changer,
      'Makes the attached hook initially hidden.',
      [
        sig([], 'Changer'),
      ]),
    m('hide:', MacroCategory.Styling, MacroKind.Changer,
      'Hides the attached hook or named hook.',
      [
        sig([], 'Changer', 'Hides the attached hook.'),
        sig([arg('hookName', 'hookName', true, { description: 'Name of hook to hide' })], 'Changer'),
      ]),

    // ─── TRANSITIONS ────────────────────────────────────────────────
    m('transition:', MacroCategory.Styling, MacroKind.Changer,
      'Applies a transition animation to the attached hook.',
      [
        sig([arg('type', 'string', true, { description: 'Transition type: "dissolve", "fade", "rumble", "slide", "slideup", "slideleft", "slideright", "slidedown", "fadeup", "fadedown", "fadeleft", "faderight", "blur", "pulse", "zoom", "instant"' })], 'Changer'),
      ],
      { aliases: ['t8n:'] }),
    m('transition-delay:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the delay before a transition begins.',
      [
        sig([arg('seconds', 'number', true, { description: 'Delay in seconds' })], 'Changer'),
      ],
      { aliases: ['t8n-delay:'] }),
    m('transition-time:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the duration of a transition animation.',
      [
        sig([arg('seconds', 'number', true, { description: 'Duration in seconds' })], 'Changer'),
      ],
      { aliases: ['t8n-time:'] }),
    m('transition-depart:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the departure transition for navigation away from the current passage.',
      [
        sig([arg('type', 'string', true, { description: 'Departure transition type' })], 'Changer'),
      ],
      { aliases: ['t8n-depart:'] }),
    m('transition-arrive:', MacroCategory.Styling, MacroKind.Changer,
      'Sets the arrival transition for navigation into a new passage.',
      [
        sig([arg('type', 'string', true, { description: 'Arrival transition type' })], 'Changer'),
      ],
      { aliases: ['t8n-arrive:'] }),
    m('transition-skip:', MacroCategory.Styling, MacroKind.Changer,
      'Causes passage transitions to be instant (skipped) for the attached hook.',
      [
        sig([], 'Changer'),
      ],
      { aliases: ['t8n-skip:'] }),
    m('animate:', MacroCategory.Styling, MacroKind.Changer,
      'Applies a CSS animation to the attached hook.',
      [
        sig([arg('property', 'string', true, { description: 'CSS property to animate' }), arg('duration', 'string', true, { description: 'Animation duration' }), arg('easing', 'string', false, { description: 'CSS easing function' })], 'Changer'),
      ]),
  ];
}
