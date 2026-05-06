/**
 * Knot v2 — AST Builder
 *
 * Converts flat BodyToken[] + RawPassage[] from the Parser into a proper
 * AST tree using format-declared astNodeTypes.
 *
 * The builder is format-agnostic in implementation but format-aware in
 * strategy. It uses `format.macroBodyStyle` to select the nesting algorithm:
 *
 *   CloseTag (SugarCube): <<if>> opens, <</if>> closes. Stack-based matching.
 *   Hook (Harlowe):       (macro:) opens, [...] closes. Hook brackets are bodies.
 *   Inline (Chapbook/etc): No macro nesting. Flat token sequence.
 *
 * CRITICAL: Core NEVER hardcodes format-specific logic. The builder reads
 * macroBodyStyle, macroDelimiters, and MacroDef flags from the active format
 * module through FormatRegistry.
 *
 * MUST NOT import from: formats/ (use FormatRegistry instead)
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, BodyToken, PassageRef, LinkResolution, SourceRange } from '../formats/_types';
import { MacroBodyStyle, PassageType, PassageRefKind } from '../hooks/hookTypes';
import { Parser, RawPassage } from './parser';
import {
  ASTNode,
  ASTNodeData,
  DocumentAST,
  PassageGroup,
  createNode,
  appendChild,
  walkTree,
} from './ast';

// ─── Public Types ──────────────────────────────────────────────

export interface BuildResult {
  /** The built document AST */
  ast: DocumentAST;
  /** Passage groups extracted during build */
  passages: PassageGroup[];
  /** Warnings encountered during construction (not diagnostics — just builder issues) */
  warnings: BuilderWarning[];
}

export interface BuilderWarning {
  readonly message: string;
  readonly range: SourceRange;
  readonly kind: 'unclosed-macro' | 'mismatched-close' | 'orphan-close' | 'unclosed-hook';
}

// ─── AST Builder ───────────────────────────────────────────────

