import { useState, useEffect, useCallback, useRef, Component, ReactNode } from 'react';
import {
  VsCodeApi,
  KnotGraphResponse,
  WebviewInboundMessage,
  PassageNodeData,
  PassageMetadataPatch,
  KnotGraphNode,
  WorkspaceFileInfo,
  EdgeKind,
} from './types';
import StoryMap from './components/StoryMap';
import Legend from './components/Legend';
import Inspector, { InspectorSelectedNode, InspectorFocusSignal } from './components/Inspector';
import CreatePassageModal, { CreatePassageRequest } from './components/CreatePassageModal';
import FindPalette from './components/FindPalette';
import FileNavigator from './components/FileNavigator';

// Acquire the VS Code API — must be called once per webview lifetime
const vscode: VsCodeApi = acquireVsCodeApi();

export { vscode };

// ── Console-to-extension logging bridge ─────────────────────────────────────

const originalConsoleError = console.error;
const originalConsoleWarn = console.warn;

console.error = (...args: unknown[]) => {
  originalConsoleError.apply(console, args);
  try {
    vscode.postMessage({
      command: 'log',
      level: 'error',
      message: args.map(a => {
        if (a instanceof Error) return a.stack || a.message;
        if (typeof a === 'object' && a !== null) try { return JSON.stringify(a); } catch { return String(a); }
        return String(a);
      }).join(' '),
    });
  } catch { /* best effort */ }
};

console.warn = (...args: unknown[]) => {
  originalConsoleWarn.apply(console, args);
  try {
    vscode.postMessage({
      command: 'log',
      level: 'warn',
      message: args.map(a => {
        if (a instanceof Error) return a.stack || a.message;
        if (typeof a === 'object' && a !== null) try { return JSON.stringify(a); } catch { return String(a); }
        return String(a);
      }).join(' '),
    });
  } catch { /* best effort */ }
};

window.addEventListener('error', (event) => {
  try {
    vscode.postMessage({
      command: 'log',
      level: 'error',
      message: `[Unhandled] ${event.message} at ${event.filename}:${event.lineno}:${event.colno}`,
    });
  } catch { /* best effort */ }
});

window.addEventListener('unhandledrejection', (event) => {
  try {
    vscode.postMessage({
      command: 'log',
      level: 'error',
      message: `[Unhandled Rejection] ${String(event.reason)}`,
    });
  } catch { /* best effort */ }
});

// ── Error Boundary ──────────────────────────────────────────────────────────

interface ErrorBoundaryState {
  hasError: boolean;
  error: string;
}

