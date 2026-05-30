import { useCallback } from 'react';

interface ToolbarProps {
  searchQuery: string;
  onSearchChange: (query: string) => void;
  onFit: () => void;
  onRefresh: () => void;
  onSavePositions: () => void;
}

export default function Toolbar({
  searchQuery,
  onSearchChange,
  onFit,
  onRefresh,
  onSavePositions,
}: ToolbarProps) {
  const handleSearchInput = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      onSearchChange(e.target.value);
    },
    [onSearchChange],
  );

  return (
    <div id="toolbar">
      <input
        type="text"
        id="searchInput"
        placeholder="Search passages..."
        value={searchQuery}
        onChange={handleSearchInput}
      />
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
