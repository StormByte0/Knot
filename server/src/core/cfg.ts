/**
 * Knot v2 — Control Flow Graph (CFG)
 *
 * Builds a per-passage control flow graph from the AST. Each passage
 * gets its own CFG consisting of basic blocks connected by conditional
 * and unconditional edges.
 *
 * The CFG captures:
 *   - Sequential flow through macros and text
 *   - Conditional branches (<<if>>/<<else>>, (if:)/(else:), etc.)
 *   - Loops (<<for>>, <<while>>)
 *   - Navigation exits (<<goto>>, [[links]], (go-to:))
 *   - Variable assignments at each point
 *
 * Design principles:
 *   - Format-agnostic: works from AST nodes + MacroDef flags
 *   - Conservative: if we can't determine flow, we assume both paths
 *   - Composable: per-passage CFGs feed into the StoryFlowGraph
 *   - No format imports: reads MacroDef flags through FormatRegistry
 *
 * MUST NOT import from: formats/ (use FormatRegistry instead)
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, MacroDef, SourceRange } from '../formats/_types';
import { MacroBodyStyle, MacroKind, PassageRefKind } from '../hooks/hookTypes';
import { ASTNode, DocumentAST, walkTree, PassageGroup } from './ast';

// ─── Public Types ──────────────────────────────────────────────

/**
 * A basic block in the control flow graph.
 * Contains a sequence of AST nodes with a single entry point.
 * The last node may branch (conditional) or fall through (unconditional).
 */
export interface BasicBlock {
  /** Unique block ID within the passage CFG */
  readonly id: string;
  /** Ordered AST nodes in this block */
  readonly nodes: ASTNode[];
  /** Character range covered by this block */
  range: SourceRange;
  /** What kind of block this is */
  kind: BlockKind;
  /** Outgoing edges from this block */
  readonly successors: CFGEdge[];
  /** Incoming edges to this block */
  readonly predecessors: CFGEdge[];
  /**
   * Variable state AFTER executing this block.
   * Maps variable name → set of possible abstract values.
   * null = variable may be any value (unknown).
   */
  variableStateAfter: VariableStateMap;
}

/**
 * The kind of a basic block — determines how it exits.
 */
export type BlockKind =
  | 'entry'       // First block of the passage
  | 'sequential'  // Falls through to next block unconditionally
  | 'conditional' // Branches based on a condition (if/else/elseif)
  | 'loop-header' // Loop condition check (for, while)
  | 'loop-body'   // Body of a loop
  | 'loop-exit'   // Exit point after a loop
  | 'navigation'  // Exits passage via goto/link/navigation macro
  | 'exit'        // End of passage (fall-through end)
  | 'merge';      // Where two branches rejoin

/**
 * A directed edge between two basic blocks.
 */
export interface CFGEdge {
  /** Source block */
  readonly from: string;  // block ID
  /** Target block */
  readonly to: string;    // block ID
  /** What kind of flow this edge represents */
  readonly kind: EdgeKind;
  /**
   * The condition that must be true for this edge to be taken.
   * null = unconditional (always taken).
   * String = raw condition text (for display/diagnostics).
   */
  readonly condition: string | null;
  /** The AST node that creates this edge (for diagnostics) */
  readonly sourceNode?: ASTNode;
  /** Source range of the branching construct */
  readonly range?: SourceRange;
}

export type EdgeKind =
  | 'unconditional'  // Sequential fall-through
  | 'true-branch'    // Condition evaluated to true
  | 'false-branch'   // Condition evaluated to false
  | 'loop-continue'  // Back edge from loop body to loop header
  | 'loop-break'     // Break out of loop
  | 'navigation'     // Exits passage (goto, link, etc.)
  | 'include';       // Transcludes another passage (include, display)

/**
 * Abstract variable value for flow analysis.
 * We don't do full type inference — we track "what we know" conservatively.
 */
export type AbstractValue =
  | { kind: 'unknown' }                              // Could be anything
  | { kind: 'literal'; value: string | number | boolean | null }  // Known exact value
  | { kind: 'type'; type: 'string' | 'number' | 'boolean' | 'object' | 'array' | 'function' | 'null' | 'undefined' }
  | { kind: 'range'; min: number; max: number }      // Numeric range
  | { kind: 'union'; values: Set<AbstractValue> }    // One of several known values
  | { kind: 'truthy' }                               // Known to be truthy (not false/0/""/null/undefined)
  | { kind: 'falsy' };                               // Known to be falsy

