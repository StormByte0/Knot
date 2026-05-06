/**
 * Knot v2 — Workspace Index
 *
 * FORMAT-AGNOSTIC — indexes passages using PassageType enum values
 * provided by the active format through the format registry.
 * All format-specific data flows through FormatModule capability bags.
 *
 * REFINED per architecture review:
 *   - Uses passageRefs from Parser (format-driven extraction)
 *   - Uses Parser.extractVariables() (delegates to format)
 *   - Uses Parser.extractMacroNames() (delegates to format)
 *   - No hardcoded <<>> or $variable patterns
 *   - Custom type IDs come from format's specialPassages
 *
 * MUST NOT import from: formats/ (use FormatRegistry)
 */

import { PassageType, LinkKind, PassageRefKind } from '../hooks/hookTypes';
import { FormatRegistry } from '../formats/formatRegistry';
import { Parser, RawPassage } from './parser';
import type { PassageRef, BodyToken } from '../formats/_types';

export interface PassageEntry {
  name: string;
  type: PassageType;
  /** For PassageType.Custom, the format-specific type ID (e.g. "widget", "header") */
  customTypeId?: string;
  tags: string[];
  uri: string;
  startOffset: number;
  endOffset: number;
  /** All passage references from format's extractPassageRefs() — single source of truth */
  passageRefs: PassageRef[];
  storyVars: Set<string>;
  tempVars: Set<string>;
  macroNames: string[];
  /** The raw body text of the passage (after :: header). Needed for StoryData JSON detection. */
  body: string;
}

export interface IndexChangeEvent {
  type: 'add' | 'remove' | 'update';
  uri: string;
  passages: PassageEntry[];
}

export type IndexChangeListener = (event: IndexChangeEvent) => void;

