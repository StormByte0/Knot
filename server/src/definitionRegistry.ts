import type { SourceRange } from './tokenTypes';
import type { InferredType } from './typeInference';

// ---------------------------------------------------------------------------
// Types — re-exported for consumers that import from workspaceIndex
// ---------------------------------------------------------------------------

export interface PassageDef {
  uri:         string;
  range:       SourceRange;
  passageName: string;
}

export interface VarDef {
  uri:          string;
  range:        SourceRange;
  passageName:  string;
  inferredType?: InferredType;
}

export interface JsDef {
  uri:          string;
  range:        SourceRange;
  inferredType: InferredType;
}

// ---------------------------------------------------------------------------
// DefinitionRegistry — manages passage, macro, variable & JS-global definitions
// ---------------------------------------------------------------------------

export class DefinitionRegistry {
  /** Primary passage definition — first-write-wins (go-to-definition target). */
  private passageDefinitions    = new Map<string, PassageDef>();
  /** All definitions for each passage name — used for duplicate detection. */
  private allPassageDefinitions = new Map<string, PassageDef[]>();
  /** Macro / widget definitions — first-write-wins. */
  private macroDefinitions      = new Map<string, PassageDef>();
  /** Story variable definitions — first-write-wins. */
  private variableDefinitions   = new Map<string, VarDef>();
  /** JS global definitions — first-write-wins. */
  private jsGlobalDefinitions   = new Map<string, JsDef>();

  // ---- Passage definitions -------------------------------------------------

  addPassageDefinition(name: string, def: PassageDef): void {
    // First-write-wins for the primary map (go-to-definition)
    if (!this.passageDefinitions.has(name)) {
      this.passageDefinitions.set(name, def);
    }

    // All definitions — for duplicate detection.
    // Deduplicate by (uri, range.start) so re-entrant calls do not double-count.
    const all = this.allPassageDefinitions.get(name) ?? [];
    if (!all.some(d => d.uri === def.uri && d.range.start === def.range.start)) {
      all.push(def);
    }
    this.allPassageDefinitions.set(name, all);
  }

  getPassageDefinition(name: string): PassageDef | undefined {
    return this.passageDefinitions.get(name);
  }

  getAllPassageDefinitions(name: string): PassageDef[] {
    return this.allPassageDefinitions.get(name) ?? [];
  }

  getPassageNames(): string[] {
    return [...this.passageDefinitions.keys()];
  }

  /** Iterate all primary passage definition keys. */
  passageKeys(): IterableIterator<string> {
    return this.passageDefinitions.keys();
  }

  hasPassage(name: string): boolean {
    return this.passageDefinitions.has(name);
  }

  // ---- Macro / widget definitions ------------------------------------------

  addMacroDefinition(name: string, def: PassageDef): void {
    if (!this.macroDefinitions.has(name)) {
      this.macroDefinitions.set(name, def);
    }
  }

  getMacroDefinition(name: string): PassageDef | undefined {
    return this.macroDefinitions.get(name);
  }

  hasMacro(name: string): boolean {
    return this.macroDefinitions.has(name);
  }

  // ---- Variable definitions ------------------------------------------------

  addVariableDefinition(name: string, def: VarDef): void {
    if (!this.variableDefinitions.has(name)) {
      this.variableDefinitions.set(name, def);
    }
  }

  getVariableDefinition(name: string): VarDef | undefined {
    return this.variableDefinitions.get(name);
  }

  // ---- JS global definitions -----------------------------------------------

  addJsGlobalDefinition(name: string, def: JsDef): void {
    if (!this.jsGlobalDefinitions.has(name)) {
      this.jsGlobalDefinitions.set(name, def);
    }
  }

  getJsGlobalDefinition(name: string): JsDef | undefined {
    return this.jsGlobalDefinitions.get(name);
  }

  getAllJsGlobals(): Map<string, JsDef> {
    return this.jsGlobalDefinitions;
  }

  // ---- Lifecycle -----------------------------------------------------------

  clear(): void {
    this.passageDefinitions.clear();
    this.allPassageDefinitions.clear();
    this.macroDefinitions.clear();
    this.variableDefinitions.clear();
    this.jsGlobalDefinitions.clear();
  }
}
