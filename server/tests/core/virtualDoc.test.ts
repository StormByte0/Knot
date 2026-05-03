import { strict as assert } from 'assert';
import { VirtualDocGenerator } from '../../src/virtualDoc';
import type { VirtualDoc, MappingEntry } from '../../src/virtualDoc';
import { parseDocument } from '../../src/parser';
import { getSugarCubeAdapter } from '../helpers/testFixtures';
import type {
  ExpressionNode,
  LiteralNode,
  StoryVarNode,
  TempVarNode,
  BinaryOpNode,
  UnaryOpNode,
  PropertyAccessNode,
  IndexAccessNode,
  CallNode,
  ArrayLiteralNode,
  ObjectLiteralNode,
  IdentifierNode,
} from '../../src/ast';

const adapter = getSugarCubeAdapter();
const gen = new VirtualDocGenerator(adapter);

// ---------------------------------------------------------------------------
// Expression node builders
// ---------------------------------------------------------------------------

function makeLiteral(kind: LiteralNode['kind'], value: LiteralNode['value']): LiteralNode {
  return { type: 'literal', kind, value, range: { start: 0, end: 1 } };
}

function makeStoryVar(name: string): StoryVarNode {
  return { type: 'storyVar', name, range: { start: 0, end: 1 } };
}

function makeTempVar(name: string): TempVarNode {
  return { type: 'tempVar', name, range: { start: 0, end: 1 } };
}

function makeIdentifier(name: string): IdentifierNode {
  return { type: 'identifier', name, range: { start: 0, end: 1 } };
}

function makeBinaryOp(operator: string, left: ExpressionNode, right: ExpressionNode): BinaryOpNode {
  return { type: 'binaryOp', operator, left, right, range: { start: 0, end: 1 } };
}

function makeUnaryOp(operator: string, operand: ExpressionNode): UnaryOpNode {
  return { type: 'unaryOp', operator, operand, range: { start: 0, end: 1 } };
}

function makePropertyAccess(object: ExpressionNode, property: string): PropertyAccessNode {
  return { type: 'propertyAccess', object, property, propertyRange: { start: 0, end: 1 }, range: { start: 0, end: 1 } };
}

function makeIndexAccess(object: ExpressionNode, index: ExpressionNode): IndexAccessNode {
  return { type: 'indexAccess', object, index, range: { start: 0, end: 1 } };
}

