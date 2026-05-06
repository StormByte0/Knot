/**
 * Knot v2 — Story Flow Graph
 *
 * Cross-passage control flow analysis that combines per-passage CFGs
 * into a workspace-wide story flow graph. This enables:
 *
 *   - Conditional reachability: "Can the player reach Treasure Room
 *     without finding the key?"
 *   - Dead branch detection: "This else clause can never execute
 *     because the condition is always true at this point"
 *   - Variable state flow: "What values can $hp have when entering
 *     the Boss Fight passage?"
 *   - Contradictory guard detection: "This if-condition can never be
 *     true given the variable state at this point"
 *
 * Architecture:
 *   - Each passage → PassageCFG (built by CFGBuilder)
 *   - Navigation edges connect passage CFGs
 *   - Variable state flows through navigation edges
 *   - Fixed-point analysis computes stable variable states across passages
 *
 * MUST NOT import from: formats/ (use FormatRegistry instead)
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, SourceRange, DiagnosticResult } from '../formats/_types';
import { PassageRefKind, PassageType } from '../hooks/hookTypes';
import { PassageGroup } from './ast';
import {
  CFGBuilder,
  PassageCFG,
  BasicBlock,
  CFGEdge,
  NavigationEdge,
  VariableStateMap,
  AbstractValue,
  getReachableNavigationEdges,
} from './cfg';
import { WorkspaceIndex } from './workspaceIndex';

// ─── Public Types ──────────────────────────────────────────────

/**
 * A node in the story flow graph — represents a passage with its CFG.
 */
export interface StoryFlowNode {
  /** Passage name */
  readonly passageName: string;
  /** Per-passage CFG */
  readonly cfg: PassageCFG;
  /** Whether this passage is reachable from Start */
  reachable: boolean;
  /**
   * Merged variable state at passage entry (after fixed-point).
   * This is the union of all possible variable states from all
   * paths that can reach this passage.
   */
  variableStateAtEntry: VariableStateMap;
}

/**
 * An edge in the story flow graph — represents navigation between passages.
 */
export interface StoryFlowEdge {
  /** Source passage name */
  readonly from: string;
  /** Target passage name */
  readonly to: string;
  /** Condition for this navigation (null = unconditional) */
  readonly condition: string | null;
  /** The navigation edge from the source passage's CFG */
  readonly sourceNavEdge: NavigationEdge;
  /** Kind of navigation (link, macro, API, etc.) */
  readonly refKind: PassageRefKind;
}

/**
 * The complete story flow graph.
 */
export interface StoryFlowGraph {
  /** All passage nodes, keyed by name */
  readonly nodes: Map<string, StoryFlowNode>;
  /** All inter-passage navigation edges */
  readonly edges: StoryFlowEdge[];
  /** The start passage name */
  readonly startPassage: string;
  /** Passages that are unreachable from start */
  readonly unreachablePassages: string[];
  /** Passages that are conditionally reachable (only via conditional links) */
  readonly conditionallyReachablePassages: string[];
  /** Dead-code conditions within passages */
  readonly deadConditions: DeadCondition[];
}

/**
 * A condition within a passage that can never be true or false
 * given the variable state at that point.
 */
export interface DeadCondition {
  /** Passage containing the dead condition */
  readonly passageName: string;
  /** The condition text */
  readonly condition: string;
  /** Why it's dead */
  readonly reason: 'always-true' | 'always-false' | 'contradictory';
  /** Source range of the condition */
  readonly range: SourceRange;
  /** The block ID in the passage CFG */
  readonly blockId: string;
}

/**
 * Result of building and analyzing the story flow graph.
 */
export interface StoryFlowAnalysis {
  readonly graph: StoryFlowGraph;
  readonly diagnostics: DiagnosticResult[];
}

// ─── Story Flow Graph Builder ──────────────────────────────────

export class StoryFlowGraphBuilder {
  private formatRegistry: FormatRegistry;
  private cfgBuilder: CFGBuilder;
  private workspaceIndex: WorkspaceIndex;

  constructor(formatRegistry: FormatRegistry, workspaceIndex: WorkspaceIndex) {
    this.formatRegistry = formatRegistry;
    this.cfgBuilder = new CFGBuilder(formatRegistry);
    this.workspaceIndex = workspaceIndex;
  }

