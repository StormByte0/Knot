/**
 * Knot v2 — Chapbook 2 Format Module
 *
 * Implements FormatModule for the Chapbook 2 story format.
 * Exports exactly one object: `chapbookModule`.
 *
 * Key Chapbook characteristics:
 *   - No macro syntax — uses {insert} syntax instead of <<macro>> or (macro:)
 *   - Inserts are the primary interactive mechanism: {embed passage: 'Name'},
 *     {reveal link: 'Click', passage: 'Target'}, {restart link: 'Restart'}, etc.
 *   - Variables use var.name and temp.name dot-notation (NOT sigiled like $name)
 *   - Links use [[ ]] with -> and <- arrows (same as Harlowe — NO pipe |)
 *   - MacroBodyStyle.Inline — inserts are self-contained, no macro bodies
 *   - YAML-like front matter delimited by --- at the top of passage bodies
 *   - Modifiers use [modifier] bracket syntax (e.g. [align center], [fade-in])
 *   - Special passages: none beyond basic Twee 3 spec
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
  MacroDef,
  MacroSignatureDef,
  MacroArgDef,
  MacroDelimiters,
  MacroCapability,
  VariableCapability,
  DiagnosticCapability,
  PassageRef,
} from '../_types';

import {
  MacroCategory,
  MacroKind,
  MacroBodyStyle,
  PassageKind,
  LinkKind,
  PassageRefKind,
} from '../../hooks/hookTypes';

// ═══════════════════════════════════════════════════════════════════
// HELPER FUNCTIONS
// ═══════════════════════════════════════════════════════════════════

/**
 * Build an insert definition (Chapbook's equivalent of a macro def).
 * Inserts use {name} syntax instead of <<name>> or (name:).
 */
function insert(
  name: string,
  category: MacroCategory,
  kind: MacroKind,
  description: string,
  signatures: MacroSignatureDef[],
  opts?: {
    aliases?: string[];
    deprecated?: boolean;
    deprecationMessage?: string;
    children?: string[];
    parents?: string[];
    categoryDetail?: string;
    hasBody?: boolean;
    isNavigation?: boolean;
    isInclude?: boolean;
    isConditional?: boolean;
    isAssignment?: boolean;
    passageArgPosition?: number;
  },
): MacroDef {
  return {
    name,
    aliases: opts?.aliases,
    category,
    categoryDetail: opts?.categoryDetail,
    kind,
    description,
    signatures,
    deprecated: opts?.deprecated,
    deprecationMessage: opts?.deprecationMessage,
    children: opts?.children,
    parents: opts?.parents,
    hasBody: opts?.hasBody,
    isNavigation: opts?.isNavigation,
    isInclude: opts?.isInclude,
    isConditional: opts?.isConditional,
    isAssignment: opts?.isAssignment,
    passageArgPosition: opts?.passageArgPosition,
  };
}

/** Build a macro argument definition. */
function arg(name: string, type: string, required: boolean, opts?: Partial<Pick<MacroArgDef, 'variadic' | 'description'>>): MacroArgDef {
  return { name, type, required, variadic: opts?.variadic, description: opts?.description };
}

/** Shorthand for a single signature. */
function sig(args: MacroArgDef[], returnType?: string, description?: string): MacroSignatureDef {
  return { args, returnType, description };
}

// ═══════════════════════════════════════════════════════════════════
// AST NODE TYPES
// ═══════════════════════════════════════════════════════════════════

