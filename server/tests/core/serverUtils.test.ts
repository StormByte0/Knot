import { strict as assert } from 'assert';
import {
  normalizeUri,
  uriToPath,
  pathToUri,
  offsetToPosition,
  inferredTypeToString,
  resolveTypePath,
  buildTypeSection,
  wordAt,
  findWordStart,
} from '../../src/serverUtils';
import { InferredType } from '../../src/typeInference';

// ---------------------------------------------------------------------------
// normalizeUri
// ---------------------------------------------------------------------------

describe('normalizeUri', () => {
  it('lowercases Windows drive letter', () => {
    assert.strictEqual(normalizeUri('file:///C:/foo'), 'file:///c:/foo');
  });

  it('percent-decodes and lowercases drive letter', () => {
    // C%3A is the percent-encoded form of C:
    assert.strictEqual(normalizeUri('file:///C%3A/foo'), 'file:///c:/foo');
  });

  it('leaves non-file URIs unchanged', () => {
    assert.strictEqual(normalizeUri('untitled:Untitled-1'), 'untitled:Untitled-1');
  });

  it('leaves already-lowercased unchanged', () => {
    assert.strictEqual(normalizeUri('file:///c:/foo'), 'file:///c:/foo');
  });

  it('handles URI without drive letter', () => {
    assert.strictEqual(normalizeUri('file:///home/user/file.tw'), 'file:///home/user/file.tw');
  });

  it('percent-decodes spaces in paths', () => {
    assert.strictEqual(normalizeUri('file:///C:/My%20Docs/file.tw'), 'file:///c:/My Docs/file.tw');
  });
});

// ---------------------------------------------------------------------------
// pathToUri
// ---------------------------------------------------------------------------

describe('pathToUri', () => {
  it('converts Unix path', () => {
    assert.strictEqual(pathToUri('/home/user/file.tw'), 'file:///home/user/file.tw');
  });

  it('converts Windows path with lowercase drive', () => {
    assert.strictEqual(pathToUri('c:\\Users\\file.tw'), 'file:///c:/Users/file.tw');
  });

  it('converts Windows path with uppercase drive to lowercase', () => {
    assert.strictEqual(pathToUri('C:\\Users\\file.tw'), 'file:///c:/Users/file.tw');
  });

  it('handles relative-looking path without leading slash', () => {
    // Non-absolute, non-Windows path gets a leading slash prepended
    const result = pathToUri('relative/path.tw');
    assert.ok(result.startsWith('file://'));
  });
});

// ---------------------------------------------------------------------------
// offsetToPosition
// ---------------------------------------------------------------------------

describe('offsetToPosition', () => {
  it('offset at start of text', () => {
    const result = offsetToPosition('hello', 0);
    assert.deepStrictEqual(result, { line: 0, character: 0 });
  });

  it('offset after newline', () => {
    const result = offsetToPosition('hello\nworld', 6);
    assert.deepStrictEqual(result, { line: 1, character: 0 });
  });

  it('offset in middle of first line', () => {
    const result = offsetToPosition('hello', 3);
    assert.deepStrictEqual(result, { line: 0, character: 3 });
  });

  it('offset at end of text', () => {
    const result = offsetToPosition('hello', 5);
    assert.deepStrictEqual(result, { line: 0, character: 5 });
  });

  it('offset beyond text length is clamped', () => {
    const result = offsetToPosition('hi', 100);
    assert.deepStrictEqual(result, { line: 0, character: 2 });
  });

  it('handles multiple newlines', () => {
    const result = offsetToPosition('a\nb\nc', 4);
    assert.deepStrictEqual(result, { line: 2, character: 0 });
  });

  it('character position in second line', () => {
    const result = offsetToPosition('hello\nworld', 8);
    assert.deepStrictEqual(result, { line: 1, character: 2 });
  });
});

// ---------------------------------------------------------------------------
// inferredTypeToString
// ---------------------------------------------------------------------------

describe('inferredTypeToString', () => {
  it('kind "number" → "number"', () => {
    assert.strictEqual(inferredTypeToString({ kind: 'number' }), 'number');
  });

  it('kind "string" → "string"', () => {
    assert.strictEqual(inferredTypeToString({ kind: 'string' }), 'string');
  });

  it('kind "object" → "object"', () => {
    assert.strictEqual(inferredTypeToString({ kind: 'object' }), 'object');
  });

  it('kind "array" with elements → "number[]" etc', () => {
    const t: InferredType = { kind: 'array', elements: { kind: 'number' } };
    assert.strictEqual(inferredTypeToString(t), 'number[]');
  });

  it('kind "array" without elements → "array"', () => {
    const t: InferredType = { kind: 'array' };
    assert.strictEqual(inferredTypeToString(t), 'array');
  });

  it('kind "boolean" → "boolean"', () => {
    assert.strictEqual(inferredTypeToString({ kind: 'boolean' }), 'boolean');
  });

  it('kind "null" → "null"', () => {
    assert.strictEqual(inferredTypeToString({ kind: 'null' }), 'null');
  });

  it('kind "unknown" → "unknown"', () => {
    assert.strictEqual(inferredTypeToString({ kind: 'unknown' }), 'unknown');
  });
});

// ---------------------------------------------------------------------------
// resolveTypePath
// ---------------------------------------------------------------------------

