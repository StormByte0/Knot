import React, { useState, useEffect, useCallback, useRef } from 'react';
import { VsCodeApi, KnotGraphResponse, WebviewInboundMessage } from './types';
import StoryMap from './components/StoryMap';
import Toolbar from './components/Toolbar';
import Legend from './components/Legend';
import Tooltip from './components/Tooltip';

// Acquire the VS Code API — must be called once per webview lifetime
const vscode: VsCodeApi = acquireVsCodeApi();

export { vscode };

export default function App() {
  const [graphData, setGraphData] = useState<KnotGraphResponse | null>(null);
  const [layout, setLayout] = useState<string>('position');
  const [searchQuery, setSearchQuery] = useState('');
  const [stats, setStats] = useState({ nodes: 0, edges: 0, broken: 0 });

  // Listen for messages from the extension
  useEffect(() => {
    const handler = (event: MessageEvent) => {
      const msg: WebviewInboundMessage = event.data;
      if (msg.command === 'updateGraph') {
        setGraphData(msg.data);
        // Compute stats
        const nodes = msg.data?.nodes?.length ?? 0;
        const edges = msg.data?.edges?.length ?? 0;
        const broken = msg.data?.edges?.filter(e => e.edge_type === 'broken').length ?? 0;
        setStats({ nodes, edges, broken });
      }
    };
    window.addEventListener('message', handler);
    return () => window.removeEventListener('message', handler);
  }, []);

  // Request initial graph data
  useEffect(() => {
    vscode.postMessage({ command: 'refreshGraph' });
  }, []);

  const handleRefresh = useCallback(() => {
    vscode.postMessage({ command: 'refreshGraph' });
  }, []);

  const handleFit = useCallback(() => {
    // StoryMap will handle this via a ref callback
    setFitRequested(Date.now());
  }, []);

  const [fitRequested, setFitRequested] = useState(0);

  const handleLayoutChange = useCallback((newLayout: string) => {
    setLayout(newLayout);
  }, []);

  const handleSearchChange = useCallback((query: string) => {
    setSearchQuery(query);
  }, []);

  return (
    <div className="app-container">
      <Toolbar
        searchQuery={searchQuery}
        onSearchChange={handleSearchChange}
        layout={layout}
        onLayoutChange={handleLayoutChange}
        onFit={handleFit}
        onRefresh={handleRefresh}
        graphData={graphData}
      />
      <StoryMap
        graphData={graphData}
        layout={layout}
        searchQuery={searchQuery}
        fitRequested={fitRequested}
        onLayoutChange={handleLayoutChange}
      />
      <Tooltip />
      <Legend />
      <div id="statusBar">
        <span id="statNodes">{stats.nodes} passages</span>
        <span id="statEdges">{stats.edges} links</span>
        <span id="statBroken">{stats.broken} broken</span>
      </div>
    </div>
  );
}
