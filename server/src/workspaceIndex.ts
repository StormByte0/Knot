import { DocumentNode, ExpressionNode, MarkupNode, ParseDiagnostic } from './ast';
import { AnalysisResult, SyntaxAnalyzer } from './analyzer';
import { IncrementalParser } from './incrementalParser';
import { SymbolKind, UserSymbol, buildSymbolTable } from './symbols';
import { TypeInference, InferredType } from './typeInference';
import { SourceRange } from './tokenTypes';
import { passageNameFromExpr } from './passageArgs';
import { FormatRegistry } from './formats/registry';
import type { StoryFormatAdapter } from './formats/types';
import { DiagnosticSeverity } from 'vscode-languageserver/node';

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

// ---------------------------------------------------------------------------
// LintConfig — severity settings for diagnostics
// ---------------------------------------------------------------------------

export interface LintConfig {
  unknownPassage:      DiagnosticSeverity;
  unknownMacro:        DiagnosticSeverity;
  duplicatePassage:    DiagnosticSeverity;
  typeMismatch:        DiagnosticSeverity;
  unreachablePassage:  DiagnosticSeverity;
  containerStructure:  DiagnosticSeverity;
}

const DEFAULT_LINT_CONFIG: LintConfig = {
  unknownPassage:     DiagnosticSeverity.Warning,
  unknownMacro:       DiagnosticSeverity.Warning,
  duplicatePassage:   DiagnosticSeverity.Error,
  typeMismatch:       DiagnosticSeverity.Error,
  unreachablePassage: DiagnosticSeverity.Warning,
  containerStructure: DiagnosticSeverity.Error,
};

// ---------------------------------------------------------------------------
// WorkspaceIndex
// ---------------------------------------------------------------------------

export class WorkspaceIndex {
  private parser   = new IncrementalParser();
  private analyzer = new SyntaxAnalyzer();
  private typer    = new TypeInference();

  // Ground truth
  private parseCache = new Map<string, ParsedFile>();

  // Maximum number of files to keep in the parse cache. When exceeded,
  // the least-recently-analyzed files are evicted to prevent unbounded
  // memory growth on very large workspaces.
  private static readonly MAX_CACHED_FILES = 500;

  // Track access order for LRU eviction
  private accessOrder: string[] = [];

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
  private _lintConfig: LintConfig = { ...DEFAULT_LINT_CONFIG };

  // ---- File management -----------------------------------------------------

  upsertFile(uri: string, text: string): void {
    const parsed = this.parser.parse(uri, text);
    this.parseCache.set(uri, parsed);
    // Update access order for LRU
    const idx = this.accessOrder.indexOf(uri);
    if (idx !== -1) this.accessOrder.splice(idx, 1);
    this.accessOrder.push(uri);
    this.evictIfNeeded();
  }

  removeFile(uri: string): void {
    this.parseCache.delete(uri);
    this.analysisCache.delete(uri);
    this.parser.evictUri(uri);
    // Remove from access order
    const idx = this.accessOrder.indexOf(uri);
    if (idx !== -1) this.accessOrder.splice(idx, 1);
  }

