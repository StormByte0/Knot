import { strict as assert } from 'assert';
import { TypeInference } from '../../src/typeInference';
import type { InferredType } from '../../src/typeInference';
import { parseDocument } from '../../src/parser';
import { getSugarCubeAdapter } from '../helpers/testFixtures';
import type { ExpressionNode, LiteralNode, BinaryOpNode, ArrayLiteralNode, ObjectLiteralNode, StoryVarNode, UnaryOpNode } from '../../src/ast';

const adapter = getSugarCubeAdapter();

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Run inferDocument on a twee source and return the result. */
function inferDoc(source: string, useAdapter = true) {
  const { ast } = parseDocument(source, useAdapter ? adapter : undefined);
  const ti = new TypeInference();
  return ti.inferDocument(ast, useAdapter ? adapter : undefined);
}

/** Build an expression node of a given type for direct infer() testing. */
function makeLiteral(kind: LiteralNode['kind'], value: LiteralNode['value']): LiteralNode {
  return { type: 'literal', kind, value, range: { start: 0, end: 1 } };
}

function makeStoryVar(name: string): StoryVarNode {
  return { type: 'storyVar', name, range: { start: 0, end: 1 } };
}

function makeBinaryOp(operator: string, left: ExpressionNode, right: ExpressionNode): BinaryOpNode {
  return { type: 'binaryOp', operator, left, right, range: { start: 0, end: 1 } };
}

function makeArrayLiteral(elements: ExpressionNode[]): ArrayLiteralNode {
  return { type: 'arrayLiteral', elements, range: { start: 0, end: 1 } };
}

function makeObjectLiteral(props: { key: string; value: ExpressionNode }[]): ObjectLiteralNode {
  return {
    type: 'objectLiteral',
    properties: props.map(p => ({ key: p.key, value: p.value, range: { start: 0, end: 1 } })),
    range: { start: 0, end: 1 },
  };
}

// ===========================================================================
// TypeInference — inferDocument
// ===========================================================================

describe('TypeInference — inferDocument', () => {
  it('infers variable types from <<set>> assignments ($x to 5 → number, $name to "hello" → string)', () => {
    const result = inferDoc(':: Start\n<<set $x to 5>>\n<<set $name to "hello">>');
    assert.strictEqual(result.assignments.get('x')?.kind, 'number');
    assert.strictEqual(result.assignments.get('name')?.kind, 'string');
  });

  it('first-write-wins for variable types', () => {
    // First assignment: $x to 5 (number). Second: $x to "hi" (string).
    // First-write-wins means the type stays 'number'.
    const result = inferDoc(':: Start\n<<set $x to 5>>\n<<set $x to "hi">>');
    assert.strictEqual(result.assignments.get('x')?.kind, 'number');
  });

  it('infers nested object types: <<set $obj to {a: 1, b: "hi"}>>', () => {
    const result = inferDoc(':: Start\n<<set $obj to {a: 1, b: "hi"}>>');
    const objType = result.assignments.get('obj');
    assert.strictEqual(objType?.kind, 'object');
    assert.strictEqual(objType?.properties?.['a']?.kind, 'number');
    assert.strictEqual(objType?.properties?.['b']?.kind, 'string');
  });

  it('infers array types: <<set $arr to [1, 2, 3]>>', () => {
    const result = inferDoc(':: Start\n<<set $arr to [1, 2, 3]>>');
    const arrType = result.assignments.get('arr');
    assert.strictEqual(arrType?.kind, 'array');
    assert.strictEqual(arrType?.elements?.kind, 'number');
  });

  it('collects JS globals from Story JavaScript passage', () => {
    const result = inferDoc(':: Story JavaScript\nvar myGlobal = 42;');
    assert.ok(result.jsGlobals.has('myGlobal'));
    assert.strictEqual(result.jsGlobals.get('myGlobal')?.inferredType.kind, 'number');
  });

  it('collects JS globals from <<script>> blocks', () => {
    const result = inferDoc(':: Start\n<<script>>\nvar inlineVar = true;\n<</script>>');
    assert.ok(result.jsGlobals.has('inlineVar'));
    assert.strictEqual(result.jsGlobals.get('inlineVar')?.inferredType.kind, 'boolean');
  });

  it('analysis ordering: StoryInit before regular passages', () => {
    // StoryInit assigns $health to 100. A later passage reassigns $health to "low".
    // Because StoryInit runs first, the type should be 'number' (first-write-wins).
    const source = [
      ':: Later',
      '<<set $health to "low">>',
      '',
      ':: StoryInit',
      '<<set $health to 100>>',
    ].join('\n');
    const result = inferDoc(source);
    assert.strictEqual(result.assignments.get('health')?.kind, 'number');
  });

  it('works without adapter (fallback behavior)', () => {
    // Without adapter, the fallback only recognizes 'set' as assignment macro
    // and 'to'/'=' as assignment operators (same defaults).
    const result = inferDoc(':: Start\n<<set $x to 5>>', false);
    assert.strictEqual(result.assignments.get('x')?.kind, 'number');
  });
});

