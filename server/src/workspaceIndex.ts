import { DocumentNode, ExpressionNode, MarkupNode, ParseDiagnostic } from './ast';
import { AnalysisResult, SyntaxAnalyzer } from './analyzer';
import { IncrementalParser } from './incrementalParser';
import { SymbolKind, UserSymbol, buildSymbolTable } from './symbols';
import { TypeInference, InferredType } from './typeInference';
import { SourceRange } from './tokenTypes';
import { PASSAGE_ARG_MACROS, passageArgIndex, passageNameFromExpr } from './passageArgs';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface ParsedFile {
  ast: DocumentNode;
  diagnostics: ParseDiagnostic[];
}

interface LinkRef {
  target:        string;
  range:         SourceRange;
  sourcePassage: string;
}

interface MacroRef {
  uri:   string;
  range: SourceRange;
}

interface PassageDef {
  uri:         string;
  range:       SourceRange;
  passageName: string;
}

interface VarDef {
  uri:          string;
  range:        SourceRange;
  passageName:  string;
  inferredType?: InferredType;
}

interface JsDef {
  uri:          string;
  range:        SourceRange;
  inferredType: InferredType;
}

interface PassageRef {
  uri:           string;
  range:         SourceRange;
  sourcePassage: string;
}

export interface IncomingLink {
  sourcePassage: string;
  uri:           string;
  /** Number of times this passage links to the target (for badge display). */
  count:         number;
}

// Passage-arg macros imported from passageArgs.ts — single source of truth

// ---------------------------------------------------------------------------
// WorkspaceIndex
// ---------------------------------------------------------------------------

export class WorkspaceIndex {
  private parser   = new IncrementalParser();
  private analyzer = new SyntaxAnalyzer();
  private typer    = new TypeInference();

  // Ground truth
  private parseCache = new Map<string, ParsedFile>();

  // Derived — rebuilt entirely by reanalyzeAll()
  private analysisCache       = new Map<string, AnalysisResult>();
  private passageDefinitions  = new Map<string, PassageDef>();
  /** All definitions for each passage name — used for duplicate detection. */
  private allPassageDefinitions = new Map<string, PassageDef[]>();
  private macroDefinitions    = new Map<string, PassageDef>();
  private variableDefinitions = new Map<string, VarDef>();
  private jsGlobalDefinitions = new Map<string, JsDef>();
  private passageReferences   = new Map<string, PassageRef[]>();
  private variableReferences  = new Map<string, Array<{ uri: string; range: SourceRange }>>();
  private macroCallSites      = new Map<string, MacroRef[]>();
  private fileLinkRefs        = new Map<string, LinkRef[]>();

  private _activeFormatId = '';

  // ---- File management -----------------------------------------------------

  upsertFile(uri: string, text: string): void {
    const parsed = this.parser.parse(uri, text);
    this.parseCache.set(uri, parsed);
  }

  removeFile(uri: string): void {
    this.parseCache.delete(uri);
  }

  getKnownUris(): string[] {
    return [...this.parseCache.keys()].sort();
  }

  hasFile(uri: string): boolean {
    return this.parseCache.has(uri);
  }

  /** Get the parsed AST for a file — avoids re-parsing in LSP handlers. */
  getParsedFile(uri: string): ParsedFile | undefined {
    return this.parseCache.get(uri);
  }

  // ---- Full reanalysis -----------------------------------------------------