export class ASTBuilder {
  private formatRegistry: FormatRegistry;
  private parser: Parser;

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
    this.parser = new Parser(formatRegistry);
  }

  /**
   * Build a complete document AST from raw Twee source text.
   */
  build(content: string, uri: string, version: number): BuildResult {
    const format = this.formatRegistry.getActiveFormat();
    const warnings: BuilderWarning[] = [];

    // Step 1: Parse document into raw passages
    const rawPassages = this.parser.parseDocument(content);

    // Step 2: Create the root Document node
    const documentRange: SourceRange = { start: 0, end: content.length };
    const documentNode = createNode('Document', documentRange);

    // Step 3: Build passage groups
    const passages: PassageGroup[] = [];

    for (const raw of rawPassages) {
      const passageType = this.parser.classifyPassageType(raw);
      const customTypeId = passageType === PassageType.Custom
        ? this.parser.getCustomTypeId(raw)
        : undefined;

      // Create PassageHeader node
      const headerRange: SourceRange = {
        start: raw.startOffset - (raw.name.length + raw.tags.join(' ').length + 4), // approximate
        end: raw.startOffset,
      };
      // More precise: find the actual header line in content
      const actualHeaderRange = this.findHeaderRange(content, raw);
      const headerNode = createNode('PassageHeader', actualHeaderRange, {
        passageName: raw.name,
        passageTags: raw.tags,
        passageType,
        customTypeId,
      });

      // Create PassageBody node
      const bodyRange: SourceRange = { start: raw.startOffset, end: raw.endOffset };
      const bodyNode = createNode('PassageBody', bodyRange);

      // Build body subtree from tokens
      this.buildBodySubtree(bodyNode, raw.bodyTokens, raw.passageRefs, format, warnings);

      // Create a passage container node that holds header + body
      const passageRange: SourceRange = {
        start: actualHeaderRange.start,
        end: raw.endOffset,
      };
      const passageNode = createNode('Passage', passageRange, {}, [headerNode, bodyNode]);

      appendChild(documentNode, passageNode);

      passages.push({
        header: headerNode,
        body: bodyNode,
        range: passageRange,
      });
    }

    const ast = new DocumentAST(documentNode, uri, version);

    return { ast, passages, warnings };
  }

  /**
   * Rebuild just a single passage's body AST (for incremental updates).
   */
  rebuildPassageBody(
    body: string,
    bodyOffset: number,
    format: FormatModule,
  ): { bodyNode: ASTNode; warnings: BuilderWarning[] } {
    const warnings: BuilderWarning[] = [];
    const bodyTokens = format.lexBody(body, bodyOffset);
    const passageRefs = format.extractPassageRefs(body, bodyOffset);

    const bodyRange: SourceRange = { start: bodyOffset, end: bodyOffset + body.length };
    const bodyNode = createNode('PassageBody', bodyRange);
    this.buildBodySubtree(bodyNode, bodyTokens, passageRefs, format, warnings);

    return { bodyNode, warnings };
  }

  // ─── Body Subtree Construction ──────────────────────────────

  /**
   * Build the body subtree from flat tokens and passage references.
   *
   * Dispatches to the appropriate nesting strategy based on macroBodyStyle.
   */
  private buildBodySubtree(
    bodyNode: ASTNode,
    tokens: BodyToken[],
    passageRefs: PassageRef[],
    format: FormatModule,
    warnings: BuilderWarning[],
  ): void {
    if (tokens.length === 0) {
      // No tokens — body is plain text or empty
      return;
    }

    switch (format.macroBodyStyle) {
      case MacroBodyStyle.CloseTag:
        this.buildCloseTagBody(bodyNode, tokens, passageRefs, format, warnings);
        break;
      case MacroBodyStyle.Hook:
        this.buildHookBody(bodyNode, tokens, passageRefs, format, warnings);
        break;
      case MacroBodyStyle.Inline:
        this.buildInlineBody(bodyNode, tokens, passageRefs, format, warnings);
        break;
      default:
        // Unknown body style — fall back to inline
        this.buildInlineBody(bodyNode, tokens, passageRefs, format, warnings);
        break;
    }
  }

  // ─── CloseTag Strategy (SugarCube) ──────────────────────────
  //
  // <<if condition>>
  //   content
  // <<else>>
  //   more content
  // <</if>>
  //
  // Uses a stack to match open macros with their close tags.
  // Children macros (like <<else>>, <<elseif>>) are siblings inside
  // the parent macro's body, not nested inside each other.

  private buildCloseTagBody(
    bodyNode: ASTNode,
    tokens: BodyToken[],
    passageRefs: PassageRef[],
    format: FormatModule,
    warnings: BuilderWarning[],
  ): void {
    const macroStack: ASTNode[] = []; // stack of open MacroCall nodes
    const hasBodyMacros = this.buildHasBodyMacroSet(format);

    let textAccumStart: number | null = null;
    let textAccumParts: string[] = [];

    const flushText = (endOffset: number) => {
      if (textAccumStart !== null && textAccumParts.length > 0) {
        const textRange: SourceRange = { start: textAccumStart, end: endOffset };
        const textNode = createNode('Text', textRange, {
          text: textAccumParts.join(''),
        });
        const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
        appendChild(parent, textNode);
        textAccumStart = null;
        textAccumParts = [];
      }
    };

    for (let i = 0; i < tokens.length; i++) {
      const token = tokens[i];

      if (token.typeId === 'text' || token.typeId === 'newline') {
        // Accumulate text tokens
        if (textAccumStart === null) {
          textAccumStart = token.range.start;
        }
        textAccumParts.push(token.text);
        continue;
      }

      // Non-text token — flush any accumulated text first
      flushText(token.range.start);

      if (token.typeId === 'macro-call') {
        const macroName = token.macroName ?? '';
        const hasBody = hasBodyMacros.has(macroName);

        const macroNode = createNode('MacroCall', token.range, {
          macroName,
          hasBody,
          rawArgs: this.extractRawArgs(token),
        });

        if (hasBody) {
          // This macro opens a body region — push onto stack
          const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
          appendChild(parent, macroNode);
          macroStack.push(macroNode);
        } else {
          // Inline macro — no body, just a leaf child
          const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
          appendChild(parent, macroNode);
        }
      } else if (token.typeId === 'macro-close') {
        const closeName = token.macroName ?? '';
        const closeNode = createNode('MacroClose', token.range, {
          macroName: closeName,
          isClosing: true,
        });

        // Try to match with the stack
        if (macroStack.length > 0) {
          // Find the matching open macro on the stack
          let matchIndex = -1;
          for (let j = macroStack.length - 1; j >= 0; j--) {
            if (macroStack[j].data.macroName === closeName) {
              matchIndex = j;
              break;
            }
          }

          if (matchIndex >= 0) {
            // Add close tag to the matching macro's parent
            const matchingMacro = macroStack[matchIndex];
            const parent = matchingMacro.parent ?? bodyNode;
            appendChild(parent, closeNode);

            // Pop all macros from stack down to the match
            // (any macros above the match are unclosed — generate warnings)
            for (let k = macroStack.length - 1; k > matchIndex; k--) {
              const unclosed = macroStack[k];
              warnings.push({
                message: `Unclosed macro: <<${unclosed.data.macroName}>>`,
                range: unclosed.range,
                kind: 'unclosed-macro',
              });
            }
            macroStack.length = matchIndex;
          } else {
            // No matching open macro — orphan close tag
            warnings.push({
              message: `Orphan close tag: <</${closeName}>>`,
              range: token.range,
              kind: 'orphan-close',
            });
            const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
            appendChild(parent, closeNode);
          }
        } else {
          // Stack is empty — orphan close tag
          warnings.push({
            message: `Orphan close tag: <</${closeName}>>`,
            range: token.range,
            kind: 'orphan-close',
          });
          appendChild(bodyNode, closeNode);
        }
      } else if (token.typeId === 'variable') {
        const varNode = createNode('Variable', token.range, {
          varName: token.varName,
          varSigil: token.varSigil,
        });
        const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
        appendChild(parent, varNode);
      } else {
        // Unknown token type — create a generic node
        const genericNode = createNode(token.typeId, token.range, {
          text: token.text,
        });
        const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
        appendChild(parent, genericNode);
      }
    }

    // Flush remaining text
    if (textAccumStart !== null) {
      flushText(tokens[tokens.length - 1]?.range.end ?? textAccumStart);
    }

    // Any remaining macros on the stack are unclosed
    for (const unclosed of macroStack) {
      warnings.push({
        message: `Unclosed macro: <<${unclosed.data.macroName}>>`,
        range: unclosed.range,
        kind: 'unclosed-macro',
      });
    }

    // Insert Link nodes for passage references
    this.insertLinkNodes(bodyNode, passageRefs);
  }

  // ─── Hook Strategy (Harlowe) ────────────────────────────────
  //
  // (if: condition)[content shown if true]
  // (else:)[content shown if false]
  //
  // Macro calls and hooks are separate but associated:
  //   - A changer macro (if:, else:, etc.) attaches to the next hook
  //   - A command macro (go-to:, print:, etc.) doesn't take a hook
  //   - A hook can be named: |hookName>[...]
  //
  // The builder groups a changer macro call with its following hook
  // into a single MacroCall AST node whose children are the hook content.

  private buildHookBody(
    bodyNode: ASTNode,
    tokens: BodyToken[],
    passageRefs: PassageRef[],
    format: FormatModule,
    warnings: BuilderWarning[],
  ): void {
    const macroStack: ASTNode[] = []; // stack of changer macros waiting for hooks
    const hasBodyMacros = this.buildHasBodyMacroSet(format);

    let textAccumStart: number | null = null;
    let textAccumParts: string[] = [];

    const flushText = (endOffset: number) => {
      if (textAccumStart !== null && textAccumParts.length > 0) {
        const textRange: SourceRange = { start: textAccumStart, end: endOffset };
        const textNode = createNode('Text', textRange, {
          text: textAccumParts.join(''),
        });
        const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
        appendChild(parent, textNode);
        textAccumStart = null;
        textAccumParts = [];
      }
    };

    for (let i = 0; i < tokens.length; i++) {
      const token = tokens[i];

      if (token.typeId === 'text' || token.typeId === 'newline') {
        if (textAccumStart === null) {
          textAccumStart = token.range.start;
        }
        textAccumParts.push(token.text);
        continue;
      }

      flushText(token.range.start);

      if (token.typeId === 'macro-call') {
        const macroName = token.macroName ?? '';
        const hasBody = hasBodyMacros.has(macroName);

        const macroNode = createNode('MacroCall', token.range, {
          macroName,
          hasBody,
          rawArgs: this.extractRawArgs(token),
        });

        if (hasBody) {
          // Changer macro — push onto stack, waiting for its hook
          const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
          appendChild(parent, macroNode);
          macroStack.push(macroNode);
        } else {
          // Command/instant macro — no hook body expected
          const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
          appendChild(parent, macroNode);
        }
      } else if (token.typeId === 'hook-open') {
        // Opening bracket [ — content belongs to the most recent changer macro
        // If no changer is waiting, the hook is a standalone (named) hook
        if (macroStack.length > 0) {
          // The hook content will be children of the top changer macro
          // (the hook-open itself becomes a child for range tracking)
          const hookOpenNode = createNode('HookOpen', token.range);
          const currentMacro = macroStack[macroStack.length - 1];
          appendChild(currentMacro, hookOpenNode);
        } else {
          // Standalone hook (e.g. |name>[...]) — add to body
          const hookOpenNode = createNode('HookOpen', token.range);
          appendChild(bodyNode, hookOpenNode);
        }
      } else if (token.typeId === 'hook-close') {
        // Closing bracket ] — pop the most recent changer from stack
        const hookCloseNode = createNode('HookClose', token.range);
        if (macroStack.length > 0) {
          const currentMacro = macroStack[macroStack.length - 1];
          appendChild(currentMacro, hookCloseNode);
          // Pop the changer — its hook body is complete
          macroStack.pop();
        } else {
          appendChild(bodyNode, hookCloseNode);
        }
      } else if (token.typeId === 'hook-name') {
        // Named hook tag (|name> or <name|)
        const hookNameNode = createNode('HookName', token.range, {
          hookName: token.text,
        });
        const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
        appendChild(parent, hookNameNode);
      } else if (token.typeId === 'variable') {
        const varNode = createNode('Variable', token.range, {
          varName: token.varName,
          varSigil: token.varSigil,
        });
        const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
        appendChild(parent, varNode);
      } else {
        const genericNode = createNode(token.typeId, token.range, {
          text: token.text,
        });
        const parent = macroStack.length > 0 ? macroStack[macroStack.length - 1] : bodyNode;
        appendChild(parent, genericNode);
      }
    }

    // Flush remaining text
    if (textAccumStart !== null) {
      flushText(tokens[tokens.length - 1]?.range.end ?? textAccumStart);
    }

    // Any remaining changer macros on the stack never got their hooks
    for (const unclosed of macroStack) {
      warnings.push({
        message: `Changer macro (${unclosed.data.macroName}) has no following hook`,
        range: unclosed.range,
        kind: 'unclosed-macro',
      });
    }

    // Insert Link nodes for passage references
    this.insertLinkNodes(bodyNode, passageRefs);
  }

  // ─── Inline Strategy (Chapbook, Snowman, Fallback) ──────────
  //
  // No macro nesting — all tokens are direct children of the body.
  // This is the simplest strategy: flat list of siblings.

  private buildInlineBody(
    bodyNode: ASTNode,
    tokens: BodyToken[],
    passageRefs: PassageRef[],
    format: FormatModule,
    warnings: BuilderWarning[],
  ): void {
    let textAccumStart: number | null = null;
    let textAccumParts: string[] = [];

    const flushText = (endOffset: number) => {
      if (textAccumStart !== null && textAccumParts.length > 0) {
        const textRange: SourceRange = { start: textAccumStart, end: endOffset };
        const textNode = createNode('Text', textRange, {
          text: textAccumParts.join(''),
        });
        appendChild(bodyNode, textNode);
        textAccumStart = null;
        textAccumParts = [];
      }
    };

    for (const token of tokens) {
      if (token.typeId === 'text' || token.typeId === 'newline') {
        if (textAccumStart === null) {
          textAccumStart = token.range.start;
        }
        textAccumParts.push(token.text);
        continue;
      }

      flushText(token.range.start);

      if (token.typeId === 'macro-call' || token.typeId === 'insert') {
        const macroNode = createNode('MacroCall', token.range, {
          macroName: token.macroName,
          insertName: token.macroName,
          rawArgs: this.extractRawArgs(token),
        });
        appendChild(bodyNode, macroNode);
      } else if (token.typeId === 'variable') {
        const varNode = createNode('Variable', token.range, {
          varName: token.varName,
          varSigil: token.varSigil,
        });
        appendChild(bodyNode, varNode);
      } else if (token.typeId === 'template-open' || token.typeId === 'template-block') {
        const templateNode = createNode('TemplateBlock', token.range, {
          isExpression: false,
          text: token.text,
        });
        appendChild(bodyNode, templateNode);
      } else if (token.typeId === 'template-expression') {
        const exprNode = createNode('TemplateBlock', token.range, {
          isExpression: true,
          text: token.text,
        });
        appendChild(bodyNode, exprNode);
      } else if (token.typeId === 'front-matter') {
        const fmNode = createNode('FrontMatter', token.range, {
          isFrontMatter: true,
          text: token.text,
        });
        appendChild(bodyNode, fmNode);
      } else if (token.typeId === 'modifier') {
        const modNode = createNode('Modifier', token.range, {
          text: token.text,
        });
        appendChild(bodyNode, modNode);
      } else {
        const genericNode = createNode(token.typeId, token.range, {
          text: token.text,
        });
        appendChild(bodyNode, genericNode);
      }
    }

    // Flush remaining text
    if (textAccumStart !== null) {
      flushText(tokens[tokens.length - 1]?.range.end ?? textAccumStart);
    }

    // Insert Link nodes for passage references
    this.insertLinkNodes(bodyNode, passageRefs);
  }

  // ─── Link Node Insertion ────────────────────────────────────

  /**
   * Insert Link nodes into the body tree based on PassageRef data.
   *
   * PassageRefs come from format.extractPassageRefs() — the single source
   * of truth for all passage references. We create Link nodes for each
   * ref that overlaps with text in the body.
   *
   * Links are inserted into the tree at the correct position by finding
   * which existing Text node(s) the link range overlaps with, and splitting
   * them as needed.
   */
  private insertLinkNodes(bodyNode: ASTNode, passageRefs: PassageRef[]): void {
    for (const ref of passageRefs) {
      if (ref.kind !== PassageRefKind.Link) {
        // Macro/API/Implicit refs don't create Link nodes —
        // they're already represented as MacroCall nodes
        continue;
      }

      const format = this.formatRegistry.getActiveFormat();
      // Resolve link body for display text and kind
      // The rawBody is the text between [[ and ]]
      let linkData: Partial<ASTNodeData> = {
        linkTarget: ref.target,
        linkKind: ref.linkKind,
      };

      // Try to resolve display text if source looks like [[...]]
      if (ref.source === '[[ ]]') {
        // Extract the raw body from the source range and resolve it
        // This gives us the display text, setter, etc.
        // For now, use the target as a fallback
      }

      const linkNode = createNode('Link', ref.range, linkData);

      // Find the correct parent and position to insert the link
      this.insertLinkIntoTree(bodyNode, linkNode, ref.range);
    }
  }

  /**
   * Insert a Link node into the tree, splitting Text nodes as needed.
   */
  private insertLinkIntoTree(
    bodyNode: ASTNode,
    linkNode: ASTNode,
    linkRange: SourceRange,
  ): void {
    // Walk the tree to find where the link range falls
    // Strategy: find the Text node(s) that overlap with the link range,
    // split them, and insert the Link node in place.

    const candidates = this.findNodesInRange(bodyNode, 'Text', linkRange);

    if (candidates.length === 0) {
      // No text node found for this link — add as direct child of body
      appendChild(bodyNode, linkNode);
      return;
    }

    // For simplicity, if a single Text node spans the entire link range,
    // split it into [before, link, after]
    const textNode = candidates[0];
    if (!textNode.parent) {
      appendChild(bodyNode, linkNode);
      return;
    }

    const parent = textNode.parent;
    const index = parent.children.indexOf(textNode);

    if (index < 0) {
      appendChild(bodyNode, linkNode);
      return;
    }

    const textStart = textNode.range.start;
    const textEnd = textNode.range.end;
    const linkStart = linkRange.start;
    const linkEnd = linkRange.end;

    // Remove the original text node
    parent.children.splice(index, 1);

    // Re-insert as: [before-text] [link] [after-text]
    const parts: ASTNode[] = [];

    if (linkStart > textStart) {
      parts.push(createNode('Text', { start: textStart, end: linkStart }, {
        text: textNode.data.text?.slice(0, linkStart - textStart),
      }));
    }

    parts.push(linkNode);

    if (linkEnd < textEnd) {
      parts.push(createNode('Text', { start: linkEnd, end: textEnd }, {
        text: textNode.data.text?.slice(linkEnd - textStart),
      }));
    }

    // Insert all parts at the original position
    for (let i = 0; i < parts.length; i++) {
      parts[i].parent = parent;
      parent.children.splice(index + i, 0, parts[i]);
    }
  }

  /**
   * Find all nodes of a given type whose range overlaps with the target range.
   */
  private findNodesInRange(root: ASTNode, nodeType: string, range: SourceRange): ASTNode[] {
    const results: ASTNode[] = [];
    walkTree(root, node => {
      if (node.nodeType === nodeType) {
        // Check overlap
        if (node.range.start < range.end && node.range.end > range.start) {
          results.push(node);
        }
      }
    });
    return results;
  }

  // ─── Helpers ────────────────────────────────────────────────

  /**
   * Build a Set of macro names that have bodies (hasBody=true).
   * Used by the nesting algorithms to decide whether to push onto the stack.
   */
  private buildHasBodyMacroSet(format: FormatModule): Set<string> {
    const set = new Set<string>();
    if (format.macros) {
      for (const macro of format.macros.builtins) {
        if (macro.hasBody) {
          set.add(macro.name);
          if (macro.aliases) {
            for (const alias of macro.aliases) {
              set.add(alias);
            }
          }
        }
      }
    }
    return set;
  }

  /**
   * Extract raw argument text from a token.
   * This is the text between the macro name and the closing delimiter.
   */
  private extractRawArgs(token: BodyToken): string {
    // The raw args are embedded in the token text after the macro name
    // For SugarCube: <<if condition>> → args = "condition"
    // For Harlowe: (if: condition) → args = "condition"
    // We don't parse args here — that's the semantic analyzer's job
    return token.text;
  }

  /**
   * Find the precise range of a passage header in the source content.
   */
  private findHeaderRange(content: string, raw: RawPassage): SourceRange {
    // Search backwards from body start for the :: header line
    const bodyStart = raw.startOffset;
    const beforeBody = content.substring(0, bodyStart);

    // Find the last :: before bodyStart
    const headerIdx = beforeBody.lastIndexOf('::');
    if (headerIdx >= 0) {
      // Find end of the header line
      const lineEnd = content.indexOf('\n', headerIdx);
      const headerEnd = lineEnd >= 0 ? lineEnd + 1 : content.length;
      return { start: headerIdx, end: headerEnd };
    }

    // Fallback: approximate range
    return { start: Math.max(0, bodyStart - 100), end: bodyStart };
  }
}
