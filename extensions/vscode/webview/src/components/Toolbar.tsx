import React, { useCallback } from 'react';
import { KnotGraphResponse } from '../types';

interface ToolbarProps {
  searchQuery: string;
  onSearchChange: (query: string) => void;
  layout: string;
  onLayoutChange: (layout: string) => void;
  onFit: () => void;
  onRefresh: () => void;
  onSavePositions: () => void;
  graphData: KnotGraphResponse | null;
}

export default function Toolbar({
  searchQuery,
  onSearchChange,
  layout,
  onLayoutChange,
  onFit,
  onRefresh,
  onSavePositions,
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
        placeholder="Search passages..."
        value={searchQuery}
        onChange={handleSearchInput}
      />
      <select
        id="layoutSelect"
        title="Layout mode"
        value={layout}
        onChange={handleLayoutChange}
      >
        <option value="position" disabled={!hasPositions}>Saved</option>
        <option value="dagre">Flow</option>
        <option value="cose">Force</option>
      </select>
      <button id="fitBtn" title="Zoom to fit all" onClick={onFit}>
        &#x25CE;
      </button>
      <button id="refreshBtn" title="Refresh graph" onClick={onRefresh}>
        &#x21BB;
      </button>
      <button
        id="saveBtn"
        title="Save layout to workspace"
        onClick={onSavePositions}
      >
        &#x1F4BE;
      </button>
    </div>
  );
}