  reanalyzeAll(): void {
    // Clear all derived state
    this.analysisCache.clear();
    this.passageDefinitions.clear();
    this.allPassageDefinitions.clear();
    this.macroDefinitions.clear();
    this.variableDefinitions.clear();
    this.jsGlobalDefinitions.clear();
    this.passageReferences.clear();
    this.variableReferences.clear();
    this.macroCallSites.clear();
    this.fileLinkRefs.clear();

    const uris = this.getKnownUris();

    // Pass 1: register definitions
    const ordered = this._scriptFirstOrder(uris);
    for (const uri of ordered) {
      this._registerDefinitions(uri);
    }

    // Pass 2: extract all passage links
    for (const uri of uris) {
      this._extractAndIndexLinks(uri);
    }

    // Pass 3: analyze (all defs + refs now complete)
    for (const uri of uris) {
      const parsed = this.parseCache.get(uri);
      if (!parsed) continue;
      const analysis = this.analyzer.analyze(parsed.ast, uri, this);

      // Inject duplicate-passage diagnostics into the analysis result
      const dupDiags = this._buildDuplicateDiagnosticsForUri(uri, parsed.ast);
      if (dupDiags.length > 0) {
        (analysis.diagnostics as ParseDiagnostic[]).push(...dupDiags);
      }

      this.analysisCache.set(uri, analysis);
      this._indexVariableRefs(parsed.ast, uri);
      this._indexCallSites(parsed.ast, uri);
    }
  }

  // ---- Duplicate passage diagnostics ---------------------------------------

  /**
   * Returns error diagnostics for any passage in this file whose name is
   * also defined in another file (or multiple times within this file).
   */
  private _buildDuplicateDiagnosticsForUri(uri: string, ast: DocumentNode): ParseDiagnostic[] {
    const diags: ParseDiagnostic[] = [];
    for (const passage of ast.passages) {
      const allDefs = this.allPassageDefinitions.get(passage.name);
      if (!allDefs || allDefs.length < 2) continue;
      // Count how many definitions belong to other URIs (or this URI more than once)
      const othersCount = allDefs.filter(d => d.uri !== uri).length;
      const selfCount   = allDefs.filter(d => d.uri === uri).length;
      if (othersCount > 0 || selfCount > 1) {
        const otherFiles = [...new Set(allDefs.filter(d => d.uri !== uri).map(d => d.uri))];
        const detail = otherFiles.length > 0
          ? `also defined in: ${otherFiles.map(u => u.split('/').pop()).join(', ')}`
          : 'defined multiple times in this file';
        diags.push({
          message: `Duplicate passage name "${passage.name}" — ${detail}`,
          range:   passage.nameRange,
          severity: 'error',
        });
      }
    }
    return diags;
  }

  // ---- Accessors -----------------------------------------------------------

  getAnalysis(uri: string): AnalysisResult | undefined {
    return this.analysisCache.get(uri);
  }

  getPassageDefinition(name: string): PassageDef | undefined {
    return this.passageDefinitions.get(name);
  }

  getMacroDefinition(name: string): PassageDef | undefined {
    return this.macroDefinitions.get(name);
  }

  getVariableDefinition(name: string): VarDef | undefined {
    return this.variableDefinitions.get(name);
  }

  getJsGlobalDefinition(name: string): JsDef | undefined {
    return this.jsGlobalDefinitions.get(name);
  }

  getAllJsGlobals(): Map<string, JsDef> {
    return this.jsGlobalDefinitions;
  }

  getPassageNames(): string[] {
    return [...this.passageDefinitions.keys()];
  }

  getReferencingFiles(passageName: string): string[] {
    return [...new Set((this.passageReferences.get(passageName) ?? []).map(r => r.uri))].sort();
  }

  /**
   * Returns deduplicated incoming links grouped by (uri, sourcePassage).
   * When the same passage links to the target multiple times (e.g. both a
   * [[link]] and a <<goto>>, or the same link written twice), the count
   * field reflects how many distinct link references were recorded so the
   * hover can show a badge like "PassageName (×2)".
   */
  getIncomingLinks(passageName: string): IncomingLink[] {
    const rawRefs = this.passageReferences.get(passageName) ?? [];

    // Group by `uri:sourcePassage` — count raw refs per group
    const grouped = new Map<string, { sourcePassage: string; uri: string; count: number }>();
    for (const ref of rawRefs) {
      const key = `${ref.uri}:${ref.sourcePassage}`;
      const existing = grouped.get(key);
      if (existing) {
        existing.count++;
      } else {
        grouped.set(key, { sourcePassage: ref.sourcePassage, uri: ref.uri, count: 1 });
      }
    }

    return [...grouped.values()];
  }

