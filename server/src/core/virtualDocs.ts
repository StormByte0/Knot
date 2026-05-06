/**
 * Knot v2 — Virtual Documents
 *
 * Creates virtual documents for embedded language regions within Twee files,
 * enabling VS Code's built-in language features (JS, CSS, HTML) to work
 * inside script passages, stylesheet passages, and template blocks.
 *
 * Virtual documents work by:
 *   1. Extracting embedded language regions from the Twee document
 *   2. Generating virtual document URIs (e.g. tweedoc:///path/file.twee/script/0)
 *   3. Providing document content for those virtual URIs on demand
 *   4. Mapping diagnostics from virtual documents back to the original Twee file
 *
 * Supported embedded languages:
 *   - [script] passages → JavaScript
 *   - [stylesheet] passages → CSS
 *   - Snowman <% %> blocks → JavaScript
 *   - SugarCube <<script>> macro bodies → JavaScript
 *   - SugarCube <<style>> macro bodies → CSS
 *
 * MUST NOT import from: formats/ (use FormatRegistry instead)
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, SourceRange } from '../formats/_types';
import { PassageType } from '../hooks/hookTypes';
import { ASTNode, DocumentAST, PassageGroup, walkTree } from './ast';
import { Parser, RawPassage } from './parser';

// ─── Public Types ──────────────────────────────────────────────

/**
 * A virtual document representing an embedded language region.
 */
export interface VirtualDocument {
  /** Virtual URI for this document (e.g. tweedoc:///path/file.twee/script/0) */
  readonly uri: string;
  /** The language ID (javascript, css, html) */
  readonly languageId: string;
  /** The content of the virtual document */
  readonly content: string;
  /** The source range in the original Twee document */
  readonly sourceRange: SourceRange;
  /** The original Twee document URI */
  readonly sourceUri: string;
  /** Version number for cache invalidation */
  version: number;
}

/**
 * Result of extracting virtual documents from a Twee file.
 */
export interface VirtualDocExtraction {
  /** All extracted virtual documents */
  readonly documents: VirtualDocument[];
  /** Source range → virtual URI mapping (for diagnostic remapping) */
  readonly rangeToUri: Map<SourceRange, string>;
}

// ─── Virtual URI Scheme ────────────────────────────────────────

/** The URI scheme used for virtual documents */
export const VIRTUAL_DOC_SCHEME = 'tweedoc';

/**
 * Build a virtual document URI.
 * Format: tweedoc:///original/path.twee/language/index
 */
export function buildVirtualUri(
  sourceUri: string,
  languageId: string,
  index: number,
): string {
  return `${VIRTUAL_DOC_SCHEME}://${sourceUri}/${languageId}/${index}`;
}

/**
 * Parse a virtual document URI back into its components.
 * Returns null if the URI is not a valid virtual document URI.
 */
export function parseVirtualUri(uri: string): { sourceUri: string; languageId: string; index: number } | null {
  if (!uri.startsWith(`${VIRTUAL_DOC_SCHEME}://`)) return null;

  const withoutScheme = uri.slice(`${VIRTUAL_DOC_SCHEME}://`.length);
  const lastSlash = withoutScheme.lastIndexOf('/');
  if (lastSlash < 0) return null;

  const indexStr = withoutScheme.slice(lastSlash + 1);
  const index = parseInt(indexStr, 10);
  if (isNaN(index)) return null;

  const remaining = withoutScheme.slice(0, lastSlash);
  const secondLastSlash = remaining.lastIndexOf('/');
  if (secondLastSlash < 0) return null;

  const languageId = remaining.slice(secondLastSlash + 1);
  const sourceUri = remaining.slice(0, secondLastSlash);

  return { sourceUri, languageId, index };
}

// ─── Virtual Document Provider ─────────────────────────────────

