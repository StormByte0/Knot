import { useState, useEffect, useCallback, Component, ReactNode } from 'react';
import { VsCodeApi, KnotGraphResponse, WebviewInboundMessage } from './types';
import StoryMap from './components/StoryMap';
import Toolbar from './components/Toolbar';
import Legend from './components/Legend';

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

// ── Main App component ──────────────────────────────────────────────────────

export default function App() {
  const [graphData, setGraphData] = useState<KnotGraphResponse | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [stats, setStats] = useState({ nodes: 0, edges: 0, broken: 0 });

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
            const nodes = data?.nodes?.length ?? 0;
            const edges = data?.edges?.length ?? 0;
            const broken = data?.edges?.filter(e => e.edge_type === 'broken').length ?? 0;
            setStats({ nodes, edges, broken });
            break;
          }
          case 'focusNode': {
            setFocusPassageName(msg.passageName);
            setFocusRequested(Date.now());
            break;
          }
          case 'restoreViewport': {
            const { x, y, zoom } = msg;
            setRestoreViewport({ x, y, zoom, ts: Date.now() });
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

  return (
    <StoryMapErrorBoundary>
      <div className="app-container">
        <Toolbar
          searchQuery={searchQuery}
          onSearchChange={handleSearchChange}
          onFit={handleFit}
          onRefresh={handleRefresh}
          onSavePositions={handleSavePositions}
        />
        <StoryMap
          graphData={graphData}
          searchQuery={searchQuery}
          fitRequested={fitRequested}
          saveRequested={saveRequested}
          focusRequested={focusRequested}
          focusPassageName={focusPassageName}
          restoreViewport={restoreViewport}
        />
        <Legend />
        <div id="statusBar">
          <span id="statNodes">{stats.nodes} passages</span>
          <span id="statEdges">{stats.edges} links</span>
          <span id="statBroken">{stats.broken} broken</span>
        </div>
      </div>
    </StoryMapErrorBoundary>
  );
}