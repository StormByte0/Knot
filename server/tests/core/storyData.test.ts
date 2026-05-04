import { strict as assert } from 'assert';
import { parseStoryData, validateStoryData, StoryData, StoryDataDiagnostic } from '../../src/storyData';
import { parseDocument } from '../../src/parser';
import { getSugarCubeAdapter } from '../helpers/testFixtures';

describe('parseStoryData', () => {
  const adapter = getSugarCubeAdapter();

  it('extracts ifid, format, formatVersion, start from valid JSON body', () => {
    const source = ':: StoryData\n{"ifid":"A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D","format":"sugarcube-2","format-version":"2.36.0","start":"Start"}';
    const { ast } = parseDocument(source, adapter);
    const result = parseStoryData(ast, adapter);

    assert.strictEqual(result.ifid, 'A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D');
    assert.strictEqual(result.format, 'sugarcube-2');
    assert.strictEqual(result.formatVersion, '2.36.0');
    assert.strictEqual(result.start, 'Start');
    assert.strictEqual(result.raw['ifid'], 'A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D');
    assert.strictEqual(result.raw['format'], 'sugarcube-2');
  });

  it('returns all-null EMPTY for document without StoryData passage', () => {
    const source = ':: Start\nHello world';
    const { ast } = parseDocument(source, adapter);
    const result = parseStoryData(ast, adapter);

    assert.strictEqual(result.ifid, null);
    assert.strictEqual(result.format, null);
    assert.strictEqual(result.formatVersion, null);
    assert.strictEqual(result.start, null);
    assert.deepStrictEqual(result.raw, {});
  });

  it('handles markup body (text nodes) instead of script body', () => {
    // StoryData is a 'special' passage, not 'script', so its body is parsed
    // as markup (MarkupNode[]). The parser produces text nodes that we
    // concatenate to reconstruct the JSON.
    const jsonStr = '{"ifid":"B2C3D4E5-F6A7-4B8C-9D0E-1F2A3B4C5D6","format":"sugarcube-2"}';
    const source = `:: StoryData\n${jsonStr}`;
    const { ast } = parseDocument(source, adapter);
    const result = parseStoryData(ast, adapter);

    assert.strictEqual(result.ifid, 'B2C3D4E5-F6A7-4B8C-9D0E-1F2A3B4C5D6');
    assert.strictEqual(result.format, 'sugarcube-2');
  });

  it('returns EMPTY for malformed JSON', () => {
    const source = ':: StoryData\n{this is not valid json';
    const { ast } = parseDocument(source, adapter);
    const result = parseStoryData(ast, adapter);

    assert.strictEqual(result.ifid, null);
    assert.strictEqual(result.format, null);
    assert.strictEqual(result.formatVersion, null);
    assert.strictEqual(result.start, null);
  });

  it('returns EMPTY for empty body', () => {
    const source = ':: StoryData\n';
    const { ast } = parseDocument(source, adapter);
    const result = parseStoryData(ast, adapter);

    assert.strictEqual(result.ifid, null);
    assert.strictEqual(result.format, null);
  });

  it('uses adapter.getStoryDataPassageName() to find passage name', () => {
    // With the SugarCube adapter, getStoryDataPassageName() returns 'StoryData'
    assert.strictEqual(adapter.getStoryDataPassageName(), 'StoryData');

    // When adapter is provided and the passage name matches, data is extracted
    const source = ':: StoryData\n{"ifid":"A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D"}';
    const { ast } = parseDocument(source, adapter);
    const result = parseStoryData(ast, adapter);
    assert.strictEqual(result.ifid, 'A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D');

    // Without adapter, sdName is undefined and no passage is found → EMPTY
    const resultNoAdapter = parseStoryData(ast);
    assert.strictEqual(resultNoAdapter.ifid, null);
  });
});

describe('validateStoryData', () => {
  it('reports error for missing IFID', () => {
    const data: StoryData = {
      ifid: null,
      format: 'sugarcube-2',
      formatVersion: '2.36.0',
      start: 'Start',
      raw: {},
    };
    const diags = validateStoryData(data, new Set(['Start']));
    assert.strictEqual(diags.length, 1);
    assert.strictEqual(diags[0]!.severity, 'error');
    assert.ok(diags[0]!.message.includes('ifid'));
  });

  it('reports warning for invalid UUID v4 format', () => {
    const data: StoryData = {
      ifid: 'not-a-uuid',
      format: 'sugarcube-2',
      formatVersion: null,
      start: null,
      raw: {},
    };
    const diags = validateStoryData(data, new Set());
    assert.strictEqual(diags.length, 1);
    assert.strictEqual(diags[0]!.severity, 'warning');
    assert.ok(diags[0]!.message.includes('not a valid UUID v4'));
  });

  it('reports error for start passage not in known names', () => {
    const data: StoryData = {
      ifid: 'A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D',
      format: 'sugarcube-2',
      formatVersion: null,
      start: 'NonExistent',
      raw: {},
    };
    const diags = validateStoryData(data, new Set(['Start', 'Other']));
    assert.strictEqual(diags.length, 1);
    assert.strictEqual(diags[0]!.severity, 'error');
    assert.ok(diags[0]!.message.includes('does not exist'));
  });

  it('returns empty for valid StoryData', () => {
    const data: StoryData = {
      ifid: 'A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D',
      format: 'sugarcube-2',
      formatVersion: '2.36.0',
      start: 'Start',
      raw: {},
    };
    const diags = validateStoryData(data, new Set(['Start']));
    assert.strictEqual(diags.length, 0);
  });

  it('valid UUID v4 passes (e.g. "A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D")', () => {
    const data: StoryData = {
      ifid: 'A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D',
      format: null,
      formatVersion: null,
      start: null,
      raw: {},
    };
    const diags = validateStoryData(data, new Set());
    // No IFID error (it's present), no invalid UUID warning (it's valid), no start error (start is null)
    assert.strictEqual(diags.length, 0);
  });

  it('reports multiple diagnostics when both IFID is missing and start is invalid', () => {
    const data: StoryData = {
      ifid: null,
      format: null,
      formatVersion: null,
      start: 'Missing',
      raw: {},
    };
    const diags = validateStoryData(data, new Set());
    assert.strictEqual(diags.length, 2);
    const severities = diags.map(d => d.severity).sort();
    assert.ok(severities.includes('error'));
  });
});
