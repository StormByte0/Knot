import { strict as assert } from 'assert';
import { passageNameFromExpr } from '../../src/passageArgs';

describe('passageNameFromExpr', () => {
  it('returns the string value for literal string expressions', () => {
    const expr = { type: 'literal' as const, kind: 'string' as const, value: 'Target' };
    assert.strictEqual(passageNameFromExpr(expr), 'Target');
  });

  it('returns null for non-string literals', () => {
    const expr = { type: 'literal' as const, kind: 'number' as const, value: 5 };
    assert.strictEqual(passageNameFromExpr(expr), null);
  });

  it('returns null for non-literal types', () => {
    const expr = { type: 'storyVar' as const, name: 'x' };
    assert.strictEqual(passageNameFromExpr(expr), null);
  });

  it('returns null for empty object', () => {
    const expr = {} as { type: string; kind?: string; value?: unknown };
    assert.strictEqual(passageNameFromExpr(expr), null);
  });

  it('handles empty string value', () => {
    const expr = { type: 'literal' as const, kind: 'string' as const, value: '' };
    assert.strictEqual(passageNameFromExpr(expr), '');
  });

  it('returns null for boolean literal', () => {
    const expr = { type: 'literal' as const, kind: 'boolean' as const, value: true };
    assert.strictEqual(passageNameFromExpr(expr), null);
  });

  it('returns null for null literal', () => {
    const expr = { type: 'literal' as const, kind: 'null' as const, value: null };
    assert.strictEqual(passageNameFromExpr(expr), null);
  });

  it('returns null for identifier', () => {
    const expr = { type: 'identifier' as const, name: 'someVar' };
    assert.strictEqual(passageNameFromExpr(expr), null);
  });

  it('returns null for binaryOp', () => {
    const left = { type: 'literal' as const, kind: 'string' as const, value: 'a' };
    const right = { type: 'literal' as const, kind: 'string' as const, value: 'b' };
    const expr = { type: 'binaryOp' as const, operator: '+', left, right };
    assert.strictEqual(passageNameFromExpr(expr), null);
  });
});