export class WorkspaceIndex {
  private passages: Map<string, PassageEntry> = new Map();
  private passagesByUri: Map<string, PassageEntry[]> = new Map();
  private formatRegistry: FormatRegistry;
  private parser: Parser;
  private listeners: IndexChangeListener[] = [];
  private _duplicateNames: Set<string> = new Set();

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
    this.parser = new Parser(formatRegistry);
  }

  indexDocument(uri: string, content: string): void {
    this.removeDocumentInternal(uri);
    const rawPassages = this.parser.parseDocument(content);
    const entries: PassageEntry[] = [];

    for (const raw of rawPassages) {
      const passageType = this.parser.classifyPassageType(raw);

      // Extract variables using format's variable capability
      const format = this.formatRegistry.getActiveFormat();
      const vars = this.extractVariables(raw.body, format);

      // Extract macro names using format's macro pattern
      const macroNames = this.extractMacroNames(raw.body, format);

      // Get custom type ID for Custom passage types
      let customTypeId: string | undefined;
      if (passageType === PassageType.Custom) {
        customTypeId = this.parser.getCustomTypeId(raw);
      }

      const entry: PassageEntry = {
        name: raw.name,
        type: passageType,
        customTypeId,
        tags: raw.tags,
        uri,
        startOffset: raw.startOffset,
        endOffset: raw.endOffset,
        passageRefs: raw.passageRefs,
        storyVars: vars.story,
        tempVars: vars.temp,
        macroNames,
        body: raw.body,
      };

      if (this.passages.has(raw.name)) {
        this._duplicateNames.add(raw.name);
      }

      this.passages.set(raw.name, entry);
      entries.push(entry);
    }

    this.passagesByUri.set(uri, entries);
    this.notifyListeners({ type: 'add', uri, passages: entries });
  }

  removeDocument(uri: string): void {
    const removed = this.removeDocumentInternal(uri);
    if (removed.length > 0) {
      this.notifyListeners({ type: 'remove', uri, passages: removed });
    }
  }

  reindexDocument(uri: string, content: string): void {
    this.removeDocumentInternal(uri);
    this.indexDocument(uri, content);
  }

  getPassage(name: string): PassageEntry | undefined {
    return this.passages.get(name);
  }

  getPassagesByType(type: PassageType): PassageEntry[] {
    return Array.from(this.passages.values()).filter(p => p.type === type);
  }

  /** Get passages by format-specific custom type ID (e.g. "widget", "header"). */
  getPassagesByCustomType(customTypeId: string): PassageEntry[] {
    return Array.from(this.passages.values()).filter(p => p.customTypeId === customTypeId);
  }

  getPassagesByUri(uri: string): PassageEntry[] {
    return this.passagesByUri.get(uri) ?? [];
  }

  getAllPassageNames(): string[] {
    return Array.from(this.passages.keys());
  }

  getAllPassages(): PassageEntry[] {
    return Array.from(this.passages.values());
  }

  hasPassage(name: string): boolean {
    return this.passages.has(name);
  }

  getDuplicateNames(): string[] {
    return Array.from(this._duplicateNames);
  }

  get size(): number {
    return this.passages.size;
  }

  /**
   * Find all passages that link to a given target passage.
   * Uses passageRefs (format-driven, single source of truth).
   * Covers [[ ]] links, navigation macros, API calls, and implicit refs.
   */
  getPassagesLinkingTo(targetName: string): PassageEntry[] {
    return Array.from(this.passages.values()).filter(p =>
      p.passageRefs.some(ref =>
        ref.kind === PassageRefKind.Link && ref.linkKind === LinkKind.Passage && ref.target === targetName
      ),
    );
  }

  /**
   * Find all passages that reference a given target passage through ANY ref kind.
   * This includes links, macros, API calls, and implicit refs.
   */
  getPassagesReferencing(targetName: string): PassageEntry[] {
    return Array.from(this.passages.values()).filter(p =>
      p.passageRefs.some(ref => ref.target === targetName),
    );
  }

  onDidChange(listener: IndexChangeListener): { dispose: () => void } {
    this.listeners.push(listener);
    return {
      dispose: () => {
        const idx = this.listeners.indexOf(listener);
        if (idx >= 0) this.listeners.splice(idx, 1);
      },
    };
  }

  clear(): void {
    this.passages.clear();
    this.passagesByUri.clear();
    this._duplicateNames.clear();
  }

  // ─── Private Helpers ─────────────────────────────────────────

  /**
   * Extract variable references using the format's variable capability.
   */
  private extractVariables(body: string, format: import('../formats/_types').FormatModule): { story: Set<string>; temp: Set<string> } {
    if (!format.variables) return { story: new Set(), temp: new Set() };

    const { sigils, variablePattern } = format.variables;
    const story = new Set<string>();
    const temp = new Set<string>();

    const regex = new RegExp(variablePattern.source, variablePattern.flags);
    let match: RegExpExecArray | null;
    while ((match = regex.exec(body)) !== null) {
      const sigilChar = match[1];
      const name = match[2];
      const sigilDef = sigils.find(s => s.sigil === sigilChar);
      if (sigilDef) {
        if (sigilDef.kind === 'story') {
          story.add(name);
        } else if (sigilDef.kind === 'temp') {
          temp.add(name);
        }
      }
    }

    return { story, temp };
  }

  /**
   * Extract macro names from passage body using the format's macroPattern.
   */
  private extractMacroNames(body: string, format: import('../formats/_types').FormatModule): string[] {
    const pattern = format.macroPattern;
    if (!pattern) return [];

    const names: string[] = [];
    const regex = new RegExp(pattern.source, pattern.flags);
    let match: RegExpExecArray | null;
    while ((match = regex.exec(body)) !== null) {
      if (match[1]) {
        names.push(match[1]);
      }
    }
    return names;
  }

  private removeDocumentInternal(uri: string): PassageEntry[] {
    const entries = this.passagesByUri.get(uri) ?? [];
    for (const entry of entries) {
      this.passages.delete(entry.name);
      this._duplicateNames.delete(entry.name);
    }
    this.passagesByUri.delete(uri);
    return entries;
  }

  private notifyListeners(event: IndexChangeEvent): void {
    for (const listener of this.listeners) {
      listener(event);
    }
  }
}