class StoryMapErrorBoundary extends Component<{ children: ReactNode }, ErrorBoundaryState> {
  state: ErrorBoundaryState = { hasError: false, error: '' };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error: error.stack || error.message || String(error) };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    originalConsoleError('[StoryMap] React Error Boundary caught:', error, info.componentStack);
    try {
      vscode.postMessage({
        command: 'log',
        level: 'error',
        message: `[ErrorBoundary] ${error.stack || error.message}\n${info.componentStack}`,
      });
    } catch { /* best effort */ }
  }

  render() {
    if (this.state.hasError) {
      return (
        <div style={{
          padding: '16px',
          color: 'var(--vscode-errorForeground, #f14c4c)',
          fontFamily: 'var(--vscode-font-family, monospace)',
          fontSize: '12px',
          whiteSpace: 'pre-wrap',
          overflow: 'auto',
          height: '100%',
        }}>
          <div style={{ fontWeight: 'bold', marginBottom: '8px' }}>Story Map Error</div>
          <div>{this.state.error}</div>
          <div style={{ marginTop: '12px', color: 'var(--vscode-descriptionForeground, #888)' }}>
            Check the &quot;Knot Story Map&quot; output channel for more details.
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

// ── Group name modal ────────────────────────────────────────────────────────
//
// Tiny prompt used for "Group selected N passages…" (context menu / G key).
// Prefills with the first member's existing group, mirroring Twine.

function GroupNameModal({
  memberCount,
  prefill,
  existingGroups,
  onCancel,
  onSubmit,
}: {
  memberCount: number;
  prefill?: string;
  existingGroups: string[];
  onCancel: () => void;
  onSubmit: (name: string) => void;
}) {
  const [name, setName] = useState(prefill ?? '');
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  const submit = () => {
    const trimmed = name.trim();
    if (!trimmed) {
      setError('Group name is required');
      return;
    }
    if (/[[\]{}]/.test(trimmed)) {
      setError("Group name cannot contain '[', ']', '{' or '}'");
      return;
    }
    onSubmit(trimmed);
  };

  return (
    <div
      className="modal-backdrop"
      onMouseDown={e => { if (e.target === e.currentTarget) onCancel(); }}
    >
      <div
        className="modal modal--compact"
        onKeyDown={e => {
          if (e.key === 'Escape') onCancel();
          if (e.key === 'Enter') { e.preventDefault(); submit(); }
        }}
      >
        <div className="modal__title">Group {memberCount} passages</div>
        <div className="modal__subtitle">
          Every selected passage gets this group in its header metadata.
        </div>
        <input
          ref={inputRef}
          className="modal__input"
          list="group-name-suggestions"
          value={name}
          onChange={e => { setName(e.target.value); setError(null); }}
          placeholder="Group name"
          spellCheck={false}
        />
        <datalist id="group-name-suggestions">
          {existingGroups.map(g => <option key={g} value={g} />)}
        </datalist>
        {error && <div className="modal__error">{error}</div>}
        <div className="modal__actions">
          <button className="inspector__btn" onClick={onCancel}>Cancel</button>
          <button className="inspector__btn inspector__btn--primary" onClick={submit}>
            Create group
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Main App component ──────────────────────────────────────────────────────

export default function App() {
  const [graphData, setGraphData] = useState<KnotGraphResponse | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedTags, setSelectedTags] = useState<Set<string>>(new Set());

  // ── Selection state (multi-select aware) ────────────────────────────
  const [selectedNodes, setSelectedNodes] = useState<InspectorSelectedNode[]>([]);
  const [focusSignal, setFocusSignal] = useState<InspectorFocusSignal | null>(null);

  // ── Find palette ────────────────────────────────────────────────────
  const [findOpen, setFindOpen] = useState(false);

  // ── View toggles (context menu) ─────────────────────────────────────
  const [showMiniMap, setShowMiniMap] = useState(true);
  const [showLegend, setShowLegend] = useState(true);
  const [snapEnabled, setSnapEnabled] = useState(true);
  const [hiddenEdgeKinds, setHiddenEdgeKinds] = useState<Set<EdgeKind>>(new Set());

  // ── Workspace files (file navigator + create-passage picker) ────────
  const [tweeFiles, setTweeFiles] = useState<WorkspaceFileInfo[]>([]);
  const [jsFiles, setJsFiles] = useState<WorkspaceFileInfo[]>([]);
  const [cssFiles, setCssFiles] = useState<WorkspaceFileInfo[]>([]);
  // `knot.build.sourceDir` (workspace-relative, '' = root): the base for
  // new files created from the passage modal's "New file…" option.
  const [sourceDir, setSourceDir] = useState('');

  // ── Create-passage modal state ──────────────────────────────────────
  const [createRequest, setCreateRequest] = useState<CreatePassageRequest | null>(null);

  // ── Group-create modal state ────────────────────────────────────────
  const [groupModal, setGroupModal] = useState<{ members: string[]; prefill?: string } | null>(null);

  // ── First-run hint toast ────────────────────────────────────────────
  const [hintVisible, setHintVisible] = useState(true);
  useEffect(() => {
    const t = setTimeout(() => setHintVisible(false), 9000);
    return () => clearTimeout(t);
  }, []);

  // Collect all unique tags from graph data
  const allTags: string[] = [];
  if (graphData?.nodes) {
    const tagSet = new Set<string>();
    for (const n of graphData.nodes) {
      for (const t of n.tags || []) {
        tagSet.add(t);
      }
    }
    for (const t of tagSet) allTags.push(t);
    allTags.sort();
  }

  // Collect groups and passage names for the inspector's autocomplete.
  const allGroups: string[] = [];
  const allPassageNames: string[] = [];
  if (graphData?.nodes) {
    const groupSet = new Set<string>();
    for (const n of graphData.nodes) {
      if (n.group) groupSet.add(n.group);
      if (!n.is_metadata && !n.is_placeholder) allPassageNames.push(n.label);
    }
    for (const g of groupSet) allGroups.push(g);
    allGroups.sort();
    allPassageNames.sort();
  }

  // The heuristic default target file for new passages: StoryData's file,
  // else the twee file with the most passages (server picks the same when
  // the request doesn't name a file).
  const defaultTargetFile = (() => {
    const real = tweeFiles.filter(f => f.kind === 'twee');
    if (real.length === 0) return undefined;
    const storyDataFile = graphData?.nodes.find(n => n.label === 'StoryData')?.file;
    const byCount = [...real].sort((a, b) => b.passageCount - a.passageCount)[0];
    return storyDataFile && real.find(f => f.uri === storyDataFile)
      ? storyDataFile
      : byCount?.uri;
  })();

  const handleTagToggle = useCallback((tag: string) => {
    setSelectedTags(prev => {
      const next = new Set(prev);
      if (next.has(tag)) {
        next.delete(tag);
      } else {
        next.add(tag);
      }
      return next;
    });
  }, []);

  const handleTagClear = useCallback(() => {
    setSelectedTags(new Set());
  }, []);

  const handleToggleEdgeKind = useCallback((kind: EdgeKind) => {
    setHiddenEdgeKinds(prev => {
      const next = new Set(prev);
      if (next.has(kind)) {
        next.delete(kind);
      } else {
        next.add(kind);
      }
      return next;
    });
  }, []);

  // FIX: restoreViewport is sent by the extension after panel creation to
  // re-apply the workspace-persisted viewport. We store it and forward it
  // to StoryMap via a dedicated prop pair (timestamp + payload).
  const [restoreViewport, setRestoreViewport] = useState<
    { x: number; y: number; zoom: number; ts: number } | null
  >(null);

  // Listen for messages from the extension
  useEffect(() => {
    const handler = (event: MessageEvent) => {
      const msg = event.data as WebviewInboundMessage;

      if (msg && typeof msg === 'object' && 'command' in msg) {
        switch (msg.command) {
          case 'updateGraph': {
            const data = msg.data;
            console.log(
              '[StoryMap] Received updateGraph:',
              data?.nodes?.length,
              'nodes,',
              data?.edges?.length,
              'edges',
            );
            setGraphData(data);
            break;
          }
          case 'focusNode': {
            // Editor-cursor driven focus: pan + select, but don't record a
            // graph-navigation history entry (the code side owns this hop).
            setFocusPassageName(msg.passageName);
            setFocusPushHistory(false);
            setFocusRequested(Date.now());
            break;
          }
          case 'restoreViewport': {
            const { x, y, zoom } = msg;
            setRestoreViewport({ x, y, zoom, ts: Date.now() });
            break;
          }
          case 'updateWorkspaceFiles': {
            setTweeFiles(msg.tweeFiles ?? []);
            setJsFiles(msg.jsFiles ?? []);
            setCssFiles(msg.cssFiles ?? []);
            setSourceDir(msg.sourceDir ?? '');
            break;
          }
        }
      }
    };

    window.addEventListener('message', handler);
    return () => window.removeEventListener('message', handler);
  }, []);

  // Request initial graph data on mount
  useEffect(() => {
    console.log('[StoryMap] App mounted, requesting initial graph data');
    vscode.postMessage({ command: 'refreshGraph' });
  }, []);

  const handleRefresh = useCallback(() => {
    vscode.postMessage({ command: 'refreshGraph' });
  }, []);

  const handleFit = useCallback(() => {
    setFitRequested(Date.now());
  }, []);

  const [fitRequested, setFitRequested] = useState(0);

  const handleSearchChange = useCallback((query: string) => {
    setSearchQuery(query);
  }, []);

  const [saveRequested, setSaveRequested] = useState(0);

  const handleSavePositions = useCallback(() => {
    setSaveRequested(Date.now());
  }, []);

  const [focusRequested, setFocusRequested] = useState(0);
  const [focusPassageName, setFocusPassageName] = useState('');
  const [focusPushHistory, setFocusPushHistory] = useState(false);

  // ── Canvas selection clear signal (inspector close button) ───────────
  // The Inspector's ✕ needs to drop BOTH the app-side selection and the
  // React Flow node flags (otherwise re-clicking the same single node
  // wouldn't re-fire onSelectionChange and the panel would stay hidden).
  const [clearSelRequested, setClearSelRequested] = useState(0);

  // ── File navigator ──────────────────────────────────────────────────

  const handleOpenFile = useCallback((file: string) => {
    vscode.postMessage({ command: 'openFile', file });
  }, []);

  // File owning the primary selection (highlight bridge in the navigator).
  const activeFile = selectedNodes.length === 1 ? selectedNodes[0].data.file : undefined;

  // ── Inspector actions ───────────────────────────────────────────────

  const handleUpdateMetadata = useCallback((passageName: string, patch: PassageMetadataPatch) => {
    vscode.postMessage({
      command: 'updatePassageMetadata',
      passage_name: passageName,
      ...patch,
    });
  }, []);

  const handleBulkMetadata = useCallback((
    names: string[],
    patch: { color?: string | null; group?: string | null; size?: { width: number; height: number } | null },
  ) => {
    vscode.postMessage({
      command: 'bulkUpdateMetadata',
      passage_names: names,
      ...patch,
    });
  }, []);

  const handleRename = useCallback((name: string, newName: string) => {
    vscode.postMessage({
      command: 'renamePassage',
      name,
      new_name: newName,
    });
  }, []);

  const handleDelete = useCallback((name: string, stripLinks: boolean) => {
    vscode.postMessage({
      command: 'deletePassage',
      passage_name: name,
      strip_links: stripLinks,
    });
    // Drop the deleted passage from the local selection immediately (the
    // graph refresh will confirm); keeps multi-delete responsive.
    setSelectedNodes(prev => prev.filter(n => n.data.label !== name));
  }, []);

  const handleSetStart = useCallback((name: string) => {
    vscode.postMessage({
      command: 'setStartPassage',
      passage_name: name,
    });
  }, []);

  const handleOpenPassage = useCallback((file: string, line: number, passageName: string) => {
    vscode.postMessage({ command: 'openPassage', file, line, passageName });
  }, []);

  const handleActionSignal = useCallback((
    field: 'rename' | 'reachable' | 'delete' | 'tags' | 'color' | 'group' | 'groupCreate',
  ) => {
    if (field === 'groupCreate') {
      const members = selectedNodes.map(n => n.data.label);
      if (members.length < 2) return;
      setGroupModal({
        members,
        prefill: selectedNodes.find(n => n.data.group)?.data.group,
      });
      return;
    }
    setFocusSignal({ field, ts: Date.now() });
  }, [selectedNodes]);

  const handleGroupSubmit = useCallback((name: string) => {
    if (groupModal && groupModal.members.length > 0) {
      handleBulkMetadata(groupModal.members, { group: name });
    }
    setGroupModal(null);
  }, [groupModal, handleBulkMetadata]);

  // ── Find palette navigation ─────────────────────────────────────────

  const handlePaletteNavigate = useCallback((node: KnotGraphNode) => {
    vscode.postMessage({ command: 'openPassage', file: node.file, line: node.line, passageName: node.label });
    setFocusPassageName(node.label);
    setFocusPushHistory(true);
    setFocusRequested(Date.now());
  }, []);

  // ── Create-passage actions ──────────────────────────────────────────

  // Live viewport mirror (updated on every pan/zoom by StoryMap; stored
  // in a ref so no re-render happens). Used to anchor context-menu-created
  // passages at the current view center.
  const viewportRef = useRef({ x: 0, y: 0, zoom: 1 });
  const handleViewportKnown = useCallback((vp: { x: number; y: number; zoom: number }) => {
    viewportRef.current = vp;
  }, []);

  const handleCreatePassageAt = useCallback((x: number, y: number, prefillName?: string) => {
    setCreateRequest({ x, y, prefillName });
  }, []);

  const handleCreatePassageAtViewCenter = useCallback(() => {
    // Convert the viewport center to graph coordinates: the canvas
    // element's bounding rect gives the exact visible area.
    const canvas = document.querySelector('.storymap-inner');
    const rect = canvas?.getBoundingClientRect();
    const cx = rect ? rect.left + rect.width / 2 : window.innerWidth / 2;
    const cy = rect ? rect.top + rect.height / 2 : window.innerHeight / 2;
    const vp = viewportRef.current;
    const x = (cx - vp.x) / vp.zoom - 80; // minus half the default node width
    const y = (cy - vp.y) / vp.zoom - 18; // minus half the default node height
    setCreateRequest({ x: Math.round(x), y: Math.round(y) });
  }, []);

  const handleCreatePassage = useCallback((
    name: string,
    tags: string[],
    position: { x: number; y: number },
    target: { file?: string; newFile?: string } = {},
  ) => {
    vscode.postMessage({
      command: 'createPassage',
      name,
      tags,
      position,
      target_file: target.file,
      new_file: target.newFile,
    });
    setCreateRequest(null);
  }, []);

  const selectionCount = selectedNodes.length;

  return (
    <StoryMapErrorBoundary>
      <div className="app-container">
        <FileNavigator
          tweeFiles={tweeFiles}
          jsFiles={jsFiles}
          cssFiles={cssFiles}
          activeFile={activeFile}
          onOpenFile={handleOpenFile}
        />
        <div className="app-main">
          <StoryMap
            graphData={graphData}
            searchQuery={searchQuery}
            selectedTags={selectedTags}
            fitRequested={fitRequested}
            saveRequested={saveRequested}
            focusRequested={focusRequested}
            focusPassageName={focusPassageName}
            focusPushHistory={focusPushHistory}
            clearSelectionRequested={clearSelRequested}
            restoreViewport={restoreViewport}
            showMiniMap={showMiniMap}
            showLegend={showLegend}
            onToggleMiniMap={() => setShowMiniMap(v => !v)}
            onToggleLegend={() => setShowLegend(v => !v)}
            snapEnabled={snapEnabled}
            onToggleSnap={() => setSnapEnabled(v => !v)}
            hiddenEdgeKinds={hiddenEdgeKinds}
            onFindPassages={() => setFindOpen(true)}
            onNewPassageAtViewCenter={handleCreatePassageAtViewCenter}
            onSelectionChange={setSelectedNodes}
            onOpenPassage={handleOpenPassage}
            onCreatePassageAt={handleCreatePassageAt}
            onSetStart={handleSetStart}
            onActionSignal={handleActionSignal}
            onRequestRefresh={handleRefresh}
            onRequestSaveLayout={handleSavePositions}
            onViewportKnown={handleViewportKnown}
          />
          {selectionCount > 1 && (
            <div className="sel-chip">
              {selectionCount} selected — drag, <kbd>Del</kbd>, <kbd>G</kbd> to group, or edit in Inspector
            </div>
          )}
          {/* Canvas-anchored overlays live INSIDE .app-main so they offset
              from the canvas area, not the whole webview — the file-nav
              rail (and its expand toggle) sits to the left and can never
              sit under the find trigger or the legend again. */}
          <FindPalette
            open={findOpen}
            onOpenChange={setFindOpen}
            nodes={graphData?.nodes ?? []}
            searchQuery={searchQuery}
            onSearchChange={handleSearchChange}
            allTags={allTags}
            selectedTags={selectedTags}
            onTagToggle={handleTagToggle}
            onTagClear={handleTagClear}
            onNavigate={handlePaletteNavigate}
          />
          <Inspector
            nodes={selectedNodes}
            allTags={allTags}
            allGroups={allGroups}
            allPassageNames={allPassageNames}
            focusSignal={focusSignal}
            onUpdateMetadata={handleUpdateMetadata}
            onBulkMetadata={handleBulkMetadata}
            onRename={handleRename}
            onDelete={handleDelete}
            onSetStart={handleSetStart}
            onClose={() => setClearSelRequested(Date.now())}
          />
          <Legend
            visible={showLegend}
            hiddenEdgeKinds={hiddenEdgeKinds}
            onToggleEdgeKind={handleToggleEdgeKind}
          />
          {hintVisible && (
            <div className="hint-toast">
              <kbd>Click</kbd> a passage to open it · <kbd>Drag</kbd> to select ·
              <kbd>Right-click</kbd> for actions · <kbd>/</kbd> to find
            </div>
          )}
        </div>
        {createRequest && (
          <CreatePassageModal
            request={createRequest}
            existingNames={new Set(allPassageNames)}
            allTags={allTags}
            tweeFiles={tweeFiles.filter(f => f.kind === 'twee')}
            defaultTargetFile={defaultTargetFile}
            sourceDir={sourceDir}
            onCancel={() => setCreateRequest(null)}
            onCreate={handleCreatePassage}
          />
        )}
        {groupModal && (
          <GroupNameModal
            memberCount={groupModal.members.length}
            prefill={groupModal.prefill}
            existingGroups={allGroups}
            onCancel={() => setGroupModal(null)}
            onSubmit={handleGroupSubmit}
          />
        )}
      </div>
    </StoryMapErrorBoundary>
  );
}