const CHAPBOOK_AST_NODE_TYPES: FormatASTNodeTypes = (() => {
  const defs: ASTNodeTypeDef[] = [
    { id: 'Document',       label: 'Document',          canHaveChildren: true,  childNodeTypeIds: ['PassageHeader', 'PassageBody'] },
    { id: 'PassageHeader',  label: 'Passage Header',    canHaveChildren: false },
    { id: 'PassageBody',    label: 'Passage Body',      canHaveChildren: true,  childNodeTypeIds: ['FrontMatter', 'InsertCall', 'Modifier', 'Variable', 'Link', 'Text'] },
    { id: 'FrontMatter',    label: 'Front Matter',      canHaveChildren: true,  childNodeTypeIds: ['Variable', 'Text'] },
    { id: 'Link',           label: 'Link',              canHaveChildren: false },
    { id: 'Text',           label: 'Text',              canHaveChildren: false },
    { id: 'InsertCall',     label: 'Insert Call',       canHaveChildren: true,  childNodeTypeIds: ['InsertCall', 'Variable', 'Link', 'Text'] },
    { id: 'Modifier',       label: 'Modifier',          canHaveChildren: true,  childNodeTypeIds: ['InsertCall', 'Variable', 'Link', 'Text'] },
    { id: 'Variable',       label: 'Variable',          canHaveChildren: false },
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

const CHAPBOOK_TOKEN_TYPES: TokenTypeDef[] = [
  { id: 'insert-open',   label: 'Insert Open',   category: 'delimiter' },
  { id: 'insert-body',   label: 'Insert Body',   category: 'delimiter' },
  { id: 'insert-close',  label: 'Insert Close',  category: 'delimiter' },
  { id: 'front-matter',  label: 'Front Matter',  category: 'delimiter' },
  { id: 'modifier',      label: 'Modifier',      category: 'delimiter' },
  { id: 'variable',      label: 'Variable',      category: 'identifier' },
  { id: 'text',          label: 'Text',          category: 'literal' },
  { id: 'newline',       label: 'Newline',        category: 'whitespace' },
  { id: 'eof',           label: 'EOF',           category: 'whitespace' },
];

// ═══════════════════════════════════════════════════════════════════
// INSERT CATALOG
// ═══════════════════════════════════════════════════════════════════

const INSERTS: MacroDef[] = [

  // ── Navigation Inserts ────────────────────────────────────────────

  insert('back link', MacroCategory.Navigation, MacroKind.Command,
    'Link to go back in history',
    [
      sig([], 'Command', 'Renders a link that navigates to the previous passage in the history.'),
    ],
    { isNavigation: true },
  ),

  insert('restart link', MacroCategory.Navigation, MacroKind.Command,
    'Link to restart the story',
    [
      sig([arg('label', 'string', false, { description: 'Optional label text for the link (default: "Restart")' })], 'Command', 'Renders a link that restarts the story from the beginning.'),
    ],
    { isNavigation: true },
  ),

  insert('undo link', MacroCategory.Navigation, MacroKind.Command,
    'Link to undo last action',
    [
      sig([], 'Command', 'Renders a link that undoes the last navigation action.'),
    ],
    { isNavigation: true },
  ),

  // ── Embedding & Transclusion ──────────────────────────────────────

  insert('embed passage', MacroCategory.Output, MacroKind.Command,
    'Embed another passage\'s content inline',
    [
      sig([arg('passage', 'string', true, { description: 'Name of the passage to embed' })], 'Command', 'Renders the content of the named passage inline at this location.'),
    ],
    { isInclude: true, passageArgPosition: 0 },
  ),

  insert('embed image', MacroCategory.Output, MacroKind.Command,
    'Embed an image',
    [
      sig([arg('image', 'string', true, { description: 'Image URL or data URI' }), arg('alt', 'string', false, { description: 'Alt text for the image' })], 'Command'),
    ],
  ),

  insert('embed url', MacroCategory.Output, MacroKind.Command,
    'Embed an external URL',
    [
      sig([arg('url', 'string', true, { description: 'The URL to embed' })], 'Command'),
    ],
  ),

  // ── Revealing ─────────────────────────────────────────────────────

  insert('reveal link', MacroCategory.Custom, MacroKind.Command,
    'Reveal hidden content on click',
    [
      sig([arg('link', 'string', true, { description: 'The link text to click' })], 'Command', 'Renders a link; when clicked, reveals the content that follows.'),
      sig([arg('link', 'string', true, { description: 'The link text to click' }), arg('passage', 'string', true, { description: 'Passage to navigate to after revealing' })], 'Command', 'Renders a link; when clicked, reveals content then navigates to the named passage.'),
    ],
    { categoryDetail: 'revealing', isNavigation: false, passageArgPosition: 1 },
  ),

  insert('insert link', MacroCategory.Custom, MacroKind.Command,
    'Insert content from another passage on click',
    [
      sig([arg('link', 'string', true, { description: 'The link text to click' }), arg('passage', 'string', false, { description: 'Passage whose content to insert' })], 'Command', 'Renders a link; when clicked, inserts the content of the named passage.'),
    ],
    { categoryDetail: 'revealing', isInclude: true, passageArgPosition: 1 },
  ),

  insert('replace link', MacroCategory.Custom, MacroKind.Command,
    'Replace content with passage content on click',
    [
      sig([arg('link', 'string', true, { description: 'The link text to click' }), arg('passage', 'string', true, { description: 'Passage whose content replaces the current content' })], 'Command', 'Renders a link; when clicked, replaces surrounding content with the named passage\'s content.'),
    ],
    { categoryDetail: 'revealing', isInclude: true, passageArgPosition: 1 },
  ),

  // ── Cycling ───────────────────────────────────────────────────────

  insert('cycling link', MacroCategory.Custom, MacroKind.Command,
    'Cycle through values on click',
    [
      sig([arg('values', 'string', true, { variadic: true, description: 'Values to cycle through' })], 'Command', 'Renders a link that cycles through the given values each time it is clicked.'),
    ],
    { categoryDetail: 'cycling' },
  ),

  // ── Conditional Display ───────────────────────────────────────────

  insert('if', MacroCategory.Control, MacroKind.Changer,
    'Conditional display — renders content when condition is truthy',
    [
      sig([arg('condition', 'expression', true, { description: 'Variable expression to evaluate' })], 'Changer', 'Renders the following content only when the condition is truthy.'),
    ],
    { children: ['else'], hasBody: true, isConditional: true },
  ),

  insert('unless', MacroCategory.Control, MacroKind.Changer,
    'Inverse conditional — renders content when condition is falsy',
    [
      sig([arg('condition', 'expression', true, { description: 'Variable expression to evaluate' })], 'Changer', 'Renders the following content only when the condition is falsy.'),
    ],
    { children: ['else'], hasBody: true, isConditional: true },
  ),

  insert('else', MacroCategory.Control, MacroKind.Changer,
    'Else clause for {if} or {unless}',
    [
      sig([], 'Changer', 'Renders content when the preceding {if} or {unless} condition was not met.'),
    ],
    { parents: ['if', 'unless'], hasBody: true },
  ),

  // ── Other Inserts ─────────────────────────────────────────────────

  insert('link to', MacroCategory.Navigation, MacroKind.Command,
    'External link',
    [
      sig([arg('url', 'string', true, { description: 'The URL to link to' })], 'Command', 'Renders a link to an external URL.'),
    ],
  ),

  insert('dropdown menu', MacroCategory.Custom, MacroKind.Command,
    'Dropdown selector',
    [
      sig([arg('options', 'string', true, { variadic: true, description: 'Dropdown options' })], 'Command', 'Renders a dropdown menu with the given options.'),
    ],
    { categoryDetail: 'input' },
  ),

  insert('text input', MacroCategory.Custom, MacroKind.Command,
    'Text input field',
    [
      sig([arg('placeholder', 'string', false, { description: 'Placeholder text for the input' })], 'Command', 'Renders a text input field.'),
    ],
    { categoryDetail: 'input' },
  ),

  insert('meter', MacroCategory.Styling, MacroKind.Command,
    'Visual progress bar',
    [
      sig([arg('value', 'number', true, { description: 'Current value (0-1)' })], 'Command', 'Renders a visual meter/progress bar at the given fraction.'),
    ],
  ),

  insert('progress bar', MacroCategory.Styling, MacroKind.Command,
    'Progress bar',
    [
      sig([arg('value', 'number', true, { description: 'Current value (0-1)' })], 'Command', 'Renders a progress bar at the given fraction.'),
    ],
  ),

  insert('toggle', MacroCategory.Variable, MacroKind.Command,
    'Toggle a boolean variable',
    [
      sig([arg('variable', 'string', true, { description: 'Variable name to toggle (e.g. var.flag)' })], 'Command', 'Renders a toggle that flips a boolean variable between true and false.'),
    ],
    { isAssignment: true },
  ),

  insert('tooltip', MacroCategory.Styling, MacroKind.Command,
    'Tooltip on hover',
    [
      sig([arg('text', 'string', true, { description: 'The visible text' }), arg('tip', 'string', true, { description: 'The tooltip text shown on hover' })], 'Command'),
    ],
  ),

  // ── Section & Redirection ─────────────────────────────────────────

  insert('section', MacroCategory.Control, MacroKind.Changer,
    'Begin a content section with scoped variables',
    [
      sig([], 'Changer', 'Begins a new content section. Sections create variable scopes and can be used with modifiers.'),
    ],
    { children: ['section-end'], hasBody: true, categoryDetail: 'section' },
  ),

  insert('section-end', MacroCategory.Control, MacroKind.Changer,
    'End a content section',
    [
      sig([], 'Changer', 'Ends the most recently opened content section.'),
    ],
    { parents: ['section'], categoryDetail: 'section', aliases: ['end section'] },
  ),

  insert('redirect to', MacroCategory.Navigation, MacroKind.Command,
    'Redirect to another passage immediately',
    [
      sig([arg('passage', 'string', true, { description: 'Name of the passage to redirect to' })], 'Command', 'Immediately navigates to the named passage without rendering the current passage content.'),
    ],
    { isNavigation: true, passageArgPosition: 0 },
  ),

  // ── Dialog Inserts ────────────────────────────────────────────────

  insert('alert', MacroCategory.Output, MacroKind.Command,
    'Show a browser alert dialog',
    [
      sig([arg('message', 'string', true, { description: 'Message to display in the alert dialog' })], 'Command', 'Shows a browser alert() dialog with the given message.'),
    ],
    { categoryDetail: 'dialog' },
  ),

  insert('confirm', MacroCategory.Output, MacroKind.Command,
    'Show a browser confirm dialog',
    [
      sig([arg('message', 'string', true, { description: 'Message to display in the confirm dialog' })], 'Command', 'Shows a browser confirm() dialog. Sets var.confirmResult to true or false.'),
    ],
    { categoryDetail: 'dialog', isAssignment: true },
  ),

  insert('prompt', MacroCategory.Output, MacroKind.Command,
    'Show a browser prompt dialog',
    [
      sig([arg('message', 'string', true, { description: 'Message to display in the prompt dialog' }), arg('default', 'string', false, { description: 'Default value for the prompt input' })], 'Command', 'Shows a browser prompt() dialog. Sets var.promptResult to the entered value.'),
    ],
    { categoryDetail: 'dialog', isAssignment: true },
  ),

  // ── Enhanced Input ────────────────────────────────────────────────

  insert('cycle', MacroCategory.Variable, MacroKind.Command,
    'Cycle through a list of values for a variable',
    [
      sig([arg('variable', 'string', true, { description: 'Variable to cycle (e.g. var.choice)' }), arg('options', 'string', true, { variadic: true, description: 'Options to cycle through' })], 'Command', 'Renders a cycling selector that rotates through the given options, updating the variable each time.'),
    ],
    { categoryDetail: 'cycling', isAssignment: true },
  ),

  insert('select link', MacroCategory.Custom, MacroKind.Command,
    'Select from options via link clicks',
    [
      sig([arg('variable', 'string', true, { description: 'Variable to set (e.g. var.choice)' }), arg('options', 'string', true, { variadic: true, description: 'Options to select from' })], 'Command', 'Renders a series of links representing options; clicking one sets the variable to that value.'),
    ],
    { categoryDetail: 'cycling', isAssignment: true },
  ),

  // ── Debug & Authoring ────────────────────────────────────────────

  insert('note', MacroCategory.System, MacroKind.Command,
    'Author\'s note visible only in debug mode',
    [
      sig([arg('text', 'string', true, { description: 'The note text to display in debug mode' })], 'Command', 'Displays a note that is only visible when running in debug/test mode. Not shown in published stories.'),
    ],
    { categoryDetail: 'debug' },
  ),

  insert('debug', MacroCategory.System, MacroKind.Command,
    'Debug output of an expression',
    [
      sig([arg('expression', 'expression', true, { description: 'Expression to evaluate and display' })], 'Command', 'Evaluates the expression and displays the result. Only active in debug mode.'),
    ],
    { categoryDetail: 'debug' },
  ),

  // ── Modifiers (bracket syntax [modifier]) ─────────────────────────
  // Chapbook 2 modifiers alter content sections using [modifier] syntax.
  // They are registered as MacroDef entries with categoryDetail: 'modifier'
  // so that the diagnostic engine can recognize them, even though they
  // use bracket syntax instead of brace syntax.

  insert('align center', MacroCategory.Styling, MacroKind.Changer,
    'Center-align the following content',
    [
      sig([], 'Changer', 'Centers all content that follows until the next modifier or section end.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('align left', MacroCategory.Styling, MacroKind.Changer,
    'Left-align the following content',
    [
      sig([], 'Changer', 'Left-aligns all content that follows until the next modifier or section end.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('align right', MacroCategory.Styling, MacroKind.Changer,
    'Right-align the following content',
    [
      sig([], 'Changer', 'Right-aligns all content that follows until the next modifier or section end.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('align justify', MacroCategory.Styling, MacroKind.Changer,
    'Justify the following content',
    [
      sig([], 'Changer', 'Justifies all content that follows until the next modifier or section end.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('transition', MacroCategory.Styling, MacroKind.Changer,
    'Apply a transition effect to the following content',
    [
      sig([], 'Changer', 'Applies a transition effect to the following content section.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('fade-in', MacroCategory.Styling, MacroKind.Changer,
    'Fade in the following content',
    [
      sig([], 'Changer', 'Fades in the content that follows this modifier.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('fade-out', MacroCategory.Styling, MacroKind.Changer,
    'Fade out the following content',
    [
      sig([], 'Changer', 'Fades out the content that follows this modifier.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),

  insert('hidden', MacroCategory.Styling, MacroKind.Changer,
    'Hide the following content initially',
    [
      sig([], 'Changer', 'Hides the content that follows this modifier. The content can be revealed later using a reveal insert.'),
    ],
    { categoryDetail: 'modifier', hasBody: true },
  ),
];

// Build alias map at module level
const ALIAS_MAP = new Map<string, string>();
for (const ins of INSERTS) {
  if (ins.aliases) {
    for (const alias of ins.aliases) {
      ALIAS_MAP.set(alias, ins.name);
    }
  }
}

// Build a set of known insert names for diagnostics
const KNOWN_INSERT_NAMES = new Set<string>();
for (const ins of INSERTS) {
  KNOWN_INSERT_NAMES.add(ins.name);
  if (ins.aliases) {
    for (const alias of ins.aliases) {
      KNOWN_INSERT_NAMES.add(alias);
    }
  }
}

// Build a set of known modifier names for diagnostics
const KNOWN_MODIFIER_NAMES = new Set<string>();
for (const ins of INSERTS) {
  if (ins.categoryDetail === 'modifier') {
    KNOWN_MODIFIER_NAMES.add(ins.name);
  }
}

// ═══════════════════════════════════════════════════════════════════
// FRONT MATTER REGEX
// ═══════════════════════════════════════════════════════════════════

/**
 * Regex for detecting a YAML-like front matter block at the start of a passage body.
 * Front matter is delimited by `---` on its own line.
 *
 * Captures: [1] = content between the delimiters (may be empty)
 *
 * Example:
 *   ---
 *   var.health: 100
 *   var.name: "Alice"
 *   temp.visited: true
 *   ---
 */
const FRONT_MATTER_RE = /^---[\t ]*\n([\s\S]*?)\n---[\t ]*(?:\n|$)/;

/**
 * Regex for individual front matter variable assignments.
 * Matches lines like: var.health: 100, temp.name: "Alice"
 *
 * Captures: [1] = prefix (var or temp), [2] = variable name,
 *           [3] = assigned value (trimmed)
 */
const FRONT_MATTER_VAR_RE = /^(var|temp)\.([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*(.+)$/gm;

/**
 * Regex for detecting modifier bracket syntax: [modifier name]
 * E.g. [align center], [fade-in], [hidden], [transition]
 *
 * Captures: [1] = modifier name
 */
const MODIFIER_RE = /^\[([a-zA-Z][\w\s-]*?)\]\s*$/gm;

// ═══════════════════════════════════════════════════════════════════
// SPECIAL PASSAGES
// ═══════════════════════════════════════════════════════════════════

const SPECIAL_PASSAGES: SpecialPassageDef[] = [];

// ═══════════════════════════════════════════════════════════════════
// BODY LEXER
// ═══════════════════════════════════════════════════════════════════

/**
 * Tokenize a Chapbook passage body.
 *
 * Recognizes:
 *   - YAML front matter (---...---) at the start of the body
 *   - {insert args} — Chapbook insert syntax (with nested brace handling)
 *   - [modifier] — Modifier bracket syntax (e.g. [align center], [fade-in])
 *   - [[link]] — Link boundaries (core handles outer brackets, but we
 *                tokenize the content within)
 *   - var.name / temp.name — Variable references
 *   - Plain text and newlines
 */
function lexBody(input: string, baseOffset: number): BodyToken[] {
  const tokens: BodyToken[] = [];
  let pos = 0;
  const len = input.length;

  // ── Front matter: ---...--- at start of body ────────────────
  // Chapbook 2 allows a YAML-like front matter section at the top
  // of passage bodies, delimited by --- on its own line.
  if (pos === 0) {
    const fmMatch = input.match(FRONT_MATTER_RE);
    if (fmMatch) {
      const fmFull = fmMatch[0];
      const fmContent = fmMatch[1];

      // Emit the front matter as a single token
      tokens.push({
        typeId: 'front-matter',
        text: fmFull,
        range: { start: baseOffset, end: baseOffset + fmFull.length },
        macroName: 'front-matter',
        isClosing: false,
      });

      // Emit tokens for each variable assignment within front matter
      FRONT_MATTER_VAR_RE.lastIndex = 0;
      let varMatch: RegExpExecArray | null;
      while ((varMatch = FRONT_MATTER_VAR_RE.exec(fmContent)) !== null) {
        const lineOffset = (fmMatch.index ?? 0) + fmMatch[0].indexOf(fmContent) + varMatch.index;
        tokens.push({
          typeId: 'variable',
          text: varMatch[0],
          range: { start: baseOffset + lineOffset, end: baseOffset + lineOffset + varMatch[0].length },
          varName: varMatch[2],
          varSigil: varMatch[1],
        });
      }

      pos += fmFull.length;
    }
  }

  while (pos < len) {
    // ── Modifier: [name] on its own line ─────────────────────
    // Chapbook modifiers like [align center], [fade-in], [hidden]
    // use bracket syntax on their own line before a content block.
    if (input[pos] === '[' && !input.slice(pos).startsWith('[[')) {
      // Check if this looks like a modifier: [word-like content]
      const modifierMatch = input.slice(pos).match(/^\[([a-zA-Z][\w\s-]*?)\]\s*$/m);
      if (modifierMatch) {
        // Verify it's a known modifier or looks like one
        const modName = modifierMatch[1].trim();
        const fullMatch = input.slice(pos, pos + modifierMatch[0].length);
        tokens.push({
          typeId: 'modifier',
          text: fullMatch,
          range: { start: baseOffset + pos, end: baseOffset + pos + fullMatch.length },
          macroName: modName,
          isClosing: false,
        });
        pos += fullMatch.length;
        continue;
      }
    }

    // ── Insert: {...} ────────────────────────────────────────
    // Chapbook inserts start with { and end with the matching }
    // We need to handle nested braces and strings inside.
    if (input[pos] === '{') {
      const insertStart = pos;
      const insertText = matchInsert(input, pos);
      if (insertText !== null) {
        // Extract the insert name from the content between braces.
        // Insert name is the first word-like sequence after the opening {.
        const innerContent = insertText.slice(1, -1).trim();
        const nameMatch = innerContent.match(/^([\w\s]+?)(?:[\s,:]|$)/);
        const insertName = nameMatch ? nameMatch[1].trim() : innerContent.split(/[\s,:]/)[0];

        // Check if this is a closing insert like {endif} or {end reveal}
        const isClosing = innerContent.startsWith('end ') || innerContent === 'endif';

        tokens.push({
          typeId: isClosing ? 'insert-close' : 'insert-open',
          text: insertText,
          range: { start: baseOffset + insertStart, end: baseOffset + insertStart + insertText.length },
          macroName: insertName,
          isClosing,
        });
        pos += insertText.length;
        continue;
      }
    }

    // ── Variable: var.name or temp.name ──────────────────────
    // Chapbook uses dot-notation variables, not sigiled ones.
    const varMatch = input.slice(pos).match(/^(var|temp)\.([a-zA-Z_][a-zA-Z0-9_]*)/);
    if (varMatch) {
      // Ensure the preceding character isn't alphanumeric (avoid partial matches)
      const prevChar = pos > 0 ? input[pos - 1] : '';
      if (/[a-zA-Z0-9_]/.test(prevChar)) {
        // Not a standalone variable reference, treat as text
        tokens.push({
          typeId: 'text',
          text: input[pos],
          range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
        });
        pos += 1;
        continue;
      }
      tokens.push({
        typeId: 'variable',
        text: varMatch[0],
        range: { start: baseOffset + pos, end: baseOffset + pos + varMatch[0].length },
        varName: varMatch[2],
        varSigil: varMatch[1],   // 'var' or 'temp' as pseudo-sigil
      });
      pos += varMatch[0].length;
      continue;
    }

    // ── Link: [[...]] ────────────────────────────────────────
    // We emit the [[ as an insert-open-like delimiter and ]] as close
    // so the core can handle link boundaries. For Chapbook, links
    // are just [[text->Target]] or [[Target<-text]] or [[Target]].
    if (input.slice(pos).startsWith('[[')) {
      const linkEnd = input.indexOf(']]', pos + 2);
      if (linkEnd !== -1) {
        const fullLink = input.slice(pos, linkEnd + 2);
        const linkBody = input.slice(pos + 2, linkEnd);
        const resolved = resolveLinkBody(linkBody);
        tokens.push({
          typeId: 'insert-open',
          text: fullLink,
          range: { start: baseOffset + pos, end: baseOffset + pos + fullLink.length },
          macroName: resolved.target || undefined,
          isClosing: false,
        });
        pos += fullLink.length;
        continue;
      }
    }

    // ── Newline ──────────────────────────────────────────────
    if (input[pos] === '\n') {
      tokens.push({
        typeId: 'newline',
        text: '\n',
        range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
      });
      pos += 1;
      continue;
    }

    // ── Text (accumulate until we hit a special token) ───────
    let textStart = pos;
    while (pos < len) {
      const remaining = input.slice(pos);
      if (
        remaining.startsWith('{') ||
        remaining.startsWith('[[') ||
        (remaining.startsWith('[') && /^\[[a-zA-Z][\w\s-]*?\]\s*$/m.test(remaining)) ||
        /^(var|temp)\.([a-zA-Z_])/.test(remaining) ||
        input[pos] === '\n'
      ) {
        // Check that var/temp isn't preceded by alphanumeric
        const varCheck = remaining.match(/^(var|temp)\.([a-zA-Z_])/);
        if (varCheck) {
          const prevChar = pos > 0 ? input[pos - 1] : '';
          if (/[a-zA-Z0-9_]/.test(prevChar)) {
            // Not a real variable start, just text — keep going
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
      // Safety: emit a single character if we're stuck
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

/**
 * Match a Chapbook insert starting at position `pos`.
 * Returns the full matched string including braces, or null if no match.
 * Handles nested braces and string literals.
 */
function matchInsert(input: string, pos: number): string | null {
  if (input[pos] !== '{') return null;
  let depth = 0;
  let i = pos;
  let inString: string | null = null;

  while (i < input.length) {
    const ch = input[i];

    // Handle string literals — they can contain } without closing the insert
    if (inString !== null) {
      if (ch === '\\') {
        i += 2; // skip escaped character
        continue;
      }
      if (ch === inString) {
        inString = null;
      }
      i += 1;
      continue;
    }

    if (ch === "'" || ch === '"') {
      inString = ch;
      i += 1;
      continue;
    }

    if (ch === '{') {
      depth += 1;
    } else if (ch === '}') {
      depth -= 1;
      if (depth === 0) {
        return input.slice(pos, i + 1);
      }
    }
    i += 1;
  }

  // Unmatched brace — return what we have as a best-effort token
  return null;
}

// ═══════════════════════════════════════════════════════════════════
// PASSAGE REFERENCE EXTRACTION
// ═══════════════════════════════════════════════════════════════════

/** Regex for [[ ]] links */
const LINK_RE = /\[\[([^\]]+?)\]\]/g;

/**
 * Specialized regex for {embed passage: 'Name'} — the most common pattern.
 * Captures the passage name from the passage: property.
 */
const EMBED_PASSAGE_RE = /\{embed\s+passage\s*:\s*['"]([^'"]+)['"]/g;

/**
 * Regex for inserts with a passage: property (reveal, replace, insert).
 * Captures: [1] = insert type, [2] = passage name
 */
const PASSAGE_PROP_RE = /\{(reveal|replace|insert)\s+link\s*:[^}]*,\s*passage\s*:\s*['"]([^'"]+)['"][^}]*\}/g;

/**
 * Regex for {redirect to: 'PassageName'} — immediate navigation insert.
 * Captures: [1] = passage name
 */
const REDIRECT_PASSAGE_RE = /\{redirect\s+to\s*:\s*['"]([^'"]+)['"][^}]*\}/g;

/**
 * Extract ALL passage references from a Chapbook passage body.
 * Single source of truth: [[ ]] links + insert passage references.
 *
 * NOTE: YAML front matter variable assignments (var.name: value) do NOT
 * produce direct passage references. Front matter sets variables that may
 * be used in conditional inserts, but the variable names themselves are
 * not passage names. No extraction is performed for front matter content.
 */
function extractPassageRefs(body: string, bodyOffset: number): PassageRef[] {
  const refs: PassageRef[] = [];

  // ── 1. [[ ]] links ─────────────────────────────────────────
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

  // ── 2. {embed passage: 'Name'} ─────────────────────────────
  EMBED_PASSAGE_RE.lastIndex = 0;
  while ((match = EMBED_PASSAGE_RE.exec(body)) !== null) {
    refs.push({
      target: match[1],
      kind: PassageRefKind.Macro,
      range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
      source: '{embed passage}',
    });
  }

  // ── 3. {reveal/replace/insert link: ..., passage: 'Name'} ──
  PASSAGE_PROP_RE.lastIndex = 0;
  while ((match = PASSAGE_PROP_RE.exec(body)) !== null) {
    const insertType = match[1]; // 'reveal', 'replace', or 'insert'
    refs.push({
      target: match[2],
      kind: PassageRefKind.Macro,
      range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
      source: `{${insertType} passage}`,
    });
  }

  // ── 4. {redirect to: 'PassageName'} ────────────────────────
  REDIRECT_PASSAGE_RE.lastIndex = 0;
  while ((match = REDIRECT_PASSAGE_RE.exec(body)) !== null) {
    refs.push({
      target: match[1],
      kind: PassageRefKind.Macro,
      range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
      source: '{redirect to}',
    });
  }

  return refs;
}

// ═══════════════════════════════════════════════════════════════════
// LINK RESOLUTION
// ═══════════════════════════════════════════════════════════════════

/**
 * Resolve the body text inside [[...]].
 *
 * Chapbook uses the same link syntax as Harlowe:
 *   - Right arrow: [[target->display text]]
 *   - Left arrow:  [[display text<-target]]
 *   - Simple:      [[target]]
 *   - NO pipe separator (unlike SugarCube)
 *   - NO setter syntax
 */
function resolveLinkBody(rawBody: string): LinkResolution {
  if (!rawBody) return { target: '', kind: LinkKind.Passage };

  // 1. Right arrow: target->text (RIGHTMOST -> is separator, matching Harlowe)
  const rightArrowIdx = rawBody.lastIndexOf('->');
  if (rightArrowIdx !== -1) {
    const target = rawBody.slice(0, rightArrowIdx).trim();
    const displayText = rawBody.slice(rightArrowIdx + 2).trim();
    const isExternal = /^https?:\/\//.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
    };
  }

  // 2. Left arrow: text<-target (arrow points away from target)
  const leftArrowIdx = rawBody.indexOf('<-');
  if (leftArrowIdx !== -1) {
    const displayText = rawBody.slice(0, leftArrowIdx).trim();
    const target = rawBody.slice(leftArrowIdx + 2).trim();
    const isExternal = /^https?:\/\//.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
    };
  }

  // 3. Simple link: [[target]]
  const target = rawBody.trim();
  return {
    target,
    kind: /^https?:\/\//.test(target) ? LinkKind.External : LinkKind.Passage,
  };
}

// ═══════════════════════════════════════════════════════════════════
// CUSTOM DIAGNOSTIC CHECK
// ═══════════════════════════════════════════════════════════════════

import type { DiagnosticCheckContext, DiagnosticResult } from '../_types';

/**
 * Custom diagnostic check for Chapbook-specific issues.
 * Catches unknown insert names, malformed insert syntax, front matter
 * issues, and unknown modifiers.
 */
function customDiagCheck(context: DiagnosticCheckContext): readonly DiagnosticResult[] {
  const results: DiagnosticResult[] = [];
  const body = context.body;

  // ── Front matter: unclosed --- block ─────────────────────────
  if (body.startsWith('---')) {
    const fmMatch = body.match(FRONT_MATTER_RE);
    if (!fmMatch) {
      // Opening --- found but no matching closing ---
      results.push({
        ruleId: 'unclosed-front-matter',
        message: 'Unclosed front matter block: missing closing ---',
        severity: 'error',
        range: { start: 0, end: 3 },
      });
    } else {
      // Check for malformed variable assignments in front matter
      const fmContent = fmMatch[1];
      const lines = fmContent.split('\n');
      let lineOffset = 4; // after opening ---\n
      for (const line of lines) {
        const trimmed = line.trim();
        if (trimmed === '' || trimmed.startsWith('#') || trimmed.startsWith('//')) {
          // Skip blank lines and comments
          lineOffset += line.length + 1;
          continue;
        }
        // Valid front matter lines: var.name: value, temp.name: value
        if (/^(var|temp)\.[a-zA-Z_][a-zA-Z0-9_]*\s*:/.test(trimmed)) {
          // Valid assignment
        } else if (/^[a-zA-Z_][a-zA-Z0-9_]*\s*:/.test(trimmed)) {
          // Could be a valid YAML key (not var/temp but still allowed in front matter)
        } else if (trimmed !== '') {
          // Unrecognized line in front matter
          results.push({
            ruleId: 'malformed-front-matter',
            message: `Unrecognized front matter entry: "${trimmed}"`,
            severity: 'warning',
            range: { start: lineOffset, end: lineOffset + line.length },
          });
        }
        lineOffset += line.length + 1;
      }
    }
  }

  // ── Match all {...} inserts in the body ──────────────────────
  const INSERT_RE = /\{([^}]+)\}/g;
  let m: RegExpExecArray | null;
  while ((m = INSERT_RE.exec(body)) !== null) {
    const inner = m[1].trim();

    // Skip closing inserts like {endif}, {end reveal}, {end section}
    if (inner.startsWith('end ') || inner === 'endif') continue;

    // Extract the insert name — first word-like token
    const nameMatch = inner.match(/^([\w\s]+?)(?:[\s,:]|$)/);
    const insertName = nameMatch ? nameMatch[1].trim() : inner.split(/[\s,:]/)[0];

    // Check if it's a known insert
    if (!KNOWN_INSERT_NAMES.has(insertName)) {
      results.push({
        ruleId: 'unknown-insert',
        message: `Unknown insert: {${insertName}}`,
        severity: 'warning',
        range: { start: m.index, end: m.index + m[0].length },
      });
    }

    // Check for malformed syntax: insert with unmatched quotes
    const quotes = (inner.match(/['"]/g) || []).length;
    if (quotes % 2 !== 0) {
      results.push({
        ruleId: 'invalid-insert-syntax',
        message: `Malformed insert syntax: unmatched quote in {${inner}}`,
        severity: 'error',
        range: { start: m.index, end: m.index + m[0].length },
      });
    }
  }

  // ── Match [modifier] bracket syntax ─────────────────────────
  MODIFIER_RE.lastIndex = 0;
  let modMatch: RegExpExecArray | null;
  while ((modMatch = MODIFIER_RE.exec(body)) !== null) {
    const modName = modMatch[1].trim();
    // Skip [[ ]] link-like patterns (shouldn't match due to regex, but be safe)
    if (modName.includes('[') || modName.includes(']')) continue;

    if (!KNOWN_MODIFIER_NAMES.has(modName)) {
      results.push({
        ruleId: 'unknown-modifier',
        message: `Unknown modifier: [${modName}]`,
        severity: 'warning',
        range: { start: modMatch.index, end: modMatch.index + modMatch[0].length },
      });
    }
  }

  return results;
}

// ═══════════════════════════════════════════════════════════════════
// THE MODULE EXPORT
// ═══════════════════════════════════════════════════════════════════

export const chapbookModule: FormatModule = {
  formatId: 'chapbook-2',
  displayName: 'Chapbook 2',
  version: '2.0.0',
  aliases: ['chapbook', 'Chapbook 2', 'chapbook2'],

  astNodeTypes: CHAPBOOK_AST_NODE_TYPES,
  tokenTypes: CHAPBOOK_TOKEN_TYPES,

  lexBody,
  extractPassageRefs,
  resolveLinkBody,
  specialPassages: SPECIAL_PASSAGES,

  macroBodyStyle: MacroBodyStyle.Inline,
  macroDelimiters: {
    open: '{',
    close: '}',
  } satisfies MacroDelimiters,
  macroPattern: /\{([\w\s]+?)(?:[\s,:}]|$\})/g,

  // ── Capability: Macros (inserts in Chapbook's case) ──────────
  macros: {
    builtins: INSERTS,
    aliases: ALIAS_MAP,
  } satisfies MacroCapability,

  // ── Capability: Variables ────────────────────────────────────
  variables: {
    sigils: [],  // Chapbook has no single-character sigils; uses prefix notation
    assignmentMacros: new Set(['toggle', 'confirm', 'prompt', 'cycle', 'select link']),
    assignmentOperators: ['='],
    comparisonOperators: [],
    variablePattern: /(var|temp)\.([a-zA-Z_][a-zA-Z0-9_]*)/g,
    triggerChars: ['v'],  // 'v' triggers because 'var.' is how variables start
  } satisfies VariableCapability,

  // ── Capability: Diagnostics ──────────────────────────────────
  diagnostics: {
    rules: [
      { id: 'unknown-insert',          description: 'Use of an unrecognized insert name',         defaultSeverity: 'warning', scope: 'passage' },
      { id: 'invalid-insert-syntax',   description: 'Malformed insert syntax',                     defaultSeverity: 'error',   scope: 'passage' },
      { id: 'unclosed-front-matter',   description: 'Front matter block missing closing ---',       defaultSeverity: 'error',   scope: 'passage' },
      { id: 'malformed-front-matter',  description: 'Unrecognized entry in front matter block',     defaultSeverity: 'warning', scope: 'passage' },
      { id: 'unknown-modifier',        description: 'Use of an unrecognized modifier in [brackets]', defaultSeverity: 'warning', scope: 'passage' },
    ],
    customCheck: customDiagCheck,
  } satisfies DiagnosticCapability,
};
