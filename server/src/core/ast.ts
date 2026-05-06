/**
 * Knot v2 — Abstract Syntax Tree
 *
 * Format-agnostic AST structure. Core builds AST trees using node type IDs
 * declared by the active format module's `astNodeTypes`. Every format must
 * provide the baseline types (Document, PassageHeader, PassageBody, Link, Text)
 * plus any format-specific ones (MacroCall, Hook, Variable, etc.).
 *
 * DESIGN PRINCIPLES:
 *   - nodeType is a string matching format.astNodeTypes.types key
 *   - Core constructs the tree; formats declare what types exist
 *   - Children are ordered; parent back-links are set during construction
 *   - SourceRange maps back to the original document for LSP features
 *   - Visitor pattern for tree traversal (avoids recursive type coupling)
 *
 * MUST NOT import from: formats/ (use FormatRegistry instead)
 */

import type { SourceRange, PassageRef, BodyToken } from '../formats/_types';
import { PassageType } from '../hooks/hookTypes';

// ─── AST Node ──────────────────────────────────────────────────

/**
 * A single node in the AST.
 *
 * `nodeType` references a type ID from the format's astNodeTypes declaration
 * (e.g. 'Document', 'PassageHeader', 'MacroCall', 'Variable', 'Link', 'Text').
 *
 * `data` carries node-type-specific payloads without polluting the interface.
 */
export interface ASTNode {
  /** Type ID from format.astNodeTypes (e.g. 'Document', 'MacroCall', 'Variable') */
  readonly nodeType: string;
  /** Ordered child nodes */
  children: ASTNode[];
  /** Source range in the document */
  range: SourceRange;
  /** Parent node — set by ASTBuilder during tree construction */
  parent: ASTNode | null;
  /** Node-type-specific payload */
  data: ASTNodeData;
}

/**
 * Node-type-specific data payload.
 * Not every field is populated for every node — only the fields
 * relevant to the nodeType are filled.
 */
export interface ASTNodeData {
  // ── PassageHeader ──────────────────────────────────────
  /** Passage name (from :: header) */
  passageName?: string;
  /** Passage tags (from [tag1 tag2] in header) */
  passageTags?: string[];
  /** Classified passage type */
  passageType?: PassageType;
  /** Format-specific custom type ID (for PassageType.Custom) */
  customTypeId?: string;

  // ── MacroCall / MacroClose ─────────────────────────────
  /** Macro name (without delimiters) */
  macroName?: string;
  /** Whether this is a closing tag (e.g. <</if>>) */
  isClosing?: boolean;
  /** Whether this macro has a body (hasBody flag from MacroDef) */
  hasBody?: boolean;
  /** Raw argument text between macro name and closing delimiter */
  rawArgs?: string;

  // ── Variable ───────────────────────────────────────────
  /** Variable name (without sigil) */
  varName?: string;
  /** Variable sigil character ('$', '_', etc.) */
  varSigil?: string;

  // ── Link ───────────────────────────────────────────────
  /** Link target passage name */
  linkTarget?: string;
  /** Link display text (may differ from target) */
  linkDisplayText?: string;
  /** Link kind from format.resolveLinkBody() */
  linkKind?: import('../formats/_types').LinkResolution['kind'];
  /** Link setter expression (SugarCube [[target|display->setter]]) */
  linkSetter?: string;

  // ── Hook (Harlowe) ─────────────────────────────────────
  /** Hook name/tag (e.g. |hookName>) */
  hookName?: string;

  // ── Template (Snowman) ─────────────────────────────────
  /** Whether this template block is an expression (<%=) vs statement (<%) */
  isExpression?: boolean;

  // ── Insert (Chapbook) ──────────────────────────────────
  /** Insert name (e.g. 'back', 'embed passage') */
  insertName?: string;

  // ── Text ───────────────────────────────────────────────
  /** Raw text content */
  text?: string;

  // ── Front Matter (Chapbook) ────────────────────────────
  /** Whether this is a YAML front matter block */
  isFrontMatter?: boolean;
}

// ─── Document AST ──────────────────────────────────────────────

/**
 * The top-level AST for a single Twee document.
 * Contains the root Document node plus cached indices for fast lookup.
 */
export class DocumentAST {
  /** The root Document node */
  readonly root: ASTNode;
  /** URI of the source document */
  readonly uri: string;
  /** Document version (for cache invalidation) */
  version: number;

  constructor(root: ASTNode, uri: string, version: number) {
    this.root = root;
    this.uri = uri;
    this.version = version;
  }

  // ─── Lookup Methods ─────────────────────────────────────────

  /**
   * Find the AST node at a given document offset.
   * Returns the deepest node whose range contains the offset.
   */
  findNodeAtOffset(offset: number): ASTNode | null {
    return findDeepestNode(this.root, offset);
  }

  /**
   * Find all AST nodes of a given type.
   */
  findNodesByType(nodeType: string): ASTNode[] {
    const results: ASTNode[] = [];
    walkTree(this.root, node => {
      if (node.nodeType === nodeType) {
        results.push(node);
      }
    });
    return results;
  }

  /**
   * Find the passage node that contains the given offset.
   */
  findPassageAtOffset(offset: number): ASTNode | null {
    for (const child of this.root.children) {
      if (offset >= child.range.start && offset <= child.range.end) {
        // This is a passage-level node (PassageHeader + PassageBody pair)
        return child;
      }
    }
    return null;
  }

