/**
 * Knot v2 — Symbol Table
 *
 * Workspace-wide symbol table for passages, macros, and variables.
 * Format-agnostic — macro and variable classification use enums from hooks.
 *
 * MUST NOT import from: formats/
 */

import { MacroCategory, PassageType } from '../hooks/hookTypes';

export enum SymbolKind {
  Passage = 'passage',
  Macro = 'macro',
  Variable = 'variable',
  StoryVariable = 'storyVariable',
  TempVariable = 'tempVariable',
}

export interface SymbolEntry {
  kind: SymbolKind;
  name: string;
  macroCategory?: MacroCategory;
  passageType?: PassageType;
  uri: string;
  startOffset: number;
  endOffset: number;
  containerName?: string;
}

export class SymbolTable {
  private symbolsByName: Map<string, SymbolEntry[]> = new Map();
  private symbolsByUri: Map<string, SymbolEntry[]> = new Map();
  private symbolsByKind: Map<SymbolKind, SymbolEntry[]> = new Map();

  /** Add a symbol to the table. */
  addSymbol(entry: SymbolEntry): void {
    // Index by name
    const byName = this.symbolsByName.get(entry.name) ?? [];
    byName.push(entry);
    this.symbolsByName.set(entry.name, byName);

    // Index by URI
    const byUri = this.symbolsByUri.get(entry.uri) ?? [];
    byUri.push(entry);
    this.symbolsByUri.set(entry.uri, byUri);

    // Index by kind
    const byKind = this.symbolsByKind.get(entry.kind) ?? [];
    byKind.push(entry);
    this.symbolsByKind.set(entry.kind, byKind);
  }

  /** Look up symbols by name. */
  lookup(name: string): SymbolEntry[] {
    return this.symbolsByName.get(name) ?? [];
  }

  /** Get all symbols for a specific document URI. */
  getByUri(uri: string): SymbolEntry[] {
    return this.symbolsByUri.get(uri) ?? [];
  }

  /** Find all symbols of a specific kind. */
  findByKind(kind: SymbolKind): SymbolEntry[] {
    return this.symbolsByKind.get(kind) ?? [];
  }

  /** Remove all symbols from a specific document. */
  removeByUri(uri: string): void {
    const entries = this.symbolsByUri.get(uri) ?? [];
    for (const entry of entries) {
      // Remove from name index
      const byName = this.symbolsByName.get(entry.name);
      if (byName) {
        const idx = byName.indexOf(entry);
        if (idx >= 0) byName.splice(idx, 1);
        if (byName.length === 0) this.symbolsByName.delete(entry.name);
      }
      // Remove from kind index
      const byKind = this.symbolsByKind.get(entry.kind);
      if (byKind) {
        const idx = byKind.indexOf(entry);
        if (idx >= 0) byKind.splice(idx, 1);
        if (byKind.length === 0) this.symbolsByKind.delete(entry.kind);
      }
    }
    this.symbolsByUri.delete(uri);
  }

  /** Clear the entire symbol table. */
  clear(): void {
    this.symbolsByName.clear();
    this.symbolsByUri.clear();
    this.symbolsByKind.clear();
  }

  /** Get total symbol count. */
  get size(): number {
    let count = 0;
    for (const entries of this.symbolsByName.values()) {
      count += entries.length;
    }
    return count;
  }
}
