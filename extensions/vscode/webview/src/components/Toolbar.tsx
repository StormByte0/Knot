import React, { useCallback } from 'react';
import { KnotGraphResponse } from '../types';

interface ToolbarProps {
  searchQuery: string;
  onSearchChange: (query: string) => void;
  layout: string;
  onLayoutChange: (layout: string) => void;
  onFit: () => void;
  onRefresh: () => void;
  graphData: KnotGraphResponse | null;
}

export default function Toolbar({
  searchQuery,
  onSearchChange,
  layout,
  onLayoutChange,
  onFit,
  onRefresh,
  graphData,
}: ToolbarProps) {
  const handleSearchInput = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      onSearchChange(e.target.value);
    },
    [onSearchChange],
  );

  const handleLayoutChange = useCallback(
    (e: React.ChangeEvent<HTMLSelectElement>) => {
      onLayoutChange(e.target.value);
    },
    [onLayoutChange],
  );

  // Determine if "Saved" option should be disabled
  const hasPositions = graphData?.nodes?.some(
    n => n.position_x != null && n.position_y != null,
  ) ?? false;

  return (
    <div id="toolbar">
      <input
        type="text"
        id="searchInput"
        placeholder="Filter passages..."
        value={searchQuery}
        onChange={handleSearchInput}
      />
      <select
        id="layoutSelect"
        title="Layout"
        value={layout}
        onChange={handleLayoutChange}
      >
        <option value="position" disabled={!hasPositions}>Saved</option>
        <option value="dagre">Flow</option>
        <option value="cose">Force</option>
      </select>
      <button id="fitBtn" title="Zoom to fit" onClick={onFit}>
        Fit
      </button>
      <button id="refreshBtn" title="Refresh" onClick={onRefresh}>
        &#x21BB;
      </button>
    </div>
  );
}
