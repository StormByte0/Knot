import * as vscode from 'vscode';
import { TweegoIntegration } from './tweegoIntegration';

// ---------------------------------------------------------------------------
// Single-instance panel
// ---------------------------------------------------------------------------

let currentPanel: vscode.WebviewPanel | undefined;

export function openSettingsPanel(
  context: vscode.ExtensionContext,
  tweego: TweegoIntegration,
): void {
  if (currentPanel) {
    currentPanel.reveal(vscode.ViewColumn.One);
    renderPanel(currentPanel, tweego);
    return;
  }

  const panel = vscode.window.createWebviewPanel(
    'knotSettings', 'knot Settings',
    vscode.ViewColumn.One,
    { enableScripts: true, retainContextWhenHidden: true },
  );

  currentPanel = panel;
  const panelSubs: vscode.Disposable[] = [];

  panel.onDidDispose(() => {
    currentPanel = undefined;
    panelSubs.forEach(d => d.dispose());
  }, null, context.subscriptions);

  panel.webview.onDidReceiveMessage(
    (msg: WebviewMessage) => handleMessage(msg, panel, tweego),
    null, panelSubs,
  );

  panelSubs.push(
    vscode.workspace.onDidChangeConfiguration(e => {
      if (e.affectsConfiguration('knot')) renderPanel(panel, tweego);
    }),
  );

  renderPanel(panel, tweego);
}

// ---------------------------------------------------------------------------
// Config snapshot
// ---------------------------------------------------------------------------

interface PanelConfig {
  execPath:             string;
  outputFile:           string;
  storyFormatsDirectory: string;
  formatOverride:       string;
  modulePaths:          string[];
  headFile:             string;
  noTrim:               boolean;
  logFiles:             boolean;
  extraArgs:            string;
  exclude:              string[];
}

function readPanelConfig(): PanelConfig {
  const project = vscode.workspace.getConfiguration('knot.project');
  const tweego  = vscode.workspace.getConfiguration('knot.tweego');
  return {
    execPath:              tweego.get<string>('path',                   'tweego'),
    outputFile:            tweego.get<string>('outputFile',             'build/index.html'),
    storyFormatsDirectory: project.get<string>('storyFormatsDirectory', '.storyformats'),
    formatOverride:        tweego.get<string>('formatOverride',         ''),
    modulePaths:           tweego.get<string[]>('modulePaths',          []),
    headFile:              tweego.get<string>('headFile',               ''),
    noTrim:                tweego.get<boolean>('noTrim',                false),
    logFiles:              tweego.get<boolean>('logFiles',              false),
    extraArgs:             tweego.get<string>('extraArgs',              ''),
    exclude:               project.get<string[]>('exclude',            []),
  };
}

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

type WebviewMessage =
  | { type: 'updateProjectConfig'; key: string; value: unknown }
  | { type: 'updateTweegoConfig';  key: string; value: unknown }
  | { type: 'browseFile';   target: 'execPath' | 'outputFile' | 'headFile' }
  | { type: 'browseDir';    target: 'storyFormatsDirectory' | 'modulePaths' }
  | { type: 'removeModulePath'; index: number }
  | { type: 'addExclude';   value: string }
  | { type: 'removeExclude'; index: number }
  | { type: 'verifyBinary'; execPath: string }
  | { type: 'listFormats' }
  | { type: 'generateIfid' };

// ---------------------------------------------------------------------------
// Message handler
// ---------------------------------------------------------------------------

