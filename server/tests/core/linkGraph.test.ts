import { strict as assert } from 'assert';
import { LinkGraph, LinkRef } from '../../src/linkGraph';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeLinkRef(overrides: Partial<LinkRef> = {}): LinkRef {
  return {
    target: 'Next',
    range: { start: 0, end: 10 },
    sourcePassage: 'Start',
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('LinkGraph', () => {
  let graph: LinkGraph;

  beforeEach(() => {
    graph = new LinkGraph();
  });

  // ---- File-level link storage ---------------------------------------------

  describe('File-level link storage', () => {
    it('setFileLinks / getFileLinks — basic round-trip', () => {
      const links = [
        makeLinkRef({ target: 'Next', sourcePassage: 'Start', range: { start: 5, end: 15 } }),
      ];
      graph.setFileLinks('file:///a.twee', links);

      const result = graph.getFileLinks('file:///a.twee');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.length, 1);
      assert.strictEqual(result![0]!.target, 'Next');
      assert.strictEqual(result![0]!.sourcePassage, 'Start');
    });

    it('getFileLinks returns undefined for unknown URI', () => {
      assert.strictEqual(graph.getFileLinks('file:///nonexistent.twee'), undefined);
    });

    it('setFileLinks replaces existing links for the same URI', () => {
      const links1 = [makeLinkRef({ target: 'A' })];
      const links2 = [makeLinkRef({ target: 'B' }), makeLinkRef({ target: 'C' })];

      graph.setFileLinks('file:///a.twee', links1);
      graph.setFileLinks('file:///a.twee', links2);

      const result = graph.getFileLinks('file:///a.twee');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.length, 2);
      assert.strictEqual(result![0]!.target, 'B');
      assert.strictEqual(result![1]!.target, 'C');
    });

    it('stores links for multiple files independently', () => {
      const linksA = [makeLinkRef({ target: 'Alpha', sourcePassage: 'A' })];
      const linksB = [makeLinkRef({ target: 'Beta', sourcePassage: 'B' })];

      graph.setFileLinks('file:///a.twee', linksA);
      graph.setFileLinks('file:///b.twee', linksB);

      const resultA = graph.getFileLinks('file:///a.twee');
      const resultB = graph.getFileLinks('file:///b.twee');
      assert.ok(resultA !== undefined);
      assert.ok(resultB !== undefined);
      assert.strictEqual(resultA![0]!.target, 'Alpha');
      assert.strictEqual(resultB![0]!.target, 'Beta');
    });
  });

  // ---- Forward adjacency ---------------------------------------------------

  describe('Forward adjacency', () => {
    it('getForwardAdjacency builds correct map from a single file', () => {
      const links = [
        makeLinkRef({ target: 'Room1', sourcePassage: 'Start', range: { start: 0, end: 5 } }),
        makeLinkRef({ target: 'Room2', sourcePassage: 'Start', range: { start: 10, end: 15 } }),
      ];
      graph.setFileLinks('file:///a.twee', links);

      const adj = graph.getForwardAdjacency();
      assert.strictEqual(adj.size, 1);
      assert.ok(adj.has('Start'));
      const targets = adj.get('Start')!;
      assert.ok(targets.has('Room1'));
      assert.ok(targets.has('Room2'));
    });

    it('getForwardAdjacency builds correct map from multiple files', () => {
      graph.setFileLinks('file:///a.twee', [
        makeLinkRef({ target: 'Room1', sourcePassage: 'Start' }),
      ]);
      graph.setFileLinks('file:///b.twee', [
        makeLinkRef({ target: 'End', sourcePassage: 'Room1' }),
      ]);

      const adj = graph.getForwardAdjacency();
      assert.strictEqual(adj.size, 2);
      assert.ok(adj.has('Start'));
      assert.ok(adj.has('Room1'));
      assert.ok(adj.get('Start')!.has('Room1'));
      assert.ok(adj.get('Room1')!.has('End'));
    });

    it('same source passage across files merges targets into one set', () => {
      graph.setFileLinks('file:///a.twee', [
        makeLinkRef({ target: 'Room1', sourcePassage: 'Start' }),
      ]);
      graph.setFileLinks('file:///b.twee', [
        makeLinkRef({ target: 'Room2', sourcePassage: 'Start' }),
      ]);

      const adj = graph.getForwardAdjacency();
      assert.strictEqual(adj.size, 1);
      assert.ok(adj.has('Start'));
      const targets = adj.get('Start')!;
      assert.strictEqual(targets.size, 2);
      assert.ok(targets.has('Room1'));
      assert.ok(targets.has('Room2'));
    });
  });

  // ---- Empty graph ---------------------------------------------------------

  describe('Empty graph', () => {
    it('getFileLinks returns undefined for unknown URI', () => {
      assert.strictEqual(graph.getFileLinks('file:///nothing.twee'), undefined);
    });

    it('getForwardAdjacency returns empty map', () => {
      const adj = graph.getForwardAdjacency();
      assert.strictEqual(adj.size, 0);
    });
  });

  // ---- Multiple links from same source passage -----------------------------

  describe('Multiple links from same source passage', () => {
    it('same source passage links to multiple targets', () => {
      const links = [
        makeLinkRef({ target: 'Room1', sourcePassage: 'Hub' }),
        makeLinkRef({ target: 'Room2', sourcePassage: 'Hub' }),
        makeLinkRef({ target: 'Room3', sourcePassage: 'Hub' }),
      ];
      graph.setFileLinks('file:///a.twee', links);

      const adj = graph.getForwardAdjacency();
      const targets = adj.get('Hub')!;
      assert.strictEqual(targets.size, 3);
      assert.ok(targets.has('Room1'));
      assert.ok(targets.has('Room2'));
      assert.ok(targets.has('Room3'));
    });

    it('duplicate target names are deduplicated in the Set', () => {
      const links = [
        makeLinkRef({ target: 'Room1', sourcePassage: 'Hub', range: { start: 0, end: 5 } }),
        makeLinkRef({ target: 'Room1', sourcePassage: 'Hub', range: { start: 10, end: 15 } }),
      ];
      graph.setFileLinks('file:///a.twee', links);

      const adj = graph.getForwardAdjacency();
      const targets = adj.get('Hub')!;
      assert.strictEqual(targets.size, 1);
      assert.ok(targets.has('Room1'));
    });
  });

  // ---- Lifecycle -----------------------------------------------------------

  describe('Lifecycle — clear()', () => {
    it('clear() removes all file links', () => {
      graph.setFileLinks('file:///a.twee', [makeLinkRef()]);
      graph.setFileLinks('file:///b.twee', [makeLinkRef()]);
      graph.clear();

      assert.strictEqual(graph.getFileLinks('file:///a.twee'), undefined);
      assert.strictEqual(graph.getFileLinks('file:///b.twee'), undefined);
    });

    it('clear() makes getForwardAdjacency return empty map', () => {
      graph.setFileLinks('file:///a.twee', [makeLinkRef()]);
      graph.clear();

      const adj = graph.getForwardAdjacency();
      assert.strictEqual(adj.size, 0);
    });

    it('graph is usable after clear()', () => {
      graph.setFileLinks('file:///a.twee', [makeLinkRef()]);
      graph.clear();

      // Can add new links after clearing
      const newLinks = [makeLinkRef({ target: 'NewTarget', sourcePassage: 'NewSource' })];
      graph.setFileLinks('file:///c.twee', newLinks);

      const result = graph.getFileLinks('file:///c.twee');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.length, 1);
      assert.strictEqual(result![0]!.target, 'NewTarget');
    });
  });
});
