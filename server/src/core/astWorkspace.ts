/**
 * Knot v2 — AST Workspace
 *
 * Manages per-document AST caches and coordinates the full analysis
 * pipeline: AST build → syntax analysis → semantic analysis → virtual docs.
 *
 * Provides a single entry point for all analysis operations:
 *   - buildAndAnalyze() — full pipeline for a document
 *   - getAST() — retrieve cached AST for a URI
 *   - getAnalysis() — retrieve cached analysis results
 *   - invalidate() — invalidate cache on document change
 *
 * The workspace also populates the symbol table from AST data and
 * feeds diagnostics to the DiagnosticEngine.
 *
 * MUST NOT import from: formats/ (use FormatRegistry instead)
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { DiagnosticResult } from '../formats/_types';
import { ASTNode, DocumentAST, PassageGroup, walkTree, findDeepestNode } from './ast';
import { ASTBuilder, BuildResult, BuilderWarning } from './astBuilder';
import { SyntaxAnalyzer, SyntaxAnalysisResult } from './syntaxAnalyzer';
import { SemanticAnalyzer, SemanticAnalysisResult } from './semanticAnalyzer';
import { VirtualDocProvider, VirtualDocExtraction } from './virtualDocs';
import { WorkspaceIndex } from './workspaceIndex';
import { SymbolTable, SymbolKind, SymbolEntry } from './symbolTable';
import { PassageType, MacroCategory } from '../hooks/hookTypes';
import { CFGBuilder, PassageCFG } from './cfg';
import { StoryFlowGraphBuilder, StoryFlowAnalysis, StoryFlowGraph } from './storyFlowGraph';

// ─── Public Types ──────────────────────────────────────────────

/**
 * Complete analysis result for a single document.
 */
export interface DocumentAnalysis {
  /** The built AST */
  readonly ast: DocumentAST;
  /** Passage groups extracted during build */
  readonly passages: PassageGroup[];
  /** Builder warnings (structural issues found during tree construction) */
  readonly builderWarnings: BuilderWarning[];
  /** Syntax analysis results */
  readonly syntax: SyntaxAnalysisResult;
  /** Semantic analysis results */
  readonly semantic: SemanticAnalysisResult;
  /** Virtual document extraction results */
  readonly virtualDocs: VirtualDocExtraction;
  /** Per-passage CFGs */
  readonly passageCFGs: Map<string, PassageCFG>;
  /** All diagnostics combined (builder + syntax + semantic + flow) */
  readonly allDiagnostics: DiagnosticResult[];
}

// ─── AST Workspace ─────────────────────────────────────────────

export class ASTWorkspace {
  private formatRegistry: FormatRegistry;
  private workspaceIndex: WorkspaceIndex;
  private symbolTable: SymbolTable;

  private astBuilder: ASTBuilder;
  private syntaxAnalyzer: SyntaxAnalyzer;
  private semanticAnalyzer: SemanticAnalyzer;
  private virtualDocProvider: VirtualDocProvider;
  private cfgBuilder: CFGBuilder;
  private storyFlowGraphBuilder: StoryFlowGraphBuilder;

  /** Cache: URI → DocumentAnalysis */
  private analysisCache: Map<string, DocumentAnalysis> = new Map();
  /** Cache: story flow analysis (workspace-wide) */
  private storyFlowCache: StoryFlowAnalysis | null = null;

  constructor(
    formatRegistry: FormatRegistry,
    workspaceIndex: WorkspaceIndex,
    symbolTable: SymbolTable,
  ) {
    this.formatRegistry = formatRegistry;
    this.workspaceIndex = workspaceIndex;
    this.symbolTable = symbolTable;

    this.astBuilder = new ASTBuilder(formatRegistry);
    this.syntaxAnalyzer = new SyntaxAnalyzer(formatRegistry);
    this.semanticAnalyzer = new SemanticAnalyzer(formatRegistry, workspaceIndex);
    this.virtualDocProvider = new VirtualDocProvider(formatRegistry);
    this.cfgBuilder = new CFGBuilder(formatRegistry);
    this.storyFlowGraphBuilder = new StoryFlowGraphBuilder(formatRegistry, workspaceIndex);
  }

