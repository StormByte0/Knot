// ---------------------------------------------------------------------------
// Shared passage-arg helpers — thin delegation to the active format adapter
//
// These functions accept an adapter and delegate to it.  They are kept as
// convenience re-exports so that callers that already have an adapter can
// use them without reaching into the adapter methods directly.
//
// For new code, prefer calling adapter.getPassageArgMacros() and
// adapter.getPassageArgIndex() directly.
// ---------------------------------------------------------------------------

import type { StoryFormatAdapter } from './formats/types';

/** Extract passage name from a macro's passage argument, or null. */
export function passageNameFromExpr(expr: { type: string; kind?: string; value?: unknown }): string | null {
  if (expr.type === 'literal' && expr.kind === 'string') return String(expr.value);
  return null;
}
