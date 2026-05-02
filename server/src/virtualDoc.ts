import { DocumentNode, ExpressionNode, MarkupNode } from './ast';

export interface MappingEntry {
  virtualStart: number;
  originalStart: number;
  length: number;
  uri: string;
}

export interface VirtualDoc {
  content: string;
  map: MappingEntry[];
}

/**
 * Generates a virtual JS document from the AST for the TypeScript language service.
 *
 * Key invariant: SC operators (to, eq, gt, ...) are normalized to JS equivalents
 * ONLY here. They are never modified in the AST itself.
 */
export class VirtualDocGenerator {
  generate(ast: DocumentNode, uri: string): VirtualDoc {
    const parts: string[] = [];
    const map: MappingEntry[] = [];
    let offset = 0;

    const emit = (source: string, originalStart: number): void => {
      if (!source) return;
      parts.push(source);
      map.push({ virtualStart: offset, originalStart, length: source.length, uri });
      offset += source.length;
    };

    // Preamble: SugarCube runtime type stubs — mapped to offset 0 of the source
    const preamble = this.buildPreamble();
    emit(preamble, 0);

    for (const passage of ast.passages) {
      if (passage.kind === 'stylesheet') continue;

      if (passage.kind === 'script' && !Array.isArray(passage.body)) {
        // Script passages: emit body JS directly (already valid JS)
        emit(passage.body.source, passage.body.range.start);
        emit('\n', passage.body.range.end);
        continue;
      }

      if (!Array.isArray(passage.body)) continue;

      // Markup passages: extract macro expression args
      this.emitMarkupExprs(passage.body, emit);
    }

    return { content: parts.join(''), map };
  }

  mapToOriginal(doc: VirtualDoc, virtualOffset: number): number | null {
    // Binary search
    let lo = 0, hi = doc.map.length - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      const entry = doc.map[mid]!;
      if (virtualOffset < entry.virtualStart) { hi = mid - 1; }
      else if (virtualOffset >= entry.virtualStart + entry.length) { lo = mid + 1; }
      else { return entry.originalStart + (virtualOffset - entry.virtualStart); }
    }
    return null;
  }

  // Alias kept for existing callers
  mapOffsetToOriginal(doc: VirtualDoc, offset: number): number | null {
    return this.mapToOriginal(doc, offset);
  }

  private emitMarkupExprs(
    nodes: MarkupNode[],
    emit: (src: string, orig: number) => void,
  ): void {
    for (const node of nodes) {
      if (node.type === 'macro') {
        for (const arg of node.args) {
          const js = this.emitExpression(arg);
          emit(`${js};\n`, arg.range.start);
        }
        if (node.body) this.emitMarkupExprs(node.body, emit);
      }
    }
  }

  emitExpression(expr: ExpressionNode): string {
    switch (expr.type) {
      case 'storyVar':       return `State.variables.${expr.name}`;
      case 'tempVar':        return `temporary.${expr.name}`;
      case 'identifier':     return expr.name;
      case 'literal':        return expr.kind === 'string' ? JSON.stringify(expr.value) : String(expr.value);
      case 'binaryOp':
        return `${this.emitExpression(expr.left)} ${this.normalizeOp(expr.operator)} ${this.emitExpression(expr.right)}`;
      case 'unaryOp':
        return `${this.normalizeOp(expr.operator)}${this.emitExpression(expr.operand)}`;
      case 'propertyAccess': return `${this.emitExpression(expr.object)}.${expr.property}`;
      case 'indexAccess':    return `${this.emitExpression(expr.object)}[${this.emitExpression(expr.index)}]`;
      case 'call':
        return `${this.emitExpression(expr.callee)}(${expr.args.map(a => this.emitExpression(a)).join(', ')})`;
      case 'arrayLiteral':
        return `[${expr.elements.map(el => this.emitExpression(el)).join(', ')}]`;
      case 'objectLiteral':
        return `{ ${expr.properties.map(p => `${JSON.stringify(p.key)}: ${this.emitExpression(p.value)}`).join(', ')} }`;
      case 'conditional':
        return `(${this.emitExpression(expr.condition)} ? ${this.emitExpression(expr.consequent)} : ${this.emitExpression(expr.alternate)})`;
      default:
        return 'undefined';
    }
  }

  // SC operators normalized to JS — ONLY in this file
  private normalizeOp(op: string): string {
    const table: Record<string, string> = {
      to: '=', eq: '===', neq: '!==', is: '===', isnot: '!==',
      gt: '>', gte: '>=', lt: '<', lte: '<=',
      and: '&&', or: '||', not: '!',
    };
    return table[op] ?? op;
  }

  private buildPreamble(): string {
    return [
      '/** @type {{ [key: string]: any }} */',
      'const State = { variables: {}, temporary: {} };',
      'const temporary = State.temporary;',
      'const Story = {};',
      'const Engine = {};',
      'const Dialog = {};',
      'const Macro = { add: () => {} };',
      '',
    ].join('\n');
  }
}