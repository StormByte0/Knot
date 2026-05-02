import * as vscode from 'vscode';
import type { BuildState } from './tweegoIntegration';

export type LspState = 'starting' | 'ready' | 'error';

interface BarState {
  lsp:          LspState;
  build:        BuildState;
  passageCount: number | null;
  format:       string | null;
  formatVersion: string | null;
  ifidMissing:  boolean;
}

const state: BarState = {
  lsp: 'starting', build: 'idle',
  passageCount: null, format: null, formatVersion: null, ifidMissing: false,
};

// Four items, left to right:
//   1. knot badge   — server status + passage count, click → full command menu
//   2. Build       — one-click build (icon changes with build state)
//   3. Watch       — one-click watch toggle (icon changes when active)
//   4. Settings    — direct access to knot Settings
let scItem:       vscode.StatusBarItem | undefined;
let buildItem:    vscode.StatusBarItem | undefined;
let watchItem:    vscode.StatusBarItem | undefined;
let settingsItem: vscode.StatusBarItem | undefined;

export function createStatusBar(context: vscode.ExtensionContext): void {
  scItem       = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 104);
  buildItem    = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 103);
  watchItem    = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 102);
  settingsItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 101);

  scItem.command       = 'knot.mainMenu';
  buildItem.command    = 'knot.build';
  // watchItem.command is set dynamically in sync() based on watch state
  settingsItem.command = 'knot.openSettings';
  settingsItem.text    = '$(settings-gear)';
  settingsItem.tooltip = 'knot: Open Settings';

  context.subscriptions.push(scItem, buildItem, watchItem, settingsItem);
  sync();
}

export function setLspStarting(): void             { state.lsp = 'starting'; sync(); }
export function setLspState(s: LspState): void     { state.lsp = s;          sync(); }
export function setBuildState(s: BuildState): void { state.build = s;        sync(); }

export function setStoryData(data: {
  format: string | null;
  formatVersion: string | null;
  passageCount: number;
  ifid: string | null;
}): void {
  state.format        = data.format;
  state.formatVersion = data.formatVersion;
  state.passageCount  = data.passageCount;
  state.ifidMissing   = data.ifid === null && (data.format !== null || data.passageCount > 0);
  sync();
}

function sync(): void {
  if (!scItem || !buildItem || !watchItem || !settingsItem) return;

  // ── knot badge ─────────────────────────────────────────────────────────────
  switch (state.lsp) {
    case 'starting':
      scItem.text            = '$(loading~spin) knot';
      scItem.tooltip         = 'knot: indexing workspace…';
      scItem.backgroundColor = undefined;
      scItem.show();
      buildItem.hide();
      watchItem.hide();
      settingsItem.hide();
      return;

    case 'error':
      scItem.text            = '$(error) knot';
      scItem.tooltip         = new vscode.MarkdownString('**knot** server error — click to open menu');
      scItem.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
      scItem.show();
      buildItem.hide();
      watchItem.hide();
      settingsItem.hide();
      return;

    case 'ready': {
      const n   = state.passageCount;
      const pfx = state.ifidMissing ? '$(warning)' : '$(check)';
      scItem.text            = n !== null ? `${pfx} knot ${n}` : `${pfx} knot`;
      scItem.tooltip         = lspTooltip(n);
      scItem.backgroundColor = state.ifidMissing
        ? new vscode.ThemeColor('statusBarItem.warningBackground')
        : undefined;
      scItem.show();
      break;
    }
  }

  // ── Build button ──────────────────────────────────────────────────────────
  switch (state.build) {
    case 'idle':
      buildItem.text            = '$(play)';
      buildItem.tooltip         = 'knot: Build (Ctrl+Alt+B)';
      buildItem.backgroundColor = undefined;
      buildItem.command         = 'knot.build';
      break;
    case 'building':
      buildItem.text            = '$(loading~spin)';
      buildItem.tooltip         = 'knot: Building…';
      buildItem.backgroundColor = undefined;
      buildItem.command         = undefined;
      break;
    case 'success':
      buildItem.text            = '$(pass)';
      buildItem.tooltip         = 'knot: Build succeeded — click to rebuild';
      buildItem.backgroundColor = undefined;
      buildItem.command         = 'knot.build';
      break;
    case 'failed':
      buildItem.text            = '$(error)';
      buildItem.tooltip         = 'knot: Build failed — click to retry';
      buildItem.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
      buildItem.command         = 'knot.build';
      break;
    case 'watching':
      buildItem.text            = '$(loading~spin)';
      buildItem.tooltip         = 'knot: Building (watch mode)…';
      buildItem.backgroundColor = undefined;
      buildItem.command         = undefined;
      break;
  }
  buildItem.show();

  // ── Watch toggle button ───────────────────────────────────────────────────
  if (state.build === 'watching') {
    watchItem.text            = '$(eye) Watch';
    watchItem.tooltip         = 'knot: Watch mode active — click to stop';
    watchItem.backgroundColor = new vscode.ThemeColor('statusBarItem.prominentBackground');
    watchItem.command         = 'knot.stopWatch';
  } else {
    watchItem.text            = '$(eye-closed)';
    watchItem.tooltip         = 'knot: Start watch mode';
    watchItem.backgroundColor = undefined;
    watchItem.command         = 'knot.startWatch';
  }
  watchItem.show();

  // ── Settings button ───────────────────────────────────────────────────────
  settingsItem.show();
}

// ---------------------------------------------------------------------------
// Tooltips
// ---------------------------------------------------------------------------

function lspTooltip(n: number | null): vscode.MarkdownString {
  const lines = ['**knot** ready — click for all commands'];
  if (n !== null) lines.push(`${n} passage${n === 1 ? '' : 's'}`);
  if (state.format) {
    lines.push(`Format: ${state.format}${state.formatVersion ? ` ${state.formatVersion}` : ''}`);
  }
  if (state.ifidMissing) {
    lines.push('', '⚠️ **IFID missing** — tweego cannot compile');
  }
  return new vscode.MarkdownString(lines.join('\n\n'), true);
}