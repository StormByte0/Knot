/**
 * Knot v2 — Chapbook 2 Output & Embedding Inserts
 *
 * Inserts for output and embedding: embed passage, embed image,
 * embed url, alert, confirm, prompt.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { MacroDef } from '../_types';
import { insert, MacroCategory, MacroKind, arg, sig } from './inserts-helpers';

export const OUTPUT_INSERTS: MacroDef[] = [

  // ── Embedding & Transclusion ──────────────────────────────────────

  insert('embed passage', MacroCategory.Output, MacroKind.Command,
    'Embed another passage\'s content inline',
    [
      sig([arg('passage', 'string', true, { description: 'Name of the passage to embed' })], 'Command', 'Renders the content of the named passage inline at this location.'),
    ],
    { isInclude: true, passageArgPosition: 0 },
  ),

  insert('embed image', MacroCategory.Output, MacroKind.Command,
    'Embed an image',
    [
      sig([arg('image', 'string', true, { description: 'Image URL or data URI' }), arg('alt', 'string', false, { description: 'Alt text for the image' })], 'Command'),
    ],
  ),

  insert('embed url', MacroCategory.Output, MacroKind.Command,
    'Embed an external URL',
    [
      sig([arg('url', 'string', true, { description: 'The URL to embed' })], 'Command'),
    ],
  ),

  // ── Dialog Inserts ────────────────────────────────────────────────

  insert('alert', MacroCategory.Output, MacroKind.Command,
    'Show a browser alert dialog',
    [
      sig([arg('message', 'string', true, { description: 'Message to display in the alert dialog' })], 'Command', 'Shows a browser alert() dialog with the given message.'),
    ],
    { categoryDetail: 'dialog' },
  ),

  insert('confirm', MacroCategory.Output, MacroKind.Command,
    'Show a browser confirm dialog',
    [
      sig([arg('message', 'string', true, { description: 'Message to display in the confirm dialog' })], 'Command', 'Shows a browser confirm() dialog. Sets var.confirmResult to true or false.'),
    ],
    { categoryDetail: 'dialog', isAssignment: true },
  ),

  insert('prompt', MacroCategory.Output, MacroKind.Command,
    'Show a browser prompt dialog',
    [
      sig([arg('message', 'string', true, { description: 'Message to display in the prompt dialog' }), arg('default', 'string', false, { description: 'Default value for the prompt input' })], 'Command', 'Shows a browser prompt() dialog. Sets var.promptResult to the entered value.'),
    ],
    { categoryDetail: 'dialog', isAssignment: true },
  ),
];
