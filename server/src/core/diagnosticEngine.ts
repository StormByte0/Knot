/**
 * Knot v2 — Diagnostic Engine
 *
 * Orchestrates diagnostic computation in a format-agnostic way.
 * Three phases of diagnostics:
 *
 *   Phase 1: Core (Twine engine) rules — always run (duplicate/unknown/unreachable passages)
 *   Phase 2: Format-specific rules — via format's DiagnosticCapability
 *   Phase 3: AST-based analysis — syntax + semantic diagnostics from the analysis pipeline
 *
 * Phase 3 is new: when an ASTWorkspace is available, the diagnostic engine
 * delegates to its buildAndAnalyze() pipeline, which produces richer diagnostics
 * than the regex-based Phase 1/2 approach. Phase 1 and 2 are kept as fallback
 * for when AST analysis hasn't been run yet.
 *
 * Diagnostic rules come from:
 *   1. CORE_DIAGNOSTIC_RULES (Twine engine level)
 *   2. format.diagnostics.rules (format-specific, declarative)
 *   3. AST syntax/semantic analysis (structural + type/flow checks)
 *
 * MUST NOT import from: formats/ (use FormatRegistry)
 */

import { LinkKind, PassageType, PassageRefKind } from '../hooks/hookTypes';
import { FormatRegistry } from '../formats/formatRegistry';
import { WorkspaceIndex, PassageEntry } from './workspaceIndex';
import { CORE_DIAGNOSTIC_RULES, DiagnosticRuleDef, DiagnosticResult } from '../formats/_types';
import type { ASTWorkspace } from './astWorkspace';
import type { StoryFlowAnalysis } from './storyFlowGraph';

/** Settings type for diagnostic severity overrides from the client. */
export interface DiagnosticSettings {
  [ruleId: string]: string | undefined;
}

export class DiagnosticEngine {
  private formatRegistry: FormatRegistry;
  private workspaceIndex: WorkspaceIndex;
  private settings: DiagnosticSettings = {};
  private astWorkspace: ASTWorkspace | null = null;

  constructor(formatRegistry: FormatRegistry, workspaceIndex: WorkspaceIndex) {
    this.formatRegistry = formatRegistry;
    this.workspaceIndex = workspaceIndex;
  }

  /** Set the AST workspace for AST-based diagnostics (Phase 3). */
  setASTWorkspace(astWorkspace: ASTWorkspace): void {
    this.astWorkspace = astWorkspace;
  }

  /** Update diagnostic severity settings from client configuration. */
  updateSettings(settings: DiagnosticSettings): void {
    this.settings = { ...this.settings, ...settings };
  }

  /** Get all diagnostic rules (core + format). */
  getAllRules(): DiagnosticRuleDef[] {
    const rules = [...CORE_DIAGNOSTIC_RULES];
    const format = this.formatRegistry.getActiveFormat();
    if (format.diagnostics) {
      rules.push(...format.diagnostics.rules);
    }
    return rules;
  }

  /**
   * Run all applicable diagnostics for a document.
   *
   * If AST analysis has been run (via ASTWorkspace), uses those results
   * which are richer and more accurate. Otherwise falls back to the
   * regex-based core + format approach.
   */
  computeDiagnostics(uri: string): DiagnosticResult[] {
    // Phase 3: AST-based diagnostics (preferred when available)
    if (this.astWorkspace) {
      const analysis = this.astWorkspace.getAnalysis(uri);
      if (analysis) {
        // Filter by severity settings
        return analysis.allDiagnostics.filter(d => this.getSeverity(d.ruleId) !== 'off');
      }
    }

    // Fallback: Phase 1 + Phase 2 (regex-based)
    const results: DiagnosticResult[] = [];

    // Phase 1: Core diagnostics (always run — Twine engine level)
    results.push(...this.runCoreDiagnostics(uri));

    // Phase 2: Format-specific diagnostics (via DiagnosticCapability)
    const format = this.formatRegistry.getActiveFormat();
    if (format.diagnostics) {
      const formatResults = this.runFormatDeclarativeRules(uri, [...format.diagnostics.rules]);
      results.push(...formatResults);

      // Custom check function (escape hatch for complex rules)
      if (format.diagnostics.customCheck) {
        const passages = this.workspaceIndex.getPassagesByUri(uri);
        for (const passage of passages) {
          const customResults = format.diagnostics.customCheck({
            passageNames: new Set(this.workspaceIndex.getAllPassageNames()),
            formatId: format.formatId,
            body: passage.body,
            bodyTokens: [],
          });
          results.push(...customResults.filter(r => this.getSeverity(r.ruleId) !== 'off'));
        }
      }
    }

    return results;
  }

  /**
   * Run diagnostics for the entire workspace (e.g., on full re-index).
   *
   * If AST analysis is available, uses the story flow graph for
   * conditional reachability and dead-code detection.
   */
  computeWorkspaceDiagnostics(): DiagnosticResult[] {
    const results: DiagnosticResult[] = [];

    // If AST workspace is available, use the story flow graph
    if (this.astWorkspace) {
      const flowAnalysis = this.astWorkspace.getStoryFlowAnalysis();
      results.push(...flowAnalysis.diagnostics.filter(d => this.getSeverity(d.ruleId) !== 'off'));
      return results;
    }

    // Fallback: simple BFS reachability
    // Duplicate passages (workspace-wide check)
    const duplicateNames = this.workspaceIndex.getDuplicateNames();
    for (const name of duplicateNames) {
      results.push({
        ruleId: 'duplicate-passage',
        message: `Duplicate passage name: "${name}"`,
        severity: (this.settings['duplicate-passage'] as DiagnosticResult['severity']) || 'error',
      });
    }

    // Unreachable passages
    const startPassage = this.workspaceIndex.getPassage('Start');
    if (startPassage) {
      const allNames = this.workspaceIndex.getAllPassageNames();
      const reachable = this.computeReachablePassages(startPassage.name);
      for (const name of allNames) {
        if (!reachable.has(name)) {
          results.push({
            ruleId: 'unreachable-passage',
            message: `Passage "${name}" is unreachable from start`,
            severity: (this.settings['unreachable-passage'] as DiagnosticResult['severity']) || 'warning',
          });
        }
      }
    }

    return results;
  }

