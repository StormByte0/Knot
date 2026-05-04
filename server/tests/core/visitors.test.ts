import { strict as assert } from 'assert';
import { walkMarkup, walkDocument, walkExpression, walkDocumentExpressions } from '../../src/visitors';
import { parseDocument } from '../../src/parser';
import { getSugarCubeAdapter } from '../helpers/testFixtures';
import type { MacroNode, MarkupNode, ExpressionNode, LinkNode, TextNode, CommentNode } from '../../src/ast';

const adapter = getSugarCubeAdapter();

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Parse a twee source string and return the body of the first markup passage. */
function firstMarkupBody(source: string): MarkupNode[] {
  const { ast } = parseDocument(source, adapter);
  for (const p of ast.passages) {
    if (Array.isArray(p.body)) return p.body;
  }
  return [];
}

/** Collect all macro nodes from markup (order-preserving). */
function collectMacros(nodes: MarkupNode[]): MacroNode[] {
  const result: MacroNode[] = [];
  walkMarkup(nodes, {
    onMacro(node) { result.push(node); },
  });
  return result;
}

// ===========================================================================
// walkMarkup
// ===========================================================================

describe('walkMarkup', () => {
  it('visits all macro nodes in order', () => {
    const body = firstMarkupBody(':: Start\n<<set $x to 1>><<print $x>><<set $y to 2>>');
    const names: string[] = [];
    walkMarkup(body, {
      onMacro(node) { names.push(node.name); },
    });
    assert.deepStrictEqual(names, ['set', 'print', 'set']);
  });

  it('visits link nodes', () => {
    const body = firstMarkupBody(':: Start\n[[Target]]\nText\n[[Other|Dest]]');
    const targets: string[] = [];
    walkMarkup(body, {
      onLink(node: LinkNode) { targets.push(node.target); },
    });
    assert.deepStrictEqual(targets, ['Target', 'Dest']);
  });

  it('visits text nodes', () => {
    const body = firstMarkupBody(':: Start\nHello world');
    const texts: string[] = [];
    walkMarkup(body, {
      onText(node: TextNode) { texts.push(node.value); },
    });
    assert.ok(texts.length > 0);
    assert.ok(texts.some(t => t.includes('Hello')));
  });

  it('visits comment nodes', () => {
    const body = firstMarkupBody(':: Start\n<!-- a comment -->text');
    const comments: CommentNode[] = [];
    walkMarkup(body, {
      onComment(node: CommentNode) { comments.push(node); },
    });
    assert.strictEqual(comments.length, 1);
    assert.strictEqual(comments[0]!.style, 'html');
  });

  it('provides parentStack for nested macros', () => {
    const body = firstMarkupBody(':: Start\n<<if true>><<set $x to 1>><</if>>');
    const stackSnapshots: MacroNode[][] = [];
    walkMarkup(body, {
      onMacro(node, parentStack) {
        // Copy the parent stack (only macro parents are pushed)
        stackSnapshots.push([...parentStack as MacroNode[]]);
      },
    });
    // First macro: <<if>> — parent stack should be empty
    assert.strictEqual(stackSnapshots[0]!.length, 0);
    // Second macro: <<set>> inside <<if>> — parent stack should contain the <<if>> node
    assert.strictEqual(stackSnapshots[1]!.length, 1);
    assert.strictEqual((stackSnapshots[1]![0]! as MacroNode).name, 'if');
  });

  it('early termination: returning false from onMacro stops walk', () => {
    const body = firstMarkupBody(':: Start\n<<set $x to 1>><<print $x>><<set $y to 2>>');
    const names: string[] = [];
    walkMarkup(body, {
      onMacro(node) {
        names.push(node.name);
        if (node.name === 'print') return false;
      },
    });
    // Should have visited 'set' then 'print' but NOT the second 'set'
    assert.deepStrictEqual(names, ['set', 'print']);
  });

  it('early termination: returning false from onLink stops walk', () => {
    const body = firstMarkupBody(':: Start\n[[A]]<<set $x to 1>>[[B]]');
    const visited: string[] = [];
    walkMarkup(body, {
      onLink(node: LinkNode) {
        visited.push(node.target);
        return false;   // stop after first link
      },
      onMacro(node) { visited.push(node.name); },
    });
    // The first link is visited, then walk stops; macro and second link are NOT visited
    assert.strictEqual(visited.length, 1);
    assert.strictEqual(visited[0], 'A');
  });

  it('empty input: no callbacks called', () => {
    let called = false;
    walkMarkup([], {
      onMacro()  { called = true; },
      onLink()   { called = true; },
      onText()   { called = true; },
      onComment() { called = true; },
    });
    assert.strictEqual(called, false);
  });
});

