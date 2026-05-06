/**
 * Knot v2 — SugarCube 2 Runtime Globals & Virtual Prelude
 *
 * Defines the runtime objects available in SugarCube's JS scope
 * (State, Engine, Story, Config, etc.) and a virtual prelude
 * for static analysis.
 */

import type { RuntimeGlobalDef } from '../_types';

export const RUNTIME_GLOBALS: RuntimeGlobalDef[] = [
  {
    name: 'State',
    description: 'SugarCube state object — manages story and temporary variables, passage history, and metadata',
    hasMembers: true,
    members: [
      { name: 'variables',  description: 'Story variables object ($-prefixed)',    type: 'object' },
      { name: 'temporary',  description: 'Temporary variables object (_-prefixed)', type: 'object' },
      { name: 'passage',    description: 'Name of the current passage',             type: 'string' },
      { name: 'history',    description: 'Array of passage history entries',        type: 'array' },
      { name: 'turns',      description: 'Number of turns taken',                   type: 'number' },
      { name: 'time',       description: 'Timestamp of the current turn',          type: 'number' },
      { name: 'metadata',   description: 'Story metadata object',                 type: 'object' },
      { name: 'prng',       description: 'Seeded PRNG function',                   type: 'function' },
      { name: 'random',     description: 'Random number function',                 type: 'function' },
    ],
  },
  {
    name: 'Engine',
    description: 'SugarCube engine — controls passage navigation and rendering',
    hasMembers: true,
    members: [
      { name: 'play',      description: 'Play a passage by name',       type: 'function' },
      { name: 'goTo',      description: 'Navigate to a passage',        type: 'function' },
      { name: 'backward',  description: 'Go back in passage history',   type: 'function' },
      { name: 'forward',   description: 'Go forward in passage history', type: 'function' },
      { name: 'restart',   description: 'Restart the story',            type: 'function' },
      { name: 'state',     description: 'Current engine state',         type: 'string' },
    ],
  },
  {
    name: 'Story',
    description: 'SugarCube story object — access to passage data and story metadata',
    hasMembers: true,
    members: [
      { name: 'get',       description: 'Get a Passage object by name',         type: 'function' },
      { name: 'passage',   description: 'Alias for get()',                     type: 'function' },
      { name: 'passages',  description: 'Map of all Passage objects',           type: 'object' },
      { name: 'title',     description: 'Story title',                         type: 'string' },
      { name: 'author',    description: 'Story author',                        type: 'string' },
      { name: 'tags',      description: 'All tags used across passages',       type: 'array' },
    ],
  },
  {
    name: 'SugarCube',
    description: 'SugarCube namespace — version info and global utilities',
    hasMembers: true,
    members: [
      { name: 'version',      description: 'SugarCube version object',           type: 'object' },
      { name: 'Buffer',       description: 'Buffer utility class',               type: 'class' },
      { name: 'Dialog',       description: 'Dialog management object',           type: 'object' },
      { name: 'Macro',        description: 'Macro management object',            type: 'object' },
      { name: 'SimpleAudio',  description: 'Audio management object',           type: 'object' },
      { name: 'LoadScreen',   description: 'Load screen management object',     type: 'object' },
      { name: 'Settings',     description: 'User settings object',               type: 'object' },
      { name: 'Setup',        description: 'Setup object for initialization',    type: 'object' },
      { name: 'UI',           description: 'UI utility object',                  type: 'object' },
    ],
  },
  {
    name: 'setup',
    description: 'User-defined setup object — initialized in StoryInit, available globally',
    hasMembers: false,
  },
  {
    name: 'Config',
    description: 'SugarCube configuration object — controls debug mode, macros, UI, visits, etc.',
    hasMembers: true,
    members: [
      { name: 'debug',     description: 'Enable debug mode (boolean)',           type: 'boolean' },
      { name: 'macros',    description: 'Macro configuration',                  type: 'object' },
      { name: 'ui',        description: 'UI configuration',                      type: 'object' },
      { name: 'visits',    description: 'Visit tracking configuration',          type: 'object' },
      { name: 'history',   description: 'History configuration',                type: 'object' },
      { name: 'saves',     description: 'Save system configuration',             type: 'object' },
      { name: 'audio',     description: 'Audio configuration',                  type: 'object' },
      { name: 'navigation',description: 'Navigation configuration',             type: 'object' },
    ],
  },
  {
    name: 'Dialog',
    description: 'Dialog management — setup, open, close, stow, body manipulation',
    hasMembers: true,
    members: [
      { name: 'setup',  description: 'Setup the dialog with content',    type: 'function' },
      { name: 'open',   description: 'Open the dialog',                  type: 'function' },
      { name: 'close',  description: 'Close the dialog',                 type: 'function' },
      { name: 'stow',   description: 'Stow (minimize) the dialog',       type: 'function' },
      { name: 'body',   description: 'Access the dialog body element',   type: 'function' },
    ],
  },
  {
    name: 'Macro',
    description: 'Macro management — add, get, delete, check, and list macros',
    hasMembers: true,
    members: [
      { name: 'add',    description: 'Add a new macro definition',         type: 'function' },
      { name: 'get',    description: 'Get a macro definition by name',     type: 'function' },
      { name: 'delete', description: 'Delete a macro by name',             type: 'function' },
      { name: 'has',    description: 'Check if a macro exists',             type: 'function' },
      { name: 'tags',   description: 'Get tags for a macro',               type: 'function' },
    ],
  },
  {
    name: 'Passage',
    description: 'Represents a passage — name, tags, text, id, description',
    hasMembers: true,
    members: [
      { name: 'name',        description: 'Passage name',        type: 'string' },
      { name: 'tags',        description: 'Passage tags',        type: 'array' },
      { name: 'text',        description: 'Passage body text',   type: 'string' },
      { name: 'id',          description: 'Passage DOM ID',      type: 'string' },
      { name: 'description',description: 'Passage description', type: 'string' },
    ],
  },
  {
    name: 'Settings',
    description: 'User settings object — define settings in StoryInit, SugarCube auto-generates the UI',
    hasMembers: false,
  },
  {
    name: 'SimpleAudio',
    description: 'Audio management — load, select, tracks, groups, playlists',
    hasMembers: true,
    members: [
      { name: 'load',     description: 'Load an audio track',           type: 'function' },
      { name: 'select',   description: 'Select audio tracks',          type: 'function' },
      { name: 'tracks',   description: 'Map of all audio tracks',      type: 'object' },
      { name: 'groups',   description: 'Map of audio groups',          type: 'object' },
      { name: 'playlists',description: 'Map of audio playlists',       type: 'object' },
    ],
  },
  {
    name: 'UI',
    description: 'UI utility — alert, confirm, prompt, rebuild, update',
    hasMembers: true,
    members: [
      { name: 'alert',   description: 'Show an alert dialog',        type: 'function' },
      { name: 'confirm', description: 'Show a confirmation dialog',  type: 'function' },
      { name: 'prompt',  description: 'Show a prompt dialog',        type: 'function' },
      { name: 'rebuild', description: 'Rebuild the UI bar',          type: 'function' },
      { name: 'update',  description: 'Update the UI bar',           type: 'function' },
    ],
  },
];

