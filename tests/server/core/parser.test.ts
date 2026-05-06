/**
 * Knot v2 — Parser Tests (Twine Engine Core)
 *
 * Tests that the parser correctly handles universal Twee 3 features
 * and delegates format-specific work to the active FormatModule
 * via FormatRegistry.
 *
 * Core parser should work:
 *   - WITH fallback format (basic passage splitting, [[link]] extraction)
 *   - WITH mock format (classification, passage refs via delegation)
 *
 * Never hardcodes <<>> or () patterns.
 */

import * as assert from 'assert';
import { Parser, RawPassage } from '../../../server/src/core/parser';
import { FormatRegistry } from '../../../server/src/formats/formatRegistry';
import { PassageType, PassageKind, MacroBodyStyle } from '../../../server/src/hooks/hookTypes';
import { createMockFormatModule, createMockRegistry } from '../../helpers/testFixtures';
import type { SpecialPassageDef } from '../../../server/src/formats/_types';

describe('Parser — Twine Engine Core', () => {

  // ─── With Fallback Format (default registry) ────────────────────

  describe('With fallback format', () => {
    let registry: FormatRegistry;
    let parser: Parser;

    beforeEach(() => {
      registry = new FormatRegistry();
      // Fallback is pre-loaded by default
      parser = new Parser(registry);
    });

    it('should split document into raw passages', () => {
      const passages = parser.parseDocument(':: First\nFirst body\n\n:: Second\nSecond body');
      assert.strictEqual(passages.length, 2);
      assert.strictEqual(passages[0].name, 'First');
      assert.strictEqual(passages[1].name, 'Second');
    });

    it('should handle empty document', () => {
      const passages = parser.parseDocument('');
      assert.strictEqual(passages.length, 0);
    });

    it('should handle document with no passage headers', () => {
      const passages = parser.parseDocument('Just some text\nNo headers');
      assert.strictEqual(passages.length, 0);
    });

    it('should classify regular passages as Story', () => {
      const raw: RawPassage = {
        name: 'MyPassage', tags: [], body: 'text',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.Story);
    });

    it('should detect StoryData passage by name', () => {
      const raw: RawPassage = {
        name: 'StoryData', tags: [], body: '{}',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.StoryData);
    });

    it('should detect Start passage by name', () => {
      const raw: RawPassage = {
        name: 'Start', tags: [], body: 'text',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.Start);
    });

    it('should NOT classify random passage as StoryData', () => {
      const raw: RawPassage = {
        name: 'MyPassage', tags: [], body: 'text',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.Story);
    });

    it('should still detect StoryData even with tags', () => {
      const raw: RawPassage = {
        name: 'StoryData', tags: ['special'], body: '{}',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.StoryData);
    });
  });

  // ─── Twee 3 Spec Tags (universal, format-independent) ──────

  describe('Twee 3 spec tags', () => {
    let registry: FormatRegistry;
    let parser: Parser;

    beforeEach(() => {
      registry = createMockRegistry();
      parser = new Parser(registry);
    });

    it('should classify [script] passages as PassageType.Script (Twee 3 spec)', () => {
      const raw: RawPassage = {
        name: 'MyScript', tags: ['script'], body: 'code',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.Script);
    });

    it('should classify [stylesheet] passages as PassageType.Stylesheet (Twee 3 spec)', () => {
      const raw: RawPassage = {
        name: 'MyCSS', tags: ['stylesheet'], body: 'css',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.Stylesheet);
    });

    it('should classify [script] even with additional tags', () => {
      const raw: RawPassage = {
        name: 'MyScript', tags: ['script', 'important'], body: 'code',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.Script);
    });

    it('should classify [stylesheet] even with additional tags', () => {
      const raw: RawPassage = {
        name: 'MyCSS', tags: ['stylesheet', 'dark-theme'], body: 'css',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.Stylesheet);
    });

    it('[script] should take priority over format classification', () => {
      // Even with a format that has specialPassages, Twee 3 spec tags win
      const mockWithSpecial = createMockFormatModule({
        formatId: 'mock',
        specialPassages: [
          { name: 'MyScript', kind: PassageKind.Special, description: 'Mock special', typeId: 'mock-special' },
        ] as readonly SpecialPassageDef[],
      });
      const reg = createMockRegistry(mockWithSpecial);
      const p = new Parser(reg);

      const raw: RawPassage = {
        name: 'MyScript', tags: ['script'], body: 'code',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(p.classifyPassageType(raw), PassageType.Script);
    });
  });

  // ─── With Mock Format Module ────────────────────────────────────

  describe('With mock format module', () => {
    let registry: FormatRegistry;
    let parser: Parser;

    beforeEach(() => {
      registry = createMockRegistry();
      parser = new Parser(registry);
    });

    it('should delegate passage classification to format specialPassages (name match)', () => {
      const mockWithSpecial = createMockFormatModule({
        formatId: 'mock',
        specialPassages: [
          { name: 'MyWidget', kind: PassageKind.Special, description: 'Widget passage', tag: 'widget', typeId: 'widget' },
        ] as readonly SpecialPassageDef[],
      });
      const reg = createMockRegistry(mockWithSpecial);
      const p = new Parser(reg);

      const raw: RawPassage = {
        name: 'MyWidget', tags: ['widget'], body: 'text',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      // Name match on specialPassages → PassageType.Custom
      assert.strictEqual(p.classifyPassageType(raw), PassageType.Custom);
    });

    it('should delegate passage classification to format specialPassages (tag match)', () => {
      const mockWithSpecial = createMockFormatModule({
        formatId: 'mock',
        specialPassages: [
          { name: '', kind: PassageKind.Special, description: 'Widget passage', tag: 'widget', typeId: 'widget' },
        ] as readonly SpecialPassageDef[],
      });
      const reg = createMockRegistry(mockWithSpecial);
      const p = new Parser(reg);

      const raw: RawPassage = {
        name: 'AnyName', tags: ['widget'], body: 'text',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      // Tag match → PassageType.Custom
      assert.strictEqual(p.classifyPassageType(raw), PassageType.Custom);
    });

    it('should return Story when format has no matching specialPassages', () => {
      const raw: RawPassage = {
        name: 'NormalPassage', tags: [], body: 'text',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.Story);
    });
  });

  // ─── Raw Passage Splitting ───────────────────────────────────

  describe('Raw passage splitting', () => {
    let parser: Parser;

    beforeEach(() => {
      parser = new Parser(createMockRegistry());
    });

    it('should preserve correct offsets for passage bodies', () => {
      const content = ':: First\nFirst body\n\n:: Second\nSecond body';
      const passages = parser.parseDocument(content);
      assert.strictEqual(passages.length, 2);

      // First passage body should start after ":: First\n"
      assert.ok(passages[0].startOffset > 0);
      assert.ok(passages[0].body.includes('First body'));

      // Second passage body should start after second header
      assert.ok(passages[1].startOffset > passages[0].startOffset);
      assert.ok(passages[1].body.includes('Second body'));
    });

    it('should extract passage refs from body via format module', () => {
      const passages = parser.parseDocument(':: Pass\nGo to [[Target]] for details');
      assert.strictEqual(passages.length, 1);
      assert.ok(passages[0].passageRefs.length >= 1);
      const targetRef = passages[0].passageRefs.find(r => r.target === 'Target');
      assert.ok(targetRef, 'Should find Target in passageRefs');
    });

    it('should handle passage with multiple links in body', () => {
      const passages = parser.parseDocument(':: Pass\nGo [[A]] then [[B]] then [[C]]');
      assert.strictEqual(passages[0].passageRefs.length, 3);
      const targets = passages[0].passageRefs.map(r => r.target);
      assert.ok(targets.includes('A'));
      assert.ok(targets.includes('B'));
      assert.ok(targets.includes('C'));
    });

    it('should handle passage with no links', () => {
      const passages = parser.parseDocument(':: Pass\nJust plain text');
      assert.strictEqual(passages[0].passageRefs.length, 0);
    });

    it('should handle passage with empty body', () => {
      const content = ':: Empty\n\n:: Next\nHas body';
      const passages = parser.parseDocument(content);
      assert.strictEqual(passages.length, 2);
      assert.ok(passages[0].body !== undefined);
    });

    it('should handle passage header with tags', () => {
      const passages = parser.parseDocument(':: MyPassage [tag1 tag2]\nBody text');
      assert.strictEqual(passages.length, 1);
      assert.strictEqual(passages[0].name, 'MyPassage');
      assert.deepStrictEqual(passages[0].tags, ['tag1', 'tag2']);
    });

    it('should provide bodyTokens from format module lexBody()', () => {
      const passages = parser.parseDocument(':: Pass\nHello world');
      // Mock format returns an eof token
      assert.ok(passages[0].bodyTokens.length >= 1);
    });
  });

  // ─── StoryData and Start Detection ───────────────────────────

  describe('StoryData and Start detection', () => {
    let parser: Parser;

    beforeEach(() => {
      parser = new Parser(createMockRegistry());
    });

    it('should detect StoryData passage by name (universal Twine concept)', () => {
      const raw: RawPassage = {
        name: 'StoryData', tags: [], body: '{"format":"SugarCube"}',
        startOffset: 0, endOffset: 30, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.StoryData);
    });

    it('should detect Start passage by name', () => {
      const raw: RawPassage = {
        name: 'Start', tags: [], body: 'Welcome',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.classifyPassageType(raw), PassageType.Start);
    });
  });

  // ─── getCustomTypeId() ─────────────────────────────────────

  describe('getCustomTypeId()', () => {
    it('should return typeId for passages matching specialPassages by name', () => {
      const mockWithSpecial = createMockFormatModule({
        formatId: 'mock',
        specialPassages: [
          { name: 'StoryInit', kind: PassageKind.Special, description: 'Init', typeId: 'init' },
        ] as readonly SpecialPassageDef[],
      });
      const reg = createMockRegistry(mockWithSpecial);
      const parser = new Parser(reg);

      const raw: RawPassage = {
        name: 'StoryInit', tags: [], body: 'code',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.getCustomTypeId(raw), 'init');
    });

    it('should return typeId for passages matching specialPassages by tag', () => {
      const mockWithSpecial = createMockFormatModule({
        formatId: 'mock',
        specialPassages: [
          { name: '', kind: PassageKind.Special, description: 'Widget', tag: 'widget', typeId: 'widget' },
        ] as readonly SpecialPassageDef[],
      });
      const reg = createMockRegistry(mockWithSpecial);
      const parser = new Parser(reg);

      const raw: RawPassage = {
        name: 'MyWidget', tags: ['widget'], body: 'code',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(parser.getCustomTypeId(raw), 'widget');
    });

    it('should return undefined for non-custom passages', () => {
      const p = new Parser(createMockRegistry());
      const raw: RawPassage = {
        name: 'MyPassage', tags: [], body: 'text',
        startOffset: 0, endOffset: 10, passageRefs: [], bodyTokens: [],
      };
      assert.strictEqual(p.getCustomTypeId(raw), undefined);
    });
  });

  // ─── getASTNodeTypes() ──────────────────────────────────────

  describe('getASTNodeTypes()', () => {
    it('should return AST node types from active format', () => {
      const parser = new Parser(createMockRegistry());
      const types = parser.getASTNodeTypes();
      assert.ok(types.Document);
      assert.ok(types.PassageHeader);
      assert.ok(types.PassageBody);
      assert.ok(types.Link);
      assert.ok(types.Text);
    });
  });
});
