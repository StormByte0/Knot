/**
 * Knot v2 — Snowman 2 Format Module
 *
 * Implements FormatModule for the Snowman 2 story format.
 * Exports exactly one object: `snowmanModule`.
 *
 * Key Snowman characteristics:
 *   - NO macro syntax — uses <% %> JavaScript execution blocks
 *     and <%= %> expression output blocks (Underscore.js templates)
 *   - Variables: s.name (story — window.story, persists) and
 *     t.name (temporary — window.passage, passage-scoped)
 *   - Passage navigation: story.show('PassageName') JS API
 *   - Links: [[Target]] and [[Text->Target]] only (right arrow,
 *     NO pipe | and NO left arrow <-)
 *   - MacroBodyStyle: Inline (no macro bodies — just JavaScript)
 *   - Special passages: None beyond Twee 3 spec
 *
 * MUST NOT import from: core/, handlers/
 */

import type {
  FormatModule,
  FormatASTNodeTypes,
  ASTNodeTypeDef,
  TokenTypeDef,
  BodyToken,
  LinkResolution,
  SpecialPassageDef,
  MacroDelimiters,
  VariableCapability,
  DiagnosticCapability,
  DiagnosticResult,
  DiagnosticCheckContext,
  PassageRef,
} from '../_types';

import {
  MacroBodyStyle,
  LinkKind,
  PassageRefKind,
} from '../../hooks/hookTypes';

// ═══════════════════════════════════════════════════════════════════
// AST NODE TYPES
// ═══════════════════════════════════════════════════════════════════

const SNOWMAN_AST_NODE_TYPES: FormatASTNodeTypes = (() => {
  const defs: ASTNodeTypeDef[] = [
    { id: 'Document',           label: 'Document',            canHaveChildren: true,  childNodeTypeIds: ['PassageHeader', 'PassageBody'] },
    { id: 'PassageHeader',      label: 'Passage Header',     canHaveChildren: false },
    { id: 'PassageBody',        label: 'Passage Body',       canHaveChildren: true,  childNodeTypeIds: ['TemplateBlock', 'TemplateExpression', 'Variable', 'Link', 'Text'] },
    { id: 'Link',              label: 'Link',               canHaveChildren: false },
    { id: 'Text',              label: 'Text',               canHaveChildren: false },
    { id: 'TemplateBlock',     label: 'Template Block',     canHaveChildren: true,  childNodeTypeIds: ['Variable', 'Text'] },
    { id: 'TemplateExpression',label: 'Template Expression', canHaveChildren: true,  childNodeTypeIds: ['Variable', 'Text'] },
    { id: 'Variable',          label: 'Variable',           canHaveChildren: false },
  ];
  const types = new Map(defs.map(d => [d.id, d]));
  return {
    types,
    Document: 'Document',
    PassageHeader: 'PassageHeader',
    PassageBody: 'PassageBody',
    Link: 'Link',
    Text: 'Text',
  };
})();

// ═══════════════════════════════════════════════════════════════════
// TOKEN TYPES
// ═══════════════════════════════════════════════════════════════════

const SNOWMAN_TOKEN_TYPES: TokenTypeDef[] = [
  { id: 'template-block',      label: 'Template Block',       category: 'delimiter' },
  { id: 'template-expression', label: 'Template Expression',  category: 'delimiter' },
  { id: 'variable',            label: 'Variable',             category: 'identifier' },
  { id: 'text',                label: 'Text',                 category: 'literal' },
  { id: 'newline',             label: 'Newline',              category: 'whitespace' },
  { id: 'eof',                 label: 'EOF',                  category: 'whitespace' },
];

// ═══════════════════════════════════════════════════════════════════
// BODY LEXER
// ═══════════════════════════════════════════════════════════════════

/**
 * Tokenize a Snowman passage body.
 *
 * Snowman's template syntax:
 *   <%= ... %> — JavaScript expression (output the result)
 *   <% ... %>  — JavaScript code block (execution, no output)
 *   s.name     — Story variable (window.story)
 *   t.name     — Temporary variable (window.passage)
 *   [[...]]    — Link boundaries
 *   Everything else is plain text or newlines.
 *
 * Order matters: <%= must be checked before <% to avoid
 * partial matches.
 */