/**
 * Maps variable name (with sigil, e.g. "$hp") to its abstract value.
 */
export type VariableStateMap = Map<string, AbstractValue>;

/**
 * Complete CFG for a single passage.
 */
export interface PassageCFG {
  /** The passage name */
  readonly passageName: string;
  /** All basic blocks, keyed by ID */
  readonly blocks: Map<string, BasicBlock>;
  /** The entry block ID */
  readonly entryBlockId: string;
  /** All exit block IDs (where passage flow leaves) */
  readonly exitBlockIds: string[];
  /** Navigation edges (links, goto, etc.) that leave this passage */
  readonly navigationEdges: NavigationEdge[];
  /**
   * Variable state at the START of this passage (before any macros execute).
   * This is set by the StoryFlowGraph when combining per-passage CFGs.
   */
  variableStateAtEntry: VariableStateMap;
  /**
   * Variable state at EACH exit point of this passage.
   * Key = exit block ID, value = variable state after that block.
   */
  readonly variableStateAtExits: Map<string, VariableStateMap>;
  /** Whether this passage has conditional navigation (links inside if/else) */
  readonly hasConditionalNavigation: boolean;
}

/**
 * A navigation edge — represents a passage transition.
 * Used by StoryFlowGraph to connect passage CFGs.
 */
export interface NavigationEdge {
  /** Target passage name */
  readonly targetPassage: string;
  /** The CFG edge that triggers this navigation */
  readonly sourceEdge: CFGEdge;
  /** Condition for this navigation (null = unconditional) */
  readonly condition: string | null;
  /** The passage reference kind */
  readonly refKind: PassageRefKind;
  /** Source range */
  readonly range: SourceRange;
}

// ─── CFG Builder ───────────────────────────────────────────────