  getVariableReferences(varName: string): Array<{ uri: string; range: SourceRange }> {
    return this.variableReferences.get(varName) ?? [];
  }

  getMacroCallSites(name: string): MacroRef[] {
    return this.macroCallSites.get(name) ?? [];
  }

  // Kept for backward compat with hover handler
  getCachedUris(): string[] { return this.getKnownUris(); }

  setActiveFormatId(id: string): void { this._activeFormatId = id; }
  getActiveFormatId(): string         { return this._activeFormatId; }

  // ---- Private: Pass 1 — definition registration --------------------------

  private _registerDefinitions(uri: string): void {
    const parsed = this.parseCache.get(uri);
    if (!parsed) return;

    const symbols    = buildSymbolTable(parsed.ast, uri);
    const typeResult = this.typer.inferDocument(parsed.ast);

    // Passages — track ALL definitions for duplicate detection
    for (const sym of symbols.table.getUserSymbols()) {
      if (sym.kind === SymbolKind.Passage) {
        const def: PassageDef = { uri, range: sym.range, passageName: sym.name };

        // First-write-wins for the primary definition map (go-to-definition)
        if (!this.passageDefinitions.has(sym.name)) {
          this.passageDefinitions.set(sym.name, def);
        }

        // All definitions — for duplicate detection.
        // Deduplicate by (uri, range.start) so re-entrant calls do not double-count.
        const all = this.allPassageDefinitions.get(sym.name) ?? [];
        if (!all.some(d => d.uri === uri && d.range.start === sym.range.start)) {
          all.push(def);
        }
        this.allPassageDefinitions.set(sym.name, all);
      }
    }

    // Macros / widgets
    for (const sym of symbols.table.getUserSymbols()) {
      if (sym.kind === SymbolKind.Macro || sym.kind === SymbolKind.Widget) {
        if (!this.macroDefinitions.has(sym.name)) {
          const passage = this._passageForOffset(parsed.ast, sym.range.start);
          this.macroDefinitions.set(sym.name, { uri, range: sym.range, passageName: passage ?? '' });
        }
      }
    }

    // Story variables (first assignment wins for type inference)
    for (const sym of symbols.table.getUserSymbols()) {
      if (sym.kind === SymbolKind.StoryVar && !this.variableDefinitions.has(sym.name)) {
        const passage = this._passageForOffset(parsed.ast, sym.range.start);
        this.variableDefinitions.set(sym.name, {
          uri, range: sym.range,
          passageName: passage ?? '',
          inferredType: typeResult.assignments.get(sym.name),
        });
      }
    }

    // JS globals
    for (const [name, def] of typeResult.jsGlobals) {
      if (!this.jsGlobalDefinitions.has(name)) {
        this.jsGlobalDefinitions.set(name, { uri, range: def.range, inferredType: def.inferredType });
      }
    }
  }

  // ---- Private: Pass 2 — link extraction ----------------------------------

  private _extractAndIndexLinks(uri: string): void {
    const parsed = this.parseCache.get(uri);
    if (!parsed) return;

    const links: LinkRef[] = [];

    for (const passage of parsed.ast.passages) {
      if (!Array.isArray(passage.body)) continue;
      const src = passage.name;

      const walk = (nodes: MarkupNode[]): void => {
        for (const node of nodes) {
          if (node.type === 'link') {
            links.push({ target: node.target, range: node.range, sourcePassage: src });
          } else if (node.type === 'macro') {
            if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
              const arg    = node.args[passageArgIndex(node.name, node.args.length)];
              const target = arg ? passageNameFromExpr(arg) : null;
              if (target && arg) links.push({ target, range: arg.range, sourcePassage: src });
            }
            if (node.body) walk(node.body);
          }
        }
      };

      walk(passage.body);
    }

    this.fileLinkRefs.set(uri, links);

