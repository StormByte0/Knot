/**
 * Knot v2 — Format Registry
 *
 * Discovers, loads, and resolves FormatModule instances.
 * This is the ONLY module that directly imports from format-specific directories.
 *
 * Format detection uses the module's identity fields (formatId, displayName, aliases)
 * instead of hardcoded string matching — formats self-identify.
 *
 * Resolution strategy:
 *   1. Exact match on formatId
 *   2. Match on aliases
 *   3. Prefix match on formatId (for versioned IDs like 'sugarcube-2.37.3')
 *   4. Fall back to fallback module
 *
 * Adding a new format:
 *   1. Create formats/<name>/index.ts implementing FormatModule
 *   2. Split into sub-files: lexer.ts, macros-*.ts, runtime.ts, snippets.ts, etc.
 *   3. Export a `FormatModule` object from index.ts
 *   4. Add a lazy loader entry in BUILTIN_LOADERS below
 *   5. That's it — no other files need modification
 */

import type { FormatModule } from './_types';

import { fallbackModule } from './fallback/index';

// ─── Lazy Loader Type ───────────────────────────────────────────

type FormatLoader = () => Promise<FormatModule> | FormatModule;

// ─── Built-in Format Loaders ────────────────────────────────────
// Lazy — formats are only loaded when needed.
// Sync for built-ins (they're always available), but the type
// supports async for future external format loading.

const BUILTIN_LOADERS: Map<string, FormatLoader> = new Map([
  ['fallback', () => fallbackModule],
  ['sugarcube-2', () => {
    const { sugarcubeModule } = require('./sugarcube/index');
    return sugarcubeModule;
  }],
  ['harlowe-3', () => {
    const { harloweModule } = require('./harlowe/index');
    return harloweModule;
  }],
  ['chapbook-2', () => {
    const { chapbookModule } = require('./chapbook/index');
    return chapbookModule;
  }],
  ['snowman-2', () => {
    const { snowmanModule } = require('./snowman/index');
    return snowmanModule;
  }],
]);

// ─── Registry ───────────────────────────────────────────────────

export class FormatRegistry {
  /** Loaded and cached format modules, keyed by formatId */
  private loaded: Map<string, FormatModule> = new Map();

  /** Pre-built alias → formatId lookup for O(1) resolution */
  private aliasIndex: Map<string, string> = new Map();

  /** Currently active format module */
  private activeFormat: FormatModule = fallbackModule;

  constructor() {
    // Pre-load the fallback (always needed)
    this.loaded.set('fallback', fallbackModule);
    this.indexAliases(fallbackModule);
  }

  /**
   * Pre-load all built-in formats.
   * Called once during server initialization.
   * For production, you could make this lazy instead.
   */
  loadBuiltinFormats(): void {
    for (const [formatId, loader] of BUILTIN_LOADERS) {
      if (!this.loaded.has(formatId)) {
        const mod = loader();
        // Handle both sync and async loaders
        if (mod instanceof Promise) {
          mod.then(m => {
            this.loaded.set(formatId, m);
            this.indexAliases(m);
          });
        } else {
          this.loaded.set(formatId, mod);
          this.indexAliases(mod);
        }
      }
    }
  }

  /**
   * Resolve a format by raw identifier string.
   * Uses the module's identity fields for matching:
   *   1. Exact match on formatId (case-insensitive)
   *   2. Match on aliases (case-insensitive)
   *   3. Prefix match on formatId (for versioned IDs)
   *   4. Fall back to fallback module
   */
  resolve(rawId: string): FormatModule {
    if (!rawId) return fallbackModule;

    const norm = rawId.toLowerCase().trim();

    // 1. Exact match on formatId
    const exact = this.loaded.get(norm);
    if (exact) return exact;

    // Also try the norm as a formatId directly (some formats have versioned IDs)
    for (const [id, mod] of this.loaded) {
      if (id.toLowerCase() === norm) return mod;
    }

    // 2. Alias match
    const aliasTarget = this.aliasIndex.get(norm);
    if (aliasTarget) {
      const mod = this.loaded.get(aliasTarget);
      if (mod) return mod;
    }

    // 3. Prefix match (e.g. 'sugarcube' matches 'sugarcube-2')
    for (const [id, mod] of this.loaded) {
      if (norm.startsWith(id) || id.startsWith(norm)) return mod;
    }

    // 4. No match — fallback
    console.warn(`[FormatRegistry] Unknown story format "${rawId}" — using fallback`);
    return fallbackModule;
  }

  /**
   * Auto-detect the story format from a StoryData passage content.
   * Reads the JSON content to extract the format name and version,
   * then resolves against registered formats.
   *
   * Returns the resolved FormatModule, or the fallback if no match.
   */
  detectFromStoryData(storyDataContent: string): FormatModule {
    try {
      const data = JSON.parse(storyDataContent.trim());
      const formatName = data.format ?? '';
      const formatVersion = data['format-version'] ?? '';

      if (!formatName) return fallbackModule;

      // Try to resolve using the format name directly
      const resolved = this.resolve(formatName);
      if (resolved.formatId !== 'fallback') {
        return resolved;
      }

      // Try with version appended (e.g. "Harlowe 3.3.8" → "harlowe-3")
      if (formatVersion) {
        const majorVersion = formatVersion.split('.')[0];
        const withVersion = `${formatName}-${majorVersion}`;
        const resolved2 = this.resolve(withVersion);
        if (resolved2.formatId !== 'fallback') {
          return resolved2;
        }
      }

      return fallbackModule;
    } catch {
      return fallbackModule;
    }
  }