  /**
   * Build and analyze the complete story flow graph.
   */
  buildAndAnalyze(passages: PassageGroup[]): StoryFlowAnalysis {
    const format = this.formatRegistry.getActiveFormat();
    const diagnostics: DiagnosticResult[] = [];

    // Step 1: Build per-passage CFGs
    const nodes = new Map<string, StoryFlowNode>();
    for (const passage of passages) {
      const passageName = passage.header.data.passageName ?? '';
      if (!passageName) continue;

      const cfg = this.cfgBuilder.buildPassageCFG(passage);
      nodes.set(passageName, {
        passageName,
        cfg,
        reachable: false,
        variableStateAtEntry: new Map(),
      });
    }

    // Step 2: Find the start passage
    const startPassage = this.findStartPassage();
    if (!startPassage || !nodes.has(startPassage)) {
      // No start passage — all passages are unreachable
      return {
        graph: {
          nodes,
          edges: [],
          startPassage: startPassage ?? '',
          unreachablePassages: Array.from(nodes.keys()),
          conditionallyReachablePassages: [],
          deadConditions: [],
        },
        diagnostics: [],
      };
    }

    // Step 3: Build inter-passage edges
    const edges = this.buildStoryFlowEdges(nodes, format);

    // Step 4: Compute reachability with variable state flow
    const { reachable, conditionallyReachable } = this.computeConditionalReachability(
      nodes, edges, startPassage, format,
    );

    // Step 5: Detect dead conditions
    const deadConditions = this.detectDeadConditions(nodes, format);

    // Step 6: Generate diagnostics
    this.generateDiagnostics(nodes, startPassage, reachable, conditionallyReachable, deadConditions, diagnostics);

    // Mark reachable/unreachable
    for (const [name, node] of nodes) {
      node.reachable = reachable.has(name);
    }

    return {
      graph: {
        nodes,
        edges,
        startPassage,
        unreachablePassages: Array.from(nodes.keys()).filter(n => !reachable.has(n)),
        conditionallyReachablePassages: Array.from(conditionallyReachable),
        deadConditions,
      },
      diagnostics,
    };
  }

  // ─── Step 2: Find Start Passage ─────────────────────────────

  private findStartPassage(): string | null {
    // Look for "Start" passage (Twee convention)
    const startNames = ['Start', 'start'];
    for (const name of startNames) {
      if (this.workspaceIndex.hasPassage(name)) {
        return name;
      }
    }

    // Fall back to the first passage
    const allNames = this.workspaceIndex.getAllPassageNames();
    return allNames.length > 0 ? allNames[0] : null;
  }

  // ─── Step 3: Build Inter-Passage Edges ──────────────────────

  private buildStoryFlowEdges(
    nodes: Map<string, StoryFlowNode>,
    format: FormatModule,
  ): StoryFlowEdge[] {
    const edges: StoryFlowEdge[] = [];

    for (const [passageName, node] of nodes) {
      // Only include navigation edges from reachable blocks
      const reachableNavEdges = getReachableNavigationEdges(node.cfg);

      for (const navEdge of reachableNavEdges) {
        const targetPassage = navEdge.targetPassage;
        if (!targetPassage || !nodes.has(targetPassage)) continue;

        edges.push({
          from: passageName,
          to: targetPassage,
          condition: navEdge.condition,
          sourceNavEdge: navEdge,
          refKind: navEdge.refKind,
        });
      }
    }

    return edges;
  }

  // ─── Step 4: Conditional Reachability ───────────────────────

