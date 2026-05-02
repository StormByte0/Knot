// ---------------------------------------------------------------------------
// SugarCube 2 — builtin macro catalog
// ---------------------------------------------------------------------------

export interface MacroDef {
  name:        string;
  description: string;
  hasBody:     boolean;
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
  { name: 'if',       hasBody: true,  description: 'Conditional block. `<<if $condition>>…<</if>>`' },
  { name: 'elseif',   hasBody: false, description: 'Else-if branch within `<<if>>`.' },
  { name: 'else',     hasBody: false, description: 'Else branch within `<<if>>`.' },
  { name: 'for',      hasBody: true,  description: 'Iteration. `<<for _i, $arr>>…<</for>>`' },
  { name: 'break',    hasBody: false, description: 'Break out of the nearest enclosing `<<for>>` loop.' },
  { name: 'continue', hasBody: false, description: 'Skip to the next iteration of the nearest `<<for>>` loop.' },
  { name: 'switch',   hasBody: true,  description: 'Switch on an expression. `<<switch $v>><<case 1>>…<</switch>>`' },
  { name: 'case',     hasBody: false, description: 'Case arm within `<<switch>>`.' },
  { name: 'default',  hasBody: false, description: 'Default arm within `<<switch>>`.' },

  // Variables
  { name: 'set',      hasBody: false, description: 'Assign a value: `<<set $var to expression>>`' },
  { name: 'unset',    hasBody: false, description: 'Remove a story variable: `<<unset $var>>`' },
  { name: 'capture',  hasBody: true,  description: 'Capture variables for use in closures.' },
  { name: 'run',      hasBody: false, description: 'Execute an expression without producing output: `<<run $arr.push("item")>>`' },

  // Output
  { name: 'print',    hasBody: false, description: 'Print the result of an expression.' },
  { name: '=',        hasBody: false, description: 'Short alias for `<<print>>`.' },
  { name: '-',        hasBody: false, description: 'Print without leading/trailing whitespace.' },
  { name: 'type',     hasBody: true,  description: 'Typewriter effect: displays text character by character.' },
  { name: 'nobr',     hasBody: true,  description: 'Remove line breaks from enclosed content.' },
  { name: 'silently', hasBody: true,  description: 'Execute enclosed code without producing output.' },

  // DOM / Display
  { name: 'append',      hasBody: true,  description: 'Append content to a selector: `<<append "#id">>…<</append>>`' },
  { name: 'prepend',     hasBody: true,  description: 'Prepend content to a selector.' },
  { name: 'replace',     hasBody: true,  description: 'Replace element content.' },
  { name: 'remove',      hasBody: false, description: 'Remove matching element(s) from the DOM.' },
  { name: 'copy',        hasBody: true,  description: 'Copy existing element content into another.' },
  { name: 'addclass',    hasBody: false, description: 'Add CSS class(es) to element(s).' },
  { name: 'removeclass', hasBody: false, description: 'Remove CSS class(es) from element(s).' },
  { name: 'toggleclass', hasBody: false, description: 'Toggle CSS class(es) on element(s).' },
  { name: 'css',         hasBody: true,  description: 'Inject inline CSS into the page.' },
  { name: 'script',      hasBody: true,  description: 'Execute JavaScript: `<<script>>…<</script>>`' },

  // Links / Interaction
  { name: 'link',        hasBody: true,  description: `Inline link with click handler: \`<<link "label" "passage">>…<</link>>\`` + PASSAGE_HINT },
  { name: 'button',      hasBody: true,  description: `Button with click handler: \`<<button "label" "passage">>…<</button>>\`` + PASSAGE_HINT },
  { name: 'linkappend',  hasBody: true,  description: 'Link that appends content when clicked: `<<linkappend "label">>…<</linkappend>>`' },
  { name: 'linkprepend', hasBody: true,  description: 'Link that prepends content when clicked: `<<linkprepend "label">>…<</linkprepend>>`' },
  { name: 'linkreplace', hasBody: true,  description: 'Link that replaces itself with content when clicked: `<<linkreplace "label">>…<</linkreplace>>`' },
  { name: 'actions',     hasBody: true,  description: 'Shorthand for a group of one-shot passage links.' + PASSAGE_HINT },
  { name: 'click',       hasBody: true,  description: 'Alias for `<<link>>` (deprecated; prefer `<<link>>`).' + PASSAGE_HINT },
  { name: 'checkbox',    hasBody: false, description: 'Bind a checkbox to a story variable.' },
  { name: 'radiobutton', hasBody: false, description: 'Bind a radio button to a story variable.' },
  { name: 'textarea',    hasBody: false, description: 'Bind a `<textarea>` to a story variable.' },
  { name: 'textbox',     hasBody: false, description: 'Bind a text input to a story variable.' },
  { name: 'numberbox',   hasBody: false, description: 'Bind a numeric input to a story variable.' },

  // Navigation / Audio / UI
  { name: 'goto',          hasBody: false, description: `Navigate to a passage: \`<<goto "passage">>\`` + PASSAGE_HINT },
  { name: 'back',          hasBody: false, description: 'Return to the previous passage.' },
  { name: 'return',        hasBody: false, description: 'Navigate using browser history.' },
  { name: 'include',       hasBody: false, description: `Include and render another passage inline: \`<<include "passage">>\`` + PASSAGE_HINT },
  { name: 'timed',         hasBody: true,  description: 'Display content after a delay: `<<timed 2s>>…<</timed>>`' },
  { name: 'repeat',        hasBody: true,  description: 'Repeat content on an interval.' },
  { name: 'stop',          hasBody: false, description: 'Stop the nearest `<<timed>>` or `<<repeat>>`.' },
  { name: 'widget',        hasBody: true,  description: 'Define a reusable custom macro.' },
  { name: 'done',          hasBody: true,  description: 'Execute code after the passage is fully rendered.' },
  { name: 'audio',         hasBody: false, description: 'Control audio: `<<audio "id" play>>`' },
  { name: 'playlist',      hasBody: false, description: 'Control an audio playlist.' },
  { name: 'masteraudio',   hasBody: false, description: 'Control the master audio.' },
  { name: 'createplaylist',hasBody: true,  description: 'Define a new audio playlist.' },
  { name: 'cacheaudio',    hasBody: false, description: 'Cache an audio track.' },
  { name: 'waitforaudio',  hasBody: false, description: 'Pause rendering until cached audio is ready.' },
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