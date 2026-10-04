import { useCallback, useState, useRef, useEffect } from 'react';

// ── Icon set ───────────────────────────────────────────────────────────────
// 14×14 line icons, 1.5 stroke, round caps — consistent, crisp at webview
// scale, and theme-aware via currentColor (no more mixed emoji glyphs).

function IconSearch() {
  return (
    <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
      <circle cx="6" cy="6" r="4.1" fill="none" stroke="currentColor" strokeWidth="1.5" />
      <path d="M9.1 9.1 L12.3 12.3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

function IconTag() {
  return (
    <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
      <path
        d="M1.8 6.4 L1.8 2.4 a0.6 0.6 0 0 1 0.6 -0.6 h4 L12.3 7.7 a0.6 0.6 0 0 1 0 0.85 L8.55 12.3 a0.6 0.6 0 0 1 -0.85 0 Z"
        fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round"
      />
      <circle cx="4.4" cy="4.4" r="1.05" fill="currentColor" />
    </svg>
  );
}

function IconFit() {
  return (
    <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
      <path
        d="M1.8 4.6 V2.9 a1.1 1.1 0 0 1 1.1 -1.1 h1.7 M9.4 1.8 h1.7 a1.1 1.1 0 0 1 1.1 1.1 v1.7 M12.2 9.4 v1.7 a1.1 1.1 0 0 1 -1.1 1.1 h-1.7 M4.6 12.2 H2.9 a1.1 1.1 0 0 1 -1.1 -1.1 V9.4"
        fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"
      />
    </svg>
  );
}

function IconRefresh() {
  return (
    <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
      <path
        d="M12.2 7 a5.2 5.2 0 1 1 -1.5 -3.67 M10.9 1.6 v2.2 h-2.2"
        fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"
      />
    </svg>
  );
}

function IconSave() {
  return (
    <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
      <path
        d="M7 1.8 V8.6 M4.3 6.2 L7 8.9 L9.7 6.2 M2.2 10.4 v0.9 a0.9 0.9 0 0 0 0.9 0.9 h7.8 a0.9 0.9 0 0 0 0.9 -0.9 v-0.9"
        fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"
      />
    </svg>
  );
}

function IconPlus() {
  return (
    <svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true">
      <path d="M7 2.2 V11.8 M2.2 7 H11.8" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  );
}

function IconX() {
  return (
    <svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true">
      <path d="M3.2 3.2 L10.8 10.8 M10.8 3.2 L3.2 10.8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}

// ── Toolbar ────────────────────────────────────────────────────────────────

interface ToolbarProps {
  searchQuery: string;
  onSearchChange: (query: string) => void;
  onFit: () => void;
  onRefresh: () => void;
  onSavePositions: () => void;
  onNewPassage: () => void;
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
  onNewPassage,
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
      <div className="toolbar-brand" aria-hidden="true">
        <svg viewBox="0 0 16 16" width="15" height="15">
          <path
            d="M4.2 2.2 C4.2 5.8 11.8 6.6 11.8 10.2 M11.8 2.2 C11.8 5.8 4.2 6.6 4.2 10.2"
            fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round"
          />
          <path
            d="M4.2 10.2 C4.2 12.4 6.4 13.4 8 13.8 C9.6 13.4 11.8 12.4 11.8 10.2"
            fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" opacity="0.55"
          />
        </svg>
        <span>Story Map</span>
      </div>

      <div className="toolbar-search">
        <span className="toolbar-search__icon"><IconSearch /></span>
        <input
          type="text"
          id="searchInput"
          placeholder="Search passages…"
          value={searchQuery}
          onChange={handleSearchInput}
        />
        {searchQuery && (
          <button
            className="toolbar-search__clear"
            title="Clear search"
            onClick={() => onSearchChange('')}
          >
            <IconX />
          </button>
        )}
      </div>

      {/* Tag filter dropdown */}
      <div className="tag-filter-wrapper" ref={tagDropdownRef}>
        <button
          id="tagFilterBtn"
          ref={tagBtnRef}
          title="Filter by tags"
          className={selectedTags.size > 0 ? 'active' : ''}
          onClick={() => setTagDropdownOpen(o => !o)}
        >
          <IconTag />
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

      <div className="toolbar-spacer" />

      <button id="fitBtn" className="toolbar-icon-btn" title="Zoom to fit (whole story)" onClick={onFit}>
        <IconFit />
      </button>
      <button id="refreshBtn" className="toolbar-icon-btn" title="Refresh graph" onClick={onRefresh}>
        <IconRefresh />
      </button>
      <button id="saveBtn" className="toolbar-icon-btn" title="Save layout to workspace" onClick={onSavePositions}>
        <IconSave />
      </button>

      <div className="toolbar-sep" />

      <button
        id="newPassageBtn"
        className="toolbar-btn--primary"
        title="New passage (at view center)"
        onClick={onNewPassage}
      >
        <IconPlus />
        New passage
      </button>
    </div>
  );
}
