import { DiagnosticSeverity } from 'vscode-languageserver/node';
import type { SourceRange } from './tokenTypes';

// ---------------------------------------------------------------------------
// Named diagnostic rules — each has a unique ID for configuration/suppression
// ---------------------------------------------------------------------------

export enum DiagnosticRule {
  UnknownPassage     = 'unknown-passage',
  UnknownMacro       = 'unknown-macro',
  DuplicatePassage   = 'duplicate-passage',
  TypeMismatch       = 'type-mismatch',
  UnreachablePassage = 'unreachable-passage',
  ContainerStructure = 'container-structure',
  DeprecatedMacro    = 'deprecated-macro',
  MissingRequiredArg = 'missing-required-arg',
  AssignmentTarget   = 'assignment-target',
}

export interface DiagnosticRuleConfig {
  severity: DiagnosticSeverity;
  enabled:  boolean;
}

export interface RuleDiagnostic {
  rule:    DiagnosticRule;
  message: string;
  range:   SourceRange;
}

// ---------------------------------------------------------------------------
// DiagnosticEngine — configurable, suppressible diagnostic rule system
// ---------------------------------------------------------------------------

export class DiagnosticEngine {
  private config = new Map<DiagnosticRule, DiagnosticRuleConfig>();

  constructor() {
    // Default severities
    this.config.set(DiagnosticRule.UnknownPassage,     { severity: DiagnosticSeverity.Warning, enabled: true });
    this.config.set(DiagnosticRule.UnknownMacro,       { severity: DiagnosticSeverity.Warning, enabled: true });
    this.config.set(DiagnosticRule.DuplicatePassage,   { severity: DiagnosticSeverity.Error,   enabled: true });
    this.config.set(DiagnosticRule.TypeMismatch,       { severity: DiagnosticSeverity.Error,   enabled: true });
    this.config.set(DiagnosticRule.UnreachablePassage, { severity: DiagnosticSeverity.Warning, enabled: true });
    this.config.set(DiagnosticRule.ContainerStructure, { severity: DiagnosticSeverity.Error,   enabled: true });
    this.config.set(DiagnosticRule.DeprecatedMacro,    { severity: DiagnosticSeverity.Warning, enabled: true });
    this.config.set(DiagnosticRule.MissingRequiredArg, { severity: DiagnosticSeverity.Error,   enabled: true });
    this.config.set(DiagnosticRule.AssignmentTarget,   { severity: DiagnosticSeverity.Error,   enabled: true });
  }

  /** Update configuration for a rule. */
  configure(rule: DiagnosticRule, config: Partial<DiagnosticRuleConfig>): void {
    const existing = this.config.get(rule);
    if (existing) {
      this.config.set(rule, { ...existing, ...config });
    }
  }

  /** Check if a rule is enabled. */
  isEnabled(rule: DiagnosticRule): boolean {
    return this.config.get(rule)?.enabled ?? true;
  }

  /** Get the severity for a rule. */
  getSeverity(rule: DiagnosticRule): DiagnosticSeverity {
    return this.config.get(rule)?.severity ?? DiagnosticSeverity.Warning;
  }

  /** Create a diagnostic only if the rule is enabled. Returns null if suppressed. */
  createDiagnostic(rule: DiagnosticRule, message: string, range: SourceRange): RuleDiagnostic | null {
    if (!this.isEnabled(rule)) return null;
    return { rule, message, range };
  }

  /** Convert a RuleDiagnostic to a ParseDiagnostic with the correct severity. */
  toParseDiagnostic(diag: RuleDiagnostic): import('./ast').ParseDiagnostic {
    const severity = this.getSeverity(diag.rule);
    return {
      message:  diag.message,
      range:    diag.range,
      severity: severity === DiagnosticSeverity.Error ? 'error' : 'warning',
    };
  }

  /** Batch convert RuleDiagnostics to ParseDiagnostics. */
  toParseDiagnostics(diags: RuleDiagnostic[]): import('./ast').ParseDiagnostic[] {
    return diags.map(d => this.toParseDiagnostic(d));
  }

  /** Configure from the legacy LintConfig format. */
  configureFromLintConfig(config: Record<string, DiagnosticSeverity>): void {
    const ruleMap: Record<string, DiagnosticRule> = {
      unknownPassage:     DiagnosticRule.UnknownPassage,
      unknownMacro:       DiagnosticRule.UnknownMacro,
      duplicatePassage:   DiagnosticRule.DuplicatePassage,
      typeMismatch:       DiagnosticRule.TypeMismatch,
      unreachablePassage: DiagnosticRule.UnreachablePassage,
      containerStructure: DiagnosticRule.ContainerStructure,
    };
    for (const [key, severity] of Object.entries(config)) {
      const rule = ruleMap[key];
      if (rule) this.configure(rule, { severity });
    }
  }

  /**
   * Build the current effective LintConfig (for backward compat with
   * getLintConfig()).
   */
  toLintConfig(): {
    unknownPassage:     DiagnosticSeverity;
    unknownMacro:       DiagnosticSeverity;
    duplicatePassage:   DiagnosticSeverity;
    typeMismatch:       DiagnosticSeverity;
    unreachablePassage: DiagnosticSeverity;
    containerStructure: DiagnosticSeverity;
  } {
    return {
      unknownPassage:     this.getSeverity(DiagnosticRule.UnknownPassage),
      unknownMacro:       this.getSeverity(DiagnosticRule.UnknownMacro),
      duplicatePassage:   this.getSeverity(DiagnosticRule.DuplicatePassage),
      typeMismatch:       this.getSeverity(DiagnosticRule.TypeMismatch),
      unreachablePassage: this.getSeverity(DiagnosticRule.UnreachablePassage),
      containerStructure: this.getSeverity(DiagnosticRule.ContainerStructure),
    };
  }
}
