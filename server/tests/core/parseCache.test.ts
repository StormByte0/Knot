import { strict as assert } from 'assert';
import { ParseCache, ParsedFile } from '../../src/parseCache';
import { parseDocument } from '../../src/parser';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Create a ParsedFile by parsing a simple passage document. */
function makeParsedFile(text: string = ':: Start\nHello'): ParsedFile {
  const result = parseDocument(text);
  return { ast: result.ast, diagnostics: result.diagnostics };
}

/** Create a unique URI for cache entries. */
function uri(index: number): string {
  return `file:///doc${index}.twee`;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('ParseCache', () => {
  let cache: ParseCache;

  beforeEach(() => {
    cache = new ParseCache();
  });

  // ---- Basic operations ----------------------------------------------------

  describe('Basic operations', () => {
    it('set / get — basic round-trip', () => {
      const parsed = makeParsedFile(':: Start\nHello');
      cache.set('file:///a.twee', parsed);

      const result = cache.get('file:///a.twee');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.ast.type, 'document');
      assert.strictEqual(result!.ast.passages.length, 1);
      assert.strictEqual(result!.ast.passages[0]!.name, 'Start');
    });

    it('get returns undefined for unknown URI', () => {
      assert.strictEqual(cache.get('file:///nonexistent.twee'), undefined);
    });

    it('has returns true for cached, false for unknown', () => {
      cache.set('file:///a.twee', makeParsedFile());

      assert.strictEqual(cache.has('file:///a.twee'), true);
      assert.strictEqual(cache.has('file:///nonexistent.twee'), false);
    });

    it('delete removes entry and returns true', () => {
      cache.set('file:///a.twee', makeParsedFile());
      const result = cache.delete('file:///a.twee');

      assert.strictEqual(result, true);
      assert.strictEqual(cache.has('file:///a.twee'), false);
      assert.strictEqual(cache.get('file:///a.twee'), undefined);
    });

    it('size returns the number of cached files', () => {
      assert.strictEqual(cache.size, 0);

      cache.set('file:///a.twee', makeParsedFile());
      assert.strictEqual(cache.size, 1);

      cache.set('file:///b.twee', makeParsedFile());
      assert.strictEqual(cache.size, 2);

      cache.delete('file:///a.twee');
      assert.strictEqual(cache.size, 1);
    });

    it('keys returns sorted URIs', () => {
      cache.set('file:///c.twee', makeParsedFile());
      cache.set('file:///a.twee', makeParsedFile());
      cache.set('file:///b.twee', makeParsedFile());

      const keys = cache.keys();
      assert.deepStrictEqual(keys, ['file:///a.twee', 'file:///b.twee', 'file:///c.twee']);
    });

    it('entries returns iterable of [uri, ParsedFile] pairs', () => {
      cache.set('file:///a.twee', makeParsedFile(':: A\nContentA'));
      cache.set('file:///b.twee', makeParsedFile(':: B\nContentB'));

      const entries = [...cache.entries()];
      assert.strictEqual(entries.length, 2);

      const entryMap = new Map(entries);
      assert.ok(entryMap.has('file:///a.twee'));
      assert.ok(entryMap.has('file:///b.twee'));
      assert.strictEqual(entryMap.get('file:///a.twee')!.ast.passages[0]!.name, 'A');
      assert.strictEqual(entryMap.get('file:///b.twee')!.ast.passages[0]!.name, 'B');
    });
  });

  // ---- LRU eviction --------------------------------------------------------

  describe('LRU eviction', () => {
    it('evictIfNeeded respects max cache size (500)', () => {
      // Fill cache with 502 entries — should evict 2 oldest
      for (let i = 0; i < 502; i++) {
        cache.set(uri(i), makeParsedFile(`:: P${i}\nContent${i}`));
      }

      assert.strictEqual(cache.size, 502);

      // Evict with no analyzed URIs — all are candidates
      cache.evictIfNeeded(new Set());

      assert.strictEqual(cache.size, 500);

      // The oldest two (uri(0) and uri(1)) should have been evicted
      assert.strictEqual(cache.has(uri(0)), false);
      assert.strictEqual(cache.has(uri(1)), false);

      // The newest entries should still be present
      assert.strictEqual(cache.has(uri(501)), true);
      assert.strictEqual(cache.has(uri(500)), true);
    });

    it('evictIfNeeded skips analyzed URIs', () => {
      // Fill cache with 502 entries
      for (let i = 0; i < 502; i++) {
        cache.set(uri(i), makeParsedFile(`:: P${i}\nContent${i}`));
      }

      // Mark uri(0) as analyzed — it should not be evicted
      const analyzedUris = new Set([uri(0)]);

      cache.evictIfNeeded(analyzedUris);

      assert.strictEqual(cache.size, 500);

      // uri(0) should be preserved because it's analyzed
      assert.strictEqual(cache.has(uri(0)), true);

      // uri(1) (the next oldest, not analyzed) should be evicted instead
      assert.strictEqual(cache.has(uri(1)), false);

      // Newest entries still present
      assert.strictEqual(cache.has(uri(501)), true);
    });

    it('evictIfNeeded stops when all remaining entries are analyzed', () => {
      // Fill cache with 502 entries
      for (let i = 0; i < 502; i++) {
        cache.set(uri(i), makeParsedFile(`:: P${i}\nContent${i}`));
      }

      // Mark ALL URIs as analyzed — nothing should be evicted
      const allUris = new Set<string>();
      for (let i = 0; i < 502; i++) {
        allUris.add(uri(i));
      }

      cache.evictIfNeeded(allUris);

      // Size remains 502 because nothing could be evicted
      assert.strictEqual(cache.size, 502);
    });

    it('evictIfNeeded does nothing when cache is under limit', () => {
      for (let i = 0; i < 10; i++) {
        cache.set(uri(i), makeParsedFile());
      }

      cache.evictIfNeeded(new Set());

      assert.strictEqual(cache.size, 10);
      // All entries should still be present
      for (let i = 0; i < 10; i++) {
        assert.strictEqual(cache.has(uri(i)), true);
      }
    });
  });

  // ---- Delete non-existent -------------------------------------------------

  describe('Delete non-existent', () => {
    it('delete returns false for non-existent URI', () => {
      assert.strictEqual(cache.delete('file:///nonexistent.twee'), false);
    });

    it('delete returns true for existing URI', () => {
      cache.set('file:///a.twee', makeParsedFile());
      assert.strictEqual(cache.delete('file:///a.twee'), true);
    });

    it('deleting same entry twice returns true then false', () => {
      cache.set('file:///a.twee', makeParsedFile());
      assert.strictEqual(cache.delete('file:///a.twee'), true);
      assert.strictEqual(cache.delete('file:///a.twee'), false);
    });
  });

  // ---- Keys sorted ---------------------------------------------------------

  describe('Keys sorted', () => {
    it('keys() returns sorted URIs regardless of insertion order', () => {
      cache.set('file:///z.twee', makeParsedFile());
      cache.set('file:///a.twee', makeParsedFile());
      cache.set('file:///m.twee', makeParsedFile());

      const keys = cache.keys();
      assert.deepStrictEqual(keys, ['file:///a.twee', 'file:///m.twee', 'file:///z.twee']);
    });

    it('keys() returns empty array for empty cache', () => {
      assert.deepStrictEqual(cache.keys(), []);
    });

    it('keys() reflects deletions', () => {
      cache.set('file:///a.twee', makeParsedFile());
      cache.set('file:///b.twee', makeParsedFile());
      cache.set('file:///c.twee', makeParsedFile());
      cache.delete('file:///b.twee');

      const keys = cache.keys();
      assert.deepStrictEqual(keys, ['file:///a.twee', 'file:///c.twee']);
    });
  });

  // ---- Lifecycle -----------------------------------------------------------

  describe('Lifecycle — clear()', () => {
    it('clear() removes all entries', () => {
      cache.set('file:///a.twee', makeParsedFile());
      cache.set('file:///b.twee', makeParsedFile());
      cache.clear();

      assert.strictEqual(cache.size, 0);
      assert.strictEqual(cache.has('file:///a.twee'), false);
      assert.strictEqual(cache.has('file:///b.twee'), false);
    });

    it('clear() makes keys() return empty array', () => {
      cache.set('file:///a.twee', makeParsedFile());
      cache.clear();

      assert.deepStrictEqual(cache.keys(), []);
    });

    it('cache is usable after clear()', () => {
      cache.set('file:///a.twee', makeParsedFile());
      cache.clear();

      cache.set('file:///b.twee', makeParsedFile(':: New\nContent'));
      assert.strictEqual(cache.size, 1);
      assert.strictEqual(cache.has('file:///b.twee'), true);

      const result = cache.get('file:///b.twee');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.ast.passages[0]!.name, 'New');
    });
  });

  // ---- Overwrite -----------------------------------------------------------

  describe('Overwrite', () => {
    it('set with same URI replaces entry', () => {
      const parsed1 = makeParsedFile(':: First\nContent1');
      const parsed2 = makeParsedFile(':: Second\nContent2');

      cache.set('file:///a.twee', parsed1);
      cache.set('file:///a.twee', parsed2);

      assert.strictEqual(cache.size, 1);
      const result = cache.get('file:///a.twee');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.ast.passages[0]!.name, 'Second');
    });

    it('overwriting does not increase size', () => {
      cache.set('file:///a.twee', makeParsedFile(':: A1\nC1'));
      cache.set('file:///a.twee', makeParsedFile(':: A2\nC2'));
      cache.set('file:///a.twee', makeParsedFile(':: A3\nC3'));

      assert.strictEqual(cache.size, 1);
    });

    it('overwriting updates access order for LRU', () => {
      // Fill to just under limit
      for (let i = 0; i < 499; i++) {
        cache.set(uri(i), makeParsedFile());
      }
      // Add two more — uri(0) is the oldest
      cache.set(uri(499), makeParsedFile());

      // Re-access uri(0) to move it to the end of LRU
      cache.set(uri(0), makeParsedFile(':: Updated\nNew'));

      // Add one more to exceed limit
      cache.set(uri(500), makeParsedFile());

      // Now evict — uri(1) should be evicted instead of uri(0)
      cache.evictIfNeeded(new Set());

      assert.strictEqual(cache.size, 500);
      // uri(0) was re-accessed so it should survive
      assert.strictEqual(cache.has(uri(0)), true);
      // uri(1) is now the oldest non-re-accessed entry
      assert.strictEqual(cache.has(uri(1)), false);
    });
  });
});
