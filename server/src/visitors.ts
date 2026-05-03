import { ExpressionNode, LinkNode, MacroNode, MarkupNode, DocumentNode, TextNode, CommentNode } from './ast';

// ---------------------------------------------------------------------------
// MacroVisitor — unified markup tree walker
// ---------------------------------------------------------------------------

/** Callback types for MacroVisitor. Return `false` from any callback to stop walking. */
export interface MacroVisitorCallbacks {
  /** Called for every MacroNode encountered. Receives the parent stack for context. */
  onMacro?: (node: MacroNode, parentStack: MarkupNode[]) => void | false;
  /** Called for every LinkNode encountered. */
  onLink?: (node: LinkNode) => void | false;
  /** Called for every text node. */
  onText?: (node: TextNode) => void | false;
  /** Called for every comment node. */
  onComment?: (node: CommentNode) => void | false;
}

/**
 * Walks a MarkupNode tree depth-first, invoking callbacks.
 * Replaces all the inline walkNodes() functions scattered across the codebase.
 *
 * If any callback returns `false`, the walk stops immediately (early termination).
 * The `parentStack` parameter in `onMacro` tracks the chain of ancestor MarkupNodes
 * (useful for structural validation like "elseif must be inside if").
 */
export function walkMarkup(nodes: MarkupNode[], callbacks: MacroVisitorCallbacks, parentStack?: MarkupNode[]): void {
  const stack = parentStack ?? [];
  for (const node of nodes) {
    switch (node.type) {
      case 'macro': {
        const result = callbacks.onMacro?.(node, stack);
        if (result === false) return;
        if (node.body) {
          stack.push(node);
          walkMarkup(node.body, callbacks, stack);
          stack.pop();
        }
        break;
      }
      case 'link': {
        const result = callbacks.onLink?.(node);
        if (result === false) return;
        break;
      }
      case 'text': {
        const result = callbacks.onText?.(node);
        if (result === false) return;
        break;
      }
      case 'comment': {
        const result = callbacks.onComment?.(node);
        if (result === false) return;
        break;
      }
    }
  }
}

/**
 * Walks all passages in a DocumentNode's markup bodies.
 * Convenience wrapper that iterates passages and skips non-markup bodies.
 */
export function walkDocument(ast: DocumentNode, callbacks: MacroVisitorCallbacks): void {
  for (const passage of ast.passages) {
    if (Array.isArray(passage.body)) {
      walkMarkup(passage.body, callbacks);
    }
  }
}

// ---------------------------------------------------------------------------
// ExpressionVisitor — unified expression tree walker
// ---------------------------------------------------------------------------

/** Callback type for ExpressionVisitor */
export type ExprVisitor = (expr: ExpressionNode) => void;

/**
 * Walks an ExpressionNode tree depth-first, invoking the visitor callback
 * on every node (pre-order traversal).
 * Replaces all the inline walkExpr/collectExprSymbols/_walkExprVars functions.
 */
export function walkExpression(expr: ExpressionNode, visitor: ExprVisitor): void {
  visitor(expr);
  switch (expr.type) {
    case 'binaryOp':
      walkExpression(expr.left, visitor);
      walkExpression(expr.right, visitor);
      break;
    case 'unaryOp':
      walkExpression(expr.operand, visitor);
      break;
    case 'propertyAccess':
      walkExpression(expr.object, visitor);
      break;
    case 'indexAccess':
      walkExpression(expr.object, visitor);
      walkExpression(expr.index, visitor);
      break;
    case 'call':
      walkExpression(expr.callee, visitor);
      for (const a of expr.args) walkExpression(a, visitor);
      break;
    case 'arrayLiteral':
      for (const el of expr.elements) walkExpression(el, visitor);
      break;
    case 'objectLiteral':
      for (const p of expr.properties) walkExpression(p.value, visitor);
      break;
    // storyVar, tempVar, identifier, literal — leaf nodes, already visited
  }
}

/**
 * Walks all expression arguments of all macros in a document.
 * Convenience combinator that handles the common pattern of:
 *   for each passage → for each macro → for each arg → walk expression
 */
export function walkDocumentExpressions(ast: DocumentNode, visitor: ExprVisitor): void {
  walkDocument(ast, {
    onMacro(node) {
      for (const arg of node.args) {
        walkExpression(arg, visitor);
      }
    },
  });
}