  /**
   * Evict oldest entries when the parse cache exceeds the limit.
   * Only evicts files not currently in the derived analysis — those are
   * likely to be needed again soon.
   */
  private evictIfNeeded(): void {
    while (this.parseCache.size > WorkspaceIndex.MAX_CACHED_FILES && this.accessOrder.length > 0) {
      const oldest = this.accessOrder[0]!;
      // Don't evict files that have active analysis results
      if (this.analysisCache.has(oldest)) {
        // Skip to next — move to end so we try other candidates
        this.accessOrder.shift();
        this.accessOrder.push(oldest);
        // Safety: if all files have analysis, stop evicting
        if (this.accessOrder.every(u => this.analysisCache.has(u))) break;
        continue;
      }
      this.parseCache.delete(oldest);
      this.accessOrder.shift();
    }
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
    // Clear the incremental parser's passage cache on full reanalysis
    // to prevent stale entries from accumulating across many sessions.
    this.parser.clearCache();

    const uris = this.getKnownUris();
    const adapter = this.getActiveAdapter();

    // Pass 1: register definitions
    const ordered = this._scriptFirstOrder(uris);
    for (const uri of ordered) {
      this._registerDefinitions(uri, adapter);
    }

    // Pass 2: extract all passage links
    for (const uri of uris) {
      this._extractAndIndexLinks(uri, adapter);
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

      // Add unreachable passage diagnostics
      const unreachableDiags = this._buildUnreachableDiagnostics(uri, parsed.ast, adapter);
      if (unreachableDiags.length > 0) {
        (analysis.diagnostics as ParseDiagnostic[]).push(...unreachableDiags);
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
   * Uses single-pass reduce instead of repeated .filter() calls — O(n) vs O(n²).
   */
  private _buildDuplicateDiagnosticsForUri(uri: string, ast: DocumentNode): ParseDiagnostic[] {
    const diags: ParseDiagnostic[] = [];
    const severity = this._lintConfig.duplicatePassage === DiagnosticSeverity.Error ? 'error' as const
      : this._lintConfig.duplicatePassage === DiagnosticSeverity.Warning ? 'warning' as const
      : 'warning' as const;

    for (const passage of ast.passages) {
      const allDefs = this.allPassageDefinitions.get(passage.name);
      if (!allDefs || allDefs.length < 2) continue;
      // Single-pass: count self vs other
      let othersCount = 0;
      let selfCount = 0;
      const otherFiles = new Set<string>();
      for (const d of allDefs) {
        if (d.uri !== uri) {
          othersCount++;
          otherFiles.add(d.uri);
        } else {
          selfCount++;
        }
      }
      if (othersCount > 0 || selfCount > 1) {
        const otherFileNames = [...otherFiles].map(u => u.split('/').pop());
        const detail = otherFileNames.length > 0
          ? `also defined in: ${otherFileNames.join(', ')}`
          : 'defined multiple times in this file';
        diags.push({
          message: `Duplicate passage name "${passage.name}" — ${detail}`,
          range:   passage.nameRange,
          severity,
        });
      }
    }
    return diags;
  }

  // ---- Unreachable passage diagnostics --------------------------------------

  /**
   * Return passage names that are unreachable from the start passage.
   * Uses BFS from the start passage and marks special passages as always reachable.
   */
  getUnreachablePassages(): string[] {
    const adapter = this.getActiveAdapter();
    const startPassage = this._getStartPassageName();
    const specialNames = adapter.getSpecialPassageNames();
    const allNames = new Set(this.passageDefinitions.keys());

    if (allNames.size === 0) return [];

    // Build adjacency list from passageReferences
    const visited = new Set<string>();

    // Always mark special passages as reachable
    for (const name of specialNames) {
      if (allNames.has(name)) visited.add(name);
    }

    // BFS from start passage
    if (startPassage && allNames.has(startPassage)) {
      const queue: string[] = [startPassage];
      visited.add(startPassage);

      while (queue.length > 0) {
        const current = queue.shift()!;
        // Find all passages that current links to
        const refs = this.passageReferences.get(current);
        if (!refs) continue;

        // Get unique targets from this passage
        const targets = new Set<string>();
        for (const ref of refs) {
          // We need the target passage name - use fileLinkRefs to find it
          // Actually, passageReferences maps target -> sources. We need source -> targets.
          // Let's use fileLinkRefs instead
        }
      }
    }

    // Build forward adjacency from fileLinkRefs
    const forwardAdj = new Map<string, Set<string>>();
    for (const [uri, links] of this.fileLinkRefs) {
      for (const link of links) {
        let targets = forwardAdj.get(link.sourcePassage);
        if (!targets) {
          targets = new Set();
          forwardAdj.set(link.sourcePassage, targets);
        }
        targets.add(link.target);
      }
    }

    // BFS from start passage using forward adjacency
    visited.clear();
    // Always mark special passages as reachable
    for (const name of specialNames) {
      if (allNames.has(name)) visited.add(name);
    }

    if (startPassage && allNames.has(startPassage)) {
      const queue: string[] = [startPassage];
      visited.add(startPassage);

      while (queue.length > 0) {
        const current = queue.shift()!;
        const targets = forwardAdj.get(current);
        if (!targets) continue;
        for (const target of targets) {
          if (!visited.has(target) && allNames.has(target)) {
            visited.add(target);
            queue.push(target);
          }
        }
      }
    }

    // Also add system passages that are always reachable (from adapter)
    const systemNames = adapter.getSystemPassageNames();
    for (const name of allNames) {
      if (systemNames.has(name)) {
        visited.add(name);
      }
    }

    const unreachable: string[] = [];
    for (const name of allNames) {
      if (!visited.has(name)) unreachable.push(name);
    }
    return unreachable;
  }

  private _buildUnreachableDiagnostics(uri: string, ast: DocumentNode, adapter: StoryFormatAdapter): ParseDiagnostic[] {
    const diags: ParseDiagnostic[] = [];
    const unreachable = new Set(this.getUnreachablePassages());
    if (unreachable.size === 0) return diags;

    const severity = this._lintConfig.unreachablePassage === DiagnosticSeverity.Error ? 'error' as const
      : this._lintConfig.unreachablePassage === DiagnosticSeverity.Warning ? 'warning' as const
      : 'warning' as const;

    for (const passage of ast.passages) {
      if (unreachable.has(passage.name)) {
        diags.push({
          message: `Passage "${passage.name}" is unreachable — no links or macros navigate to it`,
          range:   passage.nameRange,
          severity,
        });
      }
    }
    return diags;
  }

  /** Get the start passage name from StoryData. */
  private _getStartPassageName(): string | null {
    // Look through all parsed files for StoryData
    for (const [, parsed] of this.parseCache) {
      const storyDataPassage = parsed.ast.passages.find(p => p.name === 'StoryData');
      if (!storyDataPassage) continue;

      // Parse the body to find "start" field
      let source = '';
      if (!Array.isArray(storyDataPassage.body)) {
        source = 'source' in storyDataPassage.body ? storyDataPassage.body.source : '';
      } else {
        for (const node of storyDataPassage.body) {
          if (node.type === 'text') source += node.value;
        }
      }

      source = source.trim();
      if (!source) continue;

      try {
        const parsed = JSON.parse(source);
        if (parsed && typeof parsed === 'object' && typeof parsed.start === 'string') {
          return parsed.start;
        }
      } catch {
        // Ignore malformed JSON
      }
    }
    return null;
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

  getActiveAdapter(): StoryFormatAdapter {
    // Default to sugarcube-2 when no format is explicitly set.
    // This ensures SugarCube features work before StoryData is parsed.
    return FormatRegistry.resolve(this._activeFormatId || 'sugarcube-2');
  }

  setLintConfig(config: Partial<LintConfig>): void {
    this._lintConfig = { ...this._lintConfig, ...config };
  }

  getLintConfig(): LintConfig {
    return this._lintConfig;
  }

  // ---- Private: Pass 1 — definition registration --------------------------

  private _registerDefinitions(uri: string, adapter: StoryFormatAdapter): void {
    const parsed = this.parseCache.get(uri);
    if (!parsed) return;

    const symbols    = buildSymbolTable(parsed.ast, uri, adapter);
    const typeResult = this.typer.inferDocument(parsed.ast, adapter);

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

  private _extractAndIndexLinks(uri: string, adapter: StoryFormatAdapter): void {
    const parsed = this.parseCache.get(uri);
    if (!parsed) return;

    const links: LinkRef[] = [];
    const passageArgMacros = adapter.getPassageArgMacros();

    for (const passage of parsed.ast.passages) {
      if (!Array.isArray(passage.body)) continue;
      const src = passage.name;

      const walk = (nodes: MarkupNode[]): void => {
        for (const node of nodes) {
          if (node.type === 'link') {
            links.push({ target: node.target, range: node.range, sourcePassage: src });
          } else if (node.type === 'macro') {
            if (passageArgMacros.has(node.name) && node.args.length > 0) {
              const idx    = adapter.getPassageArgIndex(node.name, node.args.length);
              const arg    = node.args[idx];
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
    const adapter = this.getActiveAdapter();
    return [...uris].sort((a, b) => {
      const aAst = this.parseCache.get(a)?.ast;
      const bAst = this.parseCache.get(b)?.ast;
      // Sort by analysis priority of first passage
      const aPriority = aAst ? Math.min(...aAst.passages.map(p => adapter.getAnalysisPriority(p.name))) : 100;
      const bPriority = bAst ? Math.min(...bAst.passages.map(p => adapter.getAnalysisPriority(p.name))) : 100;
      if (aPriority !== bPriority) return aPriority - bPriority;
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
