import { useEffect, useMemo, useRef, useState } from 'react';
import { KnotGraphNode } from '../types';

// ── Find palette ───────────────────────────────────────────────────────────
//
// The toolbar is gone (limited real estate in the VS Code layout), so search
// + tag filtering live in this zero-footprint palette:
//
//   • Floating trigger pill in the canvas corner — one click to open.
//   • "/" (when not typing) or Ctrl+F / Ctrl+K / Ctrl+P from the canvas.
//   • Typing dims the canvas live (same filter pipeline the toolbar fed).
//   • Results list: ↑/↓ to walk, Enter to navigate — the code editor jumps
//     to the passage and the graph pans + selects it.
//   • Tag chips toggle tag filters; Esc closes AND clears the filter.

// Category colors mirrored from StoryMap.tsx (kept in sync by convention).
const COLORS = {
  normal: '#3a7ca5',
  start: '#2f9e6e',
  special: '#d97b3f',
  metadata: '#8e5bc0',
  unreachable: '#a87534',
};

function nodeColor(n: KnotGraphNode): string {
  if (n.color) return n.color;
  if (n.is_unreachable) return COLORS.unreachable;
  if (n.is_start) return COLORS.start;
  if (n.is_metadata) return COLORS.metadata;
  if (n.is_special) return COLORS.special;
  return COLORS.normal;
}

interface Scored {
  node: KnotGraphNode;
  score: number; // lower = better
}

function scoreNode(n: KnotGraphNode, q: string): number | null {
  const label = n.label.toLowerCase();
  if (label.startsWith(q)) return 0;
  if (label.includes(q)) return 1;
  if ((n.tags || []).some(t => t.toLowerCase().includes(q))) return 2;
  if ((n.excerpt || '').toLowerCase().includes(q)) return 3;
  return null;
}

export interface FindPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  nodes: KnotGraphNode[];
  searchQuery: string;
  onSearchChange: (q: string) => void;
  allTags: string[];
  selectedTags: Set<string>;
  onTagToggle: (tag: string) => void;
  onTagClear: () => void;
  onNavigate: (node: KnotGraphNode) => void;
}

