import type { SourceRange } from './tokenTypes';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface LinkRef {
  target:        string;
  range:         SourceRange;
  sourcePassage: string;
}

// ---------------------------------------------------------------------------
// LinkGraph — file-level link references and forward adjacency
// ---------------------------------------------------------------------------

export class LinkGraph {
  /** URI → links found in that file. */
  private fileLinkRefs = new Map<string, LinkRef[]>();

  // ---- File-level link storage ---------------------------------------------

  setFileLinks(uri: string, links: LinkRef[]): void {
    this.fileLinkRefs.set(uri, links);
  }

  getFileLinks(uri: string): LinkRef[] | undefined {
    return this.fileLinkRefs.get(uri);
  }

  // ---- Forward adjacency ---------------------------------------------------

  /**
   * Build a forward adjacency map: sourcePassage → Set of target passage names.
   * Used by the reachability / unreachable-passage analysis.
   */
  getForwardAdjacency(): Map<string, Set<string>> {
    const forwardAdj = new Map<string, Set<string>>();
    for (const [, links] of this.fileLinkRefs) {
      for (const link of links) {
        let targets = forwardAdj.get(link.sourcePassage);
        if (!targets) {
          targets = new Set();
          forwardAdj.set(link.sourcePassage, targets);
        }
        targets.add(link.target);
      }
    }
    return forwardAdj;
  }

  // ---- Lifecycle -----------------------------------------------------------

  clear(): void {
    this.fileLinkRefs.clear();
  }
}
