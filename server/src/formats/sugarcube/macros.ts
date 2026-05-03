// ---------------------------------------------------------------------------
// SugarCube 2 — builtin macro catalog
// ---------------------------------------------------------------------------

export interface MacroArgDef {
  /** Position (0-based) in the macro's argument list. */
  position: number;
  /** Display label for signature help. */
  label: string;
  /** Whether this argument accepts a passage name reference. */
  isPassageRef?: boolean;
  /** Whether this argument accepts a CSS selector. */
  isSelector?: boolean;
  /** Whether this argument accepts a variable name ($var or _var). */
  isVariable?: boolean;
  /** Whether this argument is required. */
  isRequired?: boolean;
  /** SugarCube expression (parsed) vs discrete (literal string) argument. */
  kind: 'expression' | 'string' | 'selector' | 'variable';
}

export interface MacroDef {
  name: string;
  description: string;
  hasBody: boolean;
  /** Argument signature definitions. If absent, the macro takes arbitrary args. */
  args?: MacroArgDef[];
  /** Whether this macro is deprecated. */
  deprecated?: boolean;
  /** Deprecation message if deprecated. */
  deprecationMessage?: string;
  /** Category for filtering (e.g. 'control', 'variables', 'links'). */
  category?: string;
  /** If this macro must be inside a parent macro, name the parent. For multi-parent, use containerAnyOf. */
  container?: string;
  /** If this macro must be inside one of several parent macros. */
  containerAnyOf?: string[];
}

export interface GlobalDef {
  name: string;
  description: string;
}

export const BLOCK_MACRO_NAMES: ReadonlySet<string> = new Set([
  'if', 'elseif', 'else', 'for', 'switch', 'case', 'default',
  'link', 'button', 'linkappend', 'linkprepend', 'linkreplace',
  'append', 'prepend', 'replace', 'copy',
  'widget', 'done', 'nobr', 'silently', 'capture', 'script', 'type',
  'actions', 'click',
]);

const PASSAGE_HINT = ' — *Ctrl+Space inside quotes for passage name completions*';

