import type { SourceRange } from './tokenTypes';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface PassageRef {
  uri:           string;
  range:         SourceRange;
  sourcePassage: string;
}

export interface MacroRef {
  uri:   string;
  range: SourceRange;
}

// ---------------------------------------------------------------------------
// ReferenceIndex — tracks passage, variable & macro call-site references
// ---------------------------------------------------------------------------

export class ReferenceIndex {
  private passageReferences  = new Map<string, PassageRef[]>();
  private variableReferences = new Map<string, Array<{ uri: string; range: SourceRange }>>();
  private macroCallSites     = new Map<string, MacroRef[]>();

  // ---- Passage references --------------------------------------------------

  addPassageReference(targetName: string, ref: PassageRef): void {
    const refs = this.passageReferences.get(targetName) ?? [];
    // Deduplicate by (uri, range.start) — guards against reanalyzeAll being
    // called more than once with the same content before the clear takes effect.
    if (!refs.some(r => r.uri === ref.uri && r.range.start === ref.range.start)) {
      refs.push(ref);
    }
    this.passageReferences.set(targetName, refs);
  }

  getPassageReferences(name: string): PassageRef[] {
    return this.passageReferences.get(name) ?? [];
  }

  getReferencingFiles(passageName: string): string[] {
    return [...new Set((this.passageReferences.get(passageName) ?? []).map(r => r.uri))].sort();
  }

  // ---- Variable references -------------------------------------------------

  addVariableReference(varName: string, ref: { uri: string; range: SourceRange }): void {
    const refs = this.variableReferences.get(varName) ?? [];
    if (!refs.some(r => r.uri === ref.uri && r.range.start === ref.range.start)) {
      refs.push(ref);
    }
    this.variableReferences.set(varName, refs);
  }

  getVariableReferences(varName: string): Array<{ uri: string; range: SourceRange }> {
    return this.variableReferences.get(varName) ?? [];
  }

  // ---- Macro call sites ----------------------------------------------------

  addMacroCallSite(macroName: string, ref: MacroRef): void {
    const existing = this.macroCallSites.get(macroName) ?? [];
    if (!existing.some(r => r.uri === ref.uri && r.range.start === ref.range.start)) {
      existing.push(ref);
    }
    this.macroCallSites.set(macroName, existing);
  }

  getMacroCallSites(name: string): MacroRef[] {
    return this.macroCallSites.get(name) ?? [];
  }

  hasMacroCallSite(name: string): boolean {
    return this.macroCallSites.has(name);
  }

  // ---- Lifecycle -----------------------------------------------------------

  clear(): void {
    this.passageReferences.clear();
    this.variableReferences.clear();
    this.macroCallSites.clear();
  }
}
