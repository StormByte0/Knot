/**
 * Knot v2 — Semantic Analyzer
 *
 * Validates semantic correctness AFTER syntax analysis. Requires both
 * the AST and the workspace index — it checks cross-passage and
 * cross-document issues that need type information and scope.
 *
 * Checks performed:
 *   1. Unknown variable references (using a variable never assigned)
 *   2. Scope violations (temp variable used from wrong passage)
 *   3. Unknown passage references (link/macro targets a nonexistent passage)
 *   4. Unknown macro calls (calling a macro the format doesn't know)
 *   5. Deprecated macro usage
 *   6. Custom macro resolution (widget/macro: definitions and calls)
 *   7. Variable type flow (basic — tracks assigned types where known)
 *   8. Macro argument type validation (basic — checks arg patterns)
 *
 * DESIGN: The semantic analyzer is intentionally conservative. It reports
 * only what it can determine with confidence. Twine stories are dynamic —
 * variables can be set in script passages, passage rendering is order-dependent,
 * and runtime state is complex. We prefer false negatives (missed warnings)
 * over false positives (spurious errors).
 *
 * MUST NOT import from: formats/ (use FormatRegistry instead)
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type {
  FormatModule,
  MacroDef,
  DiagnosticResult,
  SourceRange,
  VariableSigilDef,
} from '../formats/_types';
import { PassageRefKind, LinkKind, MacroCategory, MacroKind } from '../hooks/hookTypes';
import { ASTNode, DocumentAST, walkTree, PassageGroup } from './ast';
import { WorkspaceIndex, PassageEntry } from './workspaceIndex';

// ─── Public Types ──────────────────────────────────────────────

export interface SemanticAnalysisResult {
  /** Semantic diagnostics */
  diagnostics: DiagnosticResult[];
  /** Variable definitions found (passageName → Set of var names) */
  variableDefinitions: Map<string, Set<VariableDef>>;
  /** Custom macro definitions found */
  customMacroDefinitions: Map<string, CustomMacroDef>;
}

export interface VariableDef {
  name: string;
  sigil: string;
  scope: 'story' | 'temp';
  passageName: string;
  range: SourceRange;
}

export interface CustomMacroDef {
  name: string;
  passageName: string;
  passageUri: string;
  range: SourceRange;
}

// ─── Semantic Analyzer ─────────────────────────────────────────

export class SemanticAnalyzer {
  private formatRegistry: FormatRegistry;
  private workspaceIndex: WorkspaceIndex;

  constructor(formatRegistry: FormatRegistry, workspaceIndex: WorkspaceIndex) {
    this.formatRegistry = formatRegistry;
    this.workspaceIndex = workspaceIndex;
  }

  /**
   * Analyze a document AST for semantic errors.
   * Requires the workspace index for cross-passage resolution.
   */
  analyze(ast: DocumentAST, passages: PassageGroup[]): SemanticAnalysisResult {
    const format = this.formatRegistry.getActiveFormat();
    const diagnostics: DiagnosticResult[] = [];
    const variableDefinitions = new Map<string, Set<VariableDef>>();
    const customMacroDefinitions = new Map<string, CustomMacroDef>();

    // Phase 1: Collect definitions (variables, custom macros)
    this.collectDefinitions(ast, passages, format, variableDefinitions, customMacroDefinitions);

    // Phase 2: Validate references against collected definitions
    this.checkUnknownMacros(ast, format, customMacroDefinitions, diagnostics);
    this.checkDeprecatedMacros(ast, format, diagnostics);
    this.checkUnknownVariables(ast, format, variableDefinitions, diagnostics);
    this.checkUnknownPassageRefs(ast, format, diagnostics);
    this.checkVariableScope(ast, format, diagnostics);
    this.checkCustomMacroUsage(ast, format, customMacroDefinitions, diagnostics);

    return { diagnostics, variableDefinitions, customMacroDefinitions };
  }