  /**
   * Get all passage names in the document.
   */
  getPassageNames(): string[] {
    const names: string[] = [];
    walkTree(this.root, node => {
      if (node.nodeType === 'PassageHeader' && node.data.passageName) {
        names.push(node.data.passageName);
      }
    });
    return names;
  }
}

// ─── Passage Group Node ────────────────────────────────────────

/**
 * A passage group combines a PassageHeader node with its corresponding
 * PassageBody node(s). Used for convenient passage-level operations.
 */
export interface PassageGroup {
  /** The header node (:: Name [tags]) */
  header: ASTNode;
  /** The body node (content after header) */
  body: ASTNode;
  /** Combined range from header start to body end */
  range: SourceRange;
}

// ─── Visitor Pattern ───────────────────────────────────────────

/**
 * Visitor callback. Return false to skip children, true/undefined to continue.
 */
export type ASTVisitor = (node: ASTNode) => boolean | void;

/**
 * Walk the tree depth-first, calling visitor on each node.
 * If visitor returns false, children of that node are skipped.
 */
export function walkTree(root: ASTNode, visitor: ASTVisitor): void {
  const result = visitor(root);
  if (result !== false && root.children.length > 0) {
    for (const child of root.children) {
      walkTree(child, visitor);
    }
  }
}

/**
 * Walk the tree breadth-first, calling visitor on each node.
 */
export function walkTreeBreadthFirst(root: ASTNode, visitor: ASTVisitor): void {
  const queue: ASTNode[] = [root];
  while (queue.length > 0) {
    const node = queue.shift()!;
    const result = visitor(node);
    if (result !== false) {
      queue.push(...node.children);
    }
  }
}

/**
 * Find the deepest node whose range contains the given offset.
 * Used for "what's under the cursor?" queries.
 */
export function findDeepestNode(root: ASTNode, offset: number): ASTNode | null {
  // Check if offset is within this node
  if (offset < root.range.start || offset > root.range.end) {
    return null;
  }

  // Try children first (deeper = more specific)
  for (const child of root.children) {
    const found = findDeepestNode(child, offset);
    if (found) return found;
  }

  // No child contains the offset; this is the deepest node
  return root;
}

/**
 * Find the nearest ancestor of a given node type.
 */
export function findAncestor(node: ASTNode, nodeType: string): ASTNode | null {
  let current = node.parent;
  while (current) {
    if (current.nodeType === nodeType) return current;
    current = current.parent;
  }
  return null;
}

/**
 * Collect all ancestors from node to root (inclusive).
 */
export function getAncestors(node: ASTNode): ASTNode[] {
  const ancestors: ASTNode[] = [node];
  let current = node.parent;
  while (current) {
    ancestors.push(current);
    current = current.parent;
  }
  return ancestors;
}

// ─── Node Creation Helpers ─────────────────────────────────────

/**
 * Create an AST node with sensible defaults.
 */
export function createNode(
  nodeType: string,
  range: SourceRange,
  data: Partial<ASTNodeData> = {},
  children: ASTNode[] = [],
): ASTNode {
  const node: ASTNode = {
    nodeType,
    children,
    range,
    parent: null,
    data: data as ASTNodeData,
  };
  // Set parent back-links
  for (const child of children) {
    child.parent = node;
  }
  return node;
}

/**
 * Append a child node and set its parent back-link.
 */
export function appendChild(parent: ASTNode, child: ASTNode): void {
  child.parent = parent;
  parent.children.push(child);
}

/**
 * Insert a child node at a specific index and set its parent back-link.
 */
export function insertChild(parent: ASTNode, child: ASTNode, index: number): void {
  child.parent = parent;
  parent.children.splice(index, 0, child);
}

// ─── Tree Utilities ────────────────────────────────────────────

/**
 * Compute the total number of nodes in the tree.
 */
export function countNodes(root: ASTNode): number {
  let count = 0;
  walkTree(root, () => { count++; });
  return count;
}

/**
 * Compute the maximum depth of the tree.
 */
export function treeDepth(root: ASTNode): number {
  if (root.children.length === 0) return 1;
  let maxChildDepth = 0;
  for (const child of root.children) {
    maxChildDepth = Math.max(maxChildDepth, treeDepth(child));
  }
  return maxChildDepth + 1;
}

/**
 * Pretty-print the AST tree for debugging.
 */
export function printTree(root: ASTNode, indent: number = 0): string {
  const prefix = '  '.repeat(indent);
  let line = `${prefix}${root.nodeType}`;
  if (root.data.macroName) line += ` (${root.data.macroName})`;
  if (root.data.passageName) line += ` [${root.data.passageName}]`;
  if (root.data.varName) line += ` ${root.data.varSigil}${root.data.varName}`;
  if (root.data.linkTarget) line += ` -> ${root.data.linkTarget}`;
  if (root.data.text) {
    const preview = root.data.text.length > 30 ? root.data.text.slice(0, 30) + '...' : root.data.text;
    line += ` "${preview}"`;
  }
  line += ` @${root.range.start}-${root.range.end}`;

  const lines = [line];
  for (const child of root.children) {
    lines.push(printTree(child, indent + 1));
  }
  return lines.join('\n');
}
