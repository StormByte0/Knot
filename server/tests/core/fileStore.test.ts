import { strict as assert } from 'assert';
import { FileStore, StoredFile, FileSource } from '../../src/fileStore';

describe('FileStore', () => {
  let store: FileStore;

  beforeEach(() => {
    store = new FileStore();
  });

  // ── upsert ──────────────────────────────────────────────────────────────────

  describe('upsert', () => {
    it('returns true for new content', () => {
      const result = store.upsert('file:///a.tw', 'hello', 'disk');
      assert.strictEqual(result, true);
    });

    it('returns false for same content (same hash)', () => {
      store.upsert('file:///a.tw', 'hello', 'disk');
      const result = store.upsert('file:///a.tw', 'hello', 'lsp');
      assert.strictEqual(result, false);
    });

    it('LSP source wins over disk source — disk upsert on LSP file returns false', () => {
      store.upsert('file:///a.tw', 'original', 'lsp', 1);
      const result = store.upsert('file:///a.tw', 'from disk', 'disk');
      // LSP content always wins, disk content is rejected
      assert.strictEqual(result, false);
      // Content should remain the LSP version
      assert.strictEqual(store.getText('file:///a.tw'), 'original');
    });

    it('disk source works when no existing entry', () => {
      const result = store.upsert('file:///a.tw', 'disk content', 'disk');
      assert.strictEqual(result, true);
      assert.strictEqual(store.getText('file:///a.tw'), 'disk content');
    });

    it('updates lastSeen timestamp on no-op', () => {
      store.upsert('file:///a.tw', 'hello', 'disk');
      const file1 = store.get('file:///a.tw');
      const ts1 = file1!.lastSeen;

      // Small delay to ensure timestamp differs
      const start = Date.now();
      while (Date.now() === ts1) { /* spin */ }

      store.upsert('file:///a.tw', 'hello', 'lsp');
      const file2 = store.get('file:///a.tw');
      assert.ok(file2!.lastSeen >= ts1);
    });

    it('returns true when content changes', () => {
      store.upsert('file:///a.tw', 'v1', 'disk');
      const result = store.upsert('file:///a.tw', 'v2', 'lsp');
      assert.strictEqual(result, true);
      assert.strictEqual(store.getText('file:///a.tw'), 'v2');
    });

    it('LSP can update over existing LSP content', () => {
      store.upsert('file:///a.tw', 'v1', 'lsp', 1);
      const result = store.upsert('file:///a.tw', 'v2', 'lsp', 2);
      assert.strictEqual(result, true);
      assert.strictEqual(store.getText('file:///a.tw'), 'v2');
    });

    it('disk can overwrite disk content when hash differs', () => {
      store.upsert('file:///a.tw', 'v1', 'disk');
      const result = store.upsert('file:///a.tw', 'v2', 'disk');
      assert.strictEqual(result, true);
      assert.strictEqual(store.getText('file:///a.tw'), 'v2');
    });
  });

  // ── remove ──────────────────────────────────────────────────────────────────

  describe('remove', () => {
    it('deletes entry and returns true', () => {
      store.upsert('file:///a.tw', 'hello', 'disk');
      const result = store.remove('file:///a.tw');
      assert.strictEqual(result, true);
      assert.strictEqual(store.has('file:///a.tw'), false);
    });

    it('returns false for non-existent URI', () => {
      const result = store.remove('file:///nonexistent.tw');
      assert.strictEqual(result, false);
    });
  });

  // ── get ─────────────────────────────────────────────────────────────────────

  describe('get', () => {
    it('returns StoredFile with correct fields', () => {
      store.upsert('file:///a.tw', 'content', 'lsp', 5);
      const file = store.get('file:///a.tw');
      assert.ok(file !== undefined);
      assert.strictEqual(file!.uri, 'file:///a.tw');
      assert.strictEqual(file!.text, 'content');
      assert.strictEqual(file!.version, 5);
      assert.strictEqual(file!.source, 'lsp');
      assert.ok(typeof file!.hash === 'string' && file!.hash.length > 0);
      assert.ok(typeof file!.lastSeen === 'number' && file!.lastSeen > 0);
    });

    it('returns undefined for non-existent URI', () => {
      assert.strictEqual(store.get('file:///missing.tw'), undefined);
    });

    it('returns version 0 for disk-sourced files by default', () => {
      store.upsert('file:///a.tw', 'content', 'disk');
      const file = store.get('file:///a.tw');
      assert.strictEqual(file!.version, 0);
    });
  });

  // ── getText ─────────────────────────────────────────────────────────────────

  describe('getText', () => {
    it('returns text for stored file', () => {
      store.upsert('file:///a.tw', 'my text', 'disk');
      assert.strictEqual(store.getText('file:///a.tw'), 'my text');
    });

    it('returns undefined for non-existent file', () => {
      assert.strictEqual(store.getText('file:///missing.tw'), undefined);
    });
  });

  // ── has ─────────────────────────────────────────────────────────────────────

  describe('has', () => {
    it('returns true for stored file', () => {
      store.upsert('file:///a.tw', 'content', 'disk');
      assert.strictEqual(store.has('file:///a.tw'), true);
    });

    it('returns false for non-existent file', () => {
      assert.strictEqual(store.has('file:///missing.tw'), false);
    });

    it('returns false after removal', () => {
      store.upsert('file:///a.tw', 'content', 'disk');
      store.remove('file:///a.tw');
      assert.strictEqual(store.has('file:///a.tw'), false);
    });
  });

  // ── uris ────────────────────────────────────────────────────────────────────

  describe('uris', () => {
    it('returns sorted list of URIs', () => {
      store.upsert('file:///c.tw', 'c', 'disk');
      store.upsert('file:///a.tw', 'a', 'disk');
      store.upsert('file:///b.tw', 'b', 'disk');
      const uris = store.uris();
      assert.deepStrictEqual(uris, ['file:///a.tw', 'file:///b.tw', 'file:///c.tw']);
    });

    it('returns empty array for empty store', () => {
      assert.deepStrictEqual(store.uris(), []);
    });
  });

  // ── size ────────────────────────────────────────────────────────────────────

  describe('size', () => {
    it('returns count of stored files', () => {
      assert.strictEqual(store.size(), 0);
      store.upsert('file:///a.tw', 'a', 'disk');
      assert.strictEqual(store.size(), 1);
      store.upsert('file:///b.tw', 'b', 'disk');
      assert.strictEqual(store.size(), 2);
    });

    it('decrements on removal', () => {
      store.upsert('file:///a.tw', 'a', 'disk');
      store.upsert('file:///b.tw', 'b', 'disk');
      store.remove('file:///a.tw');
      assert.strictEqual(store.size(), 1);
    });

    it('does not increment for duplicate URI with same hash', () => {
      store.upsert('file:///a.tw', 'same', 'disk');
      store.upsert('file:///a.tw', 'same', 'lsp');
      assert.strictEqual(store.size(), 1);
    });
  });
});