export class VirtualDocProvider {
  private formatRegistry: FormatRegistry;
  private parser: Parser;
  /** Cache: source URI → extraction result */
  private cache: Map<string, VirtualDocExtraction> = new Map();

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
    this.parser = new Parser(formatRegistry);
  }

  /**
   * Extract virtual documents from a Twee file.
   * Caches results by source URI + version.
   */
  extract(
    content: string,
    sourceUri: string,
    version: number,
    passages: PassageGroup[],
  ): VirtualDocExtraction {
    // Check cache
    const cached = this.cache.get(sourceUri);
    if (cached && cached.documents.length > 0 && cached.documents[0].version === version) {
      return cached;
    }

    const format = this.formatRegistry.getActiveFormat();
    const documents: VirtualDocument[] = [];
    const rangeToUri = new Map<SourceRange, string>();

    let scriptIndex = 0;
    let styleIndex = 0;

    for (const passage of passages) {
      const passageType = passage.header.data.passageType;

      // Script passages → JavaScript virtual doc
      if (passageType === PassageType.Script) {
        const bodyContent = this.extractBodyContent(passage.body);
        if (bodyContent.text.trim()) {
          const vUri = buildVirtualUri(sourceUri, 'javascript', scriptIndex++);
          const vDoc: VirtualDocument = {
            uri: vUri,
            languageId: 'javascript',
            content: bodyContent.text,
            sourceRange: bodyContent.range,
            sourceUri,
            version,
          };
          documents.push(vDoc);
          rangeToUri.set(bodyContent.range, vUri);
        }
      }

      // Stylesheet passages → CSS virtual doc
      if (passageType === PassageType.Stylesheet) {
        const bodyContent = this.extractBodyContent(passage.body);
        if (bodyContent.text.trim()) {
          const vUri = buildVirtualUri(sourceUri, 'css', styleIndex++);
          const vDoc: VirtualDocument = {
            uri: vUri,
            languageId: 'css',
            content: bodyContent.text,
            sourceRange: bodyContent.range,
            sourceUri,
            version,
          };
          documents.push(vDoc);
          rangeToUri.set(bodyContent.range, vUri);
        }
      }

      // Format-specific embedded regions (<<script>>, <<style>>, <% %>)
      this.extractFormatEmbeddedDocs(passage, format, sourceUri, version, documents, rangeToUri);
    }

    const result: VirtualDocExtraction = { documents, rangeToUri };
    this.cache.set(sourceUri, result);
    return result;
  }

  /**
   * Get a cached virtual document by URI.
   */
  getVirtualDocument(uri: string): VirtualDocument | undefined {
    const parsed = parseVirtualUri(uri);
    if (!parsed) return undefined;

    const cached = this.cache.get(parsed.sourceUri);
    if (!cached) return undefined;

    return cached.documents.find(
      d => d.languageId === parsed.languageId &&
           d.uri === uri,
    );
  }

  /**
   * Map a diagnostic from a virtual document back to the source Twee document.
   * Returns the adjusted SourceRange in the original document, or null if
   * the diagnostic can't be mapped.
   */
  mapDiagnosticToSource(
    virtualUri: string,
    virtualRange: SourceRange,
  ): SourceRange | null {
    const parsed = parseVirtualUri(virtualUri);
    if (!parsed) return null;

    const cached = this.cache.get(parsed.sourceUri);
    if (!cached) return null;

    const vDoc = cached.documents.find(d => d.uri === virtualUri);
    if (!vDoc) return null;

    // Offset the virtual range by the source range start
    return {
      start: vDoc.sourceRange.start + virtualRange.start,
      end: vDoc.sourceRange.start + virtualRange.end,
    };
  }

  /**
   * Invalidate cache for a given source URI.
   */
  invalidate(sourceUri: string): void {
    this.cache.delete(sourceUri);
  }

  /**
   * Clear all cached virtual documents.
   */
  clear(): void {
    this.cache.clear();
  }

  // ─── Private Helpers ────────────────────────────────────────

  /**
   * Extract the text content from a body node.
   */
  private extractBodyContent(bodyNode: ASTNode): { text: string; range: SourceRange } {
    // Concatenate all Text children
    const parts: string[] = [];
    let start = bodyNode.range.start;
    let end = bodyNode.range.end;

    walkTree(bodyNode, node => {
      if (node.nodeType === 'Text' && node.data.text) {
        parts.push(node.data.text);
      }
    });

    return {
      text: parts.join(''),
      range: { start, end },
    };
  }

  /**
   * Extract format-specific embedded documents.
   *
   * SugarCube: <<script>>..body..<</script>> and <<style>>..body..<</style>>
   * Snowman: <% code %> and <%= expression %>
   */
  private extractFormatEmbeddedDocs(
    passage: PassageGroup,
    format: FormatModule,
    sourceUri: string,
    version: number,
    documents: VirtualDocument[],
    rangeToUri: Map<SourceRange, string>,
  ): void {
    let scriptIndex = documents.filter(d => d.languageId === 'javascript').length;
    let styleIndex = documents.filter(d => d.languageId === 'css').length;

    walkTree(passage.body, node => {
      // SugarCube <<script>> macro body → JavaScript
      if (node.nodeType === 'MacroCall' && node.data.macroName === 'script') {
        // The children of the <<script>> macro are the script content
        const scriptContent = this.extractChildText(node);
        if (scriptContent.text.trim()) {
          const vUri = buildVirtualUri(sourceUri, 'javascript', scriptIndex++);
          const vDoc: VirtualDocument = {
            uri: vUri,
            languageId: 'javascript',
            content: scriptContent.text,
            sourceRange: scriptContent.range,
            sourceUri,
            version,
          };
          documents.push(vDoc);
          rangeToUri.set(scriptContent.range, vUri);
        }
      }

      // SugarCube <<style>> macro body → CSS
      if (node.nodeType === 'MacroCall' && node.data.macroName === 'style') {
        const styleContent = this.extractChildText(node);
        if (styleContent.text.trim()) {
          const vUri = buildVirtualUri(sourceUri, 'css', styleIndex++);
          const vDoc: VirtualDocument = {
            uri: vUri,
            languageId: 'css',
            content: styleContent.text,
            sourceRange: styleContent.range,
            sourceUri,
            version,
          };
          documents.push(vDoc);
          rangeToUri.set(styleContent.range, vUri);
        }
      }

      // Snowman template blocks → JavaScript
      if (node.nodeType === 'TemplateBlock') {
        const templateText = node.data.text ?? '';
        if (templateText.trim()) {
          const vUri = buildVirtualUri(sourceUri, 'javascript', scriptIndex++);
          const vDoc: VirtualDocument = {
            uri: vUri,
            languageId: 'javascript',
            content: templateText,
            sourceRange: node.range,
            sourceUri,
            version,
          };
          documents.push(vDoc);
          rangeToUri.set(node.range, vUri);
        }
      }
    });
  }

  /**
   * Extract concatenated text from a node's direct Text children.
   */
  private extractChildText(node: ASTNode): { text: string; range: SourceRange } {
    const parts: string[] = [];
    let start = node.range.end;  // Will be adjusted
    let end = node.range.start;

    for (const child of node.children) {
      if (child.nodeType === 'Text' && child.data.text) {
        parts.push(child.data.text);
        start = Math.min(start, child.range.start);
        end = Math.max(end, child.range.end);
      }
    }

    return {
      text: parts.join(''),
      range: { start: start === node.range.end ? node.range.start : start, end: end === node.range.start ? node.range.end : end },
    };
  }
}