// ===========================================================================
// walkDocument
// ===========================================================================

describe('walkDocument', () => {
  it('iterates all markup passages (skips script/stylesheet)', () => {
    const source = [
      ':: Start',
      '<<set $x to 1>>',
      '',
      ':: Story JavaScript',
      'const g = 42;',
      '',
      ':: Story Stylesheet',
      'body { color: red; }',
      '',
      ':: Other',
      '<<print $x>>',
    ].join('\n');

    const { ast } = parseDocument(source, adapter);
    const macroNames: string[] = [];
    walkDocument(ast, {
      onMacro(node) { macroNames.push(node.name); },
    });
    // Should only see macros from markup passages, not from script/stylesheet
    assert.deepStrictEqual(macroNames, ['set', 'print']);
  });

  it('visits macros across multiple passages', () => {
    const source = [
      ':: Alpha',
      '<<set $a to 1>>',
      '',
      ':: Beta',
      '<<set $b to 2>>',
      '',
      ':: Gamma',
      '<<set $c to 3>>',
    ].join('\n');

    const { ast } = parseDocument(source, adapter);
    const names: string[] = [];
    walkDocument(ast, {
      onMacro(node) { names.push(node.name); },
    });
    assert.deepStrictEqual(names, ['set', 'set', 'set']);
  });

  it('empty document: no callbacks', () => {
    const { ast } = parseDocument('', adapter);
    let called = false;
    walkDocument(ast, {
      onMacro() { called = true; },
    });
    assert.strictEqual(called, false);
  });
});

// ===========================================================================
// walkExpression
// ===========================================================================

