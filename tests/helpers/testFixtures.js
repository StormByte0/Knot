"use strict";
/**
 * Knot v2 — Test Fixtures
 *
 * Shared mock providers for testing core/handler code.
 * CRITICAL: Tests for core/handlers MUST use these mocks,
 * never real format adapters. This enforces the boundary.
 */
Object.defineProperty(exports, "__esModule", { value: true });
exports.MockFormatProvider = exports.MockSyntaxProvider = exports.MockLinkProvider = exports.MockDiagnosticProvider = exports.MockPassageProvider = exports.MockMacroProvider = void 0;
exports.createSampleMacro = createSampleMacro;
const hookTypes_1 = require("../../server/src/hooks/hookTypes");
class MockMacroProvider {
    macros = new Map();
    addMacro(def) {
        this.macros.set(def.name, def);
        if (def.aliases)
            for (const alias of def.aliases)
                this.macros.set(alias, def);
    }
    getMacros() { return Array.from(new Set(this.macros.values())); }
    getMacroByName(name) { return this.macros.get(name); }
    isMacroKnown(name) { return this.macros.has(name); }
}
exports.MockMacroProvider = MockMacroProvider;
class MockPassageProvider {
    passageTypes = new Map();
    specialTags = [];
    storyDataName = 'StoryData';
    startName = 'Start';
    classifyFn = null;
    configure(opts) { Object.assign(this, opts); }
    getPassageTypes() { return this.passageTypes; }
    getSpecialTags() { return this.specialTags; }
    getStoryDataPassageName() { return this.storyDataName; }
    getStartPassageName() { return this.startName; }
    getPassageHeaderPattern() { return /^::\s*([^\[\]]+)(?:\s*\[([^\]]*)\])?\s*$/m; }
    getSpecialPassagePattern(_type) { return undefined; }
    classifyPassage(name, tags) {
        return this.classifyFn ? this.classifyFn(name, tags) : null;
    }
}
exports.MockPassageProvider = MockPassageProvider;
class MockDiagnosticProvider {
    supportedRules = [];
    addRule(rule, severity) { this.supportedRules.push(rule); }
    getSupportedRules() { return this.supportedRules; }
    getRuleSeverity(_rule) { return undefined; }
    checkMacroUsage(_name, _args) { return []; }
    checkPassageStructure(_pt, _c) { return []; }
}
exports.MockDiagnosticProvider = MockDiagnosticProvider;
class MockLinkProvider {
    getLinkKinds() { return [hookTypes_1.LinkKind.Passage]; }
    resolveLinkBody(rawBody) {
        const ra = rawBody.lastIndexOf('->');
        if (ra >= 0)
            return { kind: hookTypes_1.LinkKind.Passage, target: rawBody.substring(ra + 2).trim(), displayText: rawBody.substring(0, ra).trim() };
        return { kind: hookTypes_1.LinkKind.Passage, target: rawBody.trim() };
    }
    parseLinkSyntax(text) {
        const m = text.match(/^\[\[(.+?)\]\]$/);
        return m ? this.resolveLinkBody(m[1]) : undefined;
    }
    resolveLinkTarget(link) { return link.kind === hookTypes_1.LinkKind.Passage ? link.target : undefined; }
}
exports.MockLinkProvider = MockLinkProvider;
class MockSyntaxProvider {
    bodyStyle = hookTypes_1.MacroBodyStyle.Inline;
    macroPat = null;
    varPat = null;
    configure(opts) { Object.assign(this, opts); }
    getMacroBodyStyle() { return this.bodyStyle; }
    lexBody(_body) { return []; }
    getMacroPattern() { return this.macroPat; }
    getVariablePattern() { return this.varPat; }
    getMacroTriggerChars() { return []; }
    getVariableTriggerChars() { return []; }
    getMacroCallPrefix() { return ''; }
    getMacroCallSuffix() { return ''; }
    getMacroClosePrefix() { return ''; }
    getMacroCloseSuffix() { return ''; }
    classifyVariableSigil(_sigil) { return null; }
}
exports.MockSyntaxProvider = MockSyntaxProvider;
class MockFormatProvider {
    formatId = 'mock';
    formatName = 'Mock Format';
    formatVersion = '0.0.1';
    capabilities = new Set();
    macroProvider = new MockMacroProvider();
    passageProvider = new MockPassageProvider();
    diagnosticProvider = new MockDiagnosticProvider();
    linkProvider = new MockLinkProvider();
    syntaxProvider = new MockSyntaxProvider();
    getMacroProvider() { return this.macroProvider; }
    getPassageProvider() { return this.passageProvider; }
    getDiagnosticProvider() { return this.diagnosticProvider; }
    getLinkProvider() { return this.linkProvider; }
    getSyntaxProvider() { return this.syntaxProvider; }
}
exports.MockFormatProvider = MockFormatProvider;
function createSampleMacro(overrides) {
    return { name: 'testMacro', category: hookTypes_1.MacroCategory.Output, kind: hookTypes_1.MacroKind.Command, description: 'Test', signatures: [{ args: [{ name: 'arg1', type: 'string', required: true }] }], ...overrides };
}
//# sourceMappingURL=testFixtures.js.map