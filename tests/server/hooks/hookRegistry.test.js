"use strict";
/**
 * Knot v2 — HookRegistry Tests
 *
 * Tests that the hook registry correctly manages format providers
 * and that core code can resolve providers without format knowledge.
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
const testFixtures_1 = require("../../helpers/testFixtures");
describe('HookRegistry', () => {
    let registry;
    let mockProvider;
    beforeEach(() => {
        registry = new hookRegistry_1.HookRegistry();
        mockProvider = new testFixtures_1.MockFormatProvider();
    });
    it('should register and retrieve a format provider', () => {
        registry.register('mock', mockProvider);
        const retrieved = registry.getProvider('mock');
        assert.strictEqual(retrieved, mockProvider);
    });
    it('should set and get active format', () => {
        registry.register('mock', mockProvider);
        registry.setActiveFormat('mock');
        const active = registry.getActiveProvider();
        assert.strictEqual(active, mockProvider);
    });
    it('should throw when setting unregistered format as active', () => {
        assert.throws(() => registry.setActiveFormat('nonexistent'));
    });
    it('should return undefined for unregistered format', () => {
        assert.strictEqual(registry.getProvider('nonexistent'), undefined);
    });
    it('should list all available formats', () => {
        registry.register('mock', mockProvider);
        const formats = registry.getAvailableFormats();
        assert.ok(formats.includes('mock'));
    });
    it('should unregister a format', () => {
        registry.register('mock', mockProvider);
        registry.setActiveFormat('mock');
        registry.unregister('mock');
        assert.strictEqual(registry.getProvider('mock'), undefined);
        assert.strictEqual(registry.getActiveProvider(), undefined);
    });
    it('should clear all providers', () => {
        registry.register('mock1', mockProvider);
        registry.register('mock2', mockProvider);
        registry.clear();
        assert.strictEqual(registry.getAvailableFormats().length, 0);
    });
    it('should replace existing provider on re-register', () => {
        const mock2 = new testFixtures_1.MockFormatProvider();
        registry.register('mock', mockProvider);
        registry.register('mock', mock2);
        assert.strictEqual(registry.getProvider('mock'), mock2);
    });
    it('hasProvider should work correctly', () => {
        assert.strictEqual(registry.hasProvider('mock'), false);
        registry.register('mock', mockProvider);
        assert.strictEqual(registry.hasProvider('mock'), true);
        assert.strictEqual(registry.hasProvider(), false); // no active
        registry.setActiveFormat('mock');
        assert.strictEqual(registry.hasProvider(), true); // has active
    });
});
//# sourceMappingURL=hookRegistry.test.js.map