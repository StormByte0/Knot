/**
 * Knot v2 — FormatRegistry Tests
 *
 * Tests that FormatRegistry correctly manages format modules:
 *   - register() / getFormat()
 *   - setActiveFormat() / getActiveFormat()
 *   - getAvailableFormatIds()
 *   - resolve() for exact, alias, and prefix matching
 *   - detectFromStoryData() for auto-detection from JSON
 *   - loadBuiltinFormats() loads all built-in format modules
 */

import * as assert from 'assert';
import { FormatRegistry } from '../../../server/src/formats/formatRegistry';
import { createMockFormatModule } from '../../helpers/testFixtures';
import { MacroBodyStyle } from '../../../server/src/hooks/hookTypes';

describe('FormatRegistry', () => {

  // ─── register() / getFormat() ──────────────────────────────────

  describe('register() / getFormat()', () => {
    it('should register and retrieve a format module by formatId', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'test-format' });
      registry.register(mod);
      const retrieved = registry.getFormat('test-format');
      assert.strictEqual(retrieved, mod);
    });

    it('should return undefined for unregistered formatId', () => {
      const registry = new FormatRegistry();
      assert.strictEqual(registry.getFormat('nonexistent'), undefined);
    });

    it('should overwrite when re-registering the same formatId', () => {
      const registry = new FormatRegistry();
      const mod1 = createMockFormatModule({ formatId: 'test' });
      const mod2 = createMockFormatModule({ formatId: 'test', displayName: 'Test V2' });
      registry.register(mod1);
      registry.register(mod2);
      assert.strictEqual(registry.getFormat('test'), mod2);
    });

    it('should always have fallback pre-loaded', () => {
      const registry = new FormatRegistry();
      const fallback = registry.getFormat('fallback');
      assert.ok(fallback, 'Fallback should be pre-loaded');
      assert.strictEqual(fallback!.formatId, 'fallback');
    });
  });

  // ─── setActiveFormat() / getActiveFormat() ─────────────────────

  describe('setActiveFormat() / getActiveFormat()', () => {
    it('should set and get active format', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'active-test' });
      registry.register(mod);
      registry.setActiveFormat('active-test');
      assert.strictEqual(registry.getActiveFormat(), mod);
    });

    it('should fall back to fallback for unknown formatId', () => {
      const registry = new FormatRegistry();
      registry.setActiveFormat('nonexistent');
      assert.strictEqual(registry.getActiveFormat().formatId, 'fallback');
    });

    it('should fall back to fallback for undefined formatId', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'my-format' });
      registry.register(mod);
      registry.setActiveFormat('my-format');
      assert.strictEqual(registry.getActiveFormat().formatId, 'my-format');
      registry.setActiveFormat(undefined);
      assert.strictEqual(registry.getActiveFormat().formatId, 'fallback');
    });

    it('should default to fallback as active format', () => {
      const registry = new FormatRegistry();
      assert.strictEqual(registry.getActiveFormat().formatId, 'fallback');
    });
  });

  // ─── getAvailableFormatIds() ───────────────────────────────────

  describe('getAvailableFormatIds()', () => {
    it('should list all registered format IDs including fallback', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'extra' });
      registry.register(mod);
      const ids = registry.getAvailableFormatIds();
      assert.ok(ids.includes('fallback'), 'Should include fallback');
      assert.ok(ids.includes('extra'), 'Should include extra');
    });

    it('should start with only fallback', () => {
      const registry = new FormatRegistry();
      const ids = registry.getAvailableFormatIds();
      assert.strictEqual(ids.length, 1);
      assert.strictEqual(ids[0], 'fallback');
    });
  });

  // ─── hasFormat() ───────────────────────────────────────────────

  describe('hasFormat()', () => {
    it('should return true for registered format', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'check-me' });
      registry.register(mod);
      assert.strictEqual(registry.hasFormat('check-me'), true);
    });

    it('should return false for unregistered format', () => {
      const registry = new FormatRegistry();
      assert.strictEqual(registry.hasFormat('nope'), false);
    });

    it('should return true for fallback', () => {
      const registry = new FormatRegistry();
      assert.strictEqual(registry.hasFormat('fallback'), true);
    });
  });

  // ─── resolve() ─────────────────────────────────────────────────

  describe('resolve()', () => {
    it('should resolve by exact formatId', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'my-format' });
      registry.register(mod);
      const resolved = registry.resolve('my-format');
      assert.strictEqual(resolved.formatId, 'my-format');
    });

    it('should resolve case-insensitively', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'My-Format' });
      registry.register(mod);
      const resolved = registry.resolve('my-format');
      assert.strictEqual(resolved.formatId, 'My-Format');
    });

    it('should resolve by alias', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({
        formatId: 'my-format',
        aliases: ['mf', 'myfmt'],
      });
      registry.register(mod);
      assert.strictEqual(registry.resolve('mf').formatId, 'my-format');
      assert.strictEqual(registry.resolve('myfmt').formatId, 'my-format');
    });

    it('should resolve by display name', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({
        formatId: 'my-format',
        displayName: 'My Cool Format',
      });
      registry.register(mod);
      assert.strictEqual(registry.resolve('my cool format').formatId, 'my-format');
    });

    it('should resolve by prefix match', () => {
      const registry = new FormatRegistry();
      registry.loadBuiltinFormats();
      // 'sugarcube' should prefix-match 'sugarcube-2'
      const resolved = registry.resolve('sugarcube');
      assert.strictEqual(resolved.formatId, 'sugarcube-2');
    });

    it('should return fallback for empty string', () => {
      const registry = new FormatRegistry();
      assert.strictEqual(registry.resolve('').formatId, 'fallback');
    });

    it('should return fallback for unknown format', () => {
      const registry = new FormatRegistry();
      assert.strictEqual(registry.resolve('unknown-format-xyz').formatId, 'fallback');
    });
  });

  // ─── detectFromStoryData() ─────────────────────────────────────

  describe('detectFromStoryData()', () => {
    let registry: FormatRegistry;

    beforeEach(() => {
      registry = new FormatRegistry();
      registry.loadBuiltinFormats();
    });

    it('should detect SugarCube from StoryData JSON', () => {
      const json = '{"format":"SugarCube","format-version":"2.36.0"}';
      const result = registry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'sugarcube-2');
    });

    it('should detect Harlowe from StoryData JSON', () => {
      const json = '{"format":"Harlowe","format-version":"3.3.8"}';
      const result = registry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'harlowe-3');
    });

    it('should detect format case-insensitively', () => {
      const json = '{"format":"sugarcube","format-version":"2.36.0"}';
      const result = registry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'sugarcube-2');
    });

    it('should return fallback for unknown format', () => {
      const json = '{"format":"UnknownFormat","format-version":"1.0.0"}';
      const result = registry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'fallback');
    });

    it('should return fallback for invalid JSON', () => {
      const result = registry.detectFromStoryData('not json at all');
      assert.strictEqual(result.formatId, 'fallback');
    });

    it('should return fallback for JSON without format field', () => {
      const json = '{"something":"else"}';
      const result = registry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'fallback');
    });

    it('should return fallback for empty string', () => {
      const result = registry.detectFromStoryData('');
      assert.strictEqual(result.formatId, 'fallback');
    });

    it('should handle StoryData with whitespace', () => {
      const json = '  {"format":"Harlowe","format-version":"3.3.8"}  ';
      const result = registry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'harlowe-3');
    });

    it('should handle StoryData JSON with extra fields', () => {
      const json = '{"format":"SugarCube","format-version":"2.36.0","start":"Start","ifid":"ABC123"}';
      const result = registry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'sugarcube-2');
    });

    it('should detect format using format-version major number', () => {
      const json = '{"format":"Harlowe","format-version":"3.3.8"}';
      const result = registry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'harlowe-3');
    });
  });

  // ─── loadBuiltinFormats() ──────────────────────────────────────

  describe('loadBuiltinFormats()', () => {
    it('should load all built-in format modules', () => {
      const registry = new FormatRegistry();
      registry.loadBuiltinFormats();
      const ids = registry.getAvailableFormatIds();
      assert.ok(ids.includes('fallback'), 'Should include fallback');
      assert.ok(ids.includes('sugarcube-2'), 'Should include sugarcube-2');
      assert.ok(ids.includes('harlowe-3'), 'Should include harlowe-3');
      assert.ok(ids.includes('chapbook-2'), 'Should include chapbook-2');
      assert.ok(ids.includes('snowman-2'), 'Should include snowman-2');
    });

    it('should not break if called twice', () => {
      const registry = new FormatRegistry();
      registry.loadBuiltinFormats();
      registry.loadBuiltinFormats();
      const ids = registry.getAvailableFormatIds();
      // Should still have 5 formats (fallback + 4 builtins)
      assert.strictEqual(ids.length, 5);
    });

    it('should make built-in modules retrievable via getFormat()', () => {
      const registry = new FormatRegistry();
      registry.loadBuiltinFormats();
      const sc = registry.getFormat('sugarcube-2');
      assert.ok(sc, 'sugarcube-2 should be retrievable');
      assert.strictEqual(sc!.formatId, 'sugarcube-2');
      assert.strictEqual(sc!.macroBodyStyle, MacroBodyStyle.CloseTag);

      const hl = registry.getFormat('harlowe-3');
      assert.ok(hl, 'harlowe-3 should be retrievable');
      assert.strictEqual(hl!.formatId, 'harlowe-3');
      assert.strictEqual(hl!.macroBodyStyle, MacroBodyStyle.Hook);
    });
  });

  // ─── setActiveFormatModule() ───────────────────────────────────

  describe('setActiveFormatModule()', () => {
    it('should set active format directly from a module', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'direct-set' });
      registry.setActiveFormatModule(mod);
      assert.strictEqual(registry.getActiveFormat().formatId, 'direct-set');
    });

    it('should auto-register the module if not already loaded', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'auto-registered' });
      registry.setActiveFormatModule(mod);
      assert.strictEqual(registry.getFormat('auto-registered'), mod);
    });

    it('should not duplicate if module is already loaded', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({ formatId: 'already-there' });
      registry.register(mod);
      registry.setActiveFormatModule(mod);
      assert.strictEqual(registry.getFormat('already-there'), mod);
    });
  });

  // ─── Alias Resolution ──────────────────────────────────────────

  describe('Alias resolution', () => {
    it('should resolve by alias after registration', () => {
      const registry = new FormatRegistry();
      const mod = createMockFormatModule({
        formatId: 'custom-format',
        aliases: ['cf', 'custom', 'MyCustom'],
      });
      registry.register(mod);
      assert.strictEqual(registry.resolve('cf').formatId, 'custom-format');
      assert.strictEqual(registry.resolve('custom').formatId, 'custom-format');
      assert.strictEqual(registry.resolve('mycustom').formatId, 'custom-format');
    });

    it('should resolve built-in format by its display name', () => {
      const registry = new FormatRegistry();
      registry.loadBuiltinFormats();
      const resolved = registry.resolve('SugarCube 2');
      assert.strictEqual(resolved.formatId, 'sugarcube-2');
    });

    it('should resolve built-in format by its short alias', () => {
      const registry = new FormatRegistry();
      registry.loadBuiltinFormats();
      const resolved = registry.resolve('sugarcube');
      assert.strictEqual(resolved.formatId, 'sugarcube-2');
    });
  });
});