  /**
   * Set the active format by ID.
   * When undefined, falls back to the fallback module (basic Twee).
   *
   * IMPORTANT: We do NOT default to any specific format (SugarCube, Harlowe, etc.)
   * because that would bake format assumptions into the core — exactly the
   * problem the v2 redesign solves. Instead:
   *   - Fallback provides basic Twee features ([[links]], passage navigation)
   *   - Format auto-detection from StoryData switches to the real format
   *   - Heuristic detection (<< >>, (macro:), etc.) catches remaining cases
   *   - User can manually select via "Knot: Select Story Format"
   */
  setActiveFormat(formatId: string | undefined): void {
    if (formatId) {
      this.activeFormat = this.resolve(formatId);
    } else {
      this.activeFormat = fallbackModule;
    }
  }

  /**
   * Set the active format directly from a loaded module.
   */
  setActiveFormatModule(mod: FormatModule): void {
    this.activeFormat = mod;
    // Ensure it's in the loaded map
    if (!this.loaded.has(mod.formatId)) {
      this.loaded.set(mod.formatId, mod);
      this.indexAliases(mod);
    }
  }

  /**
   * Get the currently active format module.
   */
  getActiveFormat(): FormatModule {
    return this.activeFormat;
  }

  /**
   * Get a loaded format module by ID.
   */
  getFormat(formatId: string): FormatModule | undefined {
    return this.loaded.get(formatId);
  }

  /**
   * Get all loaded format IDs.
   */
  getAvailableFormatIds(): string[] {
    return Array.from(this.loaded.keys());
  }

  /**
   * Register a format module.
   * Adds it to the loaded map and indexes its aliases.
   * Does NOT change the active format.
   */
  register(mod: FormatModule): void {
    this.loaded.set(mod.formatId, mod);
    this.indexAliases(mod);
  }

  /**
   * Unregister a format module by formatId.
   * Removes it from the loaded map and cleans up alias entries.
   * If the removed format was the active format, falls back to fallback.
   */
  unregister(formatId: string): void {
    const mod = this.loaded.get(formatId);
    if (!mod) return;

    // Clean up alias entries that point to this formatId
    for (const [alias, targetId] of this.aliasIndex) {
      if (targetId === formatId) {
        this.aliasIndex.delete(alias);
      }
    }

    this.loaded.delete(formatId);

    // If the active format was removed, fall back
    if (this.activeFormat.formatId === formatId) {
      this.activeFormat = fallbackModule;
    }
  }

  /**
   * Check if a format is loaded.
   */
  hasFormat(formatId: string): boolean {
    return this.loaded.has(formatId);
  }

  /**
   * Heuristic format detection from file content samples.
   *
   * Scans text samples against each loaded format's `macroPattern` regex.
   * The format with the most matches wins. This is a LAST RESORT when
   * StoryData is missing or malformed — it uses each format's self-declared
   * pattern, so the registry never hardcodes format-specific detection logic.
   *
   * Formats without a `macroPattern` (null) don't participate in
   * heuristic detection. Those formats must rely on StoryData or
   * manual user selection.
   *
   * @param texts  Array of file content strings to scan (typically the
   *               first few files from the workspace).
   * @returns The formatId of the best-matching format, or undefined.
   */
  detectFromHeuristic(texts: readonly string[]): string | undefined {
    const scores = new Map<string, number>();

    for (const text of texts) {
      for (const [formatId, format] of this.loaded) {
        // Skip fallback — it has no macro syntax to detect
        if (formatId === 'fallback') continue;
        // Skip formats without a detection pattern
        if (!format.macroPattern) continue;

        // Reset the global regex before each test
        format.macroPattern.lastIndex = 0;
        if (format.macroPattern.test(text)) {
          scores.set(formatId, (scores.get(formatId) ?? 0) + 1);
        }
      }
    }

    // Return the format with the most hits
    let bestFormat: string | undefined;
    let bestScore = 0;
    for (const [formatId, score] of scores) {
      if (score > bestScore) {
        bestScore = score;
        bestFormat = formatId;
      }
    }

    return bestFormat;
  }

  // ─── Private Helpers ────────────────────────────────────────

  private indexAliases(mod: FormatModule): void {
    // Index the formatId itself (normalized)
    this.aliasIndex.set(mod.formatId.toLowerCase(), mod.formatId);

    // Index the display name
    this.aliasIndex.set(mod.displayName.toLowerCase(), mod.formatId);

    // Index all aliases
    for (const alias of mod.aliases) {
      this.aliasIndex.set(alias.toLowerCase(), mod.formatId);
    }
  }
}