function lexBody(input: string, baseOffset: number): BodyToken[] {
  const tokens: BodyToken[] = [];
  let pos = 0;
  const len = input.length;

  while (pos < len) {
    const remaining = input.slice(pos);

    // ── Template expression: <%= ... %> ────────────────────────
    // Must be checked BEFORE <% to avoid partial match on <%=.
    const exprMatch = remaining.match(/^<%=([\s\S]*?)%>/);
    if (exprMatch) {
      tokens.push({
        typeId: 'template-expression',
        text: exprMatch[0],
        range: { start: baseOffset + pos, end: baseOffset + pos + exprMatch[0].length },
      });
      pos += exprMatch[0].length;
      continue;
    }

    // ── Template block: <% ... %> ─────────────────────────────
    const blockMatch = remaining.match(/^<%([\s\S]*?)%>/);
    if (blockMatch) {
      tokens.push({
        typeId: 'template-block',
        text: blockMatch[0],
        range: { start: baseOffset + pos, end: baseOffset + pos + blockMatch[0].length },
      });
      pos += blockMatch[0].length;
      continue;
    }

    // ── Unclosed template block: <% without matching %> ───────
    // Detect <% that is NOT followed by %> anywhere — this is an
    // error but we still tokenize it so diagnostics can report it.
    const unclosedMatch = remaining.match(/^<%(=?)[\s\S]*$/);
    if (unclosedMatch) {
      // Consume the rest of the input as an unclosed template
      const typeId = unclosedMatch[1] === '=' ? 'template-expression' : 'template-block';
      tokens.push({
        typeId,
        text: remaining,
        range: { start: baseOffset + pos, end: baseOffset + pos + remaining.length },
      });
      pos = len;
      continue;
    }

    // ── Variable: s.name or t.name ────────────────────────────
    // Match s.identifier or t.identifier where identifier is a
    // valid JS identifier (not preceded by alphanumeric/dot, to
    // avoid matching inside longer expressions like "this.name").
    const varMatch = remaining.match(/^([st])\.([a-zA-Z_][a-zA-Z0-9_]*)/);
    if (varMatch) {
      // Check that this is not preceded by an alphanumeric char or dot
      // (e.g. "this.x" should not match "s.x" inside it)
      const prevChar = pos > 0 ? input[pos - 1] : '';
      if (!/[a-zA-Z0-9_.]/.test(prevChar)) {
        tokens.push({
          typeId: 'variable',
          text: varMatch[0],
          range: { start: baseOffset + pos, end: baseOffset + pos + varMatch[0].length },
          varName: varMatch[2],
          varSigil: varMatch[1],
        });
        pos += varMatch[0].length;
        continue;
      }
    }

    // ── Newline ───────────────────────────────────────────────
    if (input[pos] === '\n') {
      tokens.push({
        typeId: 'newline',
        text: '\n',
        range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
      });
      pos += 1;
      continue;
    }

    // ── Text (accumulate until we hit a special token) ────────
    let textStart = pos;
    while (pos < len) {
      const r = input.slice(pos);
      if (
        r.startsWith('<%') ||
        /^([st])\.([a-zA-Z_][a-zA-Z0-9_]*)/.test(r) ||
        input[pos] === '\n'
      ) {
        // For variable matches, also check the preceding character
        if (/^([st])\.([a-zA-Z_][a-zA-Z0-9_]*)/.test(r)) {
          const prevChar = pos > 0 ? input[pos - 1] : '';
          if (/[a-zA-Z0-9_.]/.test(prevChar)) {
            // Not a real variable — keep accumulating text
            pos += 1;
            continue;
          }
        }
        break;
      }
      pos += 1;
    }
    if (pos > textStart) {
      tokens.push({
        typeId: 'text',
        text: input.slice(textStart, pos),
        range: { start: baseOffset + textStart, end: baseOffset + pos },
      });
    } else {
      // Safety: always advance at least one character
      tokens.push({
        typeId: 'text',
        text: input[pos],
        range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
      });
      pos += 1;
    }
  }

  // EOF sentinel
  tokens.push({
    typeId: 'eof',
    text: '',
    range: { start: baseOffset + pos, end: baseOffset + pos },
  });

  return tokens;
}

