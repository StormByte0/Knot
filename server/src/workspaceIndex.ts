import { DocumentNode, ParseDiagnostic, ScriptBodyNode } from './ast';
import { AnalysisResult, SyntaxAnalyzer } from './analyzer';
import { IncrementalParser } from './incrementalParser';
import { SymbolKind, UserSymbol, buildSymbolTable } from './symbols';
import { TypeInference, InferredType } from './typeInference';
import { SourceRange } from './tokenTypes';
import { passageNameFromExpr } from './passageArgs';
import { FormatRegistry } from './formats/registry';
import type { StoryFormatAdapter } from './formats/types';
import { walkMarkup, walkDocument, walkExpression, walkDocumentExpressions } from './visitors';
import { DiagnosticSeverity } from 'vscode-languageserver/node';

// Decomposed components (R5)
import { DefinitionRegistry } from './definitionRegistry';
import type { PassageDef, VarDef, JsDef } from './definitionRegistry';
import { ReferenceIndex } from './referenceIndex';
import type { PassageRef, MacroRef } from './referenceIndex';
import { LinkGraph } from './linkGraph';
import type { LinkRef } from './linkGraph';
import { ParseCache } from './parseCache';
import type { ParsedFile } from './parseCache';

// Diagnostic rule system (R6)
import { DiagnosticEngine, DiagnosticRule } from './diagnosticEngine';

// Re-export types so existing consumers keep working
export type { PassageDef, VarDef, JsDef } from './definitionRegistry';
export type { PassageRef, MacroRef } from './referenceIndex';
export type { LinkRef } from './linkGraph';
export type { ParsedFile } from './parseCache';

// ---------------------------------------------------------------------------
// Types — kept here for backward compat
// ---------------------------------------------------------------------------

export interface IncomingLink {
  sourcePassage: string;
  uri:           string;
  /** Number of times this passage links to the target (for badge display). */
  count:         number;
}

// ---------------------------------------------------------------------------
// LintConfig — severity settings for diagnostics (backward compat)
// ---------------------------------------------------------------------------

export interface LintConfig {
  unknownPassage:      DiagnosticSeverity;
  unknownMacro:        DiagnosticSeverity;
  duplicatePassage:    DiagnosticSeverity;
  typeMismatch:        DiagnosticSeverity;
  unreachablePassage:  DiagnosticSeverity;
  containerStructure:  DiagnosticSeverity;
}

// ---------------------------------------------------------------------------
// WorkspaceIndex — thin coordinator delegating to decomposed components
// ---------------------------------------------------------------------------

export class WorkspaceIndex {
  private parser   = new IncrementalParser();
  private analyzer = new SyntaxAnalyzer();
  private typer    = new TypeInference();

  // Decomposed components (R5)
  private definitions  = new DefinitionRegistry();
  private references   = new ReferenceIndex();
  private links        = new LinkGraph();
  private parseCache   = new ParseCache();

  // Diagnostic rule system (R6)
  private diagnosticEngine = new DiagnosticEngine();

  // Derived — rebuilt entirely by reanalyzeAll()
  private analysisCache = new Map<string, AnalysisResult>();

  private _activeFormatId = '';

  // ---- File management -----------------------------------------------------

  upsertFile(uri: string, text: string): void {
    const parsed = this.parser.parse(uri, text);
    this.parseCache.set(uri, parsed);
  }

  removeFile(uri: string): void {
    this.parseCache.delete(uri);
    this.analysisCache.delete(uri);
    this.parser.evictUri(uri);
  }

  getKnownUris(): string[] {
    return this.parseCache.keys();
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
    this.definitions.clear();
    this.references.clear();
    this.links.clear();
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

    // Build the set of analyzed URIs for LRU eviction
    const analyzedUris = new Set<string>();

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
      analyzedUris.add(uri);
      this._indexVariableRefs(parsed.ast, uri);
      this._indexCallSites(parsed.ast, uri);
    }