export const BUILTINS: MacroDef[] = [
  // Control
  { name: 'if',       hasBody: true,  description: 'Conditional block. `<<if $condition>>…<</if>>`', category: 'control' },
  { name: 'elseif',   hasBody: false, description: 'Else-if branch within `<<if>>`.', category: 'control', containerAnyOf: ['if', 'elseif'] },
  { name: 'else',     hasBody: false, description: 'Else branch within `<<if>>`.', category: 'control', container: 'if' },
  { name: 'for',      hasBody: true,  description: 'Iteration. `<<for _i, $arr>>…<</for>>`', category: 'control' },
  { name: 'break',    hasBody: false, description: 'Break out of the nearest enclosing `<<for>>` loop.', category: 'control', container: 'for' },
  { name: 'continue', hasBody: false, description: 'Skip to the next iteration of the nearest `<<for>>` loop.', category: 'control', container: 'for' },
  { name: 'switch',   hasBody: true,  description: 'Switch on an expression. `<<switch $v>><<case 1>>…<</switch>>`', category: 'control',
    args: [{ position: 0, label: 'expression', isRequired: true, kind: 'expression' }] },
  { name: 'case',     hasBody: false, description: 'Case arm within `<<switch>>`.', category: 'control', container: 'switch' },
  { name: 'default',  hasBody: false, description: 'Default arm within `<<switch>>`.', category: 'control', container: 'switch' },

  // Variables
  { name: 'set',      hasBody: false, description: 'Assign a value: `<<set $var to expression>>`', category: 'variables',
    args: [{ position: 0, label: 'expression', kind: 'expression' }] },
  { name: 'unset',    hasBody: false, description: 'Remove a story variable: `<<unset $var>>`', category: 'variables',
    args: [{ position: 0, label: 'variable', isVariable: true, isRequired: true, kind: 'variable' }] },
  { name: 'capture',  hasBody: true,  description: 'Capture variables for use in closures.', category: 'variables',
    args: [{ position: 0, label: 'variable', isVariable: true, isRequired: true, kind: 'variable' }] },
  { name: 'run',      hasBody: false, description: 'Execute an expression without producing output: `<<run $arr.push("item")>>`', category: 'variables',
    args: [{ position: 0, label: 'expression', kind: 'expression' }] },

  // Output
  { name: 'print',    hasBody: false, description: 'Print the result of an expression.', category: 'output',
    args: [{ position: 0, label: 'expression', isRequired: true, kind: 'expression' }] },
  { name: '=',        hasBody: false, description: 'Short alias for `<<print>>`.', category: 'output',
    args: [{ position: 0, label: 'expression', isRequired: true, kind: 'expression' }] },
  { name: '-',        hasBody: false, description: 'Print without leading/trailing whitespace.', category: 'output',
    args: [{ position: 0, label: 'expression', isRequired: true, kind: 'expression' }] },
  { name: 'type',     hasBody: true,  description: 'Typewriter effect: displays text character by character.', category: 'output',
    args: [{ position: 0, label: 'speed', isRequired: true, kind: 'string' }] },
  { name: 'nobr',     hasBody: true,  description: 'Remove line breaks from enclosed content.', category: 'output' },
  { name: 'silently', hasBody: true,  description: 'Execute enclosed code without producing output.', category: 'output' },

  // DOM / Display
  { name: 'append',      hasBody: true,  description: 'Append content to a selector: `<<append "#id">>…<</append>>`', category: 'dom',
    args: [{ position: 0, label: 'selector', isSelector: true, isRequired: true, kind: 'selector' }] },
  { name: 'prepend',     hasBody: true,  description: 'Prepend content to a selector.', category: 'dom',
    args: [{ position: 0, label: 'selector', isSelector: true, isRequired: true, kind: 'selector' }] },
  { name: 'replace',     hasBody: true,  description: 'Replace element content.', category: 'dom',
    args: [{ position: 0, label: 'selector', isSelector: true, isRequired: true, kind: 'selector' }] },
  { name: 'remove',      hasBody: false, description: 'Remove matching element(s) from the DOM.', category: 'dom',
    args: [{ position: 0, label: 'selector', isSelector: true, isRequired: true, kind: 'selector' }] },
  { name: 'copy',        hasBody: true,  description: 'Copy existing element content into another.', category: 'dom',
    args: [{ position: 0, label: 'selector', isSelector: true, isRequired: true, kind: 'selector' }] },
  { name: 'addclass',    hasBody: false, description: 'Add CSS class(es) to element(s).', category: 'dom',
    args: [{ position: 0, label: 'selector', isSelector: true, isRequired: true, kind: 'selector' }, { position: 1, label: 'class', isRequired: true, kind: 'string' }] },
  { name: 'removeclass', hasBody: false, description: 'Remove CSS class(es) from element(s).', category: 'dom',
    args: [{ position: 0, label: 'selector', isSelector: true, isRequired: true, kind: 'selector' }, { position: 1, label: 'class', isRequired: true, kind: 'string' }] },
  { name: 'toggleclass', hasBody: false, description: 'Toggle CSS class(es) on element(s).', category: 'dom',
    args: [{ position: 0, label: 'selector', isSelector: true, isRequired: true, kind: 'selector' }, { position: 1, label: 'class', isRequired: true, kind: 'string' }] },
  { name: 'css',         hasBody: true,  description: 'Inject inline CSS into the page.', category: 'dom' },
  { name: 'script',      hasBody: true,  description: 'Execute JavaScript: `<<script>>…<</script>>`', category: 'dom' },

  // Links / Interaction
  { name: 'link',        hasBody: true,  description: `Inline link with click handler: \`<<link "label" "passage">>…<</link>>\`` + PASSAGE_HINT, category: 'links',
    args: [{ position: 0, label: 'label', isRequired: true, kind: 'string' }, { position: 1, label: 'passage', isPassageRef: true, isRequired: false, kind: 'string' }] },
  { name: 'button',      hasBody: true,  description: `Button with click handler: \`<<button "label" "passage">>…<</button>>\`` + PASSAGE_HINT, category: 'links',
    args: [{ position: 0, label: 'label', isRequired: true, kind: 'string' }, { position: 1, label: 'passage', isPassageRef: true, isRequired: false, kind: 'string' }] },
  { name: 'linkappend',  hasBody: true,  description: 'Link that appends content when clicked: `<<linkappend "label">>…<</linkappend>>`', category: 'links',
    args: [{ position: 0, label: 'label', isRequired: true, kind: 'string' }, { position: 1, label: 'passage', isPassageRef: true, isRequired: false, kind: 'string' }] },
  { name: 'linkprepend', hasBody: true,  description: 'Link that prepends content when clicked: `<<linkprepend "label">>…<</linkprepend>>`', category: 'links',
    args: [{ position: 0, label: 'label', isRequired: true, kind: 'string' }, { position: 1, label: 'passage', isPassageRef: true, isRequired: false, kind: 'string' }] },
  { name: 'linkreplace', hasBody: true,  description: 'Link that replaces itself with content when clicked: `<<linkreplace "label">>…<</linkreplace>>`', category: 'links',
    args: [{ position: 0, label: 'label', isRequired: true, kind: 'string' }, { position: 1, label: 'passage', isPassageRef: true, isRequired: false, kind: 'string' }] },
  { name: 'actions',     hasBody: true,  description: 'Shorthand for a group of one-shot passage links.' + PASSAGE_HINT, category: 'links',
    args: [{ position: 0, label: 'passage', isPassageRef: true, isRequired: true, kind: 'string' }] },
  { name: 'click',       hasBody: true,  description: 'Alias for `<<link>>` (deprecated; prefer `<<link>>`).' + PASSAGE_HINT, category: 'links', deprecated: true, deprecationMessage: '<<click>> is deprecated. Use <<link>> instead.',
    args: [{ position: 0, label: 'label', isRequired: true, kind: 'string' }, { position: 1, label: 'passage', isPassageRef: true, isRequired: false, kind: 'string' }] },
  { name: 'checkbox',    hasBody: false, description: 'Bind a checkbox to a story variable.', category: 'forms',
    args: [{ position: 0, label: 'label', kind: 'string' }, { position: 1, label: 'variable', isVariable: true, kind: 'variable' }, { position: 2, label: 'checked', kind: 'string' }, { position: 3, label: 'unchecked', kind: 'string' }] },
  { name: 'radiobutton', hasBody: false, description: 'Bind a radio button to a story variable.', category: 'forms',
    args: [{ position: 0, label: 'label', kind: 'string' }, { position: 1, label: 'variable', isVariable: true, kind: 'variable' }, { position: 2, label: 'value', kind: 'string' }] },
  { name: 'textarea',    hasBody: false, description: 'Bind a `<textarea>` to a story variable.', category: 'forms',
    args: [{ position: 0, label: 'variable', isVariable: true, isRequired: true, kind: 'variable' }, { position: 1, label: 'placeholder', kind: 'string' }] },
  { name: 'textbox',     hasBody: false, description: 'Bind a text input to a story variable.', category: 'forms',
    args: [{ position: 0, label: 'variable', isVariable: true, isRequired: true, kind: 'variable' }, { position: 1, label: 'placeholder', kind: 'string' }] },
  { name: 'numberbox',   hasBody: false, description: 'Bind a numeric input to a story variable.', category: 'forms',
    args: [{ position: 0, label: 'variable', isVariable: true, isRequired: true, kind: 'variable' }, { position: 1, label: 'default', kind: 'expression' }] },

  // Navigation / Audio / UI
  { name: 'goto',          hasBody: false, description: `Navigate to a passage: \`<<goto "passage">>\`` + PASSAGE_HINT, category: 'navigation',
    args: [{ position: 0, label: 'passage', isPassageRef: true, isRequired: true, kind: 'string' }] },
  { name: 'back',          hasBody: false, description: 'Return to the previous passage.', category: 'navigation' },
  { name: 'return',        hasBody: false, description: 'Navigate using browser history.', category: 'navigation' },
  { name: 'include',       hasBody: false, description: `Include and render another passage inline: \`<<include "passage">>\`` + PASSAGE_HINT, category: 'navigation',
    args: [{ position: 0, label: 'passage', isPassageRef: true, isRequired: true, kind: 'string' }] },
  { name: 'timed',         hasBody: true,  description: 'Display content after a delay: `<<timed 2s>>…<</timed>>`', category: 'timing',
    args: [{ position: 0, label: 'delay', isRequired: true, kind: 'string' }] },
  { name: 'repeat',        hasBody: true,  description: 'Repeat content on an interval.', category: 'timing',
    args: [{ position: 0, label: 'interval', isRequired: true, kind: 'string' }] },
  { name: 'stop',          hasBody: false, description: 'Stop the nearest `<<timed>>` or `<<repeat>>`.', category: 'timing', containerAnyOf: ['timed', 'repeat'] },
  { name: 'widget',        hasBody: true,  description: 'Define a reusable custom macro.', category: 'widgets',
    args: [{ position: 0, label: 'name', isRequired: true, kind: 'string' }] },
  { name: 'done',          hasBody: true,  description: 'Execute code after the passage is fully rendered.', category: 'output' },
  { name: 'audio',         hasBody: false, description: 'Control audio: `<<audio "id" play>>`', category: 'audio',
    args: [{ position: 0, label: 'id', isRequired: true, kind: 'string' }] },
  { name: 'playlist',      hasBody: false, description: 'Control an audio playlist.', category: 'audio' },
  { name: 'masteraudio',   hasBody: false, description: 'Control the master audio.', category: 'audio' },
  { name: 'createplaylist',hasBody: true,  description: 'Define a new audio playlist.', category: 'audio' },
  { name: 'cacheaudio',    hasBody: false, description: 'Cache an audio track.', category: 'audio' },
  { name: 'waitforaudio',  hasBody: false, description: 'Pause rendering until cached audio is ready.', category: 'audio' },
];