    for (const link of links) {
      const refs = this.passageReferences.get(link.target) ?? [];
      // Deduplicate by source offset — guards against reanalyzeAll being called
      // more than once with the same content before the clear takes effect.
      if (!refs.some(r => r.uri === uri && r.range.start === link.range.start)) {
        refs.push({ uri, range: link.range, sourcePassage: link.sourcePassage });
      }
      this.passageReferences.set(link.target, refs);
    }
  }

  // ---- Private: Pass 3 helpers --------------------------------------------

  private _indexVariableRefs(ast: DocumentNode, uri: string): void {
    const walk = (nodes: MarkupNode[]): void => {
      for (const node of nodes) {
        if (node.type !== 'macro') continue;
        for (const arg of node.args) this._walkExprVars(arg, uri);
        if (node.body) walk(node.body);
      }
    };
    for (const p of ast.passages) {
      if (Array.isArray(p.body)) walk(p.body);
    }
  }

  private _walkExprVars(expr: ExpressionNode, uri: string): void {
    switch (expr.type) {
      case 'storyVar': {
        const refs = this.variableReferences.get(expr.name) ?? [];
        if (!refs.some(r => r.uri === uri && r.range.start === expr.range.start)) {
          refs.push({ uri, range: expr.range });
          this.variableReferences.set(expr.name, refs);
        }
        return;
      }
      case 'binaryOp':    this._walkExprVars(expr.left, uri); this._walkExprVars(expr.right, uri); return;
      case 'unaryOp':     this._walkExprVars(expr.operand, uri); return;
      case 'propertyAccess': this._walkExprVars(expr.object, uri); return;
      case 'indexAccess': this._walkExprVars(expr.object, uri); this._walkExprVars(expr.index, uri); return;
      case 'call':        this._walkExprVars(expr.callee, uri); expr.args.forEach(a => this._walkExprVars(a, uri)); return;
      case 'arrayLiteral': expr.elements.forEach(e => this._walkExprVars(e, uri)); return;
      case 'objectLiteral': expr.properties.forEach(p => this._walkExprVars(p.value, uri)); return;
      default: return;
    }
  }

  private _indexCallSites(ast: DocumentNode, uri: string): void {
    const walk = (nodes: MarkupNode[]): void => {
      for (const node of nodes) {
        if (node.type !== 'macro') continue;
        if (this.macroDefinitions.has(node.name)) {
          const existing = this.macroCallSites.get(node.name) ?? [];
          if (!existing.some(r => r.uri === uri && r.range.start === node.nameRange.start)) {
            existing.push({ uri, range: node.nameRange });
            this.macroCallSites.set(node.name, existing);
          }
        }
        if (node.body) walk(node.body);
      }
    };
    for (const p of ast.passages) {
      if (Array.isArray(p.body)) walk(p.body);
    }
  }

  // ---- Private: utilities --------------------------------------------------

  private _passageForOffset(ast: DocumentNode, offset: number): string | null {
    for (const p of ast.passages) {
      if (offset >= p.range.start && offset <= p.range.end) return p.name;
    }
    return null;
  }

  private _scriptFirstOrder(uris: string[]): string[] {
    return [...uris].sort((a, b) => {
      const aS = this.parseCache.get(a)?.ast.passages.some(p => p.kind === 'script') ?? false;
      const bS = this.parseCache.get(b)?.ast.passages.some(p => p.kind === 'script') ?? false;
      if (aS && !bS) return -1;
      if (!aS && bS) return  1;
      return a.localeCompare(b);
    });
  }

  // ---------------------------------------------------------------------------
  // Backward-compat aliases
  // ---------------------------------------------------------------------------

  /** @deprecated Use upsertFile() + reanalyzeAll() */
  upsertDocument(uri: string, text: string): void {
    this.upsertFile(uri, text);
    this.reanalyzeAll();
  }

  /** @deprecated Use removeFile() + reanalyzeAll() */
  removeDocument(uri: string): void {
    this.removeFile(uri);
    this.reanalyzeAll();
  }
}