  /**
   * Compute which passages are reachable, considering:
   *   1. Unconditional links (always reachable)
   *   2. Conditional links (reachable, but conditionally)
   *   3. Variable state flow (affects which conditions can be true)
   *
   * Uses BFS from start passage, flowing variable state through edges.
   * A passage is "conditionally reachable" if it's only reachable
   * through conditional navigation edges.
   */
  private computeConditionalReachability(
    nodes: Map<string, StoryFlowNode>,
    edges: StoryFlowEdge[],
    startPassage: string,
    format: FormatModule,
  ): { reachable: Set<string>; conditionallyReachable: Set<string> } {
    const reachable = new Set<string>();
    const conditionallyReachable = new Set<string>();
    const visited = new Set<string>();

    // Initialize start passage
    const startNode = nodes.get(startPassage);
    if (!startNode) return { reachable, conditionallyReachable };

    // Set initial variable state for start passage
    // StoryInit might set variables, but we'll handle that separately
    startNode.variableStateAtEntry = new Map();
    reachable.add(startPassage);

    // BFS with variable state propagation
    const queue: { passageName: string; isConditional: boolean }[] = [
      { passageName: startPassage, isConditional: false },
    ];

    // Also include special passages that always "run"
    // (PassageHeader, PassageFooter, PassageReady, etc.)
    for (const [name, node] of nodes) {
      const passage = this.workspaceIndex.getPassage(name);
      // Special passages like PassageHeader/Footer run on every passage render
      // They're always "reachable" but don't affect normal navigation flow
      if (passage) {
        // Mark script/stylesheet passages as reachable
        if (passage.type === PassageType.Script || passage.type === PassageType.Stylesheet) {
          reachable.add(name);
        }
      }
    }

    let iterations = 0;
    const MAX_ITERATIONS = 100;

    while (queue.length > 0 && iterations < MAX_ITERATIONS) {
      iterations++;
      const { passageName, isConditional } = queue.shift()!;

      if (visited.has(passageName)) continue;
      visited.add(passageName);

      const node = nodes.get(passageName);
      if (!node) continue;

      // Compute variable state at exit of this passage
      // by applying the passage CFG's effects to the entry state
      const exitState = this.propagateVariableState(node);

      // Find outgoing edges from this passage
      const outEdges = edges.filter(e => e.from === passageName);

      for (const edge of outEdges) {
        const targetName = edge.to;

        if (!reachable.has(targetName)) {
          reachable.add(targetName);
        }

        if (isConditional || edge.condition !== null) {
          conditionallyReachable.add(targetName);
        }

        // Propagate variable state to target
        const targetNode = nodes.get(targetName);
        if (targetNode) {
          // Merge our exit state into the target's entry state
          const mergedState = this.mergeStates(
            targetNode.variableStateAtEntry,
            exitState,
          );

          // Refine state based on the edge condition
          if (edge.condition) {
            this.refineStateForCondition(mergedState, edge.condition, true);
          }

          targetNode.variableStateAtEntry = mergedState;

          if (!visited.has(targetName)) {
            queue.push({
              passageName: targetName,
              isConditional: isConditional || edge.condition !== null,
            });
          }
        }
      }
    }

    // Also consider special passages as always reachable
    for (const [name] of nodes) {
      const passage = this.workspaceIndex.getPassage(name);
      if (passage?.type === PassageType.Script || passage?.type === PassageType.Stylesheet) {
        // Script/stylesheet passages are not "reachable" in the navigation sense
        // but they're not unreachable either
        reachable.add(name);
      }
    }

    return { reachable, conditionallyReachable };
  }

  /**
   * Propagate variable state through a passage's CFG.
   * Returns the variable state at the exit point.
   */
  private propagateVariableState(node: StoryFlowNode): VariableStateMap {
    const entryState = node.variableStateAtEntry;

    // The CFG already computed variableStateAfter for each block
    // We need to propagate from the entry block through to exit blocks

    // For simplicity, we take the union of variable states from all exit blocks
    const result = new Map(entryState);

    for (const [blockId, block] of node.cfg.blocks) {
      for (const [varName, value] of block.variableStateAfter) {
        if (result.has(varName)) {
          result.set(varName, this.mergeValues(result.get(varName)!, value));
        } else {
          result.set(varName, value);
        }
      }
    }

    return result;
  }

  // ─── Step 5: Dead Condition Detection ───────────────────────

  /**
   * Detect conditions within passages that are always true or always false
   * given the variable state at that point.
   */
  private detectDeadConditions(
    nodes: Map<string, StoryFlowNode>,
    format: FormatModule,
  ): DeadCondition[] {
    const results: DeadCondition[] = [];

    for (const [passageName, node] of nodes) {
      if (!node.reachable) continue;

      const entryState = node.variableStateAtEntry;

      for (const [blockId, block] of node.cfg.blocks) {
        // Check conditional blocks
        if (block.kind !== 'conditional') continue;

        // Get the condition from the first MacroCall node in the block
        const conditionNode = block.nodes.find(n => n.nodeType === 'MacroCall' && n.data.isClosing !== true);
        if (!conditionNode || !conditionNode.data.rawArgs) continue;

        const condition = conditionNode.data.rawArgs.trim();
        const result = this.evaluateCondition(condition, entryState);

        if (result === 'always-true') {
          results.push({
            passageName,
            condition,
            reason: 'always-true',
            range: conditionNode.range,
            blockId,
          });
        } else if (result === 'always-false') {
          results.push({
            passageName,
            condition,
            reason: 'always-false',
            range: conditionNode.range,
            blockId,
          });
        }
      }
    }

    return results;
  }

