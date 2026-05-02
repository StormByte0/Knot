// ---------------------------------------------------------------------------
// Shared passage-arg macro definitions
//
// Single source of truth for which macros take a passage name argument and
// which argument index that is. Import from here rather than duplicating.
// ---------------------------------------------------------------------------

export const PASSAGE_ARG_MACROS = new Set([
  'link', 'button', 'linkappend', 'linkprepend', 'linkreplace',
  'include', 'display', 'goto', 'actions', 'click',
]);

// These macros take (label, passage) when 2 args are present
const LABEL_THEN_PASSAGE = new Set([
  'link', 'button', 'click', 'linkappend', 'linkprepend', 'linkreplace',
]);

export function passageArgIndex(macroName: string, argCount: number): number {
  return (LABEL_THEN_PASSAGE.has(macroName) && argCount >= 2) ? 1 : 0;
}

/** Extract passage name from a macro's passage argument, or null. */
export function passageNameFromExpr(expr: { type: string; kind?: string; value?: unknown }): string | null {
  if (expr.type === 'literal' && expr.kind === 'string') return String(expr.value);
  return null;
}