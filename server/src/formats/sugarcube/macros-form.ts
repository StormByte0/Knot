/**
 * Knot v2 — SugarCube 2 Form Input Macros
 *
 * User input elements bound to story or temporary variables.
 * textbox / textbox2 / numberbox / numberbox2 / textarea / textarea2 /
 * checkbox / radiobutton / listbox / option / optiondisabled /
 * dropdown / input
 */

import type { MacroDef } from '../_types';
import { mc, sig, arg, MacroKind } from './macros-helpers';

export function getFormMacros(): MacroDef[] {
  return [
    mc('textbox', 'form', MacroKind.Command,
      'Render a single-line text input bound to a story variable',
      [sig([arg('variable', 'variable', true, { description: 'Story variable to bind ($var)' }), arg('default', 'string', false, { description: 'Default value displayed in the input' }), arg('passage', 'passage-ref', false, { description: 'Passage to navigate to on Enter key' })])],
      { isAssignment: true },
    ),
    mc('textbox2', 'form', MacroKind.Command,
      'Render a single-line text input bound to a temporary variable',
      [sig([arg('variable', 'variable', true, { description: 'Temporary variable to bind (_var)' }), arg('default', 'string', false, { description: 'Default value displayed in the input' }), arg('passage', 'passage-ref', false, { description: 'Passage to navigate to on Enter key' })])],
      { isAssignment: true },
    ),
    mc('numberbox', 'form', MacroKind.Command,
      'Render a number input bound to a story variable',
      [sig([arg('variable', 'variable', true, { description: 'Story variable to bind ($var)' }), arg('default', 'string', false, { description: 'Default numeric value' }), arg('passage', 'passage-ref', false, { description: 'Passage to navigate to on Enter key' })])],
      { isAssignment: true },
    ),
    mc('numberbox2', 'form', MacroKind.Command,
      'Render a number input bound to a temporary variable',
      [sig([arg('variable', 'variable', true, { description: 'Temporary variable to bind (_var)' }), arg('default', 'string', false, { description: 'Default numeric value' }), arg('passage', 'passage-ref', false, { description: 'Passage to navigate to on Enter key' })])],
      { isAssignment: true },
    ),
    mc('textarea', 'form', MacroKind.Command,
      'Render a multi-line text input bound to a story variable',
      [sig([arg('variable', 'variable', true, { description: 'Story variable to bind ($var)' }), arg('default', 'string', false, { description: 'Default value displayed in the textarea' })])],
      { isAssignment: true },
    ),
    mc('textarea2', 'form', MacroKind.Command,
      'Render a multi-line text input bound to a temporary variable',
      [sig([arg('variable', 'variable', true, { description: 'Temporary variable to bind (_var)' }), arg('default', 'string', false, { description: 'Default value displayed in the textarea' })])],
      { isAssignment: true },
    ),
    mc('checkbox', 'form', MacroKind.Command,
      'Render a checkbox input bound to a story variable',
      [sig([arg('variable', 'variable', true, { description: 'Story variable to bind ($var)' }), arg('value', 'string', true, { description: 'Value stored when checked' }), arg('checked', 'expression', false, { description: 'Whether initially checked (default: false)' })])],
      { isAssignment: true },
    ),
    mc('radiobutton', 'form', MacroKind.Command,
      'Render a radio button input bound to a story variable',
      [sig([arg('variable', 'variable', true, { description: 'Story variable to bind ($var)' }), arg('value', 'string', true, { description: 'Value stored when selected' }), arg('checked', 'expression', false, { description: 'Whether initially selected (default: false)' })])],
      { isAssignment: true },
    ),
    mc('listbox', 'form', MacroKind.Changer,
      'Render a listbox (select) input bound to a story variable',
      [sig([arg('variable', 'variable', true, { description: 'Story variable to bind ($var)' })])],
      { hasBody: true, children: ['option', 'optiondisabled'], isAssignment: true },
    ),
    mc('option', 'form', MacroKind.Command,
      'Add an option to a <<listbox>>',
      [sig([arg('value', 'string', true, { description: 'Option display value' }), arg('storedValue', 'string', false, { description: 'Value stored in variable (defaults to display value)' })])],
      { parents: ['listbox'] },
    ),
    mc('optiondisabled', 'form', MacroKind.Command,
      'Add a disabled option to a <<listbox>>',
      [sig([arg('value', 'string', true, { description: 'Option display value' })])],
      { parents: ['listbox'] },
    ),
    mc('dropdown', 'form', MacroKind.Command,
      'Render a dropdown select input bound to a story variable (SugarCube 2.37+)',
      [sig([arg('variable', 'variable', true, { description: 'Story variable to bind ($var)' }), arg('options', 'string', true, { description: 'Comma-separated option values' })])],
      { isAssignment: true },
    ),
    mc('input', 'form', MacroKind.Command,
      'Render an HTML input element (SugarCube 2.37+)',
      [sig([arg('type', 'string', true, { description: 'HTML input type (text, number, email, etc.)' }), arg('name', 'string', true, { description: 'Input name / variable binding' }), arg('value', 'any', false, { description: 'Default value for the input' })])],
      { isAssignment: true },
    ),
  ];
}
