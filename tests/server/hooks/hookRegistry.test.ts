/**
 * Knot v2 — HookRegistry Tests
 *
 * Tests that the hook registry correctly manages format providers
 * and that core code can resolve providers without format knowledge.
 */

import * as assert from 'assert';
import { HookRegistry } from '../../../server/src/hooks/hookRegistry';
import { MockFormatProvider } from '../../helpers/testFixtures';
import { IFormatProvider } from '../../../server/src/hooks/formatHooks';

describe('HookRegistry', () => {
  let registry: HookRegistry;
  let mockProvider: MockFormatProvider;

  beforeEach(() => {
    registry = new HookRegistry();
    mockProvider = new MockFormatProvider();
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
    const mock2 = new MockFormatProvider();
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
