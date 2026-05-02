import { SourceRange } from './tokenTypes';

export interface BaseNode {
  range: SourceRange;
}

export interface DocumentNode extends BaseNode {
  type: 'document';
  passages: PassageNode[];
}

export type PassageKind = 'markup' | 'script' | 'stylesheet' | 'special';

export interface PassageNode extends BaseNode {
  type: 'passage';
  name: string;
  nameRange: SourceRange;
  tags: string[];
  kind: PassageKind;
  body: MarkupNode[] | ScriptBodyNode | StyleBodyNode;
}

export type MarkupNode = TextNode | MacroNode | LinkNode | CommentNode;

export interface TextNode extends BaseNode {
  type: 'text';
  value: string;
}

export interface MacroNode extends BaseNode {
  type: 'macro';
  name: string;
  nameRange: SourceRange;
  closeNameRange?: SourceRange;   // range of the name in <</name>>, if hasBody
  args: ExpressionNode[];
  hasBody: boolean;
  body: MarkupNode[] | null;
}

export interface LinkNode extends BaseNode {
  type: 'link';
  target: string;
  targetRange: SourceRange;
  display: string | null;
}

export interface CommentNode extends BaseNode {
  type: 'comment';
  style: 'html' | 'block' | 'line';
  value: string;
}

export interface ScriptBodyNode extends BaseNode {
  type: 'scriptBody';
  source: string;
}

export interface StyleBodyNode extends BaseNode {
  type: 'styleBody';
  source: string;
}

export type ExpressionNode =
  | LiteralNode
  | StoryVarNode
  | TempVarNode
  | IdentifierNode
  | BinaryOpNode
  | UnaryOpNode
  | PropertyAccessNode
  | IndexAccessNode
  | CallNode
  | ArrayLiteralNode
  | ObjectLiteralNode
  | ConditionalNode;

export interface LiteralNode extends BaseNode {
  type: 'literal';
  kind: 'string' | 'number' | 'boolean' | 'null' | 'undefined';
  value: string | number | boolean | null | undefined;
}

export interface StoryVarNode extends BaseNode {
  type: 'storyVar';
  name: string; // without $ prefix
}

export interface TempVarNode extends BaseNode {
  type: 'tempVar';
  name: string; // without _ prefix
}

export interface IdentifierNode extends BaseNode {
  type: 'identifier';
  name: string;
}

export interface BinaryOpNode extends BaseNode {
  type: 'binaryOp';
  // SC operators preserved verbatim: 'to', 'eq', 'gt', etc.
  // Only normalized in virtual doc generation
  operator: string;
  left: ExpressionNode;
  right: ExpressionNode;
}

export interface UnaryOpNode extends BaseNode {
  type: 'unaryOp';
  operator: string; // '-' '!' 'not'
  operand: ExpressionNode;
}

export interface PropertyAccessNode extends BaseNode {
  type: 'propertyAccess';
  object: ExpressionNode;
  property: string;
  propertyRange: SourceRange;
}

export interface IndexAccessNode extends BaseNode {
  type: 'indexAccess';
  object: ExpressionNode;
  index: ExpressionNode;
}

export interface CallNode extends BaseNode {
  type: 'call';
  callee: ExpressionNode;
  args: ExpressionNode[];
}

export interface ArrayLiteralNode extends BaseNode {
  type: 'arrayLiteral';
  elements: ExpressionNode[];
}

export interface ObjectLiteralNode extends BaseNode {
  type: 'objectLiteral';
  properties: ObjectProperty[];
}

export interface ObjectProperty extends BaseNode {
  key: string;
  value: ExpressionNode;
}

export interface ConditionalNode extends BaseNode {
  type: 'conditional';
  condition: ExpressionNode;
  consequent: ExpressionNode;
  alternate: ExpressionNode;
}

export interface ParseDiagnostic {
  message: string;
  range: SourceRange;
  severity?: 'error' | 'warning';
}

export interface ParseOutput {
  ast: DocumentNode;
  diagnostics: ParseDiagnostic[];
}
