import { strict as assert } from 'assert';
import { IncrementalParser } from '../../src/incrementalParser';
import { getSugarCubeAdapter } from '../helpers/testFixtures';
import type { StoryFormatAdapter } from '../../src/formats/types';

// ── Helpers ──────────────────────────────────────────────────────────────────

function makeSimpleDoc(name: string, body: string): string {
  return `:: ${name}\n${body}`;
}

function makeTwoPassageDoc(name1: string, body1: string, name2: string, body2: string): string {
  return `:: ${name1}\n${body1}\n\n:: ${name2}\n${body2}`;
}

// ── Tests ────────────────────────────────────────────────────────────────────

describe('IncrementalParser', () => {
  describe('parse', () => {
    it('should parse a simple document and return correct AST with passages', () => {
      const parser = new IncrementalParser();
      const text = makeSimpleDoc('Start', 'Hello world');
      const result = parser.parse('file:///a.twee', text);

      assert.strictEqual(result.ast.type, 'document');
      assert.strictEqual(result.ast.passages.length, 1);
      assert.strictEqual(result.ast.passages[0]!.name, 'Start');
    });

    it('should return diagnostics from parsing', () => {
      const parser = new IncrementalParser();
      // Create a document with a parsing issue
      const result = parser.parse('file:///a.twee', ':: Start\nHello');

      assert.ok(Array.isArray(result.diagnostics));
    });

    it('should handle empty document', () => {
      const parser = new IncrementalParser();
      const result = parser.parse('file:///a.twee', '');

      assert.strictEqual(result.ast.type, 'document');
      assert.strictEqual(result.ast.passages.length, 0);
      assert.strictEqual(result.diagnostics.length, 0);
    });

    it('should handle multiple passages', () => {
      const parser = new IncrementalParser();
      const text = makeTwoPassageDoc('Start', 'Hello', 'End', 'Goodbye');
      const result = parser.parse('file:///a.twee', text);

      assert.strictEqual(result.ast.passages.length, 2);
      assert.strictEqual(result.ast.passages[0]!.name, 'Start');
      assert.strictEqual(result.ast.passages[1]!.name, 'End');
    });

    it('should return AST with correct document range', () => {
      const parser = new IncrementalParser();
      const text = makeSimpleDoc('Start', 'Hello');
      const result = parser.parse('file:///a.twee', text);

      assert.strictEqual(result.ast.range.start, 0);
      assert.strictEqual(result.ast.range.end, text.length);
    });

    it('should parse with SugarCube adapter', () => {
      const parser = new IncrementalParser();
      const adapter = getSugarCubeAdapter();
      const text = makeSimpleDoc('Start', '<<set $x to 5>>');
      const result = parser.parse('file:///a.twee', text, adapter);

      assert.strictEqual(result.ast.passages.length, 1);
      assert.strictEqual(result.ast.passages[0]!.name, 'Start');
    });
  });

  describe('Caching behavior', () => {
    it('should cache parsed passages: second parse with same body reuses cache', () => {
      const parser = new IncrementalParser();
      const text = makeSimpleDoc('Start', 'Hello world');

      // First parse — populates cache
      const result1 = parser.parse('file:///a.twee', text);
      assert.strictEqual(result1.ast.passages.length, 1);

      // Cache should have 1 entry
      assert.strictEqual(parser.cacheSize, 1);

      // Second parse with identical text should reuse cache
      const result2 = parser.parse('file:///a.twee', text);
      assert.strictEqual(result2.ast.passages.length, 1);
      assert.strictEqual(result2.ast.passages[0]!.name, 'Start');
    });

    it('should update cache when passage body changes', () => {
      const parser = new IncrementalParser();
      const text1 = makeSimpleDoc('Start', 'Hello');
      const text2 = makeSimpleDoc('Start', 'Goodbye');

      parser.parse('file:///a.twee', text1);
      const result2 = parser.parse('file:///a.twee', text2);

      // Should still parse correctly with updated content
      assert.strictEqual(result2.ast.passages.length, 1);
      assert.strictEqual(result2.ast.passages[0]!.name, 'Start');
    });

    it('should cache multiple passages independently', () => {
      const parser = new IncrementalParser();
      const text = makeTwoPassageDoc('Start', 'Hello', 'End', 'Goodbye');

      parser.parse('file:///a.twee', text);
      assert.strictEqual(parser.cacheSize, 2);
    });
  });

  describe('Cache eviction', () => {
    it('should evict stale entries when passage is removed', () => {
      const parser = new IncrementalParser();
      const twoPassages = makeTwoPassageDoc('Start', 'Hello', 'End', 'Goodbye');
      const onePassage = makeSimpleDoc('Start', 'Hello');

      parser.parse('file:///a.twee', twoPassages);
      assert.strictEqual(parser.cacheSize, 2);

      // Remove the second passage
      parser.parse('file:///a.twee', onePassage);
      assert.strictEqual(parser.cacheSize, 1);
    });

    it('should evict entries when all passages removed (empty document)', () => {
      const parser = new IncrementalParser();
      const text = makeSimpleDoc('Start', 'Hello');

      parser.parse('file:///a.twee', text);
      assert.strictEqual(parser.cacheSize, 1);

      parser.parse('file:///a.twee', '');
      assert.strictEqual(parser.cacheSize, 0);
    });

    it('should not evict entries from a different URI', () => {
      const parser = new IncrementalParser();
      const text = makeSimpleDoc('Start', 'Hello');

      // Parse for URI A
      parser.parse('file:///a.twee', text);
      assert.strictEqual(parser.cacheSize, 1);

      // Parse empty for URI B — should not affect URI A's cache
      parser.parse('file:///b.twee', '');
      assert.strictEqual(parser.cacheSize, 1);
    });
  });

  describe('Passage moves / range shifts', () => {
    it('should detect passage moves and shift ranges', () => {
      const parser = new IncrementalParser();

      // First, parse with original offsets
      const text1 = ':: Start\nHello';
      const result1 = parser.parse('file:///a.twee', text1);
      const originalRange = result1.ast.passages[0]!.range;

      // Now add text before the passage, shifting its offset
      const text2 = ':: Intro\nPreamble\n\n:: Start\nHello';
      const result2 = parser.parse('file:///a.twee', text2);

      // "Start" passage should now be at a different offset
      const shiftedPassage = result2.ast.passages.find(p => p.name === 'Start');
      assert.ok(shiftedPassage !== undefined);
      assert.ok(shiftedPassage.range.start > originalRange.start, 'Expected shifted start offset');
    });

    it('should reuse cache when passage body is the same but offset changes', () => {
      const parser = new IncrementalParser();

      // Parse a single passage
      parser.parse('file:///a.twee', ':: Start\nHello');
      assert.strictEqual(parser.cacheSize, 1);

      // Add a prefix passage — "Start" moves but body text is same
      parser.parse('file:///a.twee', ':: Intro\nPre\n\n:: Start\nHello');

      // Both passages should now be cached
      assert.strictEqual(parser.cacheSize, 2);
    });
  });

  describe('evictUri', () => {
    it('should remove all cached passages for a URI', () => {
      const parser = new IncrementalParser();
      const text = makeTwoPassageDoc('Start', 'Hello', 'End', 'Goodbye');

      parser.parse('file:///a.twee', text);
      assert.strictEqual(parser.cacheSize, 2);

      parser.evictUri('file:///a.twee');
      assert.strictEqual(parser.cacheSize, 0);
    });

    it('should not affect other URIs', () => {
      const parser = new IncrementalParser();

      parser.parse('file:///a.twee', makeSimpleDoc('Start', 'Hello'));
      parser.parse('file:///b.twee', makeSimpleDoc('Other', 'World'));

      assert.strictEqual(parser.cacheSize, 2);

      parser.evictUri('file:///a.twee');
      assert.strictEqual(parser.cacheSize, 1);
    });

    it('should handle evicting a non-existent URI gracefully', () => {
      const parser = new IncrementalParser();
      parser.parse('file:///a.twee', makeSimpleDoc('Start', 'Hello'));

      assert.doesNotThrow(() => {
        parser.evictUri('file:///nonexistent.twee');
      });

      assert.strictEqual(parser.cacheSize, 1);
    });
  });

  describe('clearCache', () => {
    it('should remove everything from cache', () => {
      const parser = new IncrementalParser();

      parser.parse('file:///a.twee', makeSimpleDoc('Start', 'Hello'));
      parser.parse('file:///b.twee', makeSimpleDoc('Other', 'World'));
      assert.strictEqual(parser.cacheSize, 2);

      parser.clearCache();
      assert.strictEqual(parser.cacheSize, 0);
    });

    it('should allow re-parsing after clearing', () => {
      const parser = new IncrementalParser();
      const text = makeSimpleDoc('Start', 'Hello');

      parser.parse('file:///a.twee', text);
      parser.clearCache();
      assert.strictEqual(parser.cacheSize, 0);

      const result = parser.parse('file:///a.twee', text);
      assert.strictEqual(result.ast.passages.length, 1);
      assert.strictEqual(parser.cacheSize, 1);
    });
  });

  describe('cacheSize', () => {
    it('should return 0 for a fresh parser', () => {
      const parser = new IncrementalParser();
      assert.strictEqual(parser.cacheSize, 0);
    });

    it('should return correct count after parsing', () => {
      const parser = new IncrementalParser();
      parser.parse('file:///a.twee', makeSimpleDoc('Start', 'Hello'));
      assert.strictEqual(parser.cacheSize, 1);

      parser.parse('file:///a.twee', makeTwoPassageDoc('Start', 'Hello', 'End', 'Goodbye'));
      assert.strictEqual(parser.cacheSize, 2);
    });

    it('should return correct count after eviction', () => {
      const parser = new IncrementalParser();
      parser.parse('file:///a.twee', makeTwoPassageDoc('Start', 'Hello', 'End', 'Goodbye'));
      assert.strictEqual(parser.cacheSize, 2);

      parser.evictUri('file:///a.twee');
      assert.strictEqual(parser.cacheSize, 0);
    });
  });
});
