import type { StoryFormatAdapter } from './types';
import { SugarCubeAdapter } from './sugarcube/adapter';
import { FallbackAdapter } from './fallback/adapter';

// ---------------------------------------------------------------------------
// FormatRegistry
//
// Resolves the active StoryFormatAdapter from a format id string.
//
// Resolution order:
//   1. Exact match on the adapter's canonical id (lower-cased).
//   2. Alias match — common alternate spellings (e.g. "sugarcube", "SugarCube 2").
//   3. Prefix match — "sugarcube-2.37.3" starts with "sugarcube-2".
//   4. FallbackAdapter (safe no-op) with a logged warning.
//
// StoryData format strings seen in the wild:
//   "SugarCube"            ← Twine 2 GUI default
//   "SugarCube 2"          ← some Twine versions
//   "sugarcube-2"          ← tweego canonical
//   "SugarCube-2.37.3"     ← tweego with version
//   "sugarcube-2.37.3"
// ---------------------------------------------------------------------------

const REGISTERED: StoryFormatAdapter[] = [
  new SugarCubeAdapter(),
  // Add new adapters here as they are implemented:
  // new ChapbookAdapter(),
  // new HarloweAdapter(),
];

const FALLBACK = new FallbackAdapter();

// Pre-build a lookup from canonical id → adapter for O(1) exact resolution.
const BY_ID = new Map<string, StoryFormatAdapter>(
  REGISTERED.map(a => [a.id.toLowerCase(), a]),
);

// Alias table: alternate names → canonical adapter id.
// Keys must be lower-cased.
const ALIASES = new Map<string, string>([
  // SugarCube variants
  ['sugarcube',      'sugarcube-2'],
  ['sugarcube 2',    'sugarcube-2'],
  ['sugarcube2',     'sugarcube-2'],
  ['sugar cube',     'sugarcube-2'],
  ['sugar cube 2',   'sugarcube-2'],
]);

/** Normalise a raw StoryData format string to a lower-cased lookup key. */
function normalise(raw: string): string {
  return raw.toLowerCase().trim()
    // Remove trailing version numbers after a space: "SugarCube 2.37" → "sugarcube"
    // but keep the dash-form "sugarcube-2.37" for prefix matching
    .replace(/\s+\d[\d.]*$/, '');
}

export const FormatRegistry = {
  /**
   * Resolve an adapter for the given format id string.
   * Returns FallbackAdapter for empty / unrecognised ids.
   */
  resolve(rawId: string): StoryFormatAdapter {
    if (!rawId) return FALLBACK;

    const norm = normalise(rawId);

    // 1. Exact match on canonical id
    const exact = BY_ID.get(norm);
    if (exact) return exact;

    // 2. Alias match
    const aliasTarget = ALIASES.get(norm);
    if (aliasTarget) {
      const aliased = BY_ID.get(aliasTarget);
      if (aliased) return aliased;
    }

    // 3. Prefix match (handles versioned ids like "sugarcube-2.37.3")
    for (const [registeredId, adapter] of BY_ID) {
      if (norm.startsWith(registeredId)) return adapter;
    }

    // 4. Unknown — fall through to safe no-op
    console.warn(
      `[FormatRegistry] Unknown story format "${rawId}" — using fallback adapter. ` +
      `Language features will be limited.`,
    );
    return FALLBACK;
  },

  /** All registered adapter ids, for diagnostics / UI. */
  registeredIds(): string[] {
    return REGISTERED.map(a => a.id);
  },
};