  /**
   * Evaluate a condition against a known variable state.
   * Returns 'always-true', 'always-false', or 'unknown'.
   */
  private evaluateCondition(
    condition: string,
    state: VariableStateMap,
  ): 'always-true' | 'always-false' | 'unknown' {
    // Pattern: $var (truthiness check)
    const simpleVarPattern = /\$(\w+)/;
    const simpleMatch = condition.match(simpleVarPattern);

    if (simpleMatch && condition.trim() === `$${simpleMatch[1]}`) {
      const varName = `$${simpleMatch[1]}`;
      const value = state.get(varName);
      if (value) {
        if (value.kind === 'literal') {
          if (value.value === false || value.value === 0 || value.value === '' || value.value === null) {
            return 'always-false';
          }
          if (value.value === true || (typeof value.value === 'number' && value.value !== 0)) {
            return 'always-true';
          }
        }
        if (value.kind === 'falsy') return 'always-false';
        if (value.kind === 'truthy') return 'always-true';
      }
      return 'unknown';
    }

    // Pattern: $var == literal (comparison)
    const eqPattern = /\$(\w+)\s*(?:==|===|eq)\s*("?'?[^"')\s]+"?'?)/;
    const eqMatch = condition.match(eqPattern);
    if (eqMatch) {
      const varName = `$${eqMatch[1]}`;
      const value = state.get(varName);
      if (value && value.kind === 'literal') {
        let compareValue: string | number | boolean = eqMatch[2];
        // Strip quotes
        if ((compareValue.startsWith('"') && compareValue.endsWith('"')) ||
            (compareValue.startsWith("'") && compareValue.endsWith("'"))) {
          compareValue = compareValue.slice(1, -1);
        } else if (compareValue === 'true') {
          compareValue = true;
        } else if (compareValue === 'false') {
          compareValue = false;
        } else if (/^\d+$/.test(compareValue as string)) {
          compareValue = parseInt(compareValue as string, 10);
        }

        if (value.value === compareValue) {
          return 'always-true';
        }
        // If we know the exact value and it doesn't match, always false
        return 'always-false';
      }
    }

    // Pattern: $var > number / $var >= number
    const gtPattern = /\$(\w+)\s*(>=|>)\s*(\d+)/;
    const gtMatch = condition.match(gtPattern);
    if (gtMatch) {
      const varName = `$${gtMatch[1]}`;
      const op = gtMatch[2];
      const threshold = parseInt(gtMatch[3], 10);
      const value = state.get(varName);

      if (value) {
        if (value.kind === 'literal' && typeof value.value === 'number') {
          if (op === '>') return value.value > threshold ? 'always-true' : 'always-false';
          if (op === '>=') return value.value >= threshold ? 'always-true' : 'always-false';
        }
        if (value.kind === 'range') {
          if (op === '>' && value.min > threshold) return 'always-true';
          if (op === '>' && value.max <= threshold) return 'always-false';
          if (op === '>=' && value.min >= threshold) return 'always-true';
          if (op === '>=' && value.max < threshold) return 'always-false';
        }
      }
    }

    // Pattern: $var < number / $var <= number
    const ltPattern = /\$(\w+)\s*(<=|<)\s*(\d+)/;
    const ltMatch = condition.match(ltPattern);
    if (ltMatch) {
      const varName = `$${ltMatch[1]}`;
      const op = ltMatch[2];
      const threshold = parseInt(ltMatch[3], 10);
      const value = state.get(varName);

      if (value) {
        if (value.kind === 'literal' && typeof value.value === 'number') {
          if (op === '<') return value.value < threshold ? 'always-true' : 'always-false';
          if (op === '<=') return value.value <= threshold ? 'always-true' : 'always-false';
        }
        if (value.kind === 'range') {
          if (op === '<' && value.max < threshold) return 'always-true';
          if (op === '<' && value.min >= threshold) return 'always-false';
          if (op === '<=' && value.max <= threshold) return 'always-true';
          if (op === '<=' && value.min > threshold) return 'always-false';
        }
      }
    }

    return 'unknown';
  }

  // ─── Step 6: Generate Diagnostics ───────────────────────────