function makeCall(callee: ExpressionNode, args: ExpressionNode[]): CallNode {
  return { type: 'call', callee, args, range: { start: 0, end: 1 } };
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
// VirtualDocGenerator — generate
// ===========================================================================

describe('VirtualDocGenerator — generate', () => {
  it('generates JS from simple macro: <<set $x to 5>> → includes State.variables.x = 5', () => {
    const { ast } = parseDocument(':: Start\n<<set $x to 5>>', adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    assert.ok(doc.content.includes('State.variables.x = 5'));
  });

  it('includes runtime preamble from adapter', () => {
    const { ast } = parseDocument(':: Start\nHello', adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    const preamble = adapter.getVirtualRuntimePrelude();
    assert.ok(doc.content.startsWith(preamble));
  });

  it('skips stylesheet passages', () => {
    const source = [
      ':: Start',
      '<<set $x to 1>>',
      '',
      ':: Story Stylesheet',
      'body { background-color: #ff0000; }',
    ].join('\n');
    const { ast } = parseDocument(source, adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    // CSS-specific tokens should not appear in the virtual JS document
    assert.ok(!doc.content.includes('background-color'));
    assert.ok(!doc.content.includes('#ff0000'));
  });

  it('emits script passages directly', () => {
    const source = [
      ':: Start',
      '<<set $x to 1>>',
      '',
      ':: Story JavaScript',
      'const g = 42;',
    ].join('\n');
    const { ast } = parseDocument(source, adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    assert.ok(doc.content.includes('const g = 42;'));
  });

  it('emits markup expressions as semicolon-terminated statements', () => {
    const { ast } = parseDocument(':: Start\n<<set $x to 5>>', adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    // Each expression should be terminated with ;\n
    assert.ok(doc.content.includes('State.variables.x = 5;\n'));
  });

  it('handles nested macro bodies', () => {
    const source = ':: Start\n<<if true>><<set $x to 1>><</if>>';
    const { ast } = parseDocument(source, adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    // The if arg (true) and the set arg ($x to 1) should both be emitted
    assert.ok(doc.content.includes('true'));
    assert.ok(doc.content.includes('State.variables.x = 1'));
  });

  it('empty document: only preamble', () => {
    const { ast } = parseDocument('', adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    const preamble = adapter.getVirtualRuntimePrelude();
    assert.strictEqual(doc.content, preamble);
  });

  it('mapping entries have valid ranges', () => {
    const { ast } = parseDocument(':: Start\n<<set $x to 5>>', adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    assert.ok(doc.map.length > 0);
    for (const entry of doc.map) {
      assert.ok(entry.virtualStart >= 0);
      assert.ok(entry.length > 0);
      assert.ok(entry.originalStart >= 0);
      assert.strictEqual(typeof entry.uri, 'string');
    }
  });
});

// ===========================================================================
// VirtualDocGenerator — mapToOriginal
// ===========================================================================

describe('VirtualDocGenerator — mapToOriginal', () => {
  it('maps virtual offset back to original offset', () => {
    const { ast } = parseDocument(':: Start\n<<set $x to 5>>', adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    // Pick a virtual offset within the first mapped region after preamble
    const preambleLen = adapter.getVirtualRuntimePrelude().length;
    if (doc.map.length > 1) {
      // Second mapping entry should be an expression
      const entry = doc.map[1]!;
      const virtualOff = entry.virtualStart + 2;
      const result = gen.mapToOriginal(doc, virtualOff);
      assert.strictEqual(result, entry.originalStart + 2);
    }
  });

  it('returns null for offsets outside mapping', () => {
    const { ast } = parseDocument(':: Start\n<<set $x to 5>>', adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    // Offset beyond content length
    const result = gen.mapToOriginal(doc, doc.content.length + 100);
    assert.strictEqual(result, null);
  });

  it('handles preamble offset (mapped to 0)', () => {
    const { ast } = parseDocument(':: Start\n<<set $x to 5>>', adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    // Offset 0 is the start of the preamble mapping
    if (doc.map.length > 0) {
      const firstEntry = doc.map[0]!;
      // If the first entry is the preamble, offset 0 should map
      if (firstEntry.virtualStart === 0) {
        const result = gen.mapToOriginal(doc, 0);
        assert.strictEqual(result, firstEntry.originalStart);
      }
    }
  });

  it('binary search works for non-trivial documents', () => {
    const source = [
      ':: Start',
      '<<set $x to 5>>',
      '<<set $y to 10>>',
      '<<set $z to 15>>',
      '',
      ':: Next',
      '<<print $x>>',
      '',
      ':: Story JavaScript',
      'const g = 1;',
    ].join('\n');
    const { ast } = parseDocument(source, adapter);
    const doc = gen.generate(ast, 'file:///test.twee');
    assert.ok(doc.map.length > 3);
    // Verify every virtual offset in each mapping entry maps correctly
    for (const entry of doc.map) {
      const midOffset = entry.virtualStart + Math.floor(entry.length / 2);
      const result = gen.mapToOriginal(doc, midOffset);
      assert.strictEqual(result, entry.originalStart + Math.floor(entry.length / 2));
    }
  });
});

// ===========================================================================
// VirtualDocGenerator — emitExpression
// ===========================================================================

describe('VirtualDocGenerator — emitExpression', () => {
  it('storyVar → State.variables.name', () => {
    const result = gen.emitExpression(makeStoryVar('score'));
    assert.strictEqual(result, 'State.variables.score');
  });

  it('tempVar → temporary.name', () => {
    const result = gen.emitExpression(makeTempVar('tmp'));
    assert.strictEqual(result, 'temporary.tmp');
  });

  it('literal number → string representation', () => {
    const result = gen.emitExpression(makeLiteral('number', 42));
    assert.strictEqual(result, '42');
  });

  it('literal string → JSON.stringify\'d', () => {
    const result = gen.emitExpression(makeLiteral('string', 'hello'));
    assert.strictEqual(result, '"hello"');
  });

  it('binaryOp with sugar operator normalization (to → =)', () => {
    const expr = makeBinaryOp('to', makeStoryVar('x'), makeLiteral('number', 5));
    const result = gen.emitExpression(expr);
    assert.strictEqual(result, 'State.variables.x = 5');
  });

  it('binaryOp with sugar operator normalization (eq → ===)', () => {
    const expr = makeBinaryOp('eq', makeStoryVar('x'), makeLiteral('number', 5));
    const result = gen.emitExpression(expr);
    assert.strictEqual(result, 'State.variables.x === 5');
  });

  it('binaryOp with sugar operator normalization (and → &&)', () => {
    const expr = makeBinaryOp('and', makeStoryVar('a'), makeStoryVar('b'));
    const result = gen.emitExpression(expr);
    assert.strictEqual(result, 'State.variables.a && State.variables.b');
  });

  it('unaryOp with not → !', () => {
    const expr = makeUnaryOp('not', makeStoryVar('flag'));
    const result = gen.emitExpression(expr);
    assert.strictEqual(result, '!State.variables.flag');
  });

  it('propertyAccess → obj.prop', () => {
    const expr = makePropertyAccess(makeStoryVar('obj'), 'name');
    const result = gen.emitExpression(expr);
    assert.strictEqual(result, 'State.variables.obj.name');
  });

  it('indexAccess → obj[idx]', () => {
    const expr = makeIndexAccess(makeStoryVar('arr'), makeLiteral('number', 0));
    const result = gen.emitExpression(expr);
    assert.strictEqual(result, 'State.variables.arr[0]');
  });

  it('call → func(a, b)', () => {
    const expr = makeCall(makeIdentifier('myFunc'), [makeLiteral('number', 1), makeLiteral('number', 2)]);
    const result = gen.emitExpression(expr);
    assert.strictEqual(result, 'myFunc(1, 2)');
  });

  it('arrayLiteral → [el1, el2]', () => {
    const expr = makeArrayLiteral([makeLiteral('number', 1), makeLiteral('number', 2)]);
    const result = gen.emitExpression(expr);
    assert.strictEqual(result, '[1, 2]');
  });

  it('objectLiteral → { "key": val }', () => {
    const expr = makeObjectLiteral([
      { key: 'name', value: makeLiteral('string', 'Ada') },
      { key: 'age', value: makeLiteral('number', 30) },
    ]);
    const result = gen.emitExpression(expr);
    assert.strictEqual(result, '{ "name": "Ada", "age": 30 }');
  });
});

// ===========================================================================
// VirtualDocGenerator — normalizeOp (indirect through emitExpression)
// ===========================================================================

describe('VirtualDocGenerator — normalizeOp (indirect through emitExpression)', () => {
  // Build a binaryOp with a single storyVar left and literal right, test only the operator

  it('to → =', () => {
    const result = gen.emitExpression(makeBinaryOp('to', makeStoryVar('x'), makeLiteral('number', 1)));
    assert.ok(result.includes('='));
    assert.ok(!result.includes(' to '));
  });

  it('eq → ===', () => {
    const result = gen.emitExpression(makeBinaryOp('eq', makeStoryVar('x'), makeLiteral('number', 1)));
    assert.ok(result.includes('==='));
    assert.ok(!result.includes(' eq '));
  });

  it('neq → !==', () => {
    const result = gen.emitExpression(makeBinaryOp('neq', makeStoryVar('x'), makeLiteral('number', 1)));
    assert.ok(result.includes('!=='));
  });

  it('and → &&', () => {
    const result = gen.emitExpression(makeBinaryOp('and', makeStoryVar('x'), makeStoryVar('y')));
    assert.ok(result.includes('&&'));
  });

  it('or → ||', () => {
    const result = gen.emitExpression(makeBinaryOp('or', makeStoryVar('x'), makeStoryVar('y')));
    assert.ok(result.includes('||'));
  });

  it('not → ! (as unaryOp)', () => {
    const result = gen.emitExpression(makeUnaryOp('not', makeStoryVar('x')));
    assert.ok(result.startsWith('!'));
  });

  it('gt → >', () => {
    const result = gen.emitExpression(makeBinaryOp('gt', makeStoryVar('x'), makeLiteral('number', 5)));
    assert.ok(result.includes(' > '));
  });

  it('lt → <', () => {
    const result = gen.emitExpression(makeBinaryOp('lt', makeStoryVar('x'), makeLiteral('number', 5)));
    assert.ok(result.includes(' < '));
  });

  it('unknown op → passed through unchanged', () => {
    const result = gen.emitExpression(makeBinaryOp('+=', makeStoryVar('x'), makeLiteral('number', 1)));
    // '+=' is not in the normalization table, should pass through as-is
    assert.ok(result.includes('+='));
  });
});
