/**
 * Knot v2 — Incremental Parser
 *
 * Re-parses only the passages that changed between document versions.
 * Compares old and new parsed passage lists and produces a delta.
 *
 * MUST NOT import from: formats/
 */

import { Parser, RawPassage } from './parser';
import { PassageType } from '../hooks/hookTypes';

export interface PassageDelta {
  added: RawPassageRef[];
  removed: string[];
}

export interface RawPassageRef {
  name: string;
  type: PassageType;
  tags: string[];
  body: string;
  startOffset: number;
  endOffset: number;
}

export class IncrementalParser {
  private parser: Parser;

  constructor(parser: Parser) {
    this.parser = parser;
  }

  /**
   * Compute the delta between two versions of a document.
   * Returns only the passages that were added/modified or removed.
   */
  computeDelta(oldContent: string, newContent: string): PassageDelta {
    const oldPassages = this.parser.parseDocument(oldContent);
    const newPassages = this.parser.parseDocument(newContent);

    const oldMap = new Map(oldPassages.map(p => [p.name, p]));
    const newMap = new Map(newPassages.map(p => [p.name, p]));

    const added: RawPassageRef[] = [];
    const removed: string[] = [];

    // Find removed passages (in old but not in new)
    for (const [name] of oldMap) {
      if (!newMap.has(name)) {
        removed.push(name);
      }
    }

    // Find added or modified passages (in new but not in old, or body changed)
    for (const [name, passage] of newMap) {
      const old = oldMap.get(name);
      if (!old || old.body !== passage.body || old.tags.join(' ') !== passage.tags.join(' ')) {
        added.push({
          name: passage.name,
          type: this.parser.classifyPassageType(passage),
          tags: passage.tags,
          body: passage.body,
          startOffset: passage.startOffset,
          endOffset: passage.endOffset,
        });
      }
    }

    return { added, removed };
  }
}