  private generateDiagnostics(
    nodes: Map<string, StoryFlowNode>,
    startPassage: string,
    reachable: Set<string>,
    conditionallyReachable: Set<string>,
    deadConditions: DeadCondition[],
    diagnostics: DiagnosticResult[],
  ): void {
    // Unreachable passages
    for (const [name, node] of nodes) {
      if (!reachable.has(name)) {
        diagnostics.push({
          ruleId: 'unreachable-passage',
          message: `Passage "${name}" is unreachable from "${startPassage}"`,
          severity: 'warning',
          range: node.cfg.blocks.get(node.cfg.entryBlockId)?.range,
        });
      }
    }

    // Conditionally reachable passages (info, not warning)
    for (const name of conditionallyReachable) {
      if (reachable.has(name)) {
        // Only warn if a passage is ONLY conditionally reachable
        // and has no unconditional path
        const hasUnconditionalPath = nodes.get(name)?.cfg.navigationEdges.some(
          e => e.condition === null,
        );
        if (!hasUnconditionalPath) {
          diagnostics.push({
            ruleId: 'conditionally-reachable',
            message: `Passage "${name}" is only reachable via conditional links`,
            severity: 'hint',
            range: nodes.get(name)?.cfg.blocks.get(nodes.get(name)!.cfg.entryBlockId)?.range,
          });
        }
      }
    }

    // Dead conditions
    for (const dc of deadConditions) {
      const message = dc.reason === 'always-true'
        ? `Condition "${dc.condition}" is always true — the else branch is unreachable`
        : dc.reason === 'always-false'
          ? `Condition "${dc.condition}" is always false — this branch is unreachable`
          : `Condition "${dc.condition}" is contradictory`;

      diagnostics.push({
        ruleId: dc.reason === 'always-true' ? 'dead-else-branch' : 'dead-if-branch',
        message,
        severity: 'hint',
        range: dc.range,
      });
    }
  }

  // ─── Helpers ────────────────────────────────────────────────

  private mergeStates(a: VariableStateMap, b: VariableStateMap): VariableStateMap {
    const result = new Map(a);
    for (const [key, value] of b) {
      if (result.has(key)) {
        result.set(key, this.mergeValues(result.get(key)!, value));
      } else {
        result.set(key, value);
      }
    }
    return result;
  }

  private mergeValues(a: AbstractValue, b: AbstractValue): AbstractValue {
    if (a.kind === 'unknown' || b.kind === 'unknown') return { kind: 'unknown' };
    if (a.kind === 'literal' && b.kind === 'literal' && a.value === b.value) return a;
    if (a.kind === 'type' && b.kind === 'type' && a.type === b.type) return a;

    // Widen to union
    const values = new Set<AbstractValue>();
    if (a.kind === 'union') { for (const v of a.values) values.add(v); } else { values.add(a); }
    if (b.kind === 'union') { for (const v of b.values) values.add(v); } else { values.add(b); }
    return { kind: 'union', values };
  }

  private refineStateForCondition(state: VariableStateMap, condition: string, isTrue: boolean): void {
    // Reuse the same logic as CFGBuilder's refineStateForCondition
    const simpleVarPattern = /\$(\w+)/;
    const simpleMatch = condition.match(simpleVarPattern);

    if (simpleMatch) {
      const varName = `$${simpleMatch[1]}`;
      if (state.has(varName)) {
        if (isTrue) {
          state.set(varName, { kind: 'truthy' });
        } else {
          state.set(varName, { kind: 'falsy' });
        }
      }
    }

    const gtPattern = /\$(\w+)\s*>\s*(\d+)/;
    const gtMatch = condition.match(gtPattern);
    if (gtMatch && isTrue) {
      const varName = `$${gtMatch[1]}`;
      const threshold = parseInt(gtMatch[2], 10);
      state.set(varName, { kind: 'range', min: threshold + 1, max: Infinity });
    }

    const eqPattern = /\$(\w+)\s*(?:==|===|eq)\s*("?'?[^"')\s]+"?'?)/;
    const eqMatch = condition.match(eqPattern);
    if (eqMatch && isTrue) {
      const varName = `$${eqMatch[1]}`;
      let value: string | number | boolean = eqMatch[2];
      if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
        value = value.slice(1, -1);
      } else if (value === 'true') {
        value = true;
      } else if (value === 'false') {
        value = false;
      } else if (/^\d+$/.test(value as string)) {
        value = parseInt(value as string, 10);
      }
      state.set(varName, { kind: 'literal', value });
    }
  }
}
