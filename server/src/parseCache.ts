import type { ParseDiagnostic } from './ast';
import type { DocumentNode } from './ast';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface ParsedFile {
  ast: DocumentNode;
  diagnostics: ParseDiagnostic[];
}

// ---------------------------------------------------------------------------
// ParseCache — stores parsed files with LRU eviction
// ---------------------------------------------------------------------------

/** Maximum number of files to keep in the parse cache. */
const MAX_CACHED_FILES = 500;

export class ParseCache {
  private cache = new Map<string, ParsedFile>();
  /** Access order for LRU eviction — most-recently-used at the end. */
  private accessOrder: string[] = [];
  private readonly maxCachedFiles = MAX_CACHED_FILES;

  // ---- Basic operations ----------------------------------------------------

  set(uri: string, parsed: ParsedFile): void {
    this.cache.set(uri, parsed);
    // Update access order for LRU
    const idx = this.accessOrder.indexOf(uri);
    if (idx !== -1) this.accessOrder.splice(idx, 1);
    this.accessOrder.push(uri);
  }

  get(uri: string): ParsedFile | undefined {
    return this.cache.get(uri);
  }

  delete(uri: string): boolean {
    const existed = this.cache.delete(uri);
    if (existed) {
      const idx = this.accessOrder.indexOf(uri);
      if (idx !== -1) this.accessOrder.splice(idx, 1);
    }
    return existed;
  }

  has(uri: string): boolean {
    return this.cache.has(uri);
  }

  /** Returns sorted URIs (stable ordering for iteration). */
  keys(): string[] {
    return [...this.cache.keys()].sort();
  }

  /** Number of cached files. */
  get size(): number {
    return this.cache.size;
  }

  /** Iterate over cache entries. */
  entries(): IterableIterator<[string, ParsedFile]> {
    return this.cache.entries();
  }

  // ---- LRU eviction --------------------------------------------------------

  /**
   * Evict oldest entries when the parse cache exceeds the limit.
   * Only evicts files not currently in the derived analysis — those are
   * likely to be needed again soon.
   */
  evictIfNeeded(analyzedUris: Set<string>): void {
    while (this.cache.size > this.maxCachedFiles && this.accessOrder.length > 0) {
      const oldest = this.accessOrder[0]!;
      // Don't evict files that have active analysis results
      if (analyzedUris.has(oldest)) {
        // Skip to next — move to end so we try other candidates
        this.accessOrder.shift();
        this.accessOrder.push(oldest);
        // Safety: if all files have analysis, stop evicting
        if (this.accessOrder.every(u => analyzedUris.has(u))) break;
        continue;
      }
      this.cache.delete(oldest);
      this.accessOrder.shift();
    }
  }

  // ---- Lifecycle -----------------------------------------------------------

  clear(): void {
    this.cache.clear();
    this.accessOrder.length = 0;
  }
}