export class CFGBuilder {
  private formatRegistry: FormatRegistry;
  private blockCounter: number = 0;

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
  }

  /**
   * Build a CFG for a single passage.
   */
  buildPassageCFG(passage: PassageGroup): PassageCFG {
    this.blockCounter = 0;
    const format = this.formatRegistry.getActiveFormat();

    const blocks = new Map<string, BasicBlock>();
    const navigationEdges: NavigationEdge[] = [];
    const exitBlockIds: string[] = [];

    // Create the entry block
    const entryBlock = this.createBlock('entry', passage.body.range);
    blocks.set(entryBlock.id, entryBlock);

    // Build the CFG by walking the passage body AST
    this.buildBlocks(passage.body, format, blocks, entryBlock, exitBlockIds, navigationEdges);

    // Compute variable states through forward dataflow analysis
    this.computeVariableStates(blocks, format);

    // Detect conditional navigation
    const hasConditionalNavigation = navigationEdges.some(e => e.condition !== null);

    return {
      passageName: passage.header.data.passageName ?? '',
      blocks,
      entryBlockId: entryBlock.id,
      exitBlockIds,
      navigationEdges,
      variableStateAtEntry: new Map(),
      variableStateAtExits: new Map(),
      hasConditionalNavigation,
    };
  }

  // ─── Block Construction ──────────────────────────────────────

  /**
   * Recursively build basic blocks from an AST node's children.
   *
   * The algorithm:
   *   1. Start with the current block
   *   2. Walk children sequentially
   *   3. When we hit a conditional macro (<<if>>), split into branches
   *   4. When we hit a loop macro (<<for>>), create loop header/body/exit
   *   5. When we hit a navigation macro/link, create a navigation edge
   *   6. After all branches rejoin, continue in a merge block
   */
  private buildBlocks(
    bodyNode: ASTNode,
    format: FormatModule,
    blocks: Map<string, BasicBlock>,
    currentBlock: BasicBlock,
    exitBlockIds: string[],
    navigationEdges: NavigationEdge[],
  ): void {
    let activeBlock = currentBlock;

    for (const child of bodyNode.children) {
      if (child.nodeType === 'MacroCall') {
        const macroName = child.data.macroName ?? '';
        const macroDef = this.findMacroDef(macroName, format);

        if (macroDef?.isConditional && macroDef.hasBody) {
          // Conditional macro — split into branches
          activeBlock = this.buildConditionalBranch(
            child, macroDef, format, blocks, activeBlock, exitBlockIds, navigationEdges,
          );
        } else if (this.isLoopMacro(macroDef)) {
          // Loop macro — create loop structure
          activeBlock = this.buildLoop(
            child, macroDef!, format, blocks, activeBlock, exitBlockIds, navigationEdges,
          );
        } else if (macroDef?.isNavigation) {
          // Navigation macro — create navigation edge
          this.buildNavigationMacro(child, macroDef, format, blocks, activeBlock, navigationEdges);
          activeBlock.kind = 'navigation';
          exitBlockIds.push(activeBlock.id);
          // After navigation, start a new block for any dead code
          const deadBlock = this.createBlock('sequential', child.range);
          blocks.set(deadBlock.id, deadBlock);
          this.addEdge(activeBlock, deadBlock, 'unconditional', null);
          activeBlock = deadBlock;
        } else {
          // Regular macro — add to current block
          activeBlock.nodes.push(child);
          this.extendRange(activeBlock, child.range);
        }
      } else if (child.nodeType === 'Link') {
        // Link — creates a navigation edge
        this.buildLinkNavigation(child, blocks, activeBlock, navigationEdges);
        // Links don't exit the passage immediately (user clicks them),
        // so we don't create a new block — they're potential exits
      } else if (child.nodeType === 'MacroClose') {
        // Close tags are structural — don't create flow
        continue;
      } else {
        // Text, Variable, etc. — add to current block
        activeBlock.nodes.push(child);
        this.extendRange(activeBlock, child.range);
      }
    }
  }

  /**
   * Build a conditional branch (if/else/elseif, unless, etc.)
   *
   * Structure:
   *   [current] --true--> [true-branch] --+
   *       |                                 |
   *       +--false--> [false-branch] -------+--> [merge]
   */
  private buildConditionalBranch(
    macroNode: ASTNode,
    macroDef: MacroDef,
    format: FormatModule,
    blocks: Map<string, BasicBlock>,
    currentBlock: BasicBlock,
    exitBlockIds: string[],
    navigationEdges: NavigationEdge[],
  ): BasicBlock {
    const condition = macroNode.data.rawArgs?.trim() ?? '';

    // The current block becomes the condition check point
    currentBlock.nodes.push(macroNode);
    currentBlock.kind = 'conditional';

    // True branch: build from the macro's body children
    const trueBlock = this.createBlock('sequential', macroNode.range);
    blocks.set(trueBlock.id, trueBlock);
    this.addEdge(currentBlock, trueBlock, 'true-branch', condition, macroNode);

    // Build the true branch body
    this.buildConditionalBody(macroNode, format, blocks, trueBlock, exitBlockIds, navigationEdges);

    // Collect all branch terminal blocks for merging
    const branchTerminals: BasicBlock[] = [trueBlock];
    let nextSibling: ASTNode | null = this.findNextSiblingMacro(macroNode, macroDef, format);

    // Handle else/elseif chains
    if (nextSibling && this.isElseVariant(nextSibling, format)) {
      let parentBlock = currentBlock;
      while (nextSibling && this.isElseVariant(nextSibling, format)) {
        const elseCondition = nextSibling.data.rawArgs?.trim() ?? null;
        const elseBlock = this.createBlock('sequential', nextSibling.range);
        blocks.set(elseBlock.id, elseBlock);

        if (elseCondition) {
          // elseif — conditional
          this.addEdge(parentBlock, elseBlock, 'false-branch', null, nextSibling);
          elseBlock.kind = 'conditional';
          elseBlock.nodes.push(nextSibling);
          this.extendRange(elseBlock, nextSibling.range);

          // True branch of elseif
          const elseTrueBlock = this.createBlock('sequential', nextSibling.range);
          blocks.set(elseTrueBlock.id, elseTrueBlock);
          this.addEdge(elseBlock, elseTrueBlock, 'true-branch', elseCondition, nextSibling);
          this.buildConditionalBody(nextSibling, format, blocks, elseTrueBlock, exitBlockIds, navigationEdges);
          branchTerminals.push(elseTrueBlock);

          parentBlock = elseBlock;
        } else {
          // else — unconditional (false branch of the chain)
          this.addEdge(parentBlock, elseBlock, 'false-branch', null, nextSibling);
          this.buildConditionalBody(nextSibling, format, blocks, elseBlock, exitBlockIds, navigationEdges);
          branchTerminals.push(elseBlock);
          break; // else is always the last in the chain
        }

        nextSibling = this.findNextSiblingMacro(nextSibling, macroDef, format);
      }

      // If the chain doesn't end with else, the parent's false branch goes to merge
      if (!nextSibling || !this.isElseVariant(nextSibling, format)) {
        // The last conditional in the chain still has a false branch
        // We'll add it to the merge block below
      }
    } else {
      // No else — false branch goes directly to merge
    }

    // Create merge block where all branches rejoin
    const mergeBlock = this.createBlock('merge', macroNode.range);
    blocks.set(mergeBlock.id, mergeBlock);

    for (const terminal of branchTerminals) {
      this.addEdge(terminal, mergeBlock, 'unconditional', null);
    }

    // If there's no else clause, add a false-branch from the last conditional to merge
    if (!this.chainHasElse(macroNode, format)) {
      this.addEdge(currentBlock, mergeBlock, 'false-branch', null);
    }

    return mergeBlock;
  }

  /**
   * Build the body of a conditional branch by processing the
   * children of the conditional macro node.
   */
  private buildConditionalBody(
    macroNode: ASTNode,
    format: FormatModule,
    blocks: Map<string, BasicBlock>,
    branchBlock: BasicBlock,
    exitBlockIds: string[],
    navigationEdges: NavigationEdge[],
  ): void {
    for (const child of macroNode.children) {
      if (child.nodeType === 'MacroCall') {
        const macroName = child.data.macroName ?? '';
        const macroDef = this.findMacroDef(macroName, format);

        if (macroDef?.isConditional && macroDef.hasBody) {
          branchBlock = this.buildConditionalBranch(
            child, macroDef, format, blocks, branchBlock, exitBlockIds, navigationEdges,
          );
        } else if (this.isLoopMacro(macroDef)) {
          branchBlock = this.buildLoop(
            child, macroDef!, format, blocks, branchBlock, exitBlockIds, navigationEdges,
          );
        } else if (macroDef?.isNavigation) {
          this.buildNavigationMacro(child, macroDef, format, blocks, branchBlock, navigationEdges);
          branchBlock.kind = 'navigation';
          exitBlockIds.push(branchBlock.id);
          const deadBlock = this.createBlock('sequential', child.range);
          blocks.set(deadBlock.id, deadBlock);
          this.addEdge(branchBlock, deadBlock, 'unconditional', null);
          branchBlock = deadBlock;
        } else {
          branchBlock.nodes.push(child);
          this.extendRange(branchBlock, child.range);
        }
      } else if (child.nodeType === 'Link') {
        this.buildLinkNavigation(child, blocks, branchBlock, navigationEdges);
      } else if (child.nodeType === 'MacroClose' || child.nodeType === 'HookOpen' || child.nodeType === 'HookClose') {
        continue;
      } else {
        branchBlock.nodes.push(child);
        this.extendRange(branchBlock, child.range);
      }
    }
  }

  /**
   * Build a loop (for, while, etc.)
   *
   * Structure:
   *   [current] --> [loop-header] --true--> [loop-body] --+
   *                    ^                           |       |
   *                    +--- loop-continue ----------+       |
   *                    |                                    |
   *                    +--false--> [loop-exit] --+--> [merge]
   */
  private buildLoop(
    macroNode: ASTNode,
    macroDef: MacroDef,
    format: FormatModule,
    blocks: Map<string, BasicBlock>,
    currentBlock: BasicBlock,
    exitBlockIds: string[],
    navigationEdges: NavigationEdge[],
  ): BasicBlock {
    const condition = macroNode.data.rawArgs?.trim() ?? '';

    // Loop header — condition check
    const loopHeader = this.createBlock('loop-header', macroNode.range);
    loopHeader.nodes.push(macroNode);
    blocks.set(loopHeader.id, loopHeader);
    this.addEdge(currentBlock, loopHeader, 'unconditional', null);

    // Loop body
    const loopBody = this.createBlock('loop-body', macroNode.range);
    blocks.set(loopBody.id, loopBody);
    this.addEdge(loopHeader, loopBody, 'true-branch', condition, macroNode);

    // Build loop body contents
    this.buildConditionalBody(macroNode, format, blocks, loopBody, exitBlockIds, navigationEdges);

    // Back edge from loop body to loop header
    this.addEdge(loopBody, loopHeader, 'loop-continue', null);

    // Loop exit — false branch from header
    const loopExit = this.createBlock('loop-exit', macroNode.range);
    blocks.set(loopExit.id, loopExit);
    this.addEdge(loopHeader, loopExit, 'false-branch', null);

    // Merge block after loop
    const mergeBlock = this.createBlock('merge', macroNode.range);
    blocks.set(mergeBlock.id, mergeBlock);
    this.addEdge(loopExit, mergeBlock, 'unconditional', null);

    return mergeBlock;
  }

  /**
   * Build a navigation edge from a macro call (goto, go-to, etc.)
   */
  private buildNavigationMacro(
    macroNode: ASTNode,
    macroDef: MacroDef,
    format: FormatModule,
    blocks: Map<string, BasicBlock>,
    currentBlock: BasicBlock,
    navigationEdges: NavigationEdge[],
  ): void {
    const rawArgs = macroNode.data.rawArgs ?? '';
    const passageArgPos = macroDef.passageArgPosition ?? 0;
    const targetPassage = this.extractPassageArg(rawArgs, passageArgPos, format);

    if (targetPassage) {
      const navEdge: NavigationEdge = {
        targetPassage,
        sourceEdge: {
          from: currentBlock.id,
          to: currentBlock.id, // Navigation stays in same block
          kind: 'navigation',
          condition: null,
          sourceNode: macroNode,
          range: macroNode.range,
        },
        condition: null,
        refKind: PassageRefKind.Macro,
        range: macroNode.range,
      };
      navigationEdges.push(navEdge);
    }
  }

  /**
   * Build a navigation edge from a Link AST node.
   */
  private buildLinkNavigation(
    linkNode: ASTNode,
    blocks: Map<string, BasicBlock>,
    currentBlock: BasicBlock,
    navigationEdges: NavigationEdge[],
  ): void {
    const target = linkNode.data.linkTarget?.trim();
    if (!target) return;

    // Check if this link is inside a conditional (for condition annotation)
    const enclosingCondition = this.findEnclosingCondition(linkNode);

    const navEdge: NavigationEdge = {
      targetPassage: target,
      sourceEdge: {
        from: currentBlock.id,
        to: currentBlock.id,
        kind: 'navigation',
        condition: enclosingCondition,
        sourceNode: linkNode,
        range: linkNode.range,
      },
      condition: enclosingCondition,
      refKind: PassageRefKind.Link,
      range: linkNode.range,
    };
    navigationEdges.push(navEdge);
  }

  // ─── Variable State Computation ──────────────────────────────

  /**
   * Compute variable states through forward dataflow analysis.
   *
   * For each block, we compute the variable state after executing it
   * based on:
   *   1. The union of variable states from all predecessor blocks
   *   2. Any variable assignments within this block
   *   3. Any conditions that constrain variable values on specific edges
   *
   * This is a simplified abstract interpretation — we don't do full
   * JavaScript evaluation, just track obvious patterns.
   */
  private computeVariableStates(
    blocks: Map<string, BasicBlock>,
    format: FormatModule,
  ): void {
    if (!format.variables) return;

    // Fixed-point iteration
    let changed = true;
    let iterations = 0;
    const MAX_ITERATIONS = 50;  // Safety limit

    while (changed && iterations < MAX_ITERATIONS) {
      changed = false;
      iterations++;

      for (const [blockId, block] of blocks) {
        // Compute input state: join of all predecessor output states
        const inputState = this.joinPredecessorStates(block, blocks);

        // Apply this block's effects
        const outputState = this.applyBlockEffects(block, inputState, format);

        // Check if state changed
        if (!this.statesEqual(block.variableStateAfter, outputState)) {
          block.variableStateAfter = outputState;
          changed = true;
        }
      }
    }
  }

  /**
   * Join variable states from all predecessors of a block.
   * For each variable present in multiple predecessors, take the union.
   */
  private joinPredecessorStates(block: BasicBlock, blocks: Map<string, BasicBlock>): VariableStateMap {
    if (block.predecessors.length === 0) {
      return new Map(); // Entry block — empty state
    }

    const result = new Map<string, AbstractValue>();

    for (const predEdge of block.predecessors) {
      const predBlock = blocks.get(predEdge.from);
      if (!predBlock) continue;

      for (const [varName, value] of predBlock.variableStateAfter) {
        if (result.has(varName)) {
          // Merge: take union of possible values
          const existing = result.get(varName)!;
          result.set(varName, this.mergeValues(existing, value));
        } else {
          result.set(varName, value);
        }
      }

      // If this edge has a condition, refine the state
      if (predEdge.condition && predEdge.kind === 'true-branch') {
        this.refineStateForCondition(result, predEdge.condition, true);
      } else if (predEdge.condition && predEdge.kind === 'false-branch') {
        this.refineStateForCondition(result, predEdge.condition, false);
      }
    }

    return result;
  }

  /**
   * Apply the effects of a block's macro calls to the variable state.
   */
  private applyBlockEffects(
    block: BasicBlock,
    inputState: VariableStateMap,
    format: FormatModule,
  ): VariableStateMap {
    const outputState = new Map(inputState);

    for (const node of block.nodes) {
      if (node.nodeType !== 'MacroCall' || !node.data.macroName) continue;
      const macroName = node.data.macroName;

      // Check if this is an assignment macro
      if (format.variables?.assignmentMacros.has(macroName)) {
        this.applyAssignment(node, macroName, outputState, format);
      }
    }

    return outputState;
  }

  /**
   * Apply a variable assignment from a macro call.
   *
   * SugarCube: <<set $hp to 10>>, <<set $name to "Alice">>
   * Harlowe: (set: $hp to 10), (put: $name into "Alice")
   *
   * We extract the variable name and try to determine the assigned value.
   */
  private applyAssignment(
    macroNode: ASTNode,
    macroName: string,
    state: VariableStateMap,
    format: FormatModule,
  ): void {
    const rawArgs = macroNode.data.rawArgs ?? '';
    if (!rawArgs || !format.variables) return;

    const pattern = new RegExp(format.variables.variablePattern.source, format.variables.variablePattern.flags);
    let match: RegExpExecArray | null;

    while ((match = pattern.exec(rawArgs)) !== null) {
      const sigil = match[1];
      const varName = match[2];
      if (!varName) continue;

      const fullName = `${sigil}${varName}`;

      // Try to determine the assigned value from the expression
      const assignedValue = this.inferAssignedValue(rawArgs, fullName, format);
      state.set(fullName, assignedValue);
    }
  }

  /**
   * Try to infer the value assigned to a variable from the raw expression.
   *
   * Patterns we recognize:
   *   - $var to 10           → { kind: 'literal', value: 10 }
   *   - $var to "string"     → { kind: 'literal', value: "string" }
   *   - $var to true/false   → { kind: 'literal', value: true/false }
   *   - $var to $other       → copy $other's abstract value
   *   - $var to expr + 1     → { kind: 'type', type: 'number' }
   *   - anything else        → { kind: 'unknown' }
   */
  private inferAssignedValue(rawArgs: string, varName: string, format: FormatModule): AbstractValue {
    // Try to match: $var to <value>
    const assignPattern = new RegExp(
      `${escapeRegExp(varName)}\\s+(?:to|=)\\s+(.+)`,
      'i',
    );
    const assignMatch = rawArgs.match(assignPattern);

    if (assignMatch) {
      const valueExpr = assignMatch[1].trim();

      // Number literal
      if (/^-?\d+(\.\d+)?$/.test(valueExpr)) {
        return { kind: 'literal', value: parseFloat(valueExpr) };
      }

      // String literal
      if (/^["'].*["']$/.test(valueExpr)) {
        return { kind: 'literal', value: valueExpr.slice(1, -1) };
      }

      // Boolean literal
      if (valueExpr === 'true') return { kind: 'literal', value: true };
      if (valueExpr === 'false') return { kind: 'literal', value: false };
      if (valueExpr === 'null') return { kind: 'literal', value: null };

      // Another variable reference — copy its abstract value if known
      if (format.variables) {
        const varPattern = new RegExp(format.variables.variablePattern.source, format.variables.variablePattern.flags);
        const varMatch = varPattern.exec(valueExpr);
        if (varMatch) {
          // We can't resolve the other variable's value here (it's in the state map)
          // Return 'unknown' — the dataflow analysis will refine this
          return { kind: 'unknown' };
        }
      }

      // Numeric expression (contains math operators)
      if (/[\+\-\*\/\%]/.test(valueExpr)) {
        return { kind: 'type', type: 'number' };
      }

      // Function call or complex expression
      return { kind: 'unknown' };
    }

    return { kind: 'unknown' };
  }

  /**
   * Refine variable state based on a condition being true or false.
   *
   * Examples:
   *   condition = "$hp > 0", isTrue = true → $hp is at least 1
   *   condition = "$hasKey", isTrue = true → $hasKey is truthy
   *   condition = "$hasKey", isTrue = false → $hasKey is falsy
   */
  private refineStateForCondition(state: VariableStateMap, condition: string, isTrue: boolean): void {
    // Simple pattern: $var (truthiness check)
    const simpleVarPattern = /\$([\w]+)/;
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

    // Pattern: $var > <number>
    const gtPattern = /\$(\w+)\s*>\s*(\d+)/;
    const gtMatch = condition.match(gtPattern);
    if (gtMatch && isTrue) {
      const varName = `$${gtMatch[1]}`;
      const threshold = parseInt(gtMatch[2], 10);
      state.set(varName, { kind: 'range', min: threshold + 1, max: Infinity });
    }

    // Pattern: $var == <literal>
    const eqPattern = /\$(\w+)\s*(?:==|===|eq)\s*("?)([^"]*)\2/;
    const eqMatch = condition.match(eqPattern);
    if (eqMatch && isTrue) {
      const varName = `$${eqMatch[1]}`;
      const value = eqMatch[3].trim();
      if (/^\d+$/.test(value)) {
        state.set(varName, { kind: 'literal', value: parseInt(value, 10) });
      } else if (value === 'true') {
        state.set(varName, { kind: 'literal', value: true });
      } else if (value === 'false') {
        state.set(varName, { kind: 'literal', value: false });
      } else {
        state.set(varName, { kind: 'literal', value: value });
      }
    }
  }

  // ─── Helpers ────────────────────────────────────────────────

  private createBlock(kind: BlockKind, range: SourceRange): BasicBlock {
    const id = `B${this.blockCounter++}`;
    const block: BasicBlock = {
      id,
      nodes: [],
      range: { ...range },
      kind,
      successors: [],
      predecessors: [],
      variableStateAfter: new Map(),
    };
    return block;
  }

  private addEdge(
    from: BasicBlock,
    to: BasicBlock,
    kind: EdgeKind,
    condition: string | null,
    sourceNode?: ASTNode,
  ): void {
    const edge: CFGEdge = {
      from: from.id,
      to: to.id,
      kind,
      condition,
      sourceNode,
      range: sourceNode?.range,
    };
    from.successors.push(edge);
    to.predecessors.push(edge);
  }

  private extendRange(block: BasicBlock, range: SourceRange): void {
    block.range = {
      start: Math.min(block.range.start, range.start),
      end: Math.max(block.range.end, range.end),
    };
  }

  private findMacroDef(name: string, format: FormatModule): MacroDef | undefined {
    if (!format.macros) return undefined;

    const normalizedName = name.endsWith(':') ? name : name;
    return format.macros.builtins.find(
      m => m.name === normalizedName || (m.aliases && m.aliases.includes(normalizedName)),
    );
  }

  private isLoopMacro(macroDef: MacroDef | undefined): boolean {
    if (!macroDef) return false;
    // A macro is a loop if it has children 'break' and 'continue'
    // or if its categoryDetail is 'iteration'
    return (
      (macroDef.children?.includes('break') && macroDef.children?.includes('continue')) ||
      macroDef.categoryDetail === 'iteration'
    );
  }

  private isElseVariant(node: ASTNode, format: FormatModule): boolean {
    if (node.nodeType !== 'MacroCall') return false;
    const name = node.data.macroName ?? '';
    // Check if this macro is listed as a child of an 'if' macro
    const ifDef = this.findMacroDef('if', format);
    return ifDef?.children?.includes(name) ?? false;
  }

  private findNextSiblingMacro(node: ASTNode, parentDef: MacroDef, format: FormatModule): ASTNode | null {
    if (!node.parent) return null;
    const siblings = node.parent.children;
    const idx = siblings.indexOf(node);
    if (idx < 0 || idx + 1 >= siblings.length) return null;

    // Look for the next sibling that is an else/elseif variant
    for (let i = idx + 1; i < siblings.length; i++) {
      const sibling = siblings[i];
      if (sibling.nodeType === 'MacroCall' && this.isElseVariant(sibling, format)) {
        return sibling;
      }
      // If we hit a non-else MacroCall or text, the chain is broken
      if (sibling.nodeType === 'MacroCall' || sibling.nodeType === 'Text') {
        break;
      }
    }
    return null;
  }

  private chainHasElse(startNode: ASTNode, format: FormatModule): boolean {
    let node: ASTNode | null = startNode;
    while (node) {
      if (node.nodeType === 'MacroCall' && node.data.macroName === 'else') {
        return true;
      }
      node = this.findNextSiblingMacro(node, this.findMacroDef(node.data.macroName ?? '', format)!, format);
    }
    return false;
  }

  private findEnclosingCondition(node: ASTNode): string | null {
    let ancestor = node.parent;
    while (ancestor) {
      if (ancestor.nodeType === 'MacroCall' && ancestor.data.macroName) {
        // Check if this is a conditional macro
        const format = this.formatRegistry.getActiveFormat();
        const macroDef = this.findMacroDef(ancestor.data.macroName, format);
        if (macroDef?.isConditional) {
          return ancestor.data.rawArgs?.trim() ?? null;
        }
      }
      ancestor = ancestor.parent;
    }
    return null;
  }

  private extractPassageArg(rawArgs: string, position: number, format: FormatModule): string | null {
    // Simple argument splitting — respects quotes
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
    if (current.length > 0) args.push(current);

    if (position < args.length) {
      const arg = args[position];
      // Strip quotes
      if ((arg.startsWith('"') && arg.endsWith('"')) ||
          (arg.startsWith("'") && arg.endsWith("'"))) {
        return arg.slice(1, -1);
      }
      return arg;
    }
    return null;
  }

  /**
   * Merge two abstract values.
   */
  private mergeValues(a: AbstractValue, b: AbstractValue): AbstractValue {
    if (a.kind === 'unknown' || b.kind === 'unknown') return { kind: 'unknown' };

    if (a.kind === 'literal' && b.kind === 'literal' && a.value === b.value) {
      return a;
    }

    if (a.kind === 'type' && b.kind === 'type' && a.type === b.type) {
      return a;
    }

    if (a.kind === 'literal' && b.kind === 'type' && this.literalMatchesType(a.value, b.type)) {
      return b; // Widen to the type
    }

    if (b.kind === 'literal' && a.kind === 'type' && this.literalMatchesType(b.value, a.type)) {
      return a; // Widen to the type
    }

    // Incompatible — union
    const values = new Set<AbstractValue>();
    if (a.kind === 'union') { for (const v of a.values) values.add(v); } else { values.add(a); }
    if (b.kind === 'union') { for (const v of b.values) values.add(v); } else { values.add(b); }
    return { kind: 'union', values };
  }

  private literalMatchesType(value: string | number | boolean | null, type: string): boolean {
    if (value === null && type === 'null') return true;
    if (typeof value === 'number' && type === 'number') return true;
    if (typeof value === 'string' && type === 'string') return true;
    if (typeof value === 'boolean' && type === 'boolean') return true;
    return false;
  }

  private statesEqual(a: VariableStateMap, b: VariableStateMap): boolean {
    if (a.size !== b.size) return false;
    for (const [key, valA] of a) {
      const valB = b.get(key);
      if (!valB) return false;
      if (valA.kind !== valB.kind) return false;
      // Simplified comparison — sufficient for fixed-point detection
      if (valA.kind === 'literal' && valB.kind === 'literal' && valA.value !== valB.value) return false;
      if (valA.kind === 'type' && valB.kind === 'type' && valA.type !== valB.type) return false;
    }
    return true;
  }
}