// ═══════════════════════════════════════════════════════════════════
// PASSAGE REFERENCE EXTRACTION
// ═══════════════════════════════════════════════════════════════════

/** Match [[ ]] links */
const LINK_RE = /\[\[([^\]]+?)\]\]/g;

/**
 * Match Snowman JS API calls that reference passages.
 *
 * Supported patterns:
 *   story.show('PassageName')
 *   story.passage('PassageName')
 *   window.story.show('PassageName')
 *   window.story.passage('PassageName')
 *
 * Also matches template-delimited forms inside <% %> blocks,
 * since the regex scans the full body text.
 */
const STORY_API_RE = /(?:window\.)?story\.(show|passage)\s*\(\s*['"]([^'"]+)['"]\s*\)/g;

/**
 * Extract ALL passage references from a Snowman passage body.
 *
 * Single source of truth for: [[ ]] links + story.show/passage JS API calls
 * inside template blocks.
 */
function extractPassageRefs(body: string, bodyOffset: number): PassageRef[] {
  const refs: PassageRef[] = [];

  // ── 1. [[ ]] links ──────────────────────────────────────────
  LINK_RE.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = LINK_RE.exec(body)) !== null) {
    const rawBody = match[1];
    const resolved = resolveLinkBody(rawBody);
    if (resolved.target && resolved.kind === LinkKind.Passage) {
      refs.push({
        target: resolved.target,
        kind: PassageRefKind.Link,
        range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
        source: '[[ ]]',
        linkKind: resolved.kind,
      });
    }
  }

  // ── 2. story.show() / story.passage() / window.story.show() ──
  STORY_API_RE.lastIndex = 0;
  while ((match = STORY_API_RE.exec(body)) !== null) {
    const method = match[1]; // 'show' or 'passage'
    const passageName = match[2];
    const sourceLabel = method === 'show' ? 'story.show()' : 'story.passage()';
    refs.push({
      target: passageName,
      kind: PassageRefKind.API,
      range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
      source: sourceLabel,
    });
  }

  return refs;
}

// ═══════════════════════════════════════════════════════════════════
// LINK RESOLUTION
// ═══════════════════════════════════════════════════════════════════

/**
 * Resolve the raw body text inside [[...]] for Snowman.
 *
 * Snowman supports:
 *   [[Target]]         — simple link to Target
 *   [[Text->Target]]   — link with display text (right arrow only)
 *
 * Snowman does NOT support pipe | or left arrow <-.
 */
function resolveLinkBody(rawBody: string): LinkResolution {
  if (!rawBody) return { target: '', kind: LinkKind.Passage };

  // 1. Right arrow: [[Text->Target]]
  const rightArrowIdx = rawBody.indexOf('->');
  if (rightArrowIdx !== -1) {
    const displayText = rawBody.slice(0, rightArrowIdx).trim();
    const target = rawBody.slice(rightArrowIdx + 2).trim();
    const isExternal = /^https?:\/\//.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
    };
  }

  // 2. Simple link: [[Target]]
  const target = rawBody.trim();
  return {
    target,
    kind: /^https?:\/\//.test(target) ? LinkKind.External : LinkKind.Passage,
  };
}

// ═══════════════════════════════════════════════════════════════════
// DIAGNOSTIC CUSTOM CHECK
// ═══════════════════════════════════════════════════════════════════

/**
 * Custom diagnostic check for Snowman-specific issues that
 * can't be expressed declaratively:
 *   - Unclosed template blocks (<% without matching %>)
 *   - Malformed [[ ]] link syntax
 */
