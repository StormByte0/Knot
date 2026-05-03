import { strict as assert } from 'assert';
import { ReferenceIndex, PassageRef, MacroRef } from '../../src/referenceIndex';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makePassageRef(overrides: Partial<PassageRef> = {}): PassageRef {
  return {
    uri: 'file:///a.twee',
    range: { start: 10, end: 20 },
    sourcePassage: 'Start',
    ...overrides,
  };
}

function makeMacroRef(overrides: Partial<MacroRef> = {}): MacroRef {
  return {
    uri: 'file:///a.twee',
    range: { start: 30, end: 45 },
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('ReferenceIndex', () => {
  let index: ReferenceIndex;

  beforeEach(() => {
    index = new ReferenceIndex();
  });

  // ---- Passage references --------------------------------------------------

  describe('Passage references', () => {
    it('addPassageReference / getPassageReferences — basic round-trip', () => {
      const ref = makePassageRef({ uri: 'file:///a.twee', range: { start: 10, end: 20 }, sourcePassage: 'Start' });
      index.addPassageReference('Target', ref);

      const refs = index.getPassageReferences('Target');
      assert.strictEqual(refs.length, 1);
      assert.strictEqual(refs[0]!.uri, 'file:///a.twee');
      assert.strictEqual(refs[0]!.sourcePassage, 'Start');
      assert.strictEqual(refs[0]!.range.start, 10);
    });

    it('getPassageReferences returns empty array for unknown passage', () => {
      const refs = index.getPassageReferences('Nonexistent');
      assert.deepStrictEqual(refs, []);
    });

    it('multiple references to the same target are all tracked', () => {
      const ref1 = makePassageRef({ uri: 'file:///a.twee', range: { start: 10, end: 20 }, sourcePassage: 'Start' });
      const ref2 = makePassageRef({ uri: 'file:///b.twee', range: { start: 5, end: 15 }, sourcePassage: 'Other' });

      index.addPassageReference('Target', ref1);
      index.addPassageReference('Target', ref2);

      const refs = index.getPassageReferences('Target');
      assert.strictEqual(refs.length, 2);
    });

    it('getReferencingFiles returns deduplicated sorted URIs', () => {
      const ref1 = makePassageRef({ uri: 'file:///c.twee', range: { start: 10, end: 20 }, sourcePassage: 'C' });
      const ref2 = makePassageRef({ uri: 'file:///a.twee', range: { start: 30, end: 40 }, sourcePassage: 'A' });
      const ref3 = makePassageRef({ uri: 'file:///a.twee', range: { start: 50, end: 60 }, sourcePassage: 'A2' });
      const ref4 = makePassageRef({ uri: 'file:///b.twee', range: { start: 70, end: 80 }, sourcePassage: 'B' });

      index.addPassageReference('Target', ref1);
      index.addPassageReference('Target', ref2);
      index.addPassageReference('Target', ref3);
      index.addPassageReference('Target', ref4);

      const files = index.getReferencingFiles('Target');
      assert.deepStrictEqual(files, ['file:///a.twee', 'file:///b.twee', 'file:///c.twee']);
    });

    it('getReferencingFiles returns empty array for unknown passage', () => {
      const files = index.getReferencingFiles('Nonexistent');
      assert.deepStrictEqual(files, []);
    });
  });

  // ---- Variable references -------------------------------------------------

  describe('Variable references', () => {
    it('addVariableReference / getVariableReferences — basic round-trip', () => {
      const ref = { uri: 'file:///a.twee', range: { start: 10, end: 20 } };
      index.addVariableReference('x', ref);

      const refs = index.getVariableReferences('x');
      assert.strictEqual(refs.length, 1);
      assert.strictEqual(refs[0]!.uri, 'file:///a.twee');
      assert.strictEqual(refs[0]!.range.start, 10);
    });

    it('getVariableReferences returns empty array for unknown variable', () => {
      const refs = index.getVariableReferences('unknown');
      assert.deepStrictEqual(refs, []);
    });

    it('multiple references to the same variable are tracked', () => {
      index.addVariableReference('count', { uri: 'file:///a.twee', range: { start: 5, end: 10 } });
      index.addVariableReference('count', { uri: 'file:///b.twee', range: { start: 15, end: 20 } });

      const refs = index.getVariableReferences('count');
      assert.strictEqual(refs.length, 2);
    });

    it('deduplicates variable references with same (uri, range.start)', () => {
      const ref = { uri: 'file:///a.twee', range: { start: 5, end: 10 } };
      index.addVariableReference('count', ref);
      index.addVariableReference('count', ref);

      const refs = index.getVariableReferences('count');
      assert.strictEqual(refs.length, 1);
    });
  });

  // ---- Macro call sites ----------------------------------------------------

  describe('Macro call sites', () => {
    it('addMacroCallSite / getMacroCallSites — basic round-trip', () => {
      const ref = makeMacroRef({ uri: 'file:///a.twee', range: { start: 30, end: 45 } });
      index.addMacroCallSite('myWidget', ref);

      const sites = index.getMacroCallSites('myWidget');
      assert.strictEqual(sites.length, 1);
      assert.strictEqual(sites[0]!.uri, 'file:///a.twee');
      assert.strictEqual(sites[0]!.range.start, 30);
    });

    it('getMacroCallSites returns empty array for unknown macro', () => {
      const sites = index.getMacroCallSites('nonexistent');
      assert.deepStrictEqual(sites, []);
    });

    it('hasMacroCallSite returns true for registered, false for unknown', () => {
      index.addMacroCallSite('myWidget', makeMacroRef());
      assert.strictEqual(index.hasMacroCallSite('myWidget'), true);
      assert.strictEqual(index.hasMacroCallSite('nonexistent'), false);
    });

    it('multiple call sites for the same macro are tracked', () => {
      index.addMacroCallSite('myWidget', makeMacroRef({ uri: 'file:///a.twee', range: { start: 30, end: 45 } }));
      index.addMacroCallSite('myWidget', makeMacroRef({ uri: 'file:///b.twee', range: { start: 50, end: 65 } }));

      const sites = index.getMacroCallSites('myWidget');
      assert.strictEqual(sites.length, 2);
    });

    it('deduplicates macro call sites with same (uri, range.start)', () => {
      const ref = makeMacroRef({ uri: 'file:///a.twee', range: { start: 30, end: 45 } });
      index.addMacroCallSite('myWidget', ref);
      index.addMacroCallSite('myWidget', ref);

      const sites = index.getMacroCallSites('myWidget');
      assert.strictEqual(sites.length, 1);
    });
  });

  // ---- Deduplication (passage references) ----------------------------------

  describe('Deduplication', () => {
    it('same (uri, range.start) does not create duplicate passage reference entries', () => {
      const ref = makePassageRef({ uri: 'file:///a.twee', range: { start: 10, end: 20 }, sourcePassage: 'Start' });
      index.addPassageReference('Target', ref);
      index.addPassageReference('Target', ref); // Same uri + range.start

      const refs = index.getPassageReferences('Target');
      assert.strictEqual(refs.length, 1);
    });

    it('same uri with different range.start creates separate entries', () => {
      const ref1 = makePassageRef({ uri: 'file:///a.twee', range: { start: 10, end: 20 }, sourcePassage: 'Start' });
      const ref2 = makePassageRef({ uri: 'file:///a.twee', range: { start: 50, end: 60 }, sourcePassage: 'Start' });

      index.addPassageReference('Target', ref1);
      index.addPassageReference('Target', ref2);

      const refs = index.getPassageReferences('Target');
      assert.strictEqual(refs.length, 2);
    });

    it('same range.start with different uri creates separate entries', () => {
      const ref1 = makePassageRef({ uri: 'file:///a.twee', range: { start: 10, end: 20 }, sourcePassage: 'Start' });
      const ref2 = makePassageRef({ uri: 'file:///b.twee', range: { start: 10, end: 20 }, sourcePassage: 'Start' });

      index.addPassageReference('Target', ref1);
      index.addPassageReference('Target', ref2);

      const refs = index.getPassageReferences('Target');
      assert.strictEqual(refs.length, 2);
    });
  });

  // ---- Lifecycle -----------------------------------------------------------

  describe('Lifecycle — clear()', () => {
    it('clear() removes all passage references', () => {
      index.addPassageReference('Target', makePassageRef());
      index.clear();

      assert.deepStrictEqual(index.getPassageReferences('Target'), []);
    });

    it('clear() removes all variable references', () => {
      index.addVariableReference('x', { uri: 'file:///a.twee', range: { start: 0, end: 5 } });
      index.clear();

      assert.deepStrictEqual(index.getVariableReferences('x'), []);
    });

    it('clear() removes all macro call sites', () => {
      index.addMacroCallSite('myWidget', makeMacroRef());
      index.clear();

      assert.deepStrictEqual(index.getMacroCallSites('myWidget'), []);
      assert.strictEqual(index.hasMacroCallSite('myWidget'), false);
    });
  });

  // ---- Cross-type independence ---------------------------------------------

  describe('Cross-type independence', () => {
    it('adding passage references does not affect variable or macro references', () => {
      index.addPassageReference('Target', makePassageRef());

      assert.strictEqual(index.getVariableReferences('Target').length, 0);
      assert.strictEqual(index.getMacroCallSites('Target').length, 0);
      assert.strictEqual(index.hasMacroCallSite('Target'), false);
    });

    it('adding variable references does not affect passage or macro references', () => {
      index.addVariableReference('x', { uri: 'file:///a.twee', range: { start: 0, end: 5 } });

      assert.strictEqual(index.getPassageReferences('x').length, 0);
      assert.strictEqual(index.getMacroCallSites('x').length, 0);
    });

    it('adding macro call sites does not affect passage or variable references', () => {
      index.addMacroCallSite('myWidget', makeMacroRef());

      assert.strictEqual(index.getPassageReferences('myWidget').length, 0);
      assert.strictEqual(index.getVariableReferences('myWidget').length, 0);
    });

    it('different targets with the same name in different types are independent', () => {
      // "x" used as a passage name and a variable name — different buckets
      index.addPassageReference('x', makePassageRef({ sourcePassage: 'A' }));
      index.addVariableReference('x', { uri: 'file:///a.twee', range: { start: 5, end: 10 } });

      assert.strictEqual(index.getPassageReferences('x').length, 1);
      assert.strictEqual(index.getVariableReferences('x').length, 1);
    });
  });
});
