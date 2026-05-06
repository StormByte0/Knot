/**
 * Knot v2 — Chapbook 2 Modifier Inserts
 *
 * Modifier inserts (categoryDetail: 'modifier'): align center,
 * align left, align right, align justify, transition, fade-in,
 * fade-out, hidden.
 *
 * Chapbook 2 modifiers alter content sections using [modifier] syntax.
 * They are registered as MacroDef entries with categoryDetail: 'modifier'
 * so that the diagnostic engine can recognize them, even though they
 * use bracket syntax instead of brace syntax.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { MacroDef } from '../_types';
import { insert, MacroCategory, MacroKind, sig } from './inserts-helpers';

export const MODIFIER_INSERTS: MacroDef[] = [

  insert('align center', MacroCategory.Styling, MacroKind.Changer,
    'Center-align the following content',
    [
      sig([], 'Changer', 'Centers all content that follows until the next modifier or section end.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('align left', MacroCategory.Styling, MacroKind.Changer,
    'Left-align the following content',
    [
      sig([], 'Changer', 'Left-aligns all content that follows until the next modifier or section end.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('align right', MacroCategory.Styling, MacroKind.Changer,
    'Right-align the following content',
    [
      sig([], 'Changer', 'Right-aligns all content that follows until the next modifier or section end.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('align justify', MacroCategory.Styling, MacroKind.Changer,
    'Justify the following content',
    [
      sig([], 'Changer', 'Justifies all content that follows until the next modifier or section end.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('transition', MacroCategory.Styling, MacroKind.Changer,
    'Apply a transition effect to the following content',
    [
      sig([], 'Changer', 'Applies a transition effect to the following content section.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('fade-in', MacroCategory.Styling, MacroKind.Changer,
    'Fade in the following content',
    [
      sig([], 'Changer', 'Fades in the content that follows this modifier.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('fade-out', MacroCategory.Styling, MacroKind.Changer,
    'Fade out the following content',
    [
      sig([], 'Changer', 'Fades out the content that follows this modifier.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('hidden', MacroCategory.Styling, MacroKind.Changer,
    'Hide the following content initially',
    [
      sig([], 'Changer', 'Hides the content that follows this modifier. The content can be revealed later using a reveal insert.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),
];