  /**
   * Full analysis pipeline for a document.
   * Builds AST, runs syntax/semantic analysis, extracts virtual docs.
   * Caches the result for subsequent queries.
   */
  buildAndAnalyze(uri: string, content: string, version: number): DocumentAnalysis {
    // Step 1: Build AST
    const buildResult = this.astBuilder.build(content, uri, version);

    // Step 2: Syntax analysis
    const syntax = this.syntaxAnalyzer.analyze(buildResult.ast, buildResult.passages);

    // Step 3: Semantic analysis
    const semantic = this.semanticAnalyzer.analyze(buildResult.ast, buildResult.passages);

    // Step 4: Virtual document extraction
    const virtualDocs = this.virtualDocProvider.extract(content, uri, version, buildResult.passages);

    // Step 5: Build per-passage CFGs
    const passageCFGs = new Map<string, PassageCFG>();
    for (const passage of buildResult.passages) {
      const passageName = passage.header.data.passageName ?? '';
      if (passageName) {
        passageCFGs.set(passageName, this.cfgBuilder.buildPassageCFG(passage));
      }
    }

    // Step 6: Populate symbol table from AST
    this.populateSymbolTable(uri, buildResult);

    // Invalidate story flow cache (it will be rebuilt on demand)
    this.storyFlowCache = null;

    // Combine all diagnostics
    const allDiagnostics: DiagnosticResult[] = [
      ...buildResult.warnings.map(w => ({
        ruleId: w.kind,
        message: w.message,
        severity: 'warning' as const,
        range: w.range,
      })),
      ...syntax.diagnostics,
      ...semantic.diagnostics,
    ];

    const analysis: DocumentAnalysis = {
      ast: buildResult.ast,
      passages: buildResult.passages,
      builderWarnings: buildResult.warnings,
      syntax,
      semantic,
      virtualDocs,
      passageCFGs,
      allDiagnostics,
    };

    // Cache the result
    this.analysisCache.set(uri, analysis);

    return analysis;
  }

  /**
   * Get the cached AST for a URI.
   * Returns undefined if no analysis has been run for this URI.
   */
  getAST(uri: string): DocumentAST | undefined {
    return this.analysisCache.get(uri)?.ast;
  }

  /**
   * Get the cached analysis for a URI.
   * Returns undefined if no analysis has been run for this URI.
   */
  getAnalysis(uri: string): DocumentAnalysis | undefined {
    return this.analysisCache.get(uri);
  }

  /**
   * Get the AST node at a specific offset in a document.
   * Convenience method that combines getAST + findNodeAtOffset.
   */
  getNodeAtOffset(uri: string, offset: number): ASTNode | null {
    const ast = this.getAST(uri);
    if (!ast) return null;
    return ast.findNodeAtOffset(offset);
  }

  /**
   * Get all diagnostics for a document (combined from all analysis phases).
   */
  getDiagnostics(uri: string): DiagnosticResult[] {
    return this.analysisCache.get(uri)?.allDiagnostics ?? [];
  }

  /**
   * Get the macro nesting stack at a given offset.
   * Used by the completion handler for context-aware suggestions.
   */
  getMacroStackAtOffset(uri: string, offset: number): string[] {
    const analysis = this.analysisCache.get(uri);
    if (!analysis) return [];

    // Find the closest stack entry at or before the offset
    let closestOffset = -1;
    let closestStack: string[] = [];

    for (const [stackOffset, stack] of analysis.syntax.macroStacks) {
      if (stackOffset <= offset && stackOffset > closestOffset) {
        closestOffset = stackOffset;
        closestStack = stack;
      }
    }

    return closestStack;
  }

  /**
   * Get the per-passage CFG for a document.
   * Returns a map of passage name → PassageCFG.
   */
  getPassageCFGs(uri: string): Map<string, PassageCFG> | undefined {
    return this.analysisCache.get(uri)?.passageCFGs;
  }

