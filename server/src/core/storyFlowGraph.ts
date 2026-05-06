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
import type { FormatModule, SourceRange, DiagnosticResult, SpecialPassageDef } from '../formats/_types';
import { PassageRefKind, PassageType, PassageKind } from '../hooks/hookTypes';
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
import { WorkspaceIndex, PassageEntry } from './workspaceIndex';

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

    // Step 3: Build inter-passage edges (explicit links/macros)
    const edges = this.buildStoryFlowEdges(nodes, format);

    // Step 3b: Add virtual edges for special passages
    //   StoryInit → Start passage (runs before the first navigation)
    //   PassageHeader → every passage (runs before each passage body)
    //   PassageFooter → every passage (runs after each passage body)
    //   PassageReady/Done → every passage (post-render hooks)
    this.addSpecialPassageVirtualEdges(nodes, edges, startPassage, format);

    // Step 4: Compute reachability with variable state flow
    //   IMPORTANT: We seed the start passage's variable state with
    //   the effects of StoryInit and PassageHeader, because those
    //   run BEFORE the start passage body executes.
    const initVariableState = this.computeInitVariableState(nodes, startPassage, format);
    const { reachable, conditionallyReachable } = this.computeConditionalReachability(
      nodes, edges, startPassage, format, initVariableState,
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

  /**
   * Find the start passage by checking, in order:
   *   1. StoryData JSON's "start" property (the canonical source)
   *   2. Twee convention: passage named "Start" or "start"
   *   3. Fallback: first passage in the workspace
   *
   * StoryData is a JSON passage that contains metadata like:
   *   { "start": "My First Passage", "format": "SugarCube", ... }
   * Every Twine format stores the start passage name there.
   */
  private findStartPassage(): string | null {
    // 1. Check StoryData JSON for the "start" property
    const storyDataStart = this.extractStartFromStoryData();
    if (storyDataStart && this.workspaceIndex.hasPassage(storyDataStart)) {
      return storyDataStart;
    }

    // 2. Twee convention: "Start" passage
    const startNames = ['Start', 'start'];
    for (const name of startNames) {
      if (this.workspaceIndex.hasPassage(name)) {
        return name;
      }
    }

    // 3. Fall back to the first passage
    const allNames = this.workspaceIndex.getAllPassageNames();
    return allNames.length > 0 ? allNames[0] : null;
  }

  /**
   * Extract the "start" property from the StoryData passage.
   * StoryData is a special passage containing JSON metadata.
   */
  private extractStartFromStoryData(): string | null {
    const storyDataPassages = this.workspaceIndex.getPassagesByType(PassageType.StoryData);
    if (storyDataPassages.length === 0) return null;

    for (const entry of storyDataPassages) {
      try {
        const data = JSON.parse(entry.body.trim());
        if (data.start && typeof data.start === 'string') {
          return data.start;
        }
      } catch {
        // Malformed JSON — skip
      }
    }
    return null;
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
    initVariableState: VariableStateMap,
  ): { reachable: Set<string>; conditionallyReachable: Set<string> } {
    const reachable = new Set<string>();
    const conditionallyReachable = new Set<string>();
    const visited = new Set<string>();

    // Initialize start passage
    const startNode = nodes.get(startPassage);
    if (!startNode) return { reachable, conditionallyReachable };

    // Seed the start passage with the init variable state.
    // This includes variables set by StoryInit and PassageHeader,
    // which run before the start passage body executes.
    startNode.variableStateAtEntry = new Map(initVariableState);
    reachable.add(startPassage);

    // BFS with variable state propagation
    const queue: { passageName: string; isConditional: boolean }[] = [
      { passageName: startPassage, isConditional: false },
    ];

    // Mark special passages as always reachable — they run automatically
    // by the engine regardless of whether any link points to them.
    // This includes: StoryInit, PassageHeader, PassageFooter, etc.
    const specialPassageNames = this.getSpecialPassageNames(format);
    for (const name of specialPassageNames) {
      if (nodes.has(name)) {
        reachable.add(name);
        // Seed their variable state too
        const spNode = nodes.get(name)!;
        if (spNode.variableStateAtEntry.size === 0) {
          spNode.variableStateAtEntry = new Map(initVariableState);
        }
      }
    }

    // Script/stylesheet passages are also always "reachable" (loaded by engine)
    for (const [name] of nodes) {
      const passage = this.workspaceIndex.getPassage(name);
      if (passage?.type === PassageType.Script || passage?.type === PassageType.Stylesheet) {
        reachable.add(name);
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

  // ─── Special Passage Integration ────────────────────────────

  /**
   * Add virtual edges to the graph for special passages that run
   * automatically by the engine, even though no link points to them.
   *
   * These edges represent the engine's execution order:
   *
   *   ENGINE START:
   *     [StoryInit] ──virtual──→ [Start passage]
   *     StoryInit runs once before the first passage. Variables set
   *     here flow into every passage's entry state.
   *
   *   EVERY PASSAGE RENDER:
   *     [PassageHeader] ──virtual──→ [PassageBody] ──virtual──→ [PassageFooter]
   *     PassageHeader runs before each passage body.
   *     PassageFooter runs after each passage body.
   *     PassageReady runs after the DOM is ready.
   *     PassageDone runs after the passage transition completes.
   *
   * These "virtual" edges use PassageRefKind.Implicit because they
   * aren't authored by the user — they're engine behavior.
   */
  private addSpecialPassageVirtualEdges(
    nodes: Map<string, StoryFlowNode>,
    edges: StoryFlowEdge[],
    startPassage: string,
    format: FormatModule,
  ): void {
    if (!format.specialPassages || format.specialPassages.length === 0) return;

    // Find special passages by their typeId — format-agnostic!
    // SugarCube: StoryInit (typeId='init'), PassageHeader (typeId='header'), PassageFooter (typeId='footer')
    // Harlowe:   Startup    (typeId='init'), Header       (typeId='header'), Footer      (typeId='footer')
    // The typeId is the universal contract; passage names differ per format.
    const initPassage = this.findSpecialPassageByTypeId(format, 'init');
    const headerPassage = this.findSpecialPassageByTypeId(format, 'header');
    const footerPassage = this.findSpecialPassageByTypeId(format, 'footer');

    // ── Init passage → Start passage ───────────────────────────────
    // The init passage (StoryInit/Startup) runs once at engine startup,
    // before any passage renders. Variables set here are the "first
    // declarations" — they flow into the start passage and from there
    // into every reachable passage.
    if (initPassage && initPassage.name && nodes.has(initPassage.name)) {
      edges.push({
        from: initPassage.name,
        to: startPassage,
        condition: null,
        sourceNavEdge: {
          targetPassage: startPassage,
          sourceEdge: {
            from: 'virtual',
            to: 'virtual',
            kind: 'navigation' as const,
            condition: null,
          },
          condition: null,
          refKind: PassageRefKind.Implicit,
          range: { start: 0, end: 0 },
        },
        refKind: PassageRefKind.Implicit,
      });
    }

    // ── Interface passage → Start passage ──────────────────────────────
    // The interface passage (StoryInterface) defines the story's HTML
    // structure. It runs at engine startup, before any passage renders.
    // It's loaded automatically — no link points to it. Variables set
    // here (rare but possible) flow into the start passage.
    const interfacePassage = this.findSpecialPassageByTypeId(format, 'interface');
    if (interfacePassage && interfacePassage.name && nodes.has(interfacePassage.name)) {
      edges.push({
        from: interfacePassage.name,
        to: startPassage,
        condition: null,
        sourceNavEdge: {
          targetPassage: startPassage,
          sourceEdge: {
            from: 'virtual',
            to: 'virtual',
            kind: 'navigation' as const,
            condition: null,
          },
          condition: null,
          refKind: PassageRefKind.Implicit,
          range: { start: 0, end: 0 },
        },
        refKind: PassageRefKind.Implicit,
      });
    }

    // ── Header passage → every passage ─────────────────────────────
    // The header passage (PassageHeader/Header) content is prepended to
    // every passage render. For the graph, this means: variable state
    // from the header passage flows into every passage's entry state.
    //
    // We add a virtual edge from the header passage to every story passage.
    // This is important because the header passage may set/modify variables
    // that affect every passage's initial state.
    if (headerPassage && headerPassage.name && nodes.has(headerPassage.name)) {
      for (const [passageName, node] of nodes) {
        const entry = this.workspaceIndex.getPassage(passageName);
        // Don't add header→header, header→footer, header→storydata, etc.
        if (!entry) continue;
        if (entry.type !== PassageType.Story && entry.type !== PassageType.Start) continue;
        if (passageName === headerPassage.name) continue;

        edges.push({
          from: headerPassage.name,
          to: passageName,
          condition: null,
          sourceNavEdge: {
            targetPassage: passageName,
            sourceEdge: {
              from: 'virtual',
              to: 'virtual',
              kind: 'navigation' as const,
              condition: null,
            },
            condition: null,
            refKind: PassageRefKind.Implicit,
            range: { start: 0, end: 0 },
          },
          refKind: PassageRefKind.Implicit,
        });
      }
    }

    // The footer passage (PassageFooter/Footer) and post-render passages
    // (PassageReady/Done) run AFTER the passage body, so they don't affect
    // the entry variable state of other passages. We still mark them as
    // reachable but don't add edges FROM them to other passages (their
    // effects are local to the current render cycle).
    //
    // Widget passages (tag: widget) are also always reachable but
    // don't participate in navigation flow — they define custom macros.
  }

  /**
   * Compute the initial variable state that exists before the start
   * passage runs. This is the combined effect of:
   *   1. StoryInit (runs once at engine startup)
   *   2. PassageHeader (runs before every passage, including Start)
   *
   * This is the ONLY way to correctly track variable definitions:
   * a variable set in StoryInit like <<set $hp to 100>> must be
   * known to exist (with value 100) when the Start passage begins.
   * Without this, the semantic analyzer would flag $hp as
   * "used but never assigned" when it appears in the Start passage.
   */
  private computeInitVariableState(
    nodes: Map<string, StoryFlowNode>,
    startPassage: string,
    format: FormatModule,
  ): VariableStateMap {
    const state: VariableStateMap = new Map();

    // 1. Apply init passage effects (StoryInit/Startup — typeId='init')
    const initPassage = this.findSpecialPassageByTypeId(format, 'init');
    if (initPassage && initPassage.name && nodes.has(initPassage.name)) {
      const initNode = nodes.get(initPassage.name)!;
      // Propagate through the StoryInit CFG to get exit variable state
      const initExitState = this.propagateVariableState(initNode);
      for (const [varName, value] of initExitState) {
        state.set(varName, value);
      }
    }

    // 1b. Apply interface passage effects (StoryInterface — typeId='interface')
    //     Runs at engine startup, like StoryInit. Rarely sets variables,
    //     but if it does, those should be tracked.
    const interfacePassage = this.findSpecialPassageByTypeId(format, 'interface');
    if (interfacePassage && interfacePassage.name && nodes.has(interfacePassage.name)) {
      const interfaceNode = nodes.get(interfacePassage.name)!;
      interfaceNode.variableStateAtEntry = new Map(state);
      const interfaceExitState = this.propagateVariableState(interfaceNode);
      for (const [varName, value] of interfaceExitState) {
        if (state.has(varName)) {
          state.set(varName, this.mergeValues(state.get(varName)!, value));
        } else {
          state.set(varName, value);
        }
      }
    }

    // 2. Apply header passage effects (PassageHeader/Header — typeId='header')
    //    Runs before every passage body.
    const headerPassage = this.findSpecialPassageByTypeId(format, 'header');
    if (headerPassage && headerPassage.name && nodes.has(headerPassage.name)) {
      const headerNode = nodes.get(headerPassage.name)!;
      // The header passage sees the same variable state as the init passage produced
      headerNode.variableStateAtEntry = new Map(state);
      const headerExitState = this.propagateVariableState(headerNode);
      for (const [varName, value] of headerExitState) {
        // Merge with existing state (init passage may have set some vars too)
        if (state.has(varName)) {
          state.set(varName, this.mergeValues(state.get(varName)!, value));
        } else {
          state.set(varName, value);
        }
      }
    }

    return state;
  }

  /**
   * Get the names of all special passages defined by the current format.
   */
  private getSpecialPassageNames(format: FormatModule): string[] {
    const names: string[] = [];
    for (const sp of format.specialPassages) {
      if (sp.name) {
        // Check if this special passage actually exists in the workspace
        if (this.workspaceIndex.hasPassage(sp.name)) {
          names.push(sp.name);
        }
      }
      if (sp.tag) {
        // Tag-based special passages — find all passages with this tag
        const tagged = this.workspaceIndex.getAllPassages().filter(
          p => sp.tag && p.tags.includes(sp.tag),
        );
        for (const p of tagged) {
          names.push(p.name);
        }
      }
    }
    return names;
  }

  /**
   * Find a SpecialPassageDef by its typeId.
   */
  private findSpecialPassageByTypeId(format: FormatModule, typeId: string): SpecialPassageDef | undefined {
    return format.specialPassages.find(sp => sp.typeId === typeId);
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
