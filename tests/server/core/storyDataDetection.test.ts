/**
 * Knot v2 — StoryData Detection Tests (Twine Engine Core)
 *
 * Tests that StoryData passage detection and format auto-detection
 * work correctly at the Twine engine base layer.
 *
 * StoryData is a universal Twine concept — every story format uses it
 * to store metadata. These tests verify:
 *   - Raw text search for :: StoryData
 *   - JSON body extraction via Parser
 *   - FormatRegistry.detectFromStoryData() parsing
 *   - Format detection for known/unknown formats
 *   - Edge cases (tags, non-first position, invalid JSON)
 */

import * as assert from 'assert';
import { FormatRegistry } from '../../../server/src/formats/formatRegistry';
import { Parser } from '../../../server/src/core/parser';
import { createMockRegistry } from '../../helpers/testFixtures';

describe('StoryData Detection — Twine Engine Core', () => {

  // ─── Raw Text Search ─────────────────────────────────────────

  describe('Raw text search for :: StoryData', () => {
    it('should find StoryData passage in simple twee', () => {
      const content = ':: StoryData\n{"format":"SugarCube","format-version":"2.36.0"}';
      assert.ok(content.includes(':: StoryData'));
    });

    it('should find StoryData passage with tags', () => {
      const content = ':: StoryData [special]\n{"format":"Harlowe","format-version":"3.3.8"}';
      assert.ok(content.includes(':: StoryData'));
    });

    it('should find StoryData when not the first passage', () => {
      const content = ':: Start\nWelcome\n\n:: StoryData\n{"format":"SugarCube"}';
      assert.ok(content.includes(':: StoryData'));
    });

    it('should not find StoryData when passage is named differently', () => {
      const content = ':: MyData\n{"format":"SugarCube"}';
      assert.ok(!content.includes(':: StoryData'));
    });
  });

  // ─── Parser StoryData Extraction ─────────────────────────────

  describe('Parser extracts StoryData body', () => {
    let parser: Parser;

    beforeEach(() => {
      parser = new Parser(createMockRegistry());
    });

    it('should extract JSON body after StoryData header', () => {
      const content = ':: StoryData\n{"format":"SugarCube","format-version":"2.36.0"}';
      const passages = parser.parseDocument(content);
      assert.strictEqual(passages.length, 1);
      assert.strictEqual(passages[0].name, 'StoryData');
      assert.ok(passages[0].body.includes('"format"'));
      assert.ok(passages[0].body.includes('"SugarCube"'));
    });

    it('should extract StoryData body when not first passage', () => {
      const content = ':: Start\nWelcome\n\n:: StoryData\n{"format":"Harlowe"}';
      const passages = parser.parseDocument(content);
      const storyData = passages.find(p => p.name === 'StoryData');
      assert.ok(storyData, 'Should find StoryData passage');
      assert.ok(storyData!.body.includes('"format"'));
    });

    it('should extract StoryData body with tags', () => {
      const content = ':: StoryData [special]\n{"format":"SugarCube"}';
      const passages = parser.parseDocument(content);
      const storyData = passages.find(p => p.name === 'StoryData');
      assert.ok(storyData);
      assert.ok(storyData!.body.includes('"format"'));
      assert.ok(storyData!.tags.includes('special'));
    });
  });

  // ─── FormatRegistry.detectFromStoryData() ────────────────────

  describe('FormatRegistry.detectFromStoryData()', () => {
    let formatRegistry: FormatRegistry;

    beforeEach(() => {
      formatRegistry = new FormatRegistry();
      formatRegistry.loadBuiltinFormats();
    });

    it('should detect SugarCube from StoryData JSON', () => {
      const json = '{"format":"SugarCube","format-version":"2.36.0"}';
      const result = formatRegistry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'sugarcube-2');
    });

    it('should detect Harlowe from StoryData JSON', () => {
      const json = '{"format":"Harlowe","format-version":"3.3.8"}';
      const result = formatRegistry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'harlowe-3');
    });

    it('should detect format case-insensitively', () => {
      const json = '{"format":"sugarcube","format-version":"2.36.0"}';
      const result = formatRegistry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'sugarcube-2');
    });

    it('should return fallback for unknown format', () => {
      const json = '{"format":"UnknownFormat","format-version":"1.0.0"}';
      const result = formatRegistry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'fallback');
    });

    it('should return fallback for invalid JSON', () => {
      const result = formatRegistry.detectFromStoryData('not json at all');
      assert.strictEqual(result.formatId, 'fallback');
    });

    it('should return fallback for JSON without format field', () => {
      const json = '{"something":"else"}';
      const result = formatRegistry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'fallback');
    });

    it('should return fallback for empty string', () => {
      const result = formatRegistry.detectFromStoryData('');
      assert.strictEqual(result.formatId, 'fallback');
    });

    it('should handle StoryData with whitespace', () => {
      const json = '  {"format":"Harlowe","format-version":"3.3.8"}  ';
      const result = formatRegistry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'harlowe-3');
    });

    it('should handle StoryData JSON with extra fields', () => {
      const json = '{"format":"SugarCube","format-version":"2.36.0","start":"Start","ifid":"ABC123"}';
      const result = formatRegistry.detectFromStoryData(json);
      assert.strictEqual(result.formatId, 'sugarcube-2');
    });
  });

  // ─── Pre-scan Integration ────────────────────────────────────

  describe('Pre-scan integration', () => {
    let formatRegistry: FormatRegistry;
    let parser: Parser;

    beforeEach(() => {
      formatRegistry = new FormatRegistry();
      formatRegistry.loadBuiltinFormats();
      parser = new Parser(formatRegistry);
    });

    it('should pre-scan StoryData with tags :: StoryData [special]', () => {
      const content = ':: StoryData [special]\n{"format":"SugarCube","format-version":"2.36.0"}';
      const passages = parser.parseDocument(content);
      const storyData = passages.find(p => p.name === 'StoryData');
      assert.ok(storyData);
      assert.ok(storyData!.tags.includes('special'));

      // Body should still be the JSON
      const detected = formatRegistry.detectFromStoryData(storyData!.body);
      assert.strictEqual(detected.formatId, 'sugarcube-2');
    });

    it('should pre-scan StoryData when not the first passage', () => {
      const content = ':: Start\nWelcome\n\n:: StoryData\n{"format":"Harlowe","format-version":"3.3.8"}';
      const passages = parser.parseDocument(content);
      const storyData = passages.find(p => p.name === 'StoryData');
      assert.ok(storyData);

      const detected = formatRegistry.detectFromStoryData(storyData!.body);
      assert.strictEqual(detected.formatId, 'harlowe-3');
    });

    it('should handle full twee file pre-scan flow', () => {
      const content = [
        ':: StoryData',
        '{"format":"SugarCube","format-version":"2.36.0","ifid":"12345"}',
        '',
        ':: Start',
        'Welcome to the story. [[Go to Room 1]]',
        '',
        ':: Room 1',
        'You are in a room. [[Go back->Start]]',
      ].join('\n');

      const passages = parser.parseDocument(content);
      assert.strictEqual(passages.length, 3);

      const storyData = passages.find(p => p.name === 'StoryData');
      assert.ok(storyData);

      const detected = formatRegistry.detectFromStoryData(storyData!.body);
      assert.strictEqual(detected.formatId, 'sugarcube-2');
    });
  });
});