  /**
   * Get the PassageCFG for a specific passage in a document.
   */
  getPassageCFG(uri: string, passageName: string): PassageCFG | undefined {
    return this.analysisCache.get(uri)?.passageCFGs.get(passageName);
  }

  /**
   * Build and return the story flow graph (workspace-wide analysis).
   * This is expensive — the result is cached and only recomputed
   * when any document changes.
   *
   * Combines all per-passage CFGs into a cross-passage flow graph
   * with variable state propagation and conditional reachability.
   */
  getStoryFlowAnalysis(): StoryFlowAnalysis {
    if (this.storyFlowCache) return this.storyFlowCache;

    // Collect all passages from all analyzed documents
    const allPassages: PassageGroup[] = [];
    for (const [, analysis] of this.analysisCache) {
      allPassages.push(...analysis.passages);
    }

    this.storyFlowCache = this.storyFlowGraphBuilder.buildAndAnalyze(allPassages);
    return this.storyFlowCache;
  }

  /**
   * Get the story flow graph (convenience accessor).
   */
  getStoryFlowGraph(): StoryFlowGraph | null {
    return this.storyFlowCache?.graph ?? null;
  }

  /**
   * Invalidate the cache for a specific URI (on document change).
   */
  invalidate(uri: string): void {
    // Remove symbols for this URI from the symbol table
    this.symbolTable.removeByUri(uri);
    // Remove cached analysis
    this.analysisCache.delete(uri);
    // Remove cached virtual docs
    this.virtualDocProvider.invalidate(uri);
  }

  /**
   * Clear all caches.
   */
  clear(): void {
    this.symbolTable.clear();
    this.analysisCache.clear();
    this.virtualDocProvider.clear();
  }

  /**
   * Get the virtual document provider (for direct access to virtual docs).
   */
  getVirtualDocProvider(): VirtualDocProvider {
    return this.virtualDocProvider;
  }

  /**
   * Re-analyze all cached documents (e.g., after format change).
   */
  reanalyzeAll(documents: Map<string, { content: string; version: number }>): void {
    this.clear();

    for (const [uri, { content, version }] of documents) {
      this.buildAndAnalyze(uri, content, version);
    }
  }

  // ─── Private Helpers ────────────────────────────────────────

  /**
   * Populate the symbol table from AST data.
   * This replaces the previous approach of extracting symbols from
   * raw text using regex — the AST provides more accurate symbol info.
   */
  private populateSymbolTable(uri: string, buildResult: BuildResult): void {
    // Remove old symbols for this URI first
    this.symbolTable.removeByUri(uri);

    for (const passage of buildResult.passages) {
      const passageName = passage.header.data.passageName ?? '';
      const passageType = passage.header.data.passageType ?? PassageType.Story;

      // Add passage symbol
      this.symbolTable.addSymbol({
        kind: SymbolKind.Passage,
        name: passageName,
        passageType,
        uri,
        startOffset: passage.range.start,
        endOffset: passage.range.end,
      });

      // Add variable symbols from the passage body
      walkTree(passage.body, node => {
        if (node.nodeType === 'Variable' && node.data.varName) {
          const sigil = node.data.varSigil ?? '';
          const scope = sigil === '$' ? 'story' : sigil === '_' ? 'temp' : 'unknown';
          const kind = scope === 'story' ? SymbolKind.StoryVariable : SymbolKind.TempVariable;

          this.symbolTable.addSymbol({
            kind,
            name: `${sigil}${node.data.varName}`,
            uri,
            startOffset: node.range.start,
            endOffset: node.range.end,
            containerName: passageName,
          });
        }

        // Add macro call symbols
        if (node.nodeType === 'MacroCall' && node.data.macroName) {
          this.symbolTable.addSymbol({
            kind: SymbolKind.Macro,
            name: node.data.macroName,
            uri,
            startOffset: node.range.start,
            endOffset: node.range.end,
            containerName: passageName,
          });
        }
      });
    }
  }
}
