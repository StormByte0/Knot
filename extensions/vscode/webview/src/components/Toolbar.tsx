import { useCallback, useState, useRef, useEffect } from 'react';

interface ToolbarProps {
  searchQuery: string;
  onSearchChange: (query: string) => void;
  onFit: () => void;
  onRefresh: () => void;
  onSavePositions: () => void;
  allTags: string[];
  selectedTags: Set<string>;
  onTagToggle: (tag: string) => void;
  onTagClear: () => void;
}

export default function Toolbar({
  searchQuery,
  onSearchChange,
  onFit,
  onRefresh,
  onSavePositions,
  allTags,
  selectedTags,
  onTagToggle,
  onTagClear,
}: ToolbarProps) {
  const [tagDropdownOpen, setTagDropdownOpen] = useState(false);
  const [flipUp, setFlipUp] = useState(false);
  const tagDropdownRef = useRef<HTMLDivElement>(null);
  const tagBtnRef = useRef<HTMLButtonElement>(null);

  // Close dropdown when clicking outside
  useEffect(() => {
    if (!tagDropdownOpen) return;
    const handleClick = (e: MouseEvent) => {
      if (tagDropdownRef.current && !tagDropdownRef.current.contains(e.target as Node)) {
        setTagDropdownOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [tagDropdownOpen]);

  // Check if the dropdown would overflow the viewport bottom — if so, flip up
  useEffect(() => {
    if (!tagDropdownOpen || !tagBtnRef.current) return;
    const btnRect = tagBtnRef.current.getBoundingClientRect();
    const viewportHeight = window.innerHeight;
    const dropdownMaxHeight = 300; // approximate max height of dropdown
    setFlipUp(btnRect.bottom + dropdownMaxHeight > viewportHeight);
  }, [tagDropdownOpen]);

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
      {/* Tag filter dropdown */}
      <div className="tag-filter-wrapper" ref={tagDropdownRef}>
        <button
          id="tagFilterBtn"
          ref={tagBtnRef}
          title="Filter by tags"
          className={selectedTags.size > 0 ? 'active' : ''}
          onClick={() => setTagDropdownOpen(o => !o)}
        >
          &#x1F3F7;
          {selectedTags.size > 0 && (
            <span className="tag-filter-count">{selectedTags.size}</span>
          )}
        </button>
        {tagDropdownOpen && (
          <div id="tagDropdown" className={flipUp ? 'flip-up' : ''}>
            <div className="tag-dropdown-header">
              <span>Filter by tags</span>
              {selectedTags.size > 0 && (
                <button className="tag-clear-btn" onClick={onTagClear}>
                  Clear
                </button>
              )}
            </div>
            <div className="tag-list">
              {allTags.length === 0 ? (
                <div className="tag-empty">No tags found</div>
              ) : (
                allTags.map(tag => (
                  <label key={tag} className="tag-item">
                    <input
                      type="checkbox"
                      checked={selectedTags.has(tag)}
                      onChange={() => onTagToggle(tag)}
                    />
                    <span>{tag}</span>
                  </label>
                ))
              )}
            </div>
          </div>
        )}
      </div>
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
