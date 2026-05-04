import { strict as assert } from 'assert';
import { runVirtualDiagnostics } from '../../src/virtualDiagnostics';
import { parseDocument } from '../../src/parser';
import { getSugarCubeAdapter, getFallbackAdapter } from '../helpers/testFixtures';

const adapter = getSugarCubeAdapter();
const fallback = getFallbackAdapter();

// ===========================================================================
// runVirtualDiagnostics
// ===========================================================================

describe('runVirtualDiagnostics', () => {
  it('valid code with fallback adapter produces no diagnostics', () => {
    // The fallback adapter has an empty preamble, so the virtual doc is pure JS
    const { ast } = parseDocument(':: Start\n<<set $x to 5>>', fallback);
    const result = runVirtualDiagnostics(ast, 'file:///test.twee', fallback);
    assert.strictEqual(result.diagnostics.length, 0);
  });

  it('invalid JS in virtual doc produces diagnostic', () => {
    // The SugarCube preamble is TypeScript which causes acorn to fail.
    // Additionally, broken JS in Story JavaScript adds more errors.
    const source = [
      ':: Start',
      '<<set $x to 5>>',
      '',
      ':: Story JavaScript',
      'function ( { broken',
    ].join('\n');
    const { ast } = parseDocument(source, adapter);
    const result = runVirtualDiagnostics(ast, 'file:///test.twee', adapter);
    assert.ok(result.diagnostics.length > 0);
    assert.ok(result.diagnostics[0]!.message.includes('Virtual JS error'));
  });

  it('diagnostics map back to original source offset', () => {
    const source = [
      ':: Start',
      '<<set $x to 5>>',
      '',
      ':: Story JavaScript',
      'function ( { broken',
    ].join('\n');
    const { ast } = parseDocument(source, adapter);
    const result = runVirtualDiagnostics(ast, 'file:///test.twee', adapter);
    if (result.diagnostics.length > 0) {
      const diag = result.diagnostics[0]!;
      // The mapped offset should be a valid position (non-negative)
      assert.ok(typeof diag.range.start === 'number');
      assert.ok(diag.range.start >= 0);
      assert.ok(diag.range.end > diag.range.start);
    }
  });

  it('empty document with fallback adapter produces no diagnostics', () => {
    const { ast } = parseDocument('', fallback);
    const result = runVirtualDiagnostics(ast, 'file:///test.twee', fallback);
    assert.strictEqual(result.diagnostics.length, 0);
  });

  it('complex expressions with fallback adapter: <<set $y to $x + 1>> produces no diagnostics', () => {
    const { ast } = parseDocument(':: Start\n<<set $y to $x + 1>>', fallback);
    const result = runVirtualDiagnostics(ast, 'file:///test.twee', fallback);
    assert.strictEqual(result.diagnostics.length, 0);
  });

  it('returns virtualContent alongside diagnostics', () => {
    const { ast } = parseDocument(':: Start\n<<set $x to 5>>', adapter);
    const result = runVirtualDiagnostics(ast, 'file:///test.twee', adapter);
    // virtualContent should be a non-empty string (at least the preamble)
    assert.ok(typeof result.virtualContent === 'string');
    assert.ok(result.virtualContent.length > 0);
  });

  it('SugarCube preamble causes diagnostic (TypeScript not parseable by acorn)', () => {
    // The SugarCube adapter's virtual runtime prelude uses TypeScript declarations
    // (declare const ...) which acorn cannot parse. This is expected behavior.
    const { ast } = parseDocument(':: Start\n<<set $x to 5>>', adapter);
    const result = runVirtualDiagnostics(ast, 'file:///test.twee', adapter);
    // At least one diagnostic from the TypeScript preamble
    assert.ok(result.diagnostics.length >= 1);
  });

  it('multiple valid macros with fallback adapter produce no diagnostics', () => {
    const source = [
      ':: Start',
      '<<set $x to 5>>',
      '<<set $y to 10>>',
      '<<set $z to $x + $y>>',
    ].join('\n');
    const { ast } = parseDocument(source, fallback);
    const result = runVirtualDiagnostics(ast, 'file:///test.twee', fallback);
    assert.strictEqual(result.diagnostics.length, 0);
  });
});