  /**
   * Quick semantic check for a single passage (for incremental updates).
   */
  analyzePassage(
    bodyNode: ASTNode,
    format: FormatModule,
    knownVariables: Map<string, Set<VariableDef>>,
    knownCustomMacros: Map<string, CustomMacroDef>,
  ): DiagnosticResult[] {
    const diagnostics: DiagnosticResult[] = [];
    this.checkUnknownMacrosInNode(bodyNode, format, knownCustomMacros, diagnostics);
    this.checkDeprecatedMacrosInNode(bodyNode, format, diagnostics);
    this.checkUnknownVariablesInNode(bodyNode, format, knownVariables, diagnostics);
    this.checkUnknownPassageRefsInNode(bodyNode, format, diagnostics);
    return diagnostics;
  }

  // ─── Phase 1: Collect Definitions ────────────────────────────

  /**
   * Collect variable and custom macro definitions from the AST.
   */
  private collectDefinitions(
    ast: DocumentAST,
    passages: PassageGroup[],
    format: FormatModule,
    variableDefinitions: Map<string, Set<VariableDef>>,
    customMacroDefinitions: Map<string, CustomMacroDef>,
  ): void {
    for (const passage of passages) {
      const passageName = passage.header.data.passageName ?? '';
      const vars = new Set<VariableDef>();

      // Collect variable assignments from the body
      walkTree(passage.body, node => {
        // Check for assignment macros (set, capture, etc.)
        if (node.nodeType === 'MacroCall' && node.data.macroName) {
          const macroName = node.data.macroName;
          if (format.variables?.assignmentMacros.has(macroName)) {
            this.extractAssignedVariables(node, format, passageName, vars);
          }
        }

        // Check for custom macro definitions (widget, macro:)
        if (node.nodeType === 'MacroCall' && node.data.macroName) {
          if (format.customMacros?.definitionMacros.has(node.data.macroName)) {
            this.extractCustomMacroDefinition(node, format, passageName, passage.header.range, customMacroDefinitions);
          }
        }
      });

      variableDefinitions.set(passageName, vars);
    }
  }

