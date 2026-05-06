/**
 * Knot v2 — SugarCube 2 Output Macros
 *
 * Expression output, passage transclusion, JS execution, and text effects.
 * print / display / include / run / script / type
 *
 * Note: `embeddedLanguage` marks arguments that contain embedded JS/CSS/HTML.
 * The `embeddedBodyLanguage` concept (not yet on MacroDef) would mark
 * the body content of container macros like <<script>> as JavaScript.
 */

import type { MacroDef } from '../_types';
import { m, mc, sig, arg, MacroCategory, MacroKind } from './macros-helpers';

export function getOutputMacros(): MacroDef[] {
  return [
    m('print', MacroCategory.Output, MacroKind.Command,
      'Print the result of an expression',
      [sig([arg('expression', 'expression', true, { description: 'JavaScript expression', embeddedLanguage: 'javascript' })])],
      { aliases: ['='] },
    ),
    m('display', MacroCategory.Output, MacroKind.Command,
      'Render the content of another passage inline',
      [sig([arg('passage', 'string', true, { description: 'Passage name' })])],
      { isInclude: true, passageArgPosition: 0 },
    ),
    m('include', MacroCategory.Output, MacroKind.Command,
      'Include the content of another passage inline',
      [sig([arg('passage', 'string', true, { description: 'Passage name' })])],
      { isInclude: true, passageArgPosition: 0 },
    ),
    m('run', MacroCategory.System, MacroKind.Command,
      'Execute JavaScript code without output',
      [sig([arg('code', 'expression', true, { description: 'JavaScript code', embeddedLanguage: 'javascript' })])],
    ),
    m('script', MacroCategory.System, MacroKind.Changer,
      'Execute JavaScript code within a body block (<<script>>..code..<</script>>)',
      [sig([])],
      { hasBody: true },
      // The body of <<script>> is JavaScript — this is a context-dependent
      // embedded language switch. The semantic analyzer should treat the
      // body content as JS tokens when the enclosing macro is <<script>>.
    ),
    m('type', MacroCategory.Output, MacroKind.Command,
      'Type out text with a delay',
      [sig([arg('text', 'string', true, { description: 'Text to type' }), arg('speed', 'expression', false, { description: 'Typing speed in ms' })])],
    ),
  ];
}