function customDiagnosticCheck(context: DiagnosticCheckContext): readonly DiagnosticResult[] {
  const results: DiagnosticResult[] = [];
  const { body, bodyTokens } = context;

  // ── Check for unclosed template blocks ──────────────────────
  // A template token that spans to end-of-input with no %> is malformed.
  for (const token of bodyTokens) {
    if (
      (token.typeId === 'template-block' || token.typeId === 'template-expression') &&
      !token.text.endsWith('%>')
    ) {
      results.push({
        ruleId: 'invalid-template-syntax',
        message: `Unclosed template block: missing closing '%>'`,
        severity: 'error',
        range: token.range,
      });
    }
  }

  // ── Check for malformed [[ ]] links ────────────────────────
  // Find [[ that don't have a matching ]] before the next [[ or EOF
  let i = 0;
  while (i < body.length) {
    const openIdx = body.indexOf('[[', i);
    if (openIdx === -1) break;

    const closeIdx = body.indexOf(']]', openIdx + 2);
    if (closeIdx === -1) {
      // Unclosed link
      results.push({
        ruleId: 'invalid-link-syntax',
        message: "Unclosed link: missing closing ']]'",
        severity: 'error',
        range: { start: openIdx, end: body.length },
      });
      break; // No more links possible
    }

    // Check for empty link content: [[ ]]
    const linkContent = body.slice(openIdx + 2, closeIdx);
    if (linkContent.trim() === '') {
      results.push({
        ruleId: 'invalid-link-syntax',
        message: 'Empty link: [[ ]] must contain a passage name',
        severity: 'warning',
        range: { start: openIdx, end: closeIdx + 2 },
      });
    }

    i = closeIdx + 2;
  }

  return results;
}

// ═══════════════════════════════════════════════════════════════════
// THE MODULE EXPORT
// ═══════════════════════════════════════════════════════════════════

export const snowmanModule: FormatModule = {
  formatId: 'snowman-2',
  displayName: 'Snowman 2',
  version: '2.0.0',
  aliases: ['snowman', 'Snowman 2', 'snowman2'],

  astNodeTypes: SNOWMAN_AST_NODE_TYPES,
  tokenTypes: SNOWMAN_TOKEN_TYPES,

  lexBody,
  extractPassageRefs,
  resolveLinkBody,
  specialPassages: [] as readonly SpecialPassageDef[],  // No special passages beyond Twee 3 spec

  macroBodyStyle: MacroBodyStyle.Inline,
  macroDelimiters: {
    open: '<%',
    close: '%>',
  } satisfies MacroDelimiters,
  macroPattern: null,  // No named macro syntax — templates are not macros

  // ── Capability: Variables ───────────────────────────────────
  // Snowman uses s.name (story) and t.name (temp) as JavaScript
  // object property accesses on window.story and window.passage.
  // Modeled as pseudo-sigils for the VariableCapability interface.
  variables: {
    sigils: [
      { sigil: 's', kind: 'story', description: 'Snowman story variable object (s.name) — window.story, persists across passages' },
      { sigil: 't', kind: 'temp', description: 'Snowman temp variable object (t.name) — window.passage, scoped to current passage' },
    ],
    assignmentMacros: new Set<string>(),  // Assignments happen via JavaScript: s.name = value
    assignmentOperators: ['='],
    comparisonOperators: ['===', '!==', '==', '!=', '>', '>=', '<', '<='],
    variablePattern: /\b([st])\.([a-zA-Z_][a-zA-Z0-9_]*)/g,
    triggerChars: ['s'],
  } satisfies VariableCapability,

  // ── Capability: Diagnostics ─────────────────────────────────
  diagnostics: {
    rules: [
      {
        id: 'invalid-template-syntax',
        description: 'Malformed template block (unclosed <% or <%=)',
        defaultSeverity: 'error',
        scope: 'passage',
      },
      {
        id: 'invalid-link-syntax',
        description: 'Malformed [[ ]] link',
        defaultSeverity: 'error',
        scope: 'passage',
      },
    ],
    customCheck: customDiagnosticCheck,
  } satisfies DiagnosticCapability,

  // ── NO macros capability (Snowman has no macro syntax) ──────
  // ── NO customMacros capability (Snowman has no user-defined macros) ──
};
