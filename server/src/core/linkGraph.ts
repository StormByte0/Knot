/**
 * Knot v2 — Link Graph
 *
 * Directed graph of passage links for reachability analysis.
 * Format-agnostic — uses LinkKind enum for edge classification.
 *
 * Promises:
 *   - Build directed graph from passage links
 *   - Find unreachable passages (no path from start)
 *   - Find orphan passages (no incoming or outgoing links)
 *   - Compute strongly connected components
 *
 * Imports:
 *   - hooks/hookTypes (LinkKind enum)
 *
 * MUST NOT import from: formats/
 */

import { LinkKind } from '../hooks/hookTypes';

export interface LinkEdge {
  from: string;
  to: string;
  kind: LinkKind;
}

export class LinkGraph {
  private adjacency: Map<string, LinkEdge[]> = new Map();

  /**
   * Add an edge to the graph.
   */
  addEdge(edge: LinkEdge): void {
    const edges = this.adjacency.get(edge.from) ?? [];
    edges.push(edge);
    this.adjacency.set(edge.from, edges);
  }

  /**
   * Remove all edges originating from a specific passage.
   */
  removeEdgesFrom(passageName: string): void {
    this.adjacency.delete(passageName);
  }

  /**
   * Find all passages reachable from the start passage.
   * Used for unreachable passage detection (DiagnosticRule.UnreachablePassage).
   */
  findReachable(startPassage: string): Set<string> {
    const visited = new Set<string>();
    const queue = [startPassage];

    while (queue.length > 0) {
      const current = queue.pop()!;
      if (visited.has(current)) continue;
      visited.add(current);

      const edges = this.adjacency.get(current) ?? [];
      for (const edge of edges) {
        // Passage links and Custom links with passage-like semantics are traversable
        if ((edge.kind === LinkKind.Passage || edge.kind === LinkKind.Custom) && !visited.has(edge.to)) {
          queue.push(edge.to);
        }
      }
    }

    return visited;
  }

  /**
   * Find unreachable passages given a set of all passages and a start point.
   */
  findUnreachable(allPassages: string[], startPassage: string): string[] {
    const reachable = this.findReachable(startPassage);
    return allPassages.filter(name => !reachable.has(name));
  }

  /**
   * Clear the entire graph.
   */
  clear(): void {
    this.adjacency.clear();
  }
}
