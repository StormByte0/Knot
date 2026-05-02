import * as acorn from 'acorn';
import * as walk from 'acorn-walk';
import { DocumentNode, ExpressionNode } from './ast';
import { SourceRange } from './tokenTypes';

export interface InferredType {
  kind: 'unknown' | 'number' | 'string' | 'boolean' | 'null' | 'object' | 'array';
  properties?: Record<string, InferredType>; // for objects — only level-1 keys stored
  elements?: InferredType;                   // for arrays  — only level-1 element type
}

export interface InferenceResult {
  // varName (without $) → inferred type from first assignment
  assignments: Map<string, InferredType>;
  // JS var/const/let names declared in inline <<script>> blocks or Story JavaScript
  jsGlobals: Map<string, { inferredType: InferredType; range: SourceRange }>;
}

export class TypeInference {
  inferDocument(ast: DocumentNode): InferenceResult {
    const assignments = new Map<string, InferredType>();
    const jsGlobals   = new Map<string, { inferredType: InferredType; range: SourceRange }>();

    // Analyse StoryInit first, then everything else
    const sorted = [...ast.passages].sort((a, b) => priority(a.name) - priority(b.name));

    for (const passage of sorted) {
      // Markup passages: collect <<set>> assignments and inline <<script>> bodies
      if (Array.isArray(passage.body)) {
        this.collectSetAssignments(passage.body, assignments);
        this.collectInlineScriptGlobals(passage.body, jsGlobals);
      }

      // Script passages (Story JavaScript or [script] tag): collect JS globals
      if (!Array.isArray(passage.body) && passage.body.type === 'scriptBody') {
        collectJsGlobals(passage.body.source, passage.body.range.start, jsGlobals);
      }
    }

    return { assignments, jsGlobals };
  }

  private collectSetAssignments(
    nodes: import('./ast').MarkupNode[],
    assignments: Map<string, InferredType>,
  ): void {
    for (const node of nodes) {
      if (node.type === 'macro' && node.name === 'set') {
        const arg = node.args[0];
        if (arg?.type === 'binaryOp' && (arg.operator === 'to' || arg.operator === '=')) {
          const varName = extractVarName(arg.left);
          if (varName && !assignments.has(varName)) {
            assignments.set(varName, this.infer(arg.right));
          }
        }
      }
      if (node.type === 'macro' && node.body) {
        this.collectSetAssignments(node.body, assignments);
      }
    }
  }

  /** Walk markup nodes looking for <<script>>…<</script>> macro bodies and harvest JS globals. */
  private collectInlineScriptGlobals(
    nodes: import('./ast').MarkupNode[],
    jsGlobals: Map<string, { inferredType: InferredType; range: SourceRange }>,
  ): void {
    for (const node of nodes) {
      if (node.type !== 'macro') continue;
      if (node.name === 'script' && node.body) {
        // The body is MarkupNode[] containing text nodes — reconstruct source
        const src = node.body
          .filter(n => n.type === 'text')
          .map(n => (n as import('./ast').TextNode).value)
          .join('');
        const baseOff = node.body.length > 0 ? node.body[0]!.range.start : 0;
        if (src.trim()) collectJsGlobals(src, baseOff, jsGlobals);
      } else if (node.body) {
        this.collectInlineScriptGlobals(node.body, jsGlobals);
      }
    }
  }

  infer(expr: ExpressionNode): InferredType {
    switch (expr.type) {
      case 'literal':
        if (expr.kind === 'number')  return { kind: 'number' };
        if (expr.kind === 'string')  return { kind: 'string' };
        if (expr.kind === 'boolean') return { kind: 'boolean' };
        if (expr.kind === 'null')    return { kind: 'null' };
        return { kind: 'unknown' };

      case 'arrayLiteral': {
        const first = expr.elements[0];
        return { kind: 'array', elements: first ? this.infer(first) : { kind: 'unknown' } };
      }

      case 'objectLiteral': {
        // Store the FULL nested tree — display layer decides how deep to show
        const props: Record<string, InferredType> = {};
        for (const p of expr.properties) {
          props[p.key] = this.infer(p.value);
        }
        return { kind: 'object', properties: props };
      }

      case 'binaryOp':
        if (expr.operator === 'to' || expr.operator === '=') return this.infer(expr.right);
        return { kind: 'unknown' };

      default:
        return { kind: 'unknown' };
    }
  }
}

/** Parse a JS source string with acorn and collect top-level var/let/const/function names. */
function collectJsGlobals(source: string, baseOffset: number, out: Map<string, { inferredType: InferredType; range: SourceRange }>): void {
  try {
    const program = acorn.parse(source, { ecmaVersion: 'latest' });
    walk.simple(program as never, {
      VariableDeclaration(node: any) {
        for (const decl of node.declarations ?? []) {
          if (decl.id?.type === 'Identifier') {
            const name: string = decl.id.name;
            if (!out.has(name)) {
              out.set(name, {
                inferredType: inferJsInit(decl.init),
                range: { start: baseOffset + decl.id.start, end: baseOffset + decl.id.end },
              });
            }
          }
        }
      },
      FunctionDeclaration(node: any) {
        if (node.id?.name && !out.has(node.id.name)) {
          out.set(node.id.name, {
            inferredType: { kind: 'unknown' },
            range: { start: baseOffset + node.id.start, end: baseOffset + node.id.end },
          });
        }
      },
    });
  } catch {
    // Incomplete JS while typing — skip
  }
}

/** Best-effort type inference from a raw acorn init node. */
function inferJsInit(init: any): InferredType {
  if (!init) return { kind: 'unknown' };
  if (init.type === 'Literal') {
    if (typeof init.value === 'number')  return { kind: 'number' };
    if (typeof init.value === 'string')  return { kind: 'string' };
    if (typeof init.value === 'boolean') return { kind: 'boolean' };
    if (init.value === null)             return { kind: 'null' };
  }
  if (init.type === 'ArrayExpression') {
    const first = init.elements?.[0];
    return { kind: 'array', elements: first ? inferJsInit(first) : { kind: 'unknown' } };
  }
  if (init.type === 'ObjectExpression') {
    const props: Record<string, InferredType> = {};
    for (const p of (init.properties ?? [])) {
      const key = p.key?.name ?? p.key?.value;
      if (key) props[String(key)] = inferJsInit(p.value); // recurse fully
    }
    return { kind: 'object', properties: props };
  }
  return { kind: 'unknown' };
}

function extractVarName(expr: ExpressionNode): string | null {
  if (expr.type === 'storyVar')       return expr.name;
  if (expr.type === 'propertyAccess') return extractVarName(expr.object);
  if (expr.type === 'indexAccess')    return extractVarName(expr.object);
  return null;
}

function priority(name: string): number {
  if (name === 'StoryInit')        return 0;
  if (name === 'Story JavaScript') return 1;
  return 2;
}