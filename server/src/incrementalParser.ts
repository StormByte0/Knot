import { DocumentNode, ParseDiagnostic, ParseOutput, PassageNode } from './ast';
import { PassageSpan, extractPassageSpans, parsePassage } from './parser';

interface CachedPassage {
  node: PassageNode;
  bodyText: string;
  tagsKey: string;
}

/**
 * Passage-granularity incremental parser.
 * Parse cache and analysis cache are intentionally separate:
 *   - Parse cache: invalidated when passage body text changes
 *   - Analysis cache: invalidated when body changes OR a referenced symbol changes
 */
export class IncrementalParser {
  // key: "uri:passageName"
  private cache = new Map<string, CachedPassage>();

  parse(uri: string, text: string): ParseOutput {
    const diagnostics: ParseDiagnostic[] = [];
    const spans = extractPassageSpans(text);
    const passages: PassageNode[] = [];
    const seenKeys = new Set<string>();

    for (const span of spans) {
      const key = `${uri}:${span.name}:${span.bodyStart}`;
      seenKeys.add(key);

      const bodyText = text.slice(span.bodyStart, span.bodyEnd);
      const tagsKey = span.tags.join('|');
      const cached = this.cache.get(key);

      if (cached && cached.bodyText === bodyText && cached.tagsKey === tagsKey
          && cached.node.range.start === span.nameStart) {
        // Reuse parse tree, shift positions if the passage moved in the file
        passages.push(this.shiftIfNeeded(cached.node, span));
        continue;
      }

      const node = parsePassage(text, span, diagnostics);
      this.cache.set(key, { node, bodyText, tagsKey });
      passages.push(node);
    }

    // Evict stale entries for this URI
    for (const key of this.cache.keys()) {
      if (key.startsWith(`${uri}:`) && !seenKeys.has(key)) {
        this.cache.delete(key);
      }
    }

    const ast: DocumentNode = {
      type: 'document',
      range: { start: 0, end: text.length },
      passages,
    };

    return { ast, diagnostics };
  }

  private shiftIfNeeded(node: PassageNode, span: PassageSpan): PassageNode {
    const delta = span.nameStart - node.nameRange.start;
    if (delta === 0) return node;
    // Deep clone and shift all ranges — use structured clone instead of
    // JSON round-trip for better performance on large ASTs.
    const clone = structuredClone(node) as PassageNode;
    shiftRanges(clone as unknown as Record<string, unknown>, delta);
    return clone;
  }
}

function shiftRanges(obj: Record<string, unknown>, delta: number): void {
  if (
    'start' in obj && 'end' in obj &&
    typeof obj['start'] === 'number' && typeof obj['end'] === 'number'
  ) {
    obj['start'] = obj['start'] + delta;
    obj['end'] = obj['end'] + delta;
  }
  for (const val of Object.values(obj)) {
    if (!val || typeof val !== 'object') continue;
    if (Array.isArray(val)) {
      for (const el of val) {
        if (el && typeof el === 'object') shiftRanges(el as Record<string, unknown>, delta);
      }
    } else {
      shiftRanges(val as Record<string, unknown>, delta);
    }
  }
}