describe('resolveTypePath', () => {
  it('walks nested object properties', () => {
    const t: InferredType = {
      kind: 'object',
      properties: {
        foo: {
          kind: 'object',
          properties: {
            bar: { kind: 'string' },
          },
        },
      },
    };
    const result = resolveTypePath(t, ['foo', 'bar']);
    assert.ok(result !== null);
    assert.strictEqual(result!.kind, 'string');
  });

  it('returns first-level property', () => {
    const t: InferredType = {
      kind: 'object',
      properties: {
        name: { kind: 'string' },
      },
    };
    const result = resolveTypePath(t, ['name']);
    assert.ok(result !== null);
    assert.strictEqual(result!.kind, 'string');
  });

  it('returns null for missing property', () => {
    const t: InferredType = {
      kind: 'object',
      properties: {
        name: { kind: 'string' },
      },
    };
    assert.strictEqual(resolveTypePath(t, ['missing']), null);
  });

  it('returns null for non-object types', () => {
    const t: InferredType = { kind: 'string' };
    assert.strictEqual(resolveTypePath(t, ['anything']), null);
  });

  it('returns null for object without properties', () => {
    const t: InferredType = { kind: 'object' };
    assert.strictEqual(resolveTypePath(t, ['key']), null);
  });

  it('returns null when path goes through non-object mid-path', () => {
    const t: InferredType = {
      kind: 'object',
      properties: {
        foo: { kind: 'string' },
      },
    };
    assert.strictEqual(resolveTypePath(t, ['foo', 'bar']), null);
  });
});

// ---------------------------------------------------------------------------
// buildTypeSection
// ---------------------------------------------------------------------------

describe('buildTypeSection', () => {
  it('object with ≤20 properties: Markdown table', () => {
    const t: InferredType = {
      kind: 'object',
      properties: {
        name: { kind: 'string' },
        age: { kind: 'number' },
      },
    };
    const result = buildTypeSection(t);
    assert.ok(result.includes('Property'));
    assert.ok(result.includes('Type'));
    assert.ok(result.includes('.name'));
    assert.ok(result.includes('`string`'));
    assert.ok(result.includes('.age'));
    assert.ok(result.includes('`number`'));
  });

  it('object with >20 properties: summary count', () => {
    const props: Record<string, InferredType> = {};
    for (let i = 0; i < 25; i++) {
      props[`prop${i}`] = { kind: 'string' };
    }
    const t: InferredType = { kind: 'object', properties: props };
    const result = buildTypeSection(t);
    assert.ok(result.includes('25 properties'));
  });

  it('array: shows element type', () => {
    const t: InferredType = { kind: 'array', elements: { kind: 'number' } };
    const result = buildTypeSection(t);
    assert.ok(result.includes('number[]'));
  });

  it('array without elements: shows unknown[]', () => {
    const t: InferredType = { kind: 'array' };
    const result = buildTypeSection(t);
    assert.ok(result.includes('unknown[]'));
  });

  it('primitive: shows type', () => {
    const t: InferredType = { kind: 'string' };
    const result = buildTypeSection(t);
    assert.ok(result.includes('`string`'));
  });

  it('empty object: shows object type', () => {
    const t: InferredType = { kind: 'object', properties: {} };
    const result = buildTypeSection(t);
    assert.ok(result.includes('`object`'));
  });
});

// ---------------------------------------------------------------------------
// wordAt
// ---------------------------------------------------------------------------

describe('wordAt', () => {
  it('finds word at offset', () => {
    assert.strictEqual(wordAt('hello world', 2), 'hello');
  });

  it('finds word at beginning', () => {
    assert.strictEqual(wordAt('hello world', 0), 'hello');
  });

  it('finds second word', () => {
    assert.strictEqual(wordAt('hello world', 7), 'world');
  });

  it('returns empty string if no word at offset (all punctuation)', () => {
    assert.strictEqual(wordAt('+++===', 3), '');
  });

  it('returns empty string if no word at offset (whitespace only)', () => {
    assert.strictEqual(wordAt('   ', 1), '');
  });

  it('handles $var at start', () => {
    assert.strictEqual(wordAt('$myVar + 1', 3), '$myVar');
  });

  it('returns empty string for empty text', () => {
    assert.strictEqual(wordAt('', 0), '');
  });

  it('handles underscore identifiers', () => {
    assert.strictEqual(wordAt('_temp_var + 1', 5), '_temp_var');
  });
});

// ---------------------------------------------------------------------------
// findWordStart
// ---------------------------------------------------------------------------

describe('findWordStart', () => {
  it('returns offset of word start', () => {
    // "hello world" — word "hello" starts at 0
    assert.strictEqual(findWordStart('hello world', 2, 'hello'), 0);
  });

  it('finds word start for second occurrence', () => {
    // "hello hello" — find "hello" near offset 7
    assert.strictEqual(findWordStart('hello hello', 7, 'hello'), 6);
  });

  it('returns -1 if word not found near offset', () => {
    assert.strictEqual(findWordStart('hello world', 0, 'absent'), -1);
  });

  it('returns start when offset is at word boundary', () => {
    assert.strictEqual(findWordStart('abc', 0, 'abc'), 0);
  });

  it('returns start when offset is at end of word', () => {
    assert.strictEqual(findWordStart('abc', 3, 'abc'), 0);
  });
});