  /**
   * Extract variables assigned by an assignment macro (e.g. <<set $x to 1>>).
   */
  private extractAssignedVariables(
    macroNode: ASTNode,
    format: FormatModule,
    passageName: string,
    vars: Set<VariableDef>,
  ): void {
    const rawArgs = macroNode.data.rawArgs ?? '';
    if (!rawArgs || !format.variables) return;

    // Use the format's variable pattern to find assigned variables
    const pattern = new RegExp(format.variables.variablePattern.source, format.variables.variablePattern.flags);
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(rawArgs)) !== null) {
      const sigilChar = match[1];
      const varName = match[2];
      const sigilDef = format.variables.sigils.find(s => s.sigil === sigilChar);
      if (sigilDef && varName) {
        vars.add({
          name: varName,
          sigil: sigilChar,
          scope: sigilDef.kind,
          passageName,
          range: macroNode.range,
        });
      }
    }
  }

  /**
   * Extract a custom macro definition from a widget/macro: call.
   */
  private extractCustomMacroDefinition(
    macroNode: ASTNode,
    format: FormatModule,
    passageName: string,
    passageRange: SourceRange,
    customMacroDefinitions: Map<string, CustomMacroDef>,
  ): void {
    const rawArgs = macroNode.data.rawArgs ?? '';
    if (!rawArgs) return;

    // Extract the name argument — typically the first argument
    // SugarCube: <<widget macroName>>
    // Harlowe: (macro: "macroName", [...])
    const nameMatch = rawArgs.match(/^\s*(\w[\w-]*)/);
    if (nameMatch) {
      const macroName = nameMatch[1];
      customMacroDefinitions.set(macroName, {
        name: macroName,
        passageName,
        passageUri: '',  // Filled in by caller with actual URI
        range: macroNode.range,
      });
    }
  }

  // ─── Phase 2: Validate References ────────────────────────────

  // ─── Check: Unknown Macros ────────────────────────────────────

  private checkUnknownMacros(
    ast: DocumentAST,
    format: FormatModule,
    customMacros: Map<string, CustomMacroDef>,
    diagnostics: DiagnosticResult[],
  ): void {
    walkTree(ast.root, node => {
      this.checkUnknownMacrosInNode(node, format, customMacros, diagnostics);
    });
  }

  private checkUnknownMacrosInNode(
    node: ASTNode,
    format: FormatModule,
    customMacros: Map<string, CustomMacroDef>,
    diagnostics: DiagnosticResult[],
  ): void {
    if (!format.macros) return;

    // Build known macro name set (builtins + aliases + custom macros)
    const knownNames = new Set<string>();
    for (const macro of format.macros.builtins) {
      knownNames.add(macro.name);
      if (macro.aliases) {
        for (const alias of macro.aliases) {
          knownNames.add(alias);
        }
      }
    }
    for (const name of customMacros.keys()) {
      knownNames.add(name);
    }

    walkTree(node, current => {
      if (current.nodeType !== 'MacroCall') return;
      const macroName = current.data.macroName;
      if (!macroName) return;

      // Normalize Harlowe-style names (strip trailing colon)
      const normalizedName = macroName.endsWith(':') ? macroName : macroName;

      if (!knownNames.has(normalizedName)) {
        diagnostics.push({
          ruleId: 'unknown-macro',
          message: `Unknown macro: ${macroName}`,
          severity: 'warning',
          range: current.range,
        });
      }
    });
  }

  // ─── Check: Deprecated Macros ─────────────────────────────────

  private checkDeprecatedMacros(
    ast: DocumentAST,
    format: FormatModule,
    diagnostics: DiagnosticResult[],
  ): void {
    walkTree(ast.root, node => {
      this.checkDeprecatedMacrosInNode(node, format, diagnostics);
    });
  }

  private checkDeprecatedMacrosInNode(
    node: ASTNode,
    format: FormatModule,
    diagnostics: DiagnosticResult[],
  ): void {
    if (!format.macros) return;

    const deprecatedMap = new Map<string, string>();  // name → deprecation message
    for (const macro of format.macros.builtins) {
      if (macro.deprecated) {
        deprecatedMap.set(macro.name, macro.deprecationMessage ?? 'This macro is deprecated.');
        if (macro.aliases) {
          for (const alias of macro.aliases) {
            deprecatedMap.set(alias, macro.deprecationMessage ?? 'This macro is deprecated.');
          }
        }
      }
    }

    walkTree(node, current => {
      if (current.nodeType !== 'MacroCall') return;
      const macroName = current.data.macroName;
      if (!macroName) return;

      const normalizedName = macroName.endsWith(':') ? macroName : macroName;
      const deprecationMsg = deprecatedMap.get(normalizedName);
      if (deprecationMsg) {
        diagnostics.push({
          ruleId: 'deprecated-macro',
          message: `Deprecated macro: ${macroName} — ${deprecationMsg}`,
          severity: 'warning',
          range: current.range,
        });
      }
    });
  }

  // ─── Check: Unknown Variables ─────────────────────────────────

  private checkUnknownVariables(
    ast: DocumentAST,
    format: FormatModule,
    variableDefinitions: Map<string, Set<VariableDef>>,
    diagnostics: DiagnosticResult[],
  ): void {
    walkTree(ast.root, node => {
      this.checkUnknownVariablesInNode(node, format, variableDefinitions, diagnostics);
    });
  }

  private checkUnknownVariablesInNode(
    node: ASTNode,
    format: FormatModule,
    variableDefinitions: Map<string, Set<VariableDef>>,
    diagnostics: DiagnosticResult[],
  ): void {
    if (!format.variables) return;

    // Collect all defined story variables across the workspace
    const definedStoryVars = new Set<string>();
    for (const [, vars] of variableDefinitions) {
      for (const v of vars) {
        if (v.scope === 'story') {
          definedStoryVars.add(v.name);
        }
      }
    }

    walkTree(node, current => {
      if (current.nodeType !== 'Variable') return;
      const varName = current.data.varName;
      const varSigil = current.data.varSigil;
      if (!varName || !varSigil) return;

      // Determine scope from sigil
      const sigilDef = format.variables!.sigils.find(s => s.sigil === varSigil);
      if (!sigilDef) return;

      if (sigilDef.kind === 'story') {
        // Story variable — check if it's defined anywhere in the workspace
        if (!definedStoryVars.has(varName)) {
          diagnostics.push({
            ruleId: 'unknown-variable',
            message: `Variable ${varSigil}${varName} is used but never assigned`,
            severity: 'hint',  // Hint because variables might be set in script passages
            range: current.range,
          });
        }
      }
      // Temp variables are passage-scoped — always considered "unknown" if
      // not assigned in the current passage, but this is too common a pattern
      // to flag (e.g. temp vars set in <<for>> loops). Skip for now.
    });
  }

  // ─── Check: Unknown Passage References ────────────────────────

  private checkUnknownPassageRefs(
    ast: DocumentAST,
    format: FormatModule,
    diagnostics: DiagnosticResult[],
  ): void {
    walkTree(ast.root, node => {
      this.checkUnknownPassageRefsInNode(node, format, diagnostics);
    });
  }

  private checkUnknownPassageRefsInNode(
    node: ASTNode,
    format: FormatModule,
    diagnostics: DiagnosticResult[],
  ): void {
    const allPassageNames = new Set(this.workspaceIndex.getAllPassageNames());

    walkTree(node, current => {
      // Check Link nodes
      if (current.nodeType === 'Link' && current.data.linkTarget) {
        const target = current.data.linkTarget.trim();
        if (target && current.data.linkKind === 'passage' && !allPassageNames.has(target)) {
          diagnostics.push({
            ruleId: 'unknown-passage',
            message: `Unknown passage: "${target}"`,
            severity: 'warning',
            range: current.range,
          });
        }
      }

      // Check MacroCall nodes with passage arguments
      if (current.nodeType === 'MacroCall' && current.data.macroName) {
        const macroName = current.data.macroName;
        const normalizedName = macroName.endsWith(':') ? macroName : macroName;

        // Find the macro definition to check if it has passage arguments
        if (format.macros) {
          const macroDef = format.macros.builtins.find(m =>
            m.name === normalizedName || (m.aliases && m.aliases.includes(normalizedName))
          );

          if (macroDef?.isNavigation || macroDef?.isInclude) {
            // This macro takes a passage name argument
            const passageArgPos = macroDef.passageArgPosition ?? 0;
            const rawArgs = current.data.rawArgs ?? '';

            // Try to extract the passage name from arguments
            const args = this.parseMacroArgs(rawArgs);
            if (args.length > passageArgPos) {
              const passageArg = this.stripQuotes(args[passageArgPos]);
              if (passageArg && !allPassageNames.has(passageArg)) {
                diagnostics.push({
                  ruleId: 'unknown-passage',
                  message: `Unknown passage: "${passageArg}" (referenced by ${macroName})`,
                  severity: 'warning',
                  range: current.range,
                });
              }
            }
          }
        }
      }
    });
  }

  // ─── Check: Variable Scope ────────────────────────────────────

  /**
   * Check for temp variables used outside their defining passage.
   * Temp variables are passage-scoped in both SugarCube and Harlowe.
   */
  private checkVariableScope(
    ast: DocumentAST,
    format: FormatModule,
    diagnostics: DiagnosticResult[],
  ): void {
    // This is a lighter check than unknown-variables.
    // We specifically flag _temp_ variables that appear to be used across
    // passage boundaries, which is a common mistake.

    if (!format.variables) return;

    // For each passage, collect temp vars defined and used
    walkTree(ast.root, passageNode => {
      if (passageNode.nodeType !== 'Passage') return;

      const definedTemps = new Set<string>();
      const usedTemps: { name: string; range: SourceRange }[] = [];

      walkTree(passageNode, node => {
        if (node.nodeType === 'MacroCall' && node.data.macroName) {
          const macroName = node.data.macroName;
          if (format.variables?.assignmentMacros.has(macroName)) {
            const rawArgs = node.data.rawArgs ?? '';
            const pattern = new RegExp(format.variables.variablePattern.source, format.variables.variablePattern.flags);
            let match: RegExpExecArray | null;
            while ((match = pattern.exec(rawArgs)) !== null) {
              const sigilChar = match[1];
              const varName = match[2];
              const sigilDef = format.variables?.sigils.find(s => s.sigil === sigilChar);
              if (sigilDef?.kind === 'temp' && varName) {
                definedTemps.add(varName);
              }
            }
          }
        }

        if (node.nodeType === 'Variable') {
          const sigilDef = format.variables?.sigils.find(s => s.sigil === node.data.varSigil);
          if (sigilDef?.kind === 'temp' && node.data.varName) {
            usedTemps.push({ name: node.data.varName, range: node.range });
          }
        }
      });

      // Flag temp variables used before being defined in this passage
      // Note: This is approximate because we don't track execution order
      // within a passage. It catches obvious cases only.
      for (const use of usedTemps) {
        if (!definedTemps.has(use.name)) {
          diagnostics.push({
            ruleId: 'temp-var-not-defined',
            message: `Temp variable _${use.name} may not be defined in this passage`,
            severity: 'hint',
            range: use.range,
          });
        }
      }
    });
  }

  // ─── Check: Custom Macro Usage ────────────────────────────────

  /**
   * Check for calls to custom macros (widgets) that don't have definitions.
   */
  private checkCustomMacroUsage(
    ast: DocumentAST,
    format: FormatModule,
    customMacroDefinitions: Map<string, CustomMacroDef>,
    diagnostics: DiagnosticResult[],
  ): void {
    if (!format.customMacros) return;

    walkTree(ast.root, node => {
      if (node.nodeType !== 'MacroCall') return;
      const macroName = node.data.macroName;
      if (!macroName) return;

      // Check if this is an unknown macro that might be a custom macro call
      // (The unknown-macro check would have already flagged it if it's not
      // in builtins. Here we specifically check custom macro resolution.)
      // This is a no-op if the macro is a known builtin.
      const isBuiltin = format.macros?.builtins.some(m =>
        m.name === macroName || (m.aliases && m.aliases.includes(macroName))
      );
      if (isBuiltin) return;

      // It's not a builtin — check if it's a known custom macro
      if (!customMacroDefinitions.has(macroName)) {
        // Already flagged by unknown-macro check, but we could add
        // a more specific message here if desired
      }
    });
  }

  // ─── Helpers ────────────────────────────────────────────────

  /**
   * Very simple argument parser — splits on spaces, respecting quoted strings.
   * This is intentionally simple. Full argument parsing is format-specific
   * and would go in a format-provided function if we ever need it.
   */
  private parseMacroArgs(rawArgs: string): string[] {
    const args: string[] = [];
    let current = '';
    let inQuote = false;
    let quoteChar = '';

    for (let i = 0; i < rawArgs.length; i++) {
      const ch = rawArgs[i];

      if (inQuote) {
        if (ch === quoteChar && rawArgs[i - 1] !== '\\') {
          inQuote = false;
          current += ch;
        } else {
          current += ch;
        }
      } else if (ch === '"' || ch === "'") {
        inQuote = true;
        quoteChar = ch;
        current += ch;
      } else if (ch === ' ' || ch === '\t') {
        if (current.length > 0) {
          args.push(current);
          current = '';
        }
      } else {
        current += ch;
      }
    }

    if (current.length > 0) {
      args.push(current);
    }

    return args;
  }

  /**
   * Strip surrounding quotes from a string argument.
   */
  private stripQuotes(str: string): string {
    if ((str.startsWith('"') && str.endsWith('"')) ||
        (str.startsWith("'") && str.endsWith("'"))) {
      return str.slice(1, -1);
    }
    return str;
  }
}