// ─── Utility Functions ──────────────────────────────────────────

function escapeRegExp(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Check if a passage CFG has any reachable path from entry to a given block.
 */
export function isBlockReachable(cfg: PassageCFG, blockId: string): boolean {
  const visited = new Set<string>();
  const queue = [cfg.entryBlockId];

  while (queue.length > 0) {
    const current = queue.pop()!;
    if (current === blockId) return true;
    if (visited.has(current)) continue;
    visited.add(current);

    const block = cfg.blocks.get(current);
    if (block) {
      for (const edge of block.successors) {
        if (!visited.has(edge.to)) {
          queue.push(edge.to);
        }
      }
    }
  }

  return false;
}

/**
 * Find all navigation edges reachable from the entry block.
 * Excludes edges from dead-code blocks.
 */
export function getReachableNavigationEdges(cfg: PassageCFG): NavigationEdge[] {
  const reachable = new Set<string>();
  const queue = [cfg.entryBlockId];

  while (queue.length > 0) {
    const current = queue.pop()!;
    if (reachable.has(current)) continue;
    reachable.add(current);

    const block = cfg.blocks.get(current);
    if (block) {
      for (const edge of block.successors) {
        if (!reachable.has(edge.to)) {
          queue.push(edge.to);
        }
      }
    }
  }

  return cfg.navigationEdges.filter(e => reachable.has(e.sourceEdge.from));
}