export default function FindPalette({
  open,
  onOpenChange,
  nodes,
  searchQuery,
  onSearchChange,
  allTags,
  selectedTags,
  onTagToggle,
  onTagClear,
  onNavigate,
}: FindPaletteProps) {
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const q = searchQuery.toLowerCase().trim();

  // Ranked results (name starts-with > contains > tag match > excerpt match).
  const results = useMemo<Scored[]>(() => {
    if (!q) return [];
    const scored: Scored[] = [];
    for (const n of nodes) {
      if (n.is_metadata) continue;
      // Ghosts (broken-link placeholders) have no file to open — creating
      // them from the palette would need the create flow, which the
      // palette doesn't host. They're reachable on the canvas instead.
      if (n.is_placeholder) continue;
      const s = scoreNode(n, q);
      if (s != null) scored.push({ node: n, score: s });
    }
    scored.sort((a, b) =>
      a.score - b.score ||
      a.node.label.localeCompare(b.node.label),
    );
    return scored.slice(0, 9);
  }, [q, nodes]);

  // Tag-filtered match count (for the status chip).
  const tagMatchCount = useMemo(() => {
    if (selectedTags.size === 0) return null;
    return nodes.filter(n =>
      (n.tags || []).some(t => selectedTags.has(t)),
    ).length;
  }, [nodes, selectedTags]);

  // Keep the active index in range as results change.
  useEffect(() => {
    setActiveIndex(0);
  }, [searchQuery]);

  // Focus the input whenever the palette opens.
  useEffect(() => {
    if (open) {
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  // Scroll the active row into view.
  useEffect(() => {
    const row = listRef.current?.querySelector(`[data-idx="${activeIndex}"]`);
    row?.scrollIntoView({ block: 'nearest' });
  }, [activeIndex]);

  const close = (clearFilter: boolean) => {
    onOpenChange(false);
    if (clearFilter) {
      onSearchChange('');
      onTagClear();
    }
  };

  const choose = (n: KnotGraphNode) => {
    onNavigate(n);
    close(true);
  };

  // Global key bindings: open shortcuts + Escape (capture so the canvas
  // keyboard layer doesn't also react).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const isTyping = !!target && (
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.tagName === 'SELECT' ||
        target.isContentEditable
      );

      if (!open) {
        const mod = e.ctrlKey || e.metaKey;
        if (e.key === '/' && !isTyping && !mod && !e.altKey) {
          e.preventDefault();
          e.stopPropagation();
          onOpenChange(true);
        } else if (mod && ['f', 'k', 'p'].includes(e.key.toLowerCase()) && !isTyping) {
          e.preventDefault();
          e.stopPropagation();
          onOpenChange(true);
        }
      } else if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        close(true);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, onOpenChange, onSearchChange, onTagClear]);

  const handleInputKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (results.length > 0) setActiveIndex(i => (i + 1) % results.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (results.length > 0) setActiveIndex(i => (i - 1 + results.length) % results.length);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const pick = results[activeIndex] ?? results[0];
      if (pick) choose(pick.node);
    }
  };

  const filterActive = searchQuery.trim() !== '' || selectedTags.size > 0;

  return (
    <>
      {/* ── Floating trigger pill ─────────────────────────────────── */}
      {!open && (
        <button
          className={`find-trigger${filterActive ? ' find-trigger--active' : ''}`}
          title="Find passages ( / ) — filter the canvas by name, tag, or text"
          onClick={() => onOpenChange(true)}
        >
          <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
            <circle cx="6" cy="6" r="4.1" fill="none" stroke="currentColor" strokeWidth="1.5" />
            <path d="M9.1 9.1 L12.3 12.3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
          {filterActive && (
            <span className="find-trigger__dot" aria-label="filter active" />
          )}
        </button>
      )}

      {/* ── Palette overlay ───────────────────────────────────────── */}
      {open && (
        <div className="find-backdrop" onMouseDown={() => close(true)}>
          <div
            className="find-palette"
            onMouseDown={e => e.stopPropagation()}
            role="dialog"
            aria-label="Find passages"
          >
            <div className="find-palette__search">
              <span className="find-palette__icon">
                <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
                  <circle cx="6" cy="6" r="4.1" fill="none" stroke="currentColor" strokeWidth="1.5" />
                  <path d="M9.1 9.1 L12.3 12.3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                </svg>
              </span>
              <input
                ref={inputRef}
                className="find-palette__input"
                placeholder="Search passages by name, tag, or text…"
                value={searchQuery}
                onChange={e => onSearchChange(e.target.value)}
                onKeyDown={handleInputKeyDown}
                spellCheck={false}
              />
              {searchQuery && (
                <button
                  className="find-palette__clear"
                  title="Clear search"
                  onClick={() => onSearchChange('')}
                >
                  <svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true">
                    <path d="M3.2 3.2 L10.8 10.8 M10.8 3.2 L3.2 10.8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
                  </svg>
                </button>
              )}
              {(q && results.length > 0) && (
                <span className="find-palette__count">{results.length}</span>
              )}
            </div>

            {allTags.length > 0 && (
              <div className="find-palette__tags">
                {allTags.map(tag => (
                  <button
                    key={tag}
                    className={`find-tag${selectedTags.has(tag) ? ' find-tag--on' : ''}`}
                    onClick={() => onTagToggle(tag)}
                  >
                    {tag}
                  </button>
                ))}
              </div>
            )}

            {(q || selectedTags.size > 0) && (
              <div className="find-palette__results" ref={listRef}>
                {q && results.length === 0 && (
                  <div className="find-palette__empty">
                    No passages match “{searchQuery.trim()}”.
                  </div>
                )}
                {!q && tagMatchCount != null && (
                  <div className="find-palette__empty">
                    {tagMatchCount} passage{tagMatchCount === 1 ? '' : 's'} tagged with the selected filter{selectedTags.size > 1 ? 's' : ''}.
                  </div>
                )}
                {results.map((r, i) => (
                  <button
                    key={r.node.id}
                    data-idx={i}
                    className={`find-row${i === activeIndex ? ' find-row--active' : ''}`}
                    onMouseEnter={() => setActiveIndex(i)}
                    onClick={() => choose(r.node)}
                  >
                    <span className="find-row__dot" style={{ background: nodeColor(r.node) }} />
                    <span className="find-row__main">
                      <span className="find-row__name">{r.node.label}</span>
                      {r.node.excerpt && (
                        <span className="find-row__excerpt">
                          {r.node.excerpt.length > 72
                            ? r.node.excerpt.slice(0, 70) + '…'
                            : r.node.excerpt}
                        </span>
                      )}
                    </span>
                    <span className="find-row__meta">
                      {r.node.is_start && <span className="find-row__flag" title="Start passage">start</span>}
                      {r.node.tags.slice(0, 2).map(t => (
                        <span key={t} className="find-row__tag">{t}</span>
                      ))}
                      <span className="find-row__loc">
                        {(r.node.file.split('/').pop() || '')}:{r.node.line + 1}
                      </span>
                    </span>
                  </button>
                ))}
              </div>
            )}

            <div className="find-palette__footer">
              <span><kbd>↑</kbd><kbd>↓</kbd> browse</span>
              <span><kbd>Enter</kbd> open in editor</span>
              <span><kbd>Esc</kbd> close &amp; clear</span>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