  // ─── Core Diagnostics (Twine Engine Level) ───────────────────

  private runCoreDiagnostics(uri: string): DiagnosticResult[] {
    const results: DiagnosticResult[] = [];
    const passages = this.workspaceIndex.getPassagesByUri(uri);

    for (const passage of passages) {
      // Unknown passage references (links, macros, implicit)
      if (this.getSeverity('unknown-passage') !== 'off') {
        for (const ref of passage.passageRefs) {
          if (ref.target.trim() && !this.workspaceIndex.hasPassage(ref.target)) {
            if (ref.kind === PassageRefKind.Link && ref.linkKind === LinkKind.Passage || ref.kind === PassageRefKind.Macro || ref.kind === PassageRefKind.Implicit) {
              results.push({
                ruleId: 'unknown-passage',
                message: `Unknown passage: "${ref.target}"`,
                severity: this.getSeverity('unknown-passage', 'warning') as DiagnosticResult['severity'],
              });
            }
          }
        }
      }

      // Duplicate passages
      if (this.getSeverity('duplicate-passage') !== 'off') {
        const duplicateNames = this.workspaceIndex.getDuplicateNames();
        if (duplicateNames.includes(passage.name)) {
          results.push({
            ruleId: 'duplicate-passage',
            message: `Duplicate passage name: "${passage.name}"`,
            severity: this.getSeverity('duplicate-passage', 'error') as DiagnosticResult['severity'],
          });
        }
      }
    }

    return results;
  }

  // ─── Format Declarative Rules ────────────────────────────────

  private runFormatDeclarativeRules(uri: string, rules: DiagnosticRuleDef[]): DiagnosticResult[] {
    const results: DiagnosticResult[] = [];
    const passages = this.workspaceIndex.getPassagesByUri(uri);
    const format = this.formatRegistry.getActiveFormat();

    const activeRules = rules.filter(r => this.getSeverity(r.id) !== 'off');
    if (activeRules.length === 0) return results;

    // Unknown macro check
    if (format.macros && activeRules.some(r => r.id === 'unknown-macro')) {
      const knownNames = new Set<string>();
      for (const m of format.macros.builtins) {
        knownNames.add(m.name);
        if (m.aliases) for (const a of m.aliases) knownNames.add(a);
      }
      for (const passage of passages) {
        for (const macroName of passage.macroNames) {
          const normalizedName = macroName.endsWith(':') ? macroName : macroName;
          if (!knownNames.has(normalizedName)) {
            results.push({
              ruleId: 'unknown-macro',
              message: `Unknown macro: "${macroName}"`,
              severity: this.getSeverity('unknown-macro', 'warning') as DiagnosticResult['severity'],
            });
          }
        }
      }
    }

    // Deprecated macro check
    if (format.macros && activeRules.some(r => r.id === 'deprecated-macro')) {
      const deprecatedMacros = new Map<string, string>();
      for (const m of format.macros.builtins) {
        if (m.deprecated) {
          deprecatedMacros.set(m.name, m.deprecationMessage ?? 'This macro is deprecated.');
        }
      }
      for (const passage of passages) {
        for (const macroName of passage.macroNames) {
          const normalizedName = macroName.endsWith(':') ? macroName : macroName;
          const msg = deprecatedMacros.get(normalizedName);
          if (msg) {
            results.push({
              ruleId: 'deprecated-macro',
              message: `Deprecated macro: "${macroName}" — ${msg}`,
              severity: this.getSeverity('deprecated-macro', 'warning') as DiagnosticResult['severity'],
            });
          }
        }
      }
    }

    return results;
  }

  // ─── Reachability ────────────────────────────────────────────

  private computeReachablePassages(startName: string): Set<string> {
    const visited = new Set<string>();
    const queue = [startName];

    while (queue.length > 0) {
      const current = queue.pop()!;
      if (visited.has(current)) continue;
      visited.add(current);

      const passage = this.workspaceIndex.getPassage(current);
      if (passage) {
        for (const ref of passage.passageRefs) {
          if ((ref.linkKind === LinkKind.Passage || ref.kind === PassageRefKind.Macro || ref.kind === PassageRefKind.Implicit) && !visited.has(ref.target)) {
            queue.push(ref.target);
          }
        }
      }
    }

    return visited;
  }

  // ─── Settings Helpers ────────────────────────────────────────

  private getSeverity(ruleId: string, defaultSeverity: string = 'warning'): string {
    if (this.settings[ruleId]) {
      return this.settings[ruleId]!;
    }
    const coreRule = CORE_DIAGNOSTIC_RULES.find(r => r.id === ruleId);
    if (coreRule) {
      return coreRule.defaultSeverity;
    }
    const format = this.formatRegistry.getActiveFormat();
    if (format.diagnostics) {
      const formatRule = format.diagnostics.rules.find(r => r.id === ruleId);
      if (formatRule) {
        return formatRule.defaultSeverity;
      }
    }
    return defaultSeverity;
  }
}
