/**
 * Knot v2 — Parser
 *
 * Splits a Twee document into raw passages. Core ONLY handles the
 * Twine engine structural layer:
 *   - Splits on :: passage header lines
 *   - Extracts passage name and tags from headers
 *   - Classifies passage types (Twee 3 spec + format specialPassages)
 *
 * Everything INSIDE passage bodies is handled by the active FormatModule:
 *   - Body tokenization: format.lexBody()
 *   - Passage references: format.extractPassageRefs()
 *   - Link resolution: format.resolveLinkBody()
 *
 * Core NEVER extracts [[ ]] links on its own. The format's
 * extractPassageRefs() is the single source of truth for all
 * passage references (links, macros, API calls, implicit refs).
 *
 * MUST NOT import from: formats/ (use FormatRegistry instead)
 */

import {
  PassageType,
  PassageKind,
} from '../hooks/hookTypes';

import type {
  FormatModule,
  FormatASTNodeTypes,
  PassageRef,
} from '../formats/_types';

import { FormatRegistry } from '../formats/formatRegistry';

// ─── Public Types ──────────────────────────────────────────────

export interface RawPassage {
  name: string;
  tags: string[];
  body: string;
  startOffset: number;
  endOffset: number;
  /** Format-extracted passage references (links, macros, API calls, implicit) */
  passageRefs: PassageRef[];
  /** Format-tokenized body tokens (empty array if no format) */
  bodyTokens: import('../formats/_types').BodyToken[];
}

// ─── Regex Patterns (Twee 3 spec only) ─────────────────────────

/** Matches a passage header line: :: Name [tag1 tag2] */
const PASSAGE_HEADER_RE = /^::\s*([^\[\]\n]+?)(?:\s*\[([^\]]*)\])?\s*$/m;

/** Twee 3 spec universal tags */
const TWEE3_SCRIPT_TAG = 'script';
const TWEE3_STYLESHEET_TAG = 'stylesheet';

/** Twee 3 spec passage names */
const TWEE3_STORYDATA_NAME = 'StoryData';
const TWEE3_START_NAME = 'Start';

// ─── Parser ────────────────────────────────────────────────────

export class Parser {
  private formatRegistry: FormatRegistry;

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
  }

  /**
   * Get the active format module.
   */
  private getFormat(): FormatModule {
    return this.formatRegistry.getActiveFormat();
  }

  /**
   * Get the AST node types from the active format module.
   * Returns a default set if no format is active.
   */
  getASTNodeTypes(): FormatASTNodeTypes {
    return this.getFormat().astNodeTypes;
  }

  /**
   * Parse a full Twee document into raw passages.
   * Splits on passage header lines (:: Name [tags]).
   * Delegates body tokenization and passage reference extraction
   * to the active format's lexBody() and extractPassageRefs().
   */
  parseDocument(content: string): RawPassage[] {
    const passages: RawPassage[] = [];
    const lines = content.split('\n');

    let currentPassage: {
      name: string;
      tags: string[];
      bodyLines: string[];
      bodyStartOffset: number;
    } | null = null;

    let offset = 0;

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      const headerMatch = line.match(PASSAGE_HEADER_RE);

      if (headerMatch) {
        // Flush previous passage
        if (currentPassage) {
          this.flushPassage(currentPassage, offset - 1, passages);
        }

        const name = headerMatch[1].trim();
        const tagsStr = headerMatch[2] ?? '';
        const tags = tagsStr.split(/\s+/).map(t => t.trim()).filter(Boolean);

        currentPassage = {
          name,
          tags,
          bodyLines: [],
          bodyStartOffset: offset + line.length + 1,
        };
      } else if (currentPassage) {
        currentPassage.bodyLines.push(line);
      }

      offset += line.length + 1;
    }

    // Flush last passage
    if (currentPassage) {
      this.flushPassage(currentPassage, content.length, passages);
    }

    return passages;
  }

  /**
   * Classify a raw passage's type.
   * Core checks Twee 3 spec tags first ([script], [stylesheet]),
   * then checks format's specialPassages list for name/tag matches,
   * then falls back to PassageType.Story.
   */
  classifyPassageType(rawPassage: RawPassage): PassageType {
    // Twee 3 spec: [script] and [stylesheet] are universal
    if (rawPassage.tags.includes(TWEE3_SCRIPT_TAG)) {
      return PassageType.Script;
    }
    if (rawPassage.tags.includes(TWEE3_STYLESHEET_TAG)) {
      return PassageType.Stylesheet;
    }

    // Twee 3 spec: StoryData and Start by name
    if (rawPassage.name === TWEE3_STORYDATA_NAME) {
      return PassageType.StoryData;
    }
    if (rawPassage.name === TWEE3_START_NAME) {
      return PassageType.Start;
    }

    // Check format's specialPassages (declarative lookup)
    const format = this.getFormat();
    for (const sp of format.specialPassages) {
      // Name-based match (non-empty name)
      if (sp.name && sp.name === rawPassage.name) {
        if (sp.kind === PassageKind.Script) return PassageType.Script;
        if (sp.kind === PassageKind.Stylesheet) return PassageType.Stylesheet;
        return PassageType.Custom;  // Special passages map to Custom
      }
      // Tag-based match
      if (sp.tag && rawPassage.tags.includes(sp.tag)) {
        return PassageType.Custom;
      }
    }

    return PassageType.Story;
  }

  /**
   * Get the custom type ID for a passage classified as Custom.
   * Looks up the format's specialPassages for a matching typeId.
   */
  getCustomTypeId(rawPassage: RawPassage): string | undefined {
    const format = this.getFormat();
    for (const sp of format.specialPassages) {
      if (sp.name && sp.name === rawPassage.name && sp.typeId) {
        return sp.typeId;
      }
      if (sp.tag && rawPassage.tags.includes(sp.tag) && sp.typeId) {
        return sp.typeId;
      }
    }
    return undefined;
  }

  // ─── Private Helpers ─────────────────────────────────────────

  private flushPassage(
    current: { name: string; tags: string[]; bodyLines: string[]; bodyStartOffset: number },
    endOffset: number,
    passages: RawPassage[],
  ): void {
    const body = current.bodyLines.join('\n');

    // Get body tokens from format's lexBody
    const format = this.getFormat();
    const bodyTokens = format.lexBody(body, current.bodyStartOffset);

    // Get passage references from format's extractPassageRefs
    const passageRefs = format.extractPassageRefs(body, current.bodyStartOffset);

    passages.push({
      name: current.name,
      tags: current.tags,
      body,
      startOffset: current.bodyStartOffset,
      endOffset: endOffset,
      passageRefs,
      bodyTokens,
    });
  }
}
