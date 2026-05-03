// ---------------------------------------------------------------------------
// Shared test helpers and fixture data
// ---------------------------------------------------------------------------
import { parseDocument } from '../../src/parser';
import type { ParseOutput, DocumentNode } from '../../src/ast';
import type { StoryFormatAdapter } from '../../src/formats/types';
import { FormatRegistry } from '../../src/formats/registry';

// ── Common twee source fixtures ──────────────────────────────────────────

export const FIXTURES = {
  /** Minimal valid twee file with one passage */
  singlePassage: ':: Start\nHello world',

  /** Two passages linked together */
  linkedPassages: ':: Start\n[[Go to Next|Next]]\n\n:: Next\nYou arrived',

  /** Passage with a set macro */
  setMacro: ':: Start\n<<set $x to 5>>',

  /** Passage with a print macro */
  printMacro: ':: Start\n<<print $x>>',

  /** Script passage */
  scriptPassage: ':: Story JavaScript\nconst myGlobal = 42;',

  /** Stylesheet passage */
  stylesheetPassage: ':: Story Stylesheet\nbody { color: red; }',

  /** StoryInit with variables */
  storyInit: ':: StoryInit\n<<set $health to 100>>\n<<set $name to "Arthur">>',

  /** Widget definition */
  widgetDef: ':: Widgets\n<<widget "greet">>Hello!<</widget>>',

  /** Multiple passages with cross-references */
  multiFile1: ':: Start\n<<set $gold to 0>>\n[[Enter the dungeon|Dungeon]]\n\n:: Dungeon\n<<if $gold gt 10>>You are rich!<</if>>\n[[Return|Start]]',

  /** Passage with passage-arg macros */
  passageArgMacros: ':: Start\n<<goto "End">>\n\n:: End\nYou finished',

  /** StoryData passage */
  storyData: ':: StoryData\n{"ifid":"A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D","format":"sugarcube-2","start":"Start"}',

  /** Passage with nested macros */
  nestedMacros: ':: Start\n<<if true>><<set $x to 1>><<print $x>><</if>>',

  /** Passage with all link separators */
  allLinkTypes: ':: Start\n[[Pipe|Target]]\n[[Arrow->Target2]]\n[[Target3<-Backward]]',

  /** Empty document */
  empty: '',

  /** Passage with block comments */
  blockComments: ':: Start\n/* this is a comment */\nText after',

  /** Passage with HTML comments */
  htmlComments: ':: Start\n<!-- html comment -->\nText after',

  /** Script-tagged passage */
  scriptTagged: ':: Setup [script]\nvar setupVar = true;',
} as const;

// ── Helper: parse a fixture ──────────────────────────────────────────────

export function parseFixture(source: string, adapter?: StoryFormatAdapter): ParseOutput {
  return parseDocument(source, adapter);
}

// ── Helper: get the default (SugarCube) adapter ──────────────────────────

export function getSugarCubeAdapter(): StoryFormatAdapter {
  return FormatRegistry.resolve('sugarcube-2');
}

// ── Helper: get the fallback adapter ──────────────────────────────────────

export function getFallbackAdapter(): StoryFormatAdapter {
  return FormatRegistry.resolve('');
}
