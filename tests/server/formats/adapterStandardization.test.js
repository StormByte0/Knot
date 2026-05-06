"use strict";
/**
 * Knot v2 — Format Adapter Standardization Tests
 *
 * Every format adapter MUST export the same shape (IFormatProvider).
 * These tests verify that all adapters implement the required interface
 * consistently and return valid sub-providers.
 *
 * Tests cover:
 *   - FallbackAdapter, SugarCubeAdapter, HarloweAdapter
 *   - All required IFormatProvider properties
 *   - All sub-provider method existence
 *   - ISyntaxProvider method completeness
 *   - classifyVariableSigil correctness per format
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
const adapter_1 = require("../../../server/src/formats/fallback/adapter");
const adapter_2 = require("../../../server/src/formats/sugarcube/adapter");
const adapter_3 = require("../../../server/src/formats/harlowe/adapter");
const hookTypes_1 = require("../../../server/src/hooks/hookTypes");
// ─── Helpers ───────────────────────────────────────────────────
const REQUIRED_FORMAT_PROPS = [
    'formatId', 'formatName', 'formatVersion', 'capabilities',
];
const REQUIRED_SUB_PROVIDERS = [
    'getMacroProvider', 'getPassageProvider', 'getDiagnosticProvider',
    'getLinkProvider', 'getSyntaxProvider',
];
const SYNTAX_PROVIDER_METHODS = [
    'getMacroBodyStyle', 'lexBody', 'getMacroPattern', 'getVariablePattern',
    'getMacroTriggerChars', 'getVariableTriggerChars',
    'getMacroCallPrefix', 'getMacroCallSuffix',
    'getMacroClosePrefix', 'getMacroCloseSuffix',
    'classifyVariableSigil',
];
function assertImplementsIFormatProvider(adapter, name) {
    describe(`${name} implements IFormatProvider`, () => {
        for (const prop of REQUIRED_FORMAT_PROPS) {
            it(`should have ${prop}`, () => {
                assert.ok(adapter[prop] !== undefined, `${name} missing ${prop}`);
            });
        }
        it('should have formatId as a non-empty string', () => {
            assert.strictEqual(typeof adapter.formatId, 'string');
            assert.ok(adapter.formatId.length > 0);
        });
        it('should have formatName as a non-empty string', () => {
            assert.strictEqual(typeof adapter.formatName, 'string');
            assert.ok(adapter.formatName.length > 0);
        });
        it('should have formatVersion as a non-empty string', () => {
            assert.strictEqual(typeof adapter.formatVersion, 'string');
            assert.ok(adapter.formatVersion.length > 0);
        });
        it('should have capabilities as a Set<FormatCapability>', () => {
            assert.ok(adapter.capabilities instanceof Set);
        });
        for (const method of REQUIRED_SUB_PROVIDERS) {
            it(`should have ${method}() method`, () => {
                assert.strictEqual(typeof adapter[method], 'function');
            });
        }
        it('getMacroProvider() should return a non-null object with required methods', () => {
            const provider = adapter.getMacroProvider();
            assert.ok(provider);
            assert.strictEqual(typeof provider.getMacros, 'function');
            assert.strictEqual(typeof provider.getMacroByName, 'function');
            assert.strictEqual(typeof provider.isMacroKnown, 'function');
        });
        it('getPassageProvider() should return a non-null object with required methods', () => {
            const provider = adapter.getPassageProvider();
            assert.ok(provider);
            assert.strictEqual(typeof provider.getPassageTypes, 'function');
            assert.strictEqual(typeof provider.getSpecialTags, 'function');
            assert.strictEqual(typeof provider.getStoryDataPassageName, 'function');
            assert.strictEqual(typeof provider.getStartPassageName, 'function');
            assert.strictEqual(typeof provider.getPassageHeaderPattern, 'function');
            assert.strictEqual(typeof provider.classifyPassage, 'function');
        });
        it('getDiagnosticProvider() should return a non-null object', () => {
            const provider = adapter.getDiagnosticProvider();
            assert.ok(provider);
            assert.strictEqual(typeof provider.getSupportedRules, 'function');
        });
        it('getLinkProvider() should return a non-null object with required methods', () => {
            const provider = adapter.getLinkProvider();
            assert.ok(provider);
            assert.strictEqual(typeof provider.resolveLinkBody, 'function');
            assert.strictEqual(typeof provider.parseLinkSyntax, 'function');
            assert.strictEqual(typeof provider.resolveLinkTarget, 'function');
        });
        it('getSyntaxProvider() should return a non-null object', () => {
            const provider = adapter.getSyntaxProvider();
            assert.ok(provider);
        });
        describe(`${name} ISyntaxProvider completeness`, () => {
            let syntax;
            beforeEach(() => {
                syntax = adapter.getSyntaxProvider();
            });
            for (const method of SYNTAX_PROVIDER_METHODS) {
                it(`should implement ${method}()`, () => {
                    assert.strictEqual(typeof syntax[method], 'function', `${name} ISyntaxProvider missing ${method}()`);
                });
            }
            it('getMacroBodyStyle() should return a valid MacroBodyStyle', () => {
                const style = syntax.getMacroBodyStyle();
                const validStyles = [hookTypes_1.MacroBodyStyle.CloseTag, hookTypes_1.MacroBodyStyle.Hook, hookTypes_1.MacroBodyStyle.Inline];
                assert.ok(validStyles.includes(style), `Invalid MacroBodyStyle: ${style}`);
            });
            it('lexBody() should return an array', () => {
                const tokens = syntax.lexBody('some body text');
                assert.ok(Array.isArray(tokens));
            });
            it('getMacroTriggerChars() should return an array', () => {
                const chars = syntax.getMacroTriggerChars();
                assert.ok(Array.isArray(chars));
            });
            it('getVariableTriggerChars() should return an array', () => {
                const chars = syntax.getVariableTriggerChars();
                assert.ok(Array.isArray(chars));
            });
            it('getMacroCallPrefix() should return a string', () => {
                assert.strictEqual(typeof syntax.getMacroCallPrefix(), 'string');
            });
            it('getMacroCallSuffix() should return a string', () => {
                assert.strictEqual(typeof syntax.getMacroCallSuffix(), 'string');
            });
            it('getMacroClosePrefix() should return a string', () => {
                assert.strictEqual(typeof syntax.getMacroClosePrefix(), 'string');
            });
            it('getMacroCloseSuffix() should return a string', () => {
                assert.strictEqual(typeof syntax.getMacroCloseSuffix(), 'string');
            });
            it('classifyVariableSigil() should return story, temp, or null', () => {
                const result = syntax.classifyVariableSigil('x');
                assert.ok(result === 'story' || result === 'temp' || result === null, `classifyVariableSigil returned invalid value: ${result}`);
            });
        });
    });
}
// ─── Test All Adapters ─────────────────────────────────────────
describe('Format Adapter Standardization', () => {
    describe('FallbackAdapter', () => {
        const adapter = new adapter_1.FallbackAdapter();
        assertImplementsIFormatProvider(adapter, 'FallbackAdapter');
        describe('FallbackAdapter-specific behavior', () => {
            it('formatId should be "fallback"', () => {
                assert.strictEqual(adapter.formatId, 'fallback');
            });
            it('MacroBodyStyle should be Inline', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroBodyStyle(), hookTypes_1.MacroBodyStyle.Inline);
            });
            it('getMacroPattern() should return null', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroPattern(), null);
            });
            it('getVariablePattern() should return null', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getVariablePattern(), null);
            });
            it('getMacroTriggerChars() should return empty array', () => {
                assert.deepStrictEqual(adapter.getSyntaxProvider().getMacroTriggerChars(), []);
            });
            it('getVariableTriggerChars() should return empty array', () => {
                assert.deepStrictEqual(adapter.getSyntaxProvider().getVariableTriggerChars(), []);
            });
            it('classifyVariableSigil should return null for everything', () => {
                const syntax = adapter.getSyntaxProvider();
                assert.strictEqual(syntax.classifyVariableSigil('$'), null);
                assert.strictEqual(syntax.classifyVariableSigil('_'), null);
                assert.strictEqual(syntax.classifyVariableSigil('x'), null);
            });
            it('lexBody should return empty array', () => {
                assert.deepStrictEqual(adapter.getSyntaxProvider().lexBody('text'), []);
            });
        });
    });
    describe('SugarCubeAdapter', () => {
        const adapter = new adapter_2.SugarCubeAdapter();
        assertImplementsIFormatProvider(adapter, 'SugarCubeAdapter');
        describe('SugarCubeAdapter-specific behavior', () => {
            it('formatId should be "sugarcube-2"', () => {
                assert.strictEqual(adapter.formatId, 'sugarcube-2');
            });
            it('MacroBodyStyle should be CloseTag', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroBodyStyle(), hookTypes_1.MacroBodyStyle.CloseTag);
            });
            it('getMacroPattern() should return a RegExp', () => {
                assert.ok(adapter.getSyntaxProvider().getMacroPattern() instanceof RegExp);
            });
            it('getVariablePattern() should return a RegExp', () => {
                assert.ok(adapter.getSyntaxProvider().getVariablePattern() instanceof RegExp);
            });
            it('getMacroTriggerChars() should return ["<"]', () => {
                assert.deepStrictEqual(adapter.getSyntaxProvider().getMacroTriggerChars(), ['<']);
            });
            it('getVariableTriggerChars() should return ["$", "_"]', () => {
                assert.deepStrictEqual(adapter.getSyntaxProvider().getVariableTriggerChars(), ['$', '_']);
            });
            it('getMacroCallPrefix() should return "<<"', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroCallPrefix(), '<<');
            });
            it('getMacroCallSuffix() should return ">>"', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroCallSuffix(), '>>');
            });
            it('getMacroClosePrefix() should return "<</"', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroClosePrefix(), '<</');
            });
            it('getMacroCloseSuffix() should return ">>"', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroCloseSuffix(), '>>');
            });
            it('classifyVariableSigil: "$" → "story"', () => {
                assert.strictEqual(adapter.getSyntaxProvider().classifyVariableSigil('$'), 'story');
            });
            it('classifyVariableSigil: "_" → "temp"', () => {
                assert.strictEqual(adapter.getSyntaxProvider().classifyVariableSigil('_'), 'temp');
            });
            it('classifyVariableSigil: "x" → null', () => {
                assert.strictEqual(adapter.getSyntaxProvider().classifyVariableSigil('x'), null);
            });
            it('should have macros in the macro provider', () => {
                const macros = adapter.getMacroProvider().getMacros();
                assert.ok(macros.length > 0, 'SugarCube should have macros defined');
            });
            it('should know common macros (set, if, print)', () => {
                const provider = adapter.getMacroProvider();
                assert.ok(provider.isMacroKnown('set'));
                assert.ok(provider.isMacroKnown('if'));
                assert.ok(provider.isMacroKnown('print'));
            });
            it('should have BodyLexing capability', () => {
                assert.ok(adapter.capabilities.has(hookTypes_1.FormatCapability.BodyLexing));
            });
        });
    });
    describe('HarloweAdapter', () => {
        const adapter = new adapter_3.HarloweAdapter();
        assertImplementsIFormatProvider(adapter, 'HarloweAdapter');
        describe('HarloweAdapter-specific behavior', () => {
            it('formatId should be "harlowe-3"', () => {
                assert.strictEqual(adapter.formatId, 'harlowe-3');
            });
            it('MacroBodyStyle should be Hook', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroBodyStyle(), hookTypes_1.MacroBodyStyle.Hook);
            });
            it('getMacroPattern() should return a RegExp', () => {
                assert.ok(adapter.getSyntaxProvider().getMacroPattern() instanceof RegExp);
            });
            it('getVariablePattern() should return a RegExp', () => {
                assert.ok(adapter.getSyntaxProvider().getVariablePattern() instanceof RegExp);
            });
            it('getMacroTriggerChars() should return ["("]', () => {
                assert.deepStrictEqual(adapter.getSyntaxProvider().getMacroTriggerChars(), ['(']);
            });
            it('getVariableTriggerChars() should return ["$", "_"]', () => {
                assert.deepStrictEqual(adapter.getSyntaxProvider().getVariableTriggerChars(), ['$', '_']);
            });
            it('getMacroCallPrefix() should return "("', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroCallPrefix(), '(');
            });
            it('getMacroCallSuffix() should return ")"', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroCallSuffix(), ')');
            });
            it('getMacroClosePrefix() should return "" (Harlowe uses hooks, not close tags)', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroClosePrefix(), '');
            });
            it('getMacroCloseSuffix() should return ""', () => {
                assert.strictEqual(adapter.getSyntaxProvider().getMacroCloseSuffix(), '');
            });
            it('classifyVariableSigil: "$" → "story"', () => {
                assert.strictEqual(adapter.getSyntaxProvider().classifyVariableSigil('$'), 'story');
            });
            it('classifyVariableSigil: "_" → null (Harlowe has no temp sigil)', () => {
                assert.strictEqual(adapter.getSyntaxProvider().classifyVariableSigil('_'), null);
            });
            it('classifyVariableSigil: "x" → null', () => {
                assert.strictEqual(adapter.getSyntaxProvider().classifyVariableSigil('x'), null);
            });
            it('should have macros in the macro provider', () => {
                const macros = adapter.getMacroProvider().getMacros();
                assert.ok(macros.length > 0, 'Harlowe should have macros defined');
            });
            it('should know common macros (set:, if:, print:)', () => {
                const provider = adapter.getMacroProvider();
                assert.ok(provider.isMacroKnown('set:'));
                assert.ok(provider.isMacroKnown('if:'));
                assert.ok(provider.isMacroKnown('print:'));
            });
            it('should have HookCompletion capability', () => {
                assert.ok(adapter.capabilities.has(hookTypes_1.FormatCapability.HookCompletion));
            });
            it('should have ChangerCompletion capability', () => {
                assert.ok(adapter.capabilities.has(hookTypes_1.FormatCapability.ChangerCompletion));
            });
        });
    });
    // ─── Cross-Adapter Consistency ───────────────────────────────
    describe('Cross-adapter consistency', () => {
        const adapters = [
            new adapter_1.FallbackAdapter(),
            new adapter_2.SugarCubeAdapter(),
            new adapter_3.HarloweAdapter(),
        ];
        it('all adapters should have unique formatIds', () => {
            const ids = adapters.map(a => a.formatId);
            const uniqueIds = new Set(ids);
            assert.strictEqual(ids.length, uniqueIds.size, 'All formatIds should be unique');
        });
        it('all adapters should return non-null sub-providers from every get method', () => {
            for (const adapter of adapters) {
                assert.ok(adapter.getMacroProvider(), `${adapter.formatId}: getMacroProvider() returned null`);
                assert.ok(adapter.getPassageProvider(), `${adapter.formatId}: getPassageProvider() returned null`);
                assert.ok(adapter.getDiagnosticProvider(), `${adapter.formatId}: getDiagnosticProvider() returned null`);
                assert.ok(adapter.getLinkProvider(), `${adapter.formatId}: getLinkProvider() returned null`);
                assert.ok(adapter.getSyntaxProvider(), `${adapter.formatId}: getSyntaxProvider() returned null`);
            }
        });
        it('all adapters should have getStoryDataPassageName returning a string', () => {
            for (const adapter of adapters) {
                const name = adapter.getPassageProvider().getStoryDataPassageName();
                assert.strictEqual(typeof name, 'string', `${adapter.formatId}: getStoryDataPassageName should return string`);
                assert.ok(name.length > 0, `${adapter.formatId}: getStoryDataPassageName should be non-empty`);
            }
        });
        it('all adapters should have getStartPassageName returning a string', () => {
            for (const adapter of adapters) {
                const name = adapter.getPassageProvider().getStartPassageName();
                assert.strictEqual(typeof name, 'string', `${adapter.formatId}: getStartPassageName should return string`);
                assert.ok(name.length > 0, `${adapter.formatId}: getStartPassageName should be non-empty`);
            }
        });
        it('all adapters should have classifyPassage returning PassageKind or null', () => {
            for (const adapter of adapters) {
                const result = adapter.getPassageProvider().classifyPassage('Test', []);
                assert.ok(result === null || typeof result === 'string', `${adapter.formatId}: classifyPassage should return PassageKind or null`);
            }
        });
        it('all adapters should have lexBody returning an array', () => {
            for (const adapter of adapters) {
                const tokens = adapter.getSyntaxProvider().lexBody('test body');
                assert.ok(Array.isArray(tokens), `${adapter.formatId}: lexBody should return array`);
            }
        });
        it('all adapters should have resolveLinkBody returning ParsedLink or undefined', () => {
            for (const adapter of adapters) {
                const result = adapter.getLinkProvider().resolveLinkBody('Target');
                assert.ok(result === undefined || (result && typeof result.kind === 'string' && typeof result.target === 'string'), `${adapter.formatId}: resolveLinkBody should return ParsedLink or undefined`);
            }
        });
    });
});
//# sourceMappingURL=adapterStandardization.test.js.map