// ===========================================================================
// TypeInference — infer (single expression)
// ===========================================================================

describe('TypeInference — infer', () => {
  const ti = new TypeInference();

  it('number literal → { kind: "number" }', () => {
    const result = ti.infer(makeLiteral('number', 42));
    assert.deepStrictEqual(result, { kind: 'number' });
  });

  it('string literal → { kind: "string" }', () => {
    const result = ti.infer(makeLiteral('string', 'hello'));
    assert.deepStrictEqual(result, { kind: 'string' });
  });

  it('boolean literal → { kind: "boolean" }', () => {
    const result = ti.infer(makeLiteral('boolean', true));
    assert.deepStrictEqual(result, { kind: 'boolean' });
  });

  it('null literal → { kind: "null" }', () => {
    const result = ti.infer(makeLiteral('null', null));
    assert.deepStrictEqual(result, { kind: 'null' });
  });

  it('array literal → { kind: "array", elements: inferred type }', () => {
    const arr = makeArrayLiteral([makeLiteral('number', 1), makeLiteral('number', 2)]);
    const result = ti.infer(arr);
    assert.strictEqual(result.kind, 'array');
    assert.strictEqual(result.elements?.kind, 'number');
  });

  it('object literal → { kind: "object", properties: {...} }', () => {
    const obj = makeObjectLiteral([
      { key: 'age', value: makeLiteral('number', 25) },
      { key: 'name', value: makeLiteral('string', 'Ada') },
    ]);
    const result = ti.infer(obj);
    assert.strictEqual(result.kind, 'object');
    assert.strictEqual(result.properties?.['age']?.kind, 'number');
    assert.strictEqual(result.properties?.['name']?.kind, 'string');
  });

  it('assignment operator (to) → infers right side', () => {
    const expr = makeBinaryOp('to', makeStoryVar('x'), makeLiteral('number', 10));
    const result = ti.infer(expr);
    assert.strictEqual(result.kind, 'number');
  });

  it('assignment operator (=) → infers right side', () => {
    const expr = makeBinaryOp('=', makeStoryVar('y'), makeLiteral('string', 'hi'));
    const result = ti.infer(expr);
    assert.strictEqual(result.kind, 'string');
  });

  it('non-assignment binaryOp → { kind: "unknown" }', () => {
    const expr = makeBinaryOp('+', makeLiteral('number', 1), makeLiteral('number', 2));
    const result = ti.infer(expr);
    assert.deepStrictEqual(result, { kind: 'unknown' });
  });

  it('unknown expression → { kind: "unknown" }', () => {
    // storyVar is a leaf node that isn't a literal → unknown
    const result = ti.infer(makeStoryVar('z'));
    assert.deepStrictEqual(result, { kind: 'unknown' });
  });
});