export const BUILTIN_MAP: ReadonlyMap<string, MacroDef> = new Map(
  BUILTINS.map(m => [m.name, m]),
);

export const BUILTIN_GLOBALS: GlobalDef[] = [
  { name: 'State',       description: 'SugarCube state management API.' },
  { name: 'Story',       description: 'Story metadata and passage lookup API.' },
  { name: 'Engine',      description: 'Story engine control API.' },
  { name: 'Dialog',      description: 'Dialog box API.' },
  { name: 'Fullscreen',  description: 'Fullscreen API.' },
  { name: 'LoadScreen',  description: 'Loading screen API.' },
  { name: 'Macro',       description: 'Macro registration API (e.g. Macro.add).' },
  { name: 'Passage',     description: 'Current passage info.' },
  { name: 'Save',        description: 'Save/load API.' },
  { name: 'Setting',     description: 'Settings API.' },
  { name: 'Settings',    description: 'Settings object.' },
  { name: 'SimpleAudio', description: 'Simple audio API.' },
  { name: 'Template',    description: 'Template API.' },
  { name: 'UI',          description: 'UI utility API.' },
  { name: 'UIBar',       description: 'Story navigation bar API.' },
  { name: 'Config',      description: 'Story configuration object.' },
  { name: 'SugarCube',   description: 'Global SugarCube namespace.' },
  { name: 'setup',       description: 'Author setup object for shared data.' },
  { name: 'prehistory',  description: 'Prehistory task array.' },
  { name: 'predisplay',  description: 'Predisplay task array.' },
  { name: 'prerender',   description: 'Prerender task array.' },
  { name: 'postdisplay', description: 'Postdisplay task array.' },
  { name: 'postrender',  description: 'Postrender task array.' },
];
