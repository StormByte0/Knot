"use strict";
/**
 * Knot v2 — Parser Tests (Twine Engine Core)
 *
 * Tests that the parser correctly handles universal Twee 3 features
 * and delegates format-specific work to adapters via the hook registry.
 *
 * Core parser should work:
 *   - WITHOUT any adapter (basic passage splitting, [[link]] extraction)
 *   - WITH an adapter (classification, macro/variable extraction via delegation)
 *
 * Never hardcodes <<>> or () patterns.
 */
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
const assert = __importStar(require("assert"));
const parser_1 = require("../../../server/src/core/parser");
const hookRegistry_1 = require("../../../server/src/hooks/hookRegistry");
const hookTypes_1 = require("../../../server/src/hooks/hookTypes");
const testFixtures_1 = require("../../helpers/testFixtures");
const adapter_1 = require("../../../server/src/formats/fallback/adapter");
describe('Parser — Twine Engine Core', () => {
    // ─── Without Adapter (raw fallback) ──────────────────────────
    describe('Without adapter', () => {
        let registry;
        let parser;
        beforeEach(() => {
            registry = new hookRegistry_1.HookRegistry();
            // No adapter registered — parser should still work for basic features
            parser = new parser_1.Parser(registry);
        });
        it('should split document into raw passages', () => {
            const passages = parser.parseDocument(':: First\nFirst body\n\n:: Second\nSecond body');
            assert.strictEqual(passages.length, 2);
            assert.strictEqual(passages[0].name, 'First');
            assert.strictEqual(passages[1].name, 'Second');
        });
        it('should extract [[link]] targets from passage body', () => {
            const passages = parser.parseDocument(':: Pass\nGo to [[Target]] for details');
            assert.strictEqual(passages.length, 1);
            assert.ok(passages[0].rawLinks.includes('Target'));
        });
        it('should return empty bodyTokens when no adapter', () => {
            const passages = parser.parseDocument(':: Pass\nHello world');
            assert.strictEqual(passages[0].bodyTokens.length, 0);
        });
        it('should return empty macro names when no adapter', () => {
            const names = parser.extractMacroNames('<<set $x to 5>>');
            assert.deepStrictEqual(names, []);
        });
        it('should return empty variable sets when no adapter', () => {
            const vars = parser.extractVariables('$storyVar and _tempVar');
            assert.strictEqual(vars.story.size, 0);
            assert.strictEqual(vars.temp.size, 0);
        });
        it('should default classifyPassageType to Story when no adapter', () => {
            const raw = {
                name: 'MyPassage', tags: [], body: 'text',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Story);
        });
        it('should classifyLinks as Passage kind when no adapter', () => {
            const links = parser.classifyLinks(['Something']);
            assert.strictEqual(links.length, 1);
            assert.strictEqual(links[0].kind, hookTypes_1.LinkKind.Passage);
            assert.strictEqual(links[0].target, 'Something');
        });
    });
    // ─── Twee 3 Spec Tags (universal, adapter-independent) ──────
    describe('Twee 3 spec tags', () => {
        let registry;
        let parser;
        beforeEach(() => {
            registry = new hookRegistry_1.HookRegistry();
            parser = new parser_1.Parser(registry);
        });
        it('should classify [script] passages as PassageType.Script (Twee 3 spec)', () => {
            const raw = {
                name: 'MyScript', tags: ['script'], body: 'code',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Script);
        });
        it('should classify [stylesheet] passages as PassageType.Stylesheet (Twee 3 spec)', () => {
            const raw = {
                name: 'MyCSS', tags: ['stylesheet'], body: 'css',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Stylesheet);
        });
        it('should classify [script] even with additional tags', () => {
            const raw = {
                name: 'MyScript', tags: ['script', 'important'], body: 'code',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Script);
        });
        it('should classify [stylesheet] even with additional tags', () => {
            const raw = {
                name: 'MyCSS', tags: ['stylesheet', 'dark-theme'], body: 'css',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Stylesheet);
        });
        it('[script] should take priority over adapter classification', () => {
            // Even with an adapter that would classify as Special, Twee 3 spec tags win
            const mockProvider = new testFixtures_1.MockFormatProvider();
            mockProvider.passageProvider.configure({
                classifyFn: (_name, _tags) => hookTypes_1.PassageKind.Special,
            });
            registry.register('mock', mockProvider);
            registry.setActiveFormat('mock');
            parser = new parser_1.Parser(registry);
            const raw = {
                name: 'MyScript', tags: ['script'], body: 'code',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Script);
        });
    });
    // ─── With FallbackAdapter ────────────────────────────────────
    describe('With FallbackAdapter', () => {
        let registry;
        let parser;
        beforeEach(() => {
            registry = new hookRegistry_1.HookRegistry();
            const fallback = new adapter_1.FallbackAdapter();
            registry.register('fallback', fallback);
            registry.setActiveFormat('fallback');
            parser = new parser_1.Parser(registry);
        });
        it('should classify regular passages as Story', () => {
            const raw = {
                name: 'MyPassage', tags: [], body: 'text',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Story);
        });
        it('should detect StoryData passage by name', () => {
            const raw = {
                name: 'StoryData', tags: [], body: '{}',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.StoryData);
        });
        it('should detect Start passage by name', () => {
            const raw = {
                name: 'Start', tags: [], body: 'text',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Start);
        });
        it('should classify links using FallbackAdapter link provider', () => {
            const links = parser.classifyLinks(['Target']);
            assert.strictEqual(links.length, 1);
            assert.strictEqual(links[0].kind, hookTypes_1.LinkKind.Passage);
        });
        it('should return empty macro names (FallbackAdapter has no macro pattern)', () => {
            const names = parser.extractMacroNames('some text');
            assert.deepStrictEqual(names, []);
        });
        it('should return empty variable sets (FallbackAdapter has no variable pattern)', () => {
            const vars = parser.extractVariables('$x');
            assert.strictEqual(vars.story.size, 0);
            assert.strictEqual(vars.temp.size, 0);
        });
        it('should return empty body tokens (FallbackAdapter returns empty)', () => {
            const passages = parser.parseDocument(':: Pass\nHello world');
            assert.strictEqual(passages[0].bodyTokens.length, 0);
        });
    });
    // ─── With Mock Adapter ───────────────────────────────────────
    describe('With mock adapter', () => {
        let registry;
        let mockProvider;
        let parser;
        beforeEach(() => {
            registry = new hookRegistry_1.HookRegistry();
            mockProvider = new testFixtures_1.MockFormatProvider();
            registry.register('mock', mockProvider);
            registry.setActiveFormat('mock');
            parser = new parser_1.Parser(registry);
        });
        it('should delegate passage classification to adapter', () => {
            mockProvider.passageProvider.configure({
                classifyFn: (_name, tags) => {
                    if (tags.includes('widget'))
                        return hookTypes_1.PassageKind.Special;
                    return null;
                },
            });
            const raw = {
                name: 'MyWidget', tags: ['widget'], body: 'text',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            const result = parser.classifyPassageType(raw);
            assert.ok(result !== hookTypes_1.PassageType.Story, 'Should not be Story when adapter classifies as Special');
        });
        it('should delegate macro extraction to adapter', () => {
            mockProvider.syntaxProvider.configure({
                macroPat: /\((\w+):/g,
            });
            const names = parser.extractMacroNames('(set: $x to 5) and (if: $x > 3)[text]');
            assert.ok(names.includes('set'), 'Should find set: macro');
            assert.ok(names.includes('if'), 'Should find if: macro');
        });
        it('should delegate variable extraction to adapter with sigil classification', () => {
            mockProvider.syntaxProvider.configure({
                varPat: /([$_])(\w+)/g,
            });
            // MockSyntaxProvider.classifyVariableSigil returns null by default
            // So no variables will be classified
            const vars = parser.extractVariables('$storyVar and _tempVar');
            // Default mock returns null for all sigils, so nothing classified
            assert.strictEqual(vars.story.size, 0);
            assert.strictEqual(vars.temp.size, 0);
        });
        it('should delegate link classification to adapter', () => {
            const links = parser.classifyLinks(['Target']);
            assert.strictEqual(links.length, 1);
            assert.strictEqual(links[0].kind, hookTypes_1.LinkKind.Passage);
        });
    });
    // ─── Raw Passage Splitting ───────────────────────────────────
    describe('Raw passage splitting', () => {
        let parser;
        beforeEach(() => {
            parser = new parser_1.Parser(new hookRegistry_1.HookRegistry());
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
        it('should handle passage with multiple links in body', () => {
            const passages = parser.parseDocument(':: Pass\nGo [[A]] then [[B]] then [[C]]');
            assert.strictEqual(passages[0].rawLinks.length, 3);
            assert.ok(passages[0].rawLinks.includes('A'));
            assert.ok(passages[0].rawLinks.includes('B'));
            assert.ok(passages[0].rawLinks.includes('C'));
        });
        it('should handle passage with no links', () => {
            const passages = parser.parseDocument(':: Pass\nJust plain text');
            assert.strictEqual(passages[0].rawLinks.length, 0);
        });
        it('should handle empty document', () => {
            const passages = parser.parseDocument('');
            assert.strictEqual(passages.length, 0);
        });
        it('should handle document with no passage headers', () => {
            const passages = parser.parseDocument('Just some text\nNo headers');
            assert.strictEqual(passages.length, 0);
        });
        it('should handle passage with empty body', () => {
            const content = ':: Empty\n\n:: Next\nHas body';
            const passages = parser.parseDocument(content);
            assert.strictEqual(passages.length, 2);
            // Empty passage body should be empty string (just the blank line)
            assert.ok(passages[0].body !== undefined);
        });
        it('should extract links from passage body correctly', () => {
            const passages = parser.parseDocument(':: Pass\nCheck [[Target]] for details');
            assert.strictEqual(passages.length, 1);
            assert.ok(passages[0].rawLinks.includes('Target'));
        });
        it('should handle passage header with tags', () => {
            const passages = parser.parseDocument(':: MyPassage [tag1 tag2]\nBody text');
            assert.strictEqual(passages.length, 1);
            assert.strictEqual(passages[0].name, 'MyPassage');
            assert.deepStrictEqual(passages[0].tags, ['tag1', 'tag2']);
        });
    });
    // ─── StoryData and Start Detection ───────────────────────────
    describe('StoryData and Start detection', () => {
        let registry;
        let parser;
        beforeEach(() => {
            registry = new hookRegistry_1.HookRegistry();
            const fallback = new adapter_1.FallbackAdapter();
            registry.register('fallback', fallback);
            registry.setActiveFormat('fallback');
            parser = new parser_1.Parser(registry);
        });
        it('should detect StoryData passage by name (universal Twine concept)', () => {
            const raw = {
                name: 'StoryData', tags: [], body: '{"format":"SugarCube"}',
                startOffset: 0, endOffset: 30, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.StoryData);
        });
        it('should detect Start passage by name', () => {
            const raw = {
                name: 'Start', tags: [], body: 'Welcome',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Start);
        });
        it('should NOT classify random passage as StoryData', () => {
            const raw = {
                name: 'MyPassage', tags: [], body: 'text',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.Story);
        });
        it('should still detect StoryData even with tags', () => {
            // StoryData with tags — the [script]/[stylesheet] check happens first,
            // but StoryData typically won't have those tags
            const raw = {
                name: 'StoryData', tags: ['special'], body: '{}',
                startOffset: 0, endOffset: 10, rawLinks: [], bodyTokens: [],
            };
            // With FallbackAdapter, classifyPassage returns null, so
            // it falls through to name check → StoryData
            assert.strictEqual(parser.classifyPassageType(raw), hookTypes_1.PassageType.StoryData);
        });
    });
});
//# sourceMappingURL=parser.test.js.map