    // Evict old parse cache entries
    this.parseCache.evictIfNeeded(analyzedUris);
  }

  // ---- Duplicate passage diagnostics ---------------------------------------

  /**
   * Returns error diagnostics for any passage in this file whose name is
   * also defined in another file (or multiple times within this file).
   * Uses single-pass reduce instead of repeated .filter() calls — O(n) vs O(n²).
   */
  private _buildDuplicateDiagnosticsForUri(uri: string, ast: DocumentNode): ParseDiagnostic[] {
    const diags: ParseDiagnostic[] = [];

    if (!this.diagnosticEngine.isEnabled(DiagnosticRule.DuplicatePassage)) return diags;

    const severity = this.diagnosticEngine.getSeverity(DiagnosticRule.DuplicatePassage) === DiagnosticSeverity.Error ? 'error' as const
      : 'warning' as const;

    for (const passage of ast.passages) {
      const allDefs = this.definitions.getAllPassageDefinitions(passage.name);
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
    const allNames = new Set(this.definitions.passageKeys());

    if (allNames.size === 0) return [];

    // Build forward adjacency from LinkGraph
    const forwardAdj = this.links.getForwardAdjacency();

    // BFS from start passage using forward adjacency
    const visited = new Set<string>();
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

    if (!this.diagnosticEngine.isEnabled(DiagnosticRule.UnreachablePassage)) return diags;

    const unreachable = new Set(this.getUnreachablePassages());
    if (unreachable.size === 0) return diags;

    const severity = this.diagnosticEngine.getSeverity(DiagnosticRule.UnreachablePassage) === DiagnosticSeverity.Error ? 'error' as const
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
    const adapter = this.getActiveAdapter();
    const sdName = adapter.getStoryDataPassageName();
    if (!sdName) return null;
    // Look through all parsed files for StoryData
    for (const [, parsed] of this.parseCache.entries()) {
      const storyDataPassage = parsed.ast.passages.find(p => p.name === sdName);
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

  // ---- Accessors — delegate to components ----------------------------------

  getAnalysis(uri: string): AnalysisResult | undefined {
    return this.analysisCache.get(uri);
  }

  getPassageDefinition(name: string): PassageDef | undefined {
    return this.definitions.getPassageDefinition(name);
  }

  getMacroDefinition(name: string): PassageDef | undefined {
    return this.definitions.getMacroDefinition(name);
  }

  getVariableDefinition(name: string): VarDef | undefined {
    return this.definitions.getVariableDefinition(name);
  }

  getJsGlobalDefinition(name: string): JsDef | undefined {
    return this.definitions.getJsGlobalDefinition(name);
  }

  getAllJsGlobals(): Map<string, JsDef> {
    return this.definitions.getAllJsGlobals();
  }

  getPassageNames(): string[] {
    return this.definitions.getPassageNames();
  }

  getReferencingFiles(passageName: string): string[] {
    return this.references.getReferencingFiles(passageName);
  }

  /**
   * Returns deduplicated incoming links grouped by (uri, sourcePassage).
   * When the same passage links to the target multiple times (e.g. both a
   * [[link]] and a <<goto>>, or the same link written twice), the count
   * field reflects how many distinct link references were recorded so the
   * hover can show a badge like "PassageName (×2)".
   */
  getIncomingLinks(passageName: string): IncomingLink[] {
    const rawRefs = this.references.getPassageReferences(passageName);

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
    return this.references.getVariableReferences(varName);
  }

  getMacroCallSites(name: string): MacroRef[] {
    return this.references.getMacroCallSites(name);
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

  // ---- Lint config — delegates to DiagnosticEngine (backward compat) -------

  setLintConfig(config: Partial<LintConfig>): void {
    this.diagnosticEngine.configureFromLintConfig(config as Record<string, DiagnosticSeverity>);
  }

  getLintConfig(): LintConfig {
    return this.diagnosticEngine.toLintConfig();
  }

  /** Expose the DiagnosticEngine for the analyzer and other consumers. */
  getDiagnosticEngine(): DiagnosticEngine {
    return this.diagnosticEngine;
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
        this.definitions.addPassageDefinition(sym.name, def);
      }
    }

    // Macros / widgets
    for (const sym of symbols.table.getUserSymbols()) {
      if (sym.kind === SymbolKind.Macro || sym.kind === SymbolKind.Widget) {
        if (!this.definitions.hasMacro(sym.name)) {
          const passage = this._passageForOffset(parsed.ast, sym.range.start);
          this.definitions.addMacroDefinition(sym.name, { uri, range: sym.range, passageName: passage ?? '' });
        }
      }
    }

    // Story variables (first assignment wins for type inference)
    for (const sym of symbols.table.getUserSymbols()) {
      if (sym.kind === SymbolKind.StoryVar && !this.definitions.getVariableDefinition(sym.name)) {
        const passage = this._passageForOffset(parsed.ast, sym.range.start);
        this.definitions.addVariableDefinition(sym.name, {
          uri, range: sym.range,
          passageName: passage ?? '',
          inferredType: typeResult.assignments.get(sym.name),
        });
      }
    }

    // JS globals
    for (const [name, def] of typeResult.jsGlobals) {
      if (!this.definitions.getJsGlobalDefinition(name)) {
        this.definitions.addJsGlobalDefinition(name, { uri, range: def.range, inferredType: def.inferredType });
      }
    }
  }

  // ---- Private: Pass 2 — link extraction ----------------------------------

  private _extractAndIndexLinks(uri: string, adapter: StoryFormatAdapter): void {
    const parsed = this.parseCache.get(uri);
    if (!parsed) return;

    const linkRefs: LinkRef[] = [];
    const passageArgMacros = adapter.getPassageArgMacros();
    const implicitPatterns = adapter.getImplicitPassagePatterns();
    const passageRefApis = adapter.getPassageRefApiCalls();

    // Build a quick lookup for API calls: "ObjectName.method" → true
    const apiCallSet = new Set<string>();
    for (const api of passageRefApis) {
      for (const method of api.methods) {
        apiCallSet.add(`${api.objectName}.${method}`);
      }
    }

    // Walk each passage separately so we can capture the source passage name
    for (const passage of parsed.ast.passages) {
      const src = passage.name;

      // ── Standard links: [[Target]] and <<goto "Target">> ─────────────────
      if (Array.isArray(passage.body)) {
        walkMarkup(passage.body, {
          onLink(node) {
            linkRefs.push({ target: node.target, range: node.range, sourcePassage: src });
          },
          onMacro(node) {
            if (passageArgMacros.has(node.name) && node.args.length > 0) {
              const idx    = adapter.getPassageArgIndex(node.name, node.args.length);
              const arg    = node.args[idx];
              const target = arg ? passageNameFromExpr(arg) : null;
              if (target && arg) linkRefs.push({ target, range: arg.range, sourcePassage: src });
            }

            // ── Implicit passage refs in macro expression args ───────────
            // Walk expression trees looking for Engine.play("name"), Story.get("name"), etc.
            if (apiCallSet.size > 0) {
              for (const arg of node.args) {
                walkExpression(arg, expr => {
                  if (expr.type !== 'call') return;
                  const callee = expr.callee;
                  if (callee.type !== 'propertyAccess') return;
                  if (callee.object.type !== 'identifier') return;
                  const key = `${callee.object.name}.${callee.property}`;
                  if (!apiCallSet.has(key)) return;
                  // First string argument is the passage name
                  if (expr.args.length === 0) return;
                  const firstArg = expr.args[0]!;
                  if (firstArg.type === 'literal' && firstArg.kind === 'string' && typeof firstArg.value === 'string') {
                    linkRefs.push({
                      target: firstArg.value,
                      range: firstArg.range,
                      sourcePassage: src,
                    });
                  }
                });
              }
            }
          },
          // ── Implicit passage refs in text nodes (data-passage, etc.) ────
          onText(node) {
            if (implicitPatterns.length === 0) return;
            for (const ip of implicitPatterns) {
              // Reset lastIndex since these patterns use the /g flag
              ip.pattern.lastIndex = 0;
              let m: RegExpExecArray | null;
              while ((m = ip.pattern.exec(node.value)) !== null) {
                const name = m[1];
                if (!name) continue;
                // Calculate offset within the passage source
                const matchOffset = node.range.start + m.index;
                linkRefs.push({
                  target: name,
                  range: { start: matchOffset, end: matchOffset + m[0].length },
                  sourcePassage: src,
                });
              }
            }
          },
        });
      }

      // ── Script passages: scan for implicit passage refs in JS source ────
      if (passage.body && typeof passage.body === 'object' && (passage.body as ScriptBodyNode).type === 'scriptBody') {
        const scriptBody = passage.body as ScriptBodyNode;
        if (implicitPatterns.length > 0) {
          for (const ip of implicitPatterns) {
            ip.pattern.lastIndex = 0;
            let m: RegExpExecArray | null;
            while ((m = ip.pattern.exec(scriptBody.source)) !== null) {
              const name = m[1];
              if (!name) continue;
              const matchOffset = scriptBody.range.start + m.index;
              linkRefs.push({
                target: name,
                range: { start: matchOffset, end: matchOffset + m[0].length },
                sourcePassage: src,
              });
            }
          }
        }
      }
    }

    this.links.setFileLinks(uri, linkRefs);

    for (const link of linkRefs) {
      this.references.addPassageReference(link.target, { uri, range: link.range, sourcePassage: link.sourcePassage });
    }
  }

  // ---- Private: Pass 3 helpers --------------------------------------------

  private _indexVariableRefs(ast: DocumentNode, uri: string): void {
    const refs = this.references;
    walkDocumentExpressions(ast, expr => {
      if (expr.type === 'storyVar') {
        refs.addVariableReference(expr.name, { uri, range: expr.range });
      }
    });
  }

  private _indexCallSites(ast: DocumentNode, uri: string): void {
    const defs = this.definitions;
    const refs = this.references;
    walkDocument(ast, {
      onMacro(node) {
        if (defs.hasMacro(node.name)) {
          refs.addMacroCallSite(node.name, { uri, range: node.nameRange });
        }
      },
    });
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
    const cache = this.parseCache;
    return [...uris].sort((a, b) => {
      const aAst = cache.get(a)?.ast;
      const bAst = cache.get(b)?.ast;
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