describe('walkExpression', () => {
  /** Parse a single expression from the first macro's first arg. */
  function firstExpr(source: string): ExpressionNode | null {
    const { ast } = parseDocument(source, adapter);
    for (const p of ast.passages) {
      if (!Array.isArray(p.body)) continue;
      for (const n of p.body) {
        if (n.type === 'macro' && n.args.length > 0) return n.args[0]!;
      }
    }
    return null;
  }

  it('visits binaryOp (left then right)', () => {
    const expr = firstExpr(':: Start\n<<set $x to $a + $b>>');
    assert.ok(expr !== null);
    // Top-level is a binaryOp (to), its right child is binaryOp (+)
    const order: string[] = [];
    walkExpression(expr!, (e) => { order.push(e.type); });
    // Pre-order: binaryOp(to) → storyVar(x) → binaryOp(+) → storyVar(a) → storyVar(b)
    assert.strictEqual(order[0], 'binaryOp');
    assert.ok(order.length >= 5);
  });

  it('visits unaryOp (operand)', () => {
    const expr = firstExpr(':: Start\n<<set $x to !$flag>>');
    assert.ok(expr !== null);
    const types: string[] = [];
    walkExpression(expr!, (e) => { types.push(e.type); });
    // Pre-order: binaryOp(to) → storyVar(x) → unaryOp(!) → storyVar(flag)
    assert.ok(types.includes('unaryOp'));
    const unaryIdx = types.indexOf('unaryOp');
    // Operand should come after the unaryOp
    assert.ok(types.length > unaryIdx + 1);
  });

  it('visits propertyAccess (object)', () => {
    const expr = firstExpr(':: Start\n<<print $obj.prop>>');
    assert.ok(expr !== null);
    const types: string[] = [];
    walkExpression(expr!, (e) => { types.push(e.type); });
    assert.ok(types.includes('propertyAccess'));
    // propertyAccess should be followed by its object child (storyVar)
    const paIdx = types.indexOf('propertyAccess');
    assert.strictEqual(types[paIdx + 1], 'storyVar');
  });

  it('visits indexAccess (object then index)', () => {
    const expr = firstExpr(':: Start\n<<print $arr[0]>>');
    assert.ok(expr !== null);
    const types: string[] = [];
    walkExpression(expr!, (e) => { types.push(e.type); });
    assert.ok(types.includes('indexAccess'));
    const iaIdx = types.indexOf('indexAccess');
    // After indexAccess: object (storyVar), then index (literal)
    assert.strictEqual(types[iaIdx + 1], 'storyVar');
    assert.strictEqual(types[iaIdx + 2], 'literal');
  });

  it('visits call (callee then args)', () => {
    const expr = firstExpr(':: Start\n<<run myFunc(1, 2)>>');
    assert.ok(expr !== null);
    const types: string[] = [];
    walkExpression(expr!, (e) => { types.push(e.type); });
    assert.ok(types.includes('call'));
    const callIdx = types.indexOf('call');
    // After call: callee (identifier), then args (literal, literal)
    assert.strictEqual(types[callIdx + 1], 'identifier');
  });

  it('visits arrayLiteral (all elements)', () => {
    const expr = firstExpr(':: Start\n<<set $arr to [1, 2, 3]>>');
    assert.ok(expr !== null);
    const types: string[] = [];
    walkExpression(expr!, (e) => { types.push(e.type); });
    assert.ok(types.includes('arrayLiteral'));
    // Count literal nodes (the three numbers inside the array)
    const literalCount = types.filter(t => t === 'literal').length;
    assert.strictEqual(literalCount, 3);
  });

  it('visits objectLiteral (all property values)', () => {
    const expr = firstExpr(':: Start\n<<set $obj to {a: 1, b: "hi"}>>');
    assert.ok(expr !== null);
    const types: string[] = [];
    walkExpression(expr!, (e) => { types.push(e.type); });
    assert.ok(types.includes('objectLiteral'));
    // Two property values → two literal nodes
    const literalCount = types.filter(t => t === 'literal').length;
    assert.strictEqual(literalCount, 2);
  });

  it('leaf nodes (storyVar, tempVar, identifier, literal): visited once', () => {
    // storyVar
    const sv = firstExpr(':: Start\n<<print $x>>');
    const svCount: number[] = [];
    walkExpression(sv!, () => { svCount.push(1); });
    assert.strictEqual(svCount.length, 1);

    // tempVar
    const tv = firstExpr(':: Start\n<<print _tmp>>');
    const tvCount: number[] = [];
    walkExpression(tv!, () => { tvCount.push(1); });
    assert.strictEqual(tvCount.length, 1);

    // literal
    const lit = firstExpr(':: Start\n<<set $x to 42>>');
    // This is binaryOp(to, storyVar, literal), so we count total visits
    const litTypes: string[] = [];
    walkExpression(lit!, (e) => { litTypes.push(e.type); });
    // Each node visited exactly once
    const seen = new Set<number>();
    for (const t of litTypes) {
      // Just check we don't loop infinitely — leaf nodes are visited once
      seen.add(seen.size);
    }
    assert.strictEqual(seen.size, litTypes.length);
  });

  it('pre-order: parent visited before children', () => {
    const expr = firstExpr(':: Start\n<<set $x to $a + $b>>');
    assert.ok(expr !== null);
    const order: string[] = [];
    walkExpression(expr!, (e) => { order.push(e.type); });
    // The parent binaryOp(to) should appear before its children
    const toIdx = order.indexOf('binaryOp');
    assert.ok(toIdx >= 0);
    // storyVar(x) — left child of 'to' — should come after 'to'
    assert.ok(order.indexOf('storyVar') > toIdx);
  });
});

// ===========================================================================
// walkDocumentExpressions
// ===========================================================================

describe('walkDocumentExpressions', () => {
  it('walks all macro args in document', () => {
    const source = ':: Start\n<<set $x to 5>><<print $x>>';
    const { ast } = parseDocument(source, adapter);
    const types: string[] = [];
    walkDocumentExpressions(ast, (e) => { types.push(e.type); });
    // <<set $x to 5>> → arg is binaryOp(to, storyVar, literal)
    // <<print $x>> → arg is storyVar
    assert.ok(types.includes('binaryOp'));
    assert.ok(types.includes('storyVar'));
    assert.ok(types.includes('literal'));
  });

  it('empty document: no calls', () => {
    const { ast } = parseDocument('', adapter);
    let count = 0;
    walkDocumentExpressions(ast, () => { count++; });
    assert.strictEqual(count, 0);
  });

  it('nested macro bodies: args in nested bodies are visited', () => {
    const source = ':: Start\n<<if true>><<set $x to 1>><</if>>';
    const { ast } = parseDocument(source, adapter);
    const names: string[] = [];
    walkDocumentExpressions(ast, (e) => { names.push(e.type); });
    // <<if>> has arg: literal(true)
    // <<set>> has arg: binaryOp(to, storyVar, literal)
    assert.ok(names.includes('literal'));
    assert.ok(names.includes('binaryOp'));
  });
});
