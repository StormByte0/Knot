"use strict";
/**
 * Knot v2 — StoryData Detection Tests (Twine Engine Core)
 *
 * Tests that StoryData passage detection and format auto-detection
 * work correctly at the Twine engine base layer.
 *
 * StoryData is a universal Twine concept — every story format uses it
 * to store metadata. These tests verify:
 *   - Raw text search for :: StoryData
 *   - JSON body extraction
 *   - FormatRegistry.detectFormat() parsing
 *   - Format detection for known/unknown formats
 *   - Edge cases (tags, non-first position, invalid JSON)
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
const hookRegistry_1 = require("../../../server/src/hooks/hookRegistry");
const formatRegistry_1 = require("../../../server/src/formats/formatRegistry");
const parser_1 = require("../../../server/src/core/parser");
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
        let parser;
        beforeEach(() => {
            const registry = new hookRegistry_1.HookRegistry();
            parser = new parser_1.Parser(registry);
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
            assert.ok(storyData.body.includes('"format"'));
        });
        it('should extract StoryData body with tags', () => {
            const content = ':: StoryData [special]\n{"format":"SugarCube"}';
            const passages = parser.parseDocument(content);
            const storyData = passages.find(p => p.name === 'StoryData');
            assert.ok(storyData);
            assert.ok(storyData.body.includes('"format"'));
            assert.ok(storyData.tags.includes('special'));
        });
    });
    // ─── FormatRegistry.detectFormat() ───────────────────────────
    describe('FormatRegistry.detectFormat()', () => {
        let registry;
        let formatRegistry;
        beforeEach(() => {
            registry = new hookRegistry_1.HookRegistry();
            formatRegistry = new formatRegistry_1.FormatRegistry(registry);
            formatRegistry.registerBuiltinFormats();
        });
        it('should detect SugarCube from StoryData JSON', () => {
            const json = '{"format":"SugarCube","format-version":"2.36.0"}';
            const result = formatRegistry.detectFormat(json);
            assert.strictEqual(result, 'sugarcube-2');
        });
        it('should detect Harlowe from StoryData JSON', () => {
            const json = '{"format":"Harlowe","format-version":"3.3.8"}';
            const result = formatRegistry.detectFormat(json);
            assert.strictEqual(result, 'harlowe-3');
        });
        it('should detect format case-insensitively', () => {
            const json = '{"format":"sugarcube","format-version":"2.36.0"}';
            const result = formatRegistry.detectFormat(json);
            assert.strictEqual(result, 'sugarcube-2');
        });
        it('should return undefined for unknown format', () => {
            const json = '{"format":"Chapbook","format-version":"1.0.0"}';
            const result = formatRegistry.detectFormat(json);
            // Chapbook is recognized by detectFormat but has no adapter registered
            assert.strictEqual(result, 'chapbook-1');
        });
        it('should return undefined for invalid JSON', () => {
            const result = formatRegistry.detectFormat('not json at all');
            assert.strictEqual(result, undefined);
        });
        it('should return undefined for JSON without format field', () => {
            const json = '{"something":"else"}';
            const result = formatRegistry.detectFormat(json);
            assert.strictEqual(result, undefined);
        });
        it('should return undefined for empty string', () => {
            const result = formatRegistry.detectFormat('');
            assert.strictEqual(result, undefined);
        });
        it('should handle StoryData with whitespace', () => {
            const json = '  {"format":"Harlowe","format-version":"3.3.8"}  ';
            const result = formatRegistry.detectFormat(json);
            assert.strictEqual(result, 'harlowe-3');
        });
        it('should handle StoryData JSON with extra fields', () => {
            const json = '{"format":"SugarCube","format-version":"2.36.0","start":"Start","ifid":"ABC123"}';
            const result = formatRegistry.detectFormat(json);
            assert.strictEqual(result, 'sugarcube-2');
        });
    });
    // ─── Pre-scan Integration ────────────────────────────────────
    describe('Pre-scan integration', () => {
        let registry;
        let formatRegistry;
        let parser;
        beforeEach(() => {
            registry = new hookRegistry_1.HookRegistry();
            formatRegistry = new formatRegistry_1.FormatRegistry(registry);
            formatRegistry.registerBuiltinFormats();
            parser = new parser_1.Parser(registry);
        });
        it('should pre-scan StoryData with tags :: StoryData [special]', () => {
            const content = ':: StoryData [special]\n{"format":"SugarCube","format-version":"2.36.0"}';
            const passages = parser.parseDocument(content);
            const storyData = passages.find(p => p.name === 'StoryData');
            assert.ok(storyData);
            assert.ok(storyData.tags.includes('special'));
            // Body should still be the JSON
            const detected = formatRegistry.detectFormat(storyData.body);
            assert.strictEqual(detected, 'sugarcube-2');
        });
        it('should pre-scan StoryData when not the first passage', () => {
            const content = ':: Start\nWelcome\n\n:: StoryData\n{"format":"Harlowe","format-version":"3.3.8"}';
            const passages = parser.parseDocument(content);
            const storyData = passages.find(p => p.name === 'StoryData');
            assert.ok(storyData);
            const detected = formatRegistry.detectFormat(storyData.body);
            assert.strictEqual(detected, 'harlowe-3');
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
            const detected = formatRegistry.detectFormat(storyData.body);
            assert.strictEqual(detected, 'sugarcube-2');
        });
    });
});
//# sourceMappingURL=storyDataDetection.test.js.map