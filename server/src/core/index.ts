/**
 * Knot v2 — Core Re-exports
 *
 * CRITICAL: Nothing in this directory may import from formats/.
 * All format data must flow through FormatRegistry + FormatModule.
 */

// ── Existing modules ───────────────────────────────────────────
export { WorkspaceIndex, PassageEntry, IndexChangeEvent, IndexChangeListener } from './workspaceIndex';
export { Parser, RawPassage } from './parser';
export { DocumentStore } from './documentStore';
export { IncrementalParser, PassageDelta, RawPassageRef } from './incrementalParser';
export { ReferenceIndex, ReferenceEntry } from './referenceIndex';
export { LinkGraph, LinkEdge } from './linkGraph';
export { SymbolTable, SymbolKind, SymbolEntry } from './symbolTable';
export { DiagnosticEngine, DiagnosticSettings } from './diagnosticEngine';

// ── New: AST & Analysis ────────────────────────────────────────
export {
  ASTNode,
  ASTNodeData,
  DocumentAST,
  PassageGroup,
  ASTVisitor,
  walkTree,
  walkTreeBreadthFirst,
  findDeepestNode,
  findAncestor,
  getAncestors,
  createNode,
  appendChild,
  insertChild,
  countNodes,
  treeDepth,
  printTree,
} from './ast';

export {
  ASTBuilder,
  BuildResult,
  BuilderWarning,
} from './astBuilder';

export {
  SyntaxAnalyzer,
  SyntaxAnalysisResult,
} from './syntaxAnalyzer';

export {
  SemanticAnalyzer,
  SemanticAnalysisResult,
  VariableDef,
  CustomMacroDef,
} from './semanticAnalyzer';

export {
  VirtualDocProvider,
  VirtualDocument,
  VirtualDocExtraction,
  VIRTUAL_DOC_SCHEME,
  buildVirtualUri,
  parseVirtualUri,
} from './virtualDocs';

export {
  ASTWorkspace,
  DocumentAnalysis,
} from './astWorkspace';

// ── New: Control Flow Graph ──────────────────────────────────────
export {
  CFGBuilder,
  PassageCFG,
  BasicBlock,
  CFGEdge,
  BlockKind,
  EdgeKind,
  NavigationEdge,
  VariableStateMap,
  AbstractValue,
  isBlockReachable,
  getReachableNavigationEdges,
} from './cfg';

export {
  StoryFlowGraphBuilder,
  StoryFlowAnalysis,
  StoryFlowGraph,
  StoryFlowNode,
  StoryFlowEdge,
  DeadCondition,
} from './storyFlowGraph';