/** JavaScript code that sets up a virtual runtime for static analysis */
export const VIRTUAL_RUNTIME_PRELUDE = `
// SugarCube 2 Virtual Runtime Prelude
// Simulates the SugarCube runtime environment for static analysis
var State = { variables: {}, temporary: {}, passage: '', history: [], turns: 0, time: 0, metadata: {} };
var Engine = { play: function(){}, goTo: function(){}, backward: function(){}, forward: function(){}, restart: function(){}, state: 'idle' };
var Story = { get: function(){ return { name:'', tags:[], text:'', id:'', description:'' }; }, passage: function(){}, passages: {}, title: '', author: '', tags: [] };
var SugarCube = { version: {}, Buffer: {}, Dialog: {}, Macro: {}, SimpleAudio: {}, LoadScreen: {}, Settings: {}, Setup: {}, UI: {} };
var setup = {};
var Config = { debug: false, macros: {}, ui: {}, visits: {}, history: {}, saves: {}, audio: {}, navigation: {} };
var Dialog = { setup: function(){}, open: function(){}, close: function(){}, stow: function(){}, body: function(){} };
var Macro = { add: function(){}, get: function(){}, delete: function(){}, has: function(){}, tags: function(){} };
var Passage = { name: '', tags: [], text: '', id: '', description: '' };
var Settings = {};
var SimpleAudio = { load: function(){}, select: function(){}, tracks: {}, groups: {}, playlists: {} };
var UI = { alert: function(){}, confirm: function(){}, prompt: function(){}, rebuild: function(){}, update: function(){} };
`.trim();