async function handleMessage(
  msg: WebviewMessage,
  panel: vscode.WebviewPanel,
  tweego: TweegoIntegration,
): Promise<void> {
  const projectCfg = vscode.workspace.getConfiguration('knot.project');
  const tweegoCfg  = vscode.workspace.getConfiguration('knot.tweego');
  const target     = vscode.ConfigurationTarget.Workspace;

  switch (msg.type) {
    case 'updateProjectConfig':
      await projectCfg.update(msg.key, msg.value, target);
      break;

    case 'updateTweegoConfig':
      await tweegoCfg.update(msg.key, msg.value, target);
      break;

    case 'browseFile': {
      const result = await vscode.window.showOpenDialog({ canSelectMany: false, canSelectFiles: true });
      if (!result?.[0]) break;
      const fsPath = result[0].fsPath;
      if (msg.target === 'execPath')    await tweegoCfg.update('path',       fsPath, target);
      if (msg.target === 'outputFile')  await tweegoCfg.update('outputFile', fsPath, target);
      if (msg.target === 'headFile')    await tweegoCfg.update('headFile',   fsPath, target);
      renderPanel(panel, tweego);
      break;
    }

    case 'browseDir': {
      const result = await vscode.window.showOpenDialog({ canSelectMany: false, canSelectFolders: true });
      if (!result?.[0]) break;
      const fsPath = result[0].fsPath;
      if (msg.target === 'storyFormatsDirectory') {
        await projectCfg.update('storyFormatsDirectory', makeRelative(fsPath), target);
      } else if (msg.target === 'modulePaths') {
        const current = tweegoCfg.get<string[]>('modulePaths', []);
        await tweegoCfg.update('modulePaths', [...current, fsPath], target);
      }
      renderPanel(panel, tweego);
      break;
    }

    case 'removeModulePath': {
      const current = tweegoCfg.get<string[]>('modulePaths', []);
      current.splice(msg.index, 1);
      await tweegoCfg.update('modulePaths', current, target);
      renderPanel(panel, tweego);
      break;
    }

    case 'addExclude': {
      if (!msg.value.trim()) break;
      const current = projectCfg.get<string[]>('exclude', []);
      await projectCfg.update('exclude', [...current, msg.value.trim()], target);
      renderPanel(panel, tweego);
      break;
    }

    case 'removeExclude': {
      const current = projectCfg.get<string[]>('exclude', []);
      current.splice(msg.index, 1);
      await projectCfg.update('exclude', current, target);
      renderPanel(panel, tweego);
      break;
    }

    case 'verifyBinary': {
      panel.webview.postMessage({ type: 'verifyStart' });
      const result = await tweego.verifyBinaryAt(msg.execPath.trim() || 'tweego');
      panel.webview.postMessage({ type: 'verifyResult', ok: result.ok, version: result.version });
      break;
    }

    case 'listFormats': {
      panel.webview.postMessage({ type: 'formatsStart' });
      const formats = await tweego.listFormats();
      panel.webview.postMessage({ type: 'formatsResult', formats });
      break;
    }

    case 'generateIfid': {
      const ifid = generateUuidV4().toUpperCase();
      await vscode.env.clipboard.writeText(ifid);
      panel.webview.postMessage({ type: 'ifidResult', ifid });
      break;
    }
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeRelative(absPath: string): string {
  const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!root) return absPath;
  const rel = absPath.startsWith(root)
    ? absPath.slice(root.length).replace(/^[\\/]/, '')
    : absPath;
  return rel || absPath;
}

function renderPanel(panel: vscode.WebviewPanel, _tweego: TweegoIntegration): void {
  panel.webview.html = buildHtml(readPanelConfig());
}

function generateUuidV4(): string {
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, c => {
    const r = (Math.random() * 16) | 0;
    return (c === 'x' ? r : (r & 0x3) | 0x8).toString(16);
  });
}

