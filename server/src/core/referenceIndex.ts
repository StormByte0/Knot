/**
 * Knot v2 — Reference Index
 *
 * Tracks cross-references between passages (which passage links to which).
 * Format-agnostic — uses LinkKind enum for link classification.
 *
 * Promises:
 *   - Track all references between passages
 *   - Find all references to a given passage
 *   - Support rename by finding all reference sites
 *
 * Imports:
 *   - hooks/hookTypes (LinkKind enum)
 *   - hooks/hookRegistry (format provider for link classification)
 *
 * MUST NOT import from: formats/
 */

import { LinkKind } from '../hooks/hookTypes';

export interface ReferenceEntry {
  /** URI of the document containing the reference */
  uri: string;
  /** Name of the passage containing the reference */
  sourcePassage: string;
  /** Target passage being referenced */
  targetPassage: string;
  /** What kind of link this reference is */
  kind: LinkKind;
  /** Start offset of the reference in the source document */
  startOffset: number;
  /** End offset of the reference in the source document */
  endOffset: number;
}

export class ReferenceIndex {
  /** Map: target passage name → references pointing to it */
  private referencesByTarget: Map<string, ReferenceEntry[]> = new Map();

  /**
   * Add a reference to the index.
   */
  addReference(entry: ReferenceEntry): void {
    const existing = this.referencesByTarget.get(entry.targetPassage) ?? [];
    existing.push(entry);
    this.referencesByTarget.set(entry.targetPassage, existing);
  }

  /**
   * Find all references pointing to a given passage.
   */
  findReferences(targetPassage: string): ReferenceEntry[] {
    return this.referencesByTarget.get(targetPassage) ?? [];
  }

  /**
   * Remove all references originating from a specific document.
   */
  removeReferencesByUri(uri: string): void {
    for (const [target, refs] of this.referencesByTarget) {
      const filtered = refs.filter(r => r.uri !== uri);
      if (filtered.length === 0) {
        this.referencesByTarget.delete(target);
      } else {
        this.referencesByTarget.set(target, filtered);
      }
    }
  }

  /**
   * Clear the entire reference index.
   */
  clear(): void {
    this.referencesByTarget.clear();
  }
}
