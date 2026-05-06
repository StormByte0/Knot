/**
 * Knot v2 — SugarCube 2 Misc Macros
 *
 * Widget definition, debug control, and deprecated macros.
 * widget / debug / click (deprecated) / autoupdate (deprecated)
 */

import type { MacroDef } from '../_types';
import { mc, sig, arg, MacroKind } from './macros-helpers';

export function getMiscMacros(): MacroDef[] {
  return [
    // ── Widget ───────────────────────────────────────────────────
    mc('widget', 'widget', MacroKind.Command,
      'Define a widget macro',
      [sig([arg('name', 'string', true, { description: 'Widget name' })])],
      { hasBody: true },
    ),

    // ── Debugging ────────────────────────────────────────────────
    mc('debug', 'debugging', MacroKind.Command,
      'Toggle debug view',
      [sig([])],
    ),

    // ── Deprecated ───────────────────────────────────────────────
    mc('click', 'interactive', MacroKind.Command,
      'Interactive click handler (deprecated — use <<link>>)',
      [sig([arg('target', 'string', true, { description: 'Passage name or selector' })])],
      { deprecated: true, deprecationMessage: 'Use <<link>> instead of <<click>>.' },
    ),
    mc('autoupdate', 'system', MacroKind.Command,
      'Auto-update content (deprecated)',
      [sig([arg('selector', 'string', true, { description: 'CSS selector' }), arg('interval', 'string', true, { description: 'Update interval' })])],
      { deprecated: true, deprecationMessage: 'Removed in SugarCube 2.37.0.' },
    ),
  ];
}