function esc(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

function buildHtml(cfg: PanelConfig): string {
  const modulePathTags = cfg.modulePaths.map((p, i) => `
    <div class="tag">
      <span>${esc(p)}</span>
      <button class="tag-remove" onclick="send({type:'removeModulePath',index:${i}})" title="Remove">×</button>
    </div>`).join('');

  const excludeTags = cfg.exclude.map((p, i) => `
    <div class="tag">
      <span>${esc(p)}</span>
      <button class="tag-remove" onclick="send({type:'removeExclude',index:${i}})" title="Remove">×</button>
    </div>`).join('');

  return /* html */`<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>knot Settings</title>
<style>
  *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

  body {
    font-family: var(--vscode-font-family);
    font-size: var(--vscode-font-size);
    color: var(--vscode-foreground);
    background: var(--vscode-editor-background);
    padding-bottom: 3rem;
  }

  /* ── Layout ── */
  .page { max-width: 680px; margin: 0 auto; padding: 0 24px; }

  .page-header {
    padding: 20px 0 16px;
    border-bottom: 1px solid var(--vscode-widget-border);
    margin-bottom: 8px;
  }
  .page-header h1 {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: .01em;
  }
  .page-header p {
    margin-top: 4px;
    font-size: 12px;
    color: var(--vscode-descriptionForeground);
  }

  /* ── Sections ── */
  .section { margin-top: 24px; }

  .section-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 14px;
  }
  .section-header h2 {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: .08em;
    color: var(--vscode-descriptionForeground);
  }
  .section-divider {
    flex: 1;
    height: 1px;
    background: var(--vscode-widget-border);
  }

  /* ── Fields ── */
  .field { margin-bottom: 16px; }
  .field:last-child { margin-bottom: 0; }

  .field-label {
    display: block;
    font-size: 12px;
    font-weight: 500;
    margin-bottom: 5px;
  }
  .field-hint {
    font-size: 11px;
    color: var(--vscode-descriptionForeground);
    margin-top: 4px;
    line-height: 1.5;
  }
  .field-hint code {
    font-family: var(--vscode-editor-font-family);
    background: var(--vscode-textCodeBlock-background);
    padding: 0 3px;
    border-radius: 2px;
  }

  /* ── Inputs ── */
  .input-row { display: flex; gap: 6px; align-items: stretch; }

  input[type="text"] {
    flex: 1;
    height: 28px;
    padding: 0 8px;
    background: var(--vscode-input-background);
    color: var(--vscode-input-foreground);
    border: 1px solid var(--vscode-input-border, transparent);
    border-radius: 2px;
    font-family: var(--vscode-editor-font-family);
    font-size: 12px;
    outline: none;
  }
  input[type="text"]:focus {
    border-color: var(--vscode-focusBorder);
  }
  input[type="text"]::placeholder {
    color: var(--vscode-input-placeholderForeground);
  }

  /* ── Buttons ── */
  button {
    height: 28px;
    padding: 0 10px;
    background: var(--vscode-button-secondaryBackground, transparent);
    color: var(--vscode-button-secondaryForeground, var(--vscode-foreground));
    border: 1px solid var(--vscode-button-border, var(--vscode-widget-border));
    border-radius: 2px;
    cursor: pointer;
    font-size: 12px;
    font-family: var(--vscode-font-family);
    white-space: nowrap;
    transition: background 0.1s;
  }
  button:hover { background: var(--vscode-list-hoverBackground); }
  button:active { opacity: .8; }
  button:disabled { opacity: .45; cursor: default; }

  button.primary {
    background: var(--vscode-button-background);
    color: var(--vscode-button-foreground);
    border-color: transparent;
  }
  button.primary:hover { background: var(--vscode-button-hoverBackground); }

  button.danger {
    background: transparent;
    color: var(--vscode-errorForeground);
    border-color: var(--vscode-inputValidation-errorBorder);
  }

  /* ── Status badges ── */
  .status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 3px;
    margin-top: 6px;
    min-height: 26px;
  }
  .status.hidden { display: none; }
  .status.loading {
    color: var(--vscode-descriptionForeground);
    background: var(--vscode-input-background);
  }
  .status.ok {
    color: var(--vscode-testing-iconPassed, #4ec9b0);
    background: color-mix(in srgb, var(--vscode-testing-iconPassed, #4ec9b0) 12%, transparent);
  }
  .status.error {
    color: var(--vscode-errorForeground);
    background: var(--vscode-inputValidation-errorBackground);
  }
  .status.info {
    color: var(--vscode-foreground);
    background: var(--vscode-input-background);
  }
  .spinner {
    width: 12px; height: 12px;
    border: 2px solid currentColor;
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin .7s linear infinite;
    flex-shrink: 0;
  }
  @keyframes spin { to { transform: rotate(360deg); } }

  /* ── Toggle rows ── */
  .toggle-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    padding: 10px 12px;
    background: var(--vscode-input-background);
    border: 1px solid var(--vscode-input-border, transparent);
    border-radius: 3px;
    margin-bottom: 8px;
  }
  .toggle-row:last-child { margin-bottom: 0; }
  .toggle-label { font-size: 12px; font-weight: 500; }
  .toggle-hint { font-size: 11px; color: var(--vscode-descriptionForeground); margin-top: 2px; }

  input[type="checkbox"] {
    width: 16px; height: 16px;
    flex-shrink: 0;
    margin-top: 2px;
    accent-color: var(--vscode-focusBorder);
    cursor: pointer;
  }

  /* ── Tags ── */
  .tag-list { display: flex; flex-wrap: wrap; gap: 5px; margin-bottom: 8px; min-height: 0; }
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    background: var(--vscode-badge-background);
    color: var(--vscode-badge-foreground);
    border-radius: 12px;
    padding: 2px 4px 2px 10px;
    font-size: 11px;
    font-family: var(--vscode-editor-font-family);
    max-width: 100%;
  }
  .tag span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tag-remove {
    background: none;
    border: none;
    color: inherit;
    font-size: 15px;
    line-height: 1;
    height: auto;
    padding: 0 4px;
    opacity: .7;
    cursor: pointer;
  }
  .tag-remove:hover { opacity: 1; background: none; }

  /* ── Formats list ── */
  .formats-list {
    margin-top: 6px;
    background: var(--vscode-input-background);
    border: 1px solid var(--vscode-input-border, transparent);
    border-radius: 3px;
    max-height: 160px;
    overflow-y: auto;
    display: none;
  }
  .format-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 10px;
    cursor: pointer;
    font-size: 12px;
    border-bottom: 1px solid var(--vscode-widget-border);
  }
  .format-item:last-child { border-bottom: none; }
  .format-item:hover { background: var(--vscode-list-hoverBackground); }
  .format-item .format-id { font-family: var(--vscode-editor-font-family); font-weight: 500; }
  .format-item .format-meta { font-size: 11px; color: var(--vscode-descriptionForeground); }

  /* ── IFID box ── */
  .ifid-box {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    background: var(--vscode-inputValidation-warningBackground, rgba(204,167,0,.1));
    border: 1px solid var(--vscode-inputValidation-warningBorder, #cca700);
    border-radius: 3px;
  }
  .ifid-box p { flex: 1; font-size: 12px; line-height: 1.5; }
  .ifid-result {
    margin-top: 8px;
    padding: 6px 10px;
    background: var(--vscode-input-background);
    border: 1px solid var(--vscode-input-border, transparent);
    border-radius: 3px;
    font-family: var(--vscode-editor-font-family);
    font-size: 12px;
    display: none;
  }
  .ifid-result .ifid-value { font-weight: 600; letter-spacing: .05em; }
  .ifid-result .ifid-note { color: var(--vscode-descriptionForeground); margin-top: 2px; font-size: 11px; }
</style>
</head>
<body>
<div class="page">

  <div class="page-header">
    <h1>knot Language Server — Settings</h1>
    <p>Configure the Tweego compiler and project structure. Changes are saved to your workspace settings.</p>
  </div>

  <!-- ══ Tweego compiler ══════════════════════════════════════════════════ -->
  <div class="section">
    <div class="section-header">
      <h2>Tweego compiler</h2>
      <div class="section-divider"></div>
    </div>

    <div class="field">
      <label class="field-label" for="execPath">Executable path</label>
      <div class="input-row">
        <input type="text" id="execPath" value="${esc(cfg.execPath)}" placeholder="tweego"
          oninput="debounce('exec', () => send({type:'updateTweegoConfig', key:'path', value:this.value}))">
        <button onclick="send({type:'browseFile', target:'execPath'})">Browse…</button>
        <button class="primary" id="verifyBtn" onclick="verify()">Verify</button>
      </div>
      <div id="verifyStatus" class="status hidden"></div>
      <p class="field-hint">Leave as <code>tweego</code> if it is on your PATH, or use Browse to locate the binary.</p>
    </div>

    <div class="field">
      <label class="field-label" for="outputFile">Output file</label>
      <div class="input-row">
        <input type="text" id="outputFile" value="${esc(cfg.outputFile)}" placeholder="build/index.html"
          oninput="debounce('output', () => send({type:'updateTweegoConfig', key:'outputFile', value:this.value}))">
        <button onclick="send({type:'browseFile', target:'outputFile'})">Browse…</button>
      </div>
      <p class="field-hint">Path to the compiled HTML file, relative to the workspace root.</p>
    </div>

    <div class="field">
      <label class="field-label" for="storyFormatsDir">Story formats directory</label>
      <div class="input-row">
        <input type="text" id="storyFormatsDir" value="${esc(cfg.storyFormatsDirectory)}" placeholder=".storyformats"
          oninput="debounce('fmtDir', () => send({type:'updateProjectConfig', key:'storyFormatsDirectory', value:this.value}))">
        <button onclick="send({type:'browseDir', target:'storyFormatsDirectory'})">Browse…</button>
      </div>
      <p class="field-hint">Folder containing SugarCube (and other) story format packages. Added to <code>TWEEGO_PATH</code> automatically.</p>
    </div>

    <div class="field">
      <label class="field-label" for="formatOverride">Story format override</label>
      <div class="input-row">
        <input type="text" id="formatOverride" value="${esc(cfg.formatOverride)}" placeholder="(read from StoryData)"
          oninput="debounce('fmt', () => send({type:'updateTweegoConfig', key:'formatOverride', value:this.value}))">
        <button id="listFormatsBtn" onclick="listFormats()">List formats</button>
      </div>
      <div id="formatsList" class="formats-list"></div>
      <p class="field-hint">Overrides the format declared in StoryData. Leave empty to use StoryData.</p>
    </div>
  </div>

  <!-- ══ Build options ════════════════════════════════════════════════════ -->
  <div class="section">
    <div class="section-header">
      <h2>Build options</h2>
      <div class="section-divider"></div>
    </div>

    <div class="toggle-row">
      <div>
        <div class="toggle-label">Trim passage whitespace</div>
        <div class="toggle-hint">Strip leading/trailing whitespace from passage content (tweego default: on).</div>
      </div>
      <input type="checkbox" ${cfg.noTrim ? '' : 'checked'}
        onchange="send({type:'updateTweegoConfig', key:'noTrim', value:!this.checked})" title="Trim whitespace">
    </div>

    <div class="toggle-row">
      <div>
        <div class="toggle-label">Log processed files</div>
        <div class="toggle-hint">Print every input file tweego processes to the knot output channel (<code>--log-files</code>).</div>
      </div>
      <input type="checkbox" ${cfg.logFiles ? 'checked' : ''}
        onchange="send({type:'updateTweegoConfig', key:'logFiles', value:this.checked})" title="Log files">
    </div>

    <div class="field" style="margin-top:12px">
      <label class="field-label" for="extraArgs">Extra arguments</label>
      <input type="text" id="extraArgs" style="width:100%" value="${esc(cfg.extraArgs)}" placeholder="e.g. --twee2-compat"
        oninput="debounce('extra', () => send({type:'updateTweegoConfig', key:'extraArgs', value:this.value}))">
      <p class="field-hint">Appended verbatim to every tweego invocation.</p>
    </div>
  </div>

  <!-- ══ Module & head injection ══════════════════════════════════════════ -->
  <div class="section">
    <div class="section-header">
      <h2>Module &amp; head injection</h2>
      <div class="section-divider"></div>
    </div>

    <div class="field">
      <label class="field-label">Module directories <code>-m</code></label>
      <div class="tag-list">${modulePathTags}</div>
      <button onclick="send({type:'browseDir', target:'modulePaths'})">+ Add directory</button>
      <p class="field-hint">CSS, JS, and font files bundled into <code>&lt;head&gt;</code> of the compiled HTML.</p>
    </div>

    <div class="field">
      <label class="field-label" for="headFile">Head injection file <code>--head</code></label>
      <div class="input-row">
        <input type="text" id="headFile" value="${esc(cfg.headFile)}" placeholder="(none)"
          oninput="debounce('head', () => send({type:'updateTweegoConfig', key:'headFile', value:this.value}))">
        <button onclick="send({type:'browseFile', target:'headFile'})">Browse…</button>
      </div>
      <p class="field-hint">Contents of this file are appended verbatim to <code>&lt;head&gt;</code>.</p>
    </div>
  </div>

  <!-- ══ Project ══════════════════════════════════════════════════════════ -->
  <div class="section">
    <div class="section-header">
      <h2>Project</h2>
      <div class="section-divider"></div>
    </div>

    <div class="field">
      <label class="field-label">Exclude patterns</label>
      <div class="tag-list">${excludeTags}</div>
      <div class="input-row">
        <input type="text" id="excludeInput" placeholder="e.g. vendor/** or *.generated.twee"
          onkeydown="if(event.key==='Enter') addExclude()">
        <button onclick="addExclude()">Add</button>
      </div>
      <p class="field-hint">Glob patterns for files to skip during indexing and compilation.</p>
    </div>
  </div>

  <!-- ══ IFID ═════════════════════════════════════════════════════════════ -->
  <div class="section">
    <div class="section-header">
      <h2>IFID helper</h2>
      <div class="section-divider"></div>
    </div>

    <div class="ifid-box">
      <p>Missing an IFID? Tweego requires one in <code>StoryData</code> to compile. Generate one and paste it into your <code>StoryData</code> passage.</p>
      <button class="primary" onclick="genIfid()">Generate IFID</button>
    </div>
    <div id="ifidResult" class="ifid-result"></div>
  </div>

</div><!-- /page -->

<script>
const vscode = acquireVsCodeApi();
const timers = {};

function send(msg) { vscode.postMessage(msg); }

function debounce(key, fn) {
  clearTimeout(timers[key]);
  timers[key] = setTimeout(fn, 400);
}

function verify() {
  const execPath = document.getElementById('execPath').value;
  const btn = document.getElementById('verifyBtn');
  btn.disabled = true;
  send({ type: 'verifyBinary', execPath });
}

function listFormats() {
  const btn = document.getElementById('listFormatsBtn');
  btn.disabled = true;
  const list = document.getElementById('formatsList');
  list.style.display = 'block';
  list.innerHTML = '<div style="padding:8px 10px;font-size:12px;color:var(--vscode-descriptionForeground)">Loading formats…</div>';
  send({ type: 'listFormats' });
}

function pickFormat(id) {
  document.getElementById('formatOverride').value = id;
  send({ type: 'updateTweegoConfig', key: 'formatOverride', value: id });
  document.getElementById('formatsList').style.display = 'none';
}

function addExclude() {
  const el = document.getElementById('excludeInput');
  const v = el.value.trim();
  if (!v) return;
  send({ type: 'addExclude', value: v });
  el.value = '';
}

function genIfid() {
  send({ type: 'generateIfid' });
}

function showStatus(id, kind, html) {
  const el = document.getElementById(id);
  el.className = 'status ' + kind;
  el.innerHTML = html;
}

window.addEventListener('message', e => {
  const msg = e.data;

  if (msg.type === 'verifyStart') {
    showStatus('verifyStatus', 'loading',
      '<span class="spinner"></span><span>Verifying…</span>');
  }

  if (msg.type === 'verifyResult') {
    document.getElementById('verifyBtn').disabled = false;
    if (msg.ok) {
      showStatus('verifyStatus', 'ok',
        '<span>✓</span><span>Found — tweego v' + esc(msg.version) + '</span>');
    } else {
      showStatus('verifyStatus', 'error',
        '<span>✗</span><span>Not found or failed to run. Check the path above.</span>');
    }
  }

  if (msg.type === 'formatsStart') {
    // handled inline in listFormats()
  }

  if (msg.type === 'formatsResult') {
    document.getElementById('listFormatsBtn').disabled = false;
    const list = document.getElementById('formatsList');
    if (!msg.formats || msg.formats.length === 0) {
      list.innerHTML = '<div style="padding:8px 10px;font-size:12px;color:var(--vscode-descriptionForeground)">No formats found. Check your story formats directory.</div>';
      return;
    }
    list.innerHTML = msg.formats.map(f =>
      '<div class="format-item" onclick="pickFormat(' + JSON.stringify(f.id) + ')">' +
        '<span class="format-id">' + esc(f.id) + '</span>' +
        '<span class="format-meta">' + esc(f.name) + ' ' + esc(f.version) + '</span>' +
      '</div>'
    ).join('');
  }

  if (msg.type === 'ifidResult') {
    const el = document.getElementById('ifidResult');
    el.style.display = 'block';
    el.innerHTML =
      '<div class="ifid-value">' + esc(msg.ifid) + '</div>' +
      '<div class="ifid-note">Copied to clipboard — paste into your StoryData passage.</div>';
  }
});

function esc(s) {
  return String(s)
    .replace(/&/g,'&amp;')
    .replace(/</g,'&lt;')
    .replace(/>/g,'&gt;')
    .replace(/"/g,'&quot;');
}
</script>
</body>
</html>`;
}