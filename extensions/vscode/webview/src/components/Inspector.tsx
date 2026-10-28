import { useCallback, useEffect, useRef, useState } from 'react';
import { PassageNodeData, PassageMetadataPatch } from '../types';

// ---------------------------------------------------------------------------
// Color presets (Twine-ish palette used by the node category colors)
// ---------------------------------------------------------------------------

const COLOR_PRESETS: { value: string; label: string }[] = [
  { value: '#2d6a9f', label: 'Ocean' },
  { value: '#2e7d32', label: 'Forest' },
  { value: '#e65100', label: 'Ember' },
  { value: '#6a1b9a', label: 'Violet' },
  { value: '#bf6900', label: 'Amber' },
  { value: '#c62828', label: 'Crimson' },
  { value: '#1b8a6b', label: 'Teal' },
  { value: '#455a64', label: 'Slate' },
];

/** Field-focus / arm signals sent from the Story Map (context menu or
 *  keyboard) to steer the inspector's attention. */
export interface InspectorFocusSignal {
  field: 'rename' | 'reachable' | 'delete' | 'tags' | 'color' | 'group';
  ts: number;
}

// Panel sizing: the inspector floats over the canvas; the metadata
// sections (tags, size, color…) need breathing room. The panel stretches
// to the FULL canvas height by default (top 12 → bottom 12) so every
// section is visible without scrolling — the natural expansion for this
// panel is VERTICAL, not horizontal. Two grips: left edge (width) and
// bottom edge (height); both persist in webview localStorage. A stored
// height overrides the full-height stretch; double-click either grip to
// snap it back to its default.
const INSPECTOR_MIN_W = 280;
const INSPECTOR_MAX_W = 560;
const INSPECTOR_DEFAULT_W = 344;
const INSPECTOR_W_KEY = 'knot.storymap.inspectorWidth';

const INSPECTOR_MIN_H = 240;
const INSPECTOR_H_KEY = 'knot.storymap.inspectorHeight';

function loadInspectorWidth(): number {
  try {
    const saved = Number(localStorage.getItem(INSPECTOR_W_KEY));
    if (Number.isFinite(saved) && saved >= INSPECTOR_MIN_W && saved <= INSPECTOR_MAX_W) {
      return saved;
    }
  } catch { /* storage unavailable */ }
  return INSPECTOR_DEFAULT_W;
}

/** Stored height, or null when the panel should stretch to full height
 * (the default — expansion is vertical so the metadata sections fit). */
function loadInspectorHeight(): number | null {
  try {
    const saved = Number(localStorage.getItem(INSPECTOR_H_KEY));
    if (Number.isFinite(saved) && saved >= INSPECTOR_MIN_H) {
      return saved;
    }
  } catch { /* storage unavailable */ }
  return null;
}

export interface InspectorSelectedNode {
  id: string;
  data: PassageNodeData;
}

interface InspectorProps {
  /** Current passage selection (0 = hidden, 1 = single editor, N = bulk). */
  nodes: InspectorSelectedNode[];
  allTags: string[];
  allGroups: string[];
  allPassageNames: string[];
  focusSignal: InspectorFocusSignal | null;
  onUpdateMetadata: (passageName: string, patch: PassageMetadataPatch) => void;
  /** Atomic bulk edit (multi-select): one patch, one request, one refresh. */
  onBulkMetadata: (
    names: string[],
    patch: { color?: string | null; group?: string | null; size?: { width: number; height: number } | null },
  ) => void;
  onRename: (name: string, newName: string) => void;
  onDelete: (name: string, stripLinks: boolean) => void;
  onSetStart: (name: string) => void;
  onClose: () => void;
}

export default function Inspector({
  nodes,
  allTags,
  allGroups,
  allPassageNames,
  focusSignal,
  onUpdateMetadata,
  onBulkMetadata,
  onRename,
  onDelete,
  onSetStart,
  onClose,
}: InspectorProps) {
  // ── Tag editor state ──────────────────────────────────────────────────
  const [tagDraft, setTagDraft] = useState('');

  // ── Rename state ──────────────────────────────────────────────────────
  const [renaming, setRenaming] = useState(false);
  const [renameDraft, setRenameDraft] = useState('');
  const renameInputRef = useRef<HTMLInputElement>(null);

  // ── Field editors ─────────────────────────────────────────────────────
  const [groupDraft, setGroupDraft] = useState('');
  const [reachableDraft, setReachableDraft] = useState('');
  const [sizeWDraft, setSizeWDraft] = useState('');
  const [sizeHDraft, setSizeHDraft] = useState('');
  const [colorDraft, setColorDraft] = useState('');
  const reachableInputRef = useRef<HTMLInputElement>(null);
  const tagInputRef = useRef<HTMLInputElement>(null);
  const colorSectionRef = useRef<HTMLElement>(null);
  const groupInputRef = useRef<HTMLInputElement>(null);

  // ── Delete confirm (two-step) ─────────────────────────────────────────
  const [deleteArmed, setDeleteArmed] = useState(false);
  const [stripLinks, setStripLinks] = useState(true);
  const deleteTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Resizable panel: width via the left-edge grip, height via the
  // bottom-edge grip. Both persist; null height = full-height stretch.
  const [width, setWidth] = useState<number>(loadInspectorWidth);
  const [height, setHeight] = useState<number | null>(loadInspectorHeight);
  const panelRef = useRef<HTMLElement>(null);

  // Width dragging tracks the panel's right edge (the panel stays
  // anchored to the canvas's top-right) and clamps to [MIN, MAX];
  // persists on release. Double-click resets to the default width.
  const startResize = useCallback((e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    document.body.classList.add('inspector-resizing');
    const move = (ev: PointerEvent) => {
      const rect = panelRef.current?.getBoundingClientRect();
      const rightEdge = rect?.right ?? 0;
      if (!rightEdge) return;
      setWidth(Math.min(INSPECTOR_MAX_W, Math.max(INSPECTOR_MIN_W, rightEdge - ev.clientX)));
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.classList.remove('inspector-resizing');
      const w = panelRef.current?.getBoundingClientRect().width;
      if (w) {
        try { localStorage.setItem(INSPECTOR_W_KEY, String(Math.round(w))); } catch { /* ignore */ }
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }, []);

  // Height dragging grows/shrinks the panel DOWNWARD from its top anchor
  // and clamps to [MIN_H, canvas height − margins]; persists on release.
  // Double-click restores the full-height stretch (the default).
  const startHeightResize = useCallback((e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    document.body.classList.add('inspector-resizing-v');
    const move = (ev: PointerEvent) => {
      const panel = panelRef.current;
      const rect = panel?.getBoundingClientRect();
      if (!panel || !rect) return;
      const parentH = panel.parentElement?.clientHeight ?? window.innerHeight;
      const max = Math.max(INSPECTOR_MIN_H, parentH - 24);
      setHeight(Math.min(max, Math.max(INSPECTOR_MIN_H, ev.clientY - rect.top)));
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.classList.remove('inspector-resizing-v');
      const h = panelRef.current?.getBoundingClientRect().height;
      if (h) {
        try { localStorage.setItem(INSPECTOR_H_KEY, String(Math.round(h))); } catch { /* ignore */ }
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }, []);

  const resetWidth = useCallback(() => {
    setWidth(INSPECTOR_DEFAULT_W);
    try { localStorage.setItem(INSPECTOR_W_KEY, String(INSPECTOR_DEFAULT_W)); } catch { /* ignore */ }
  }, []);

  const resetHeight = useCallback(() => {
    setHeight(null);
    try { localStorage.removeItem(INSPECTOR_H_KEY); } catch { /* ignore */ }
  }, []);

  const single = nodes.length === 1 ? nodes[0] : null;
  const d = single?.data;

  // Bulk-edit tag draft is shared with single mode (same input).
  const isMulti = nodes.length > 1;
  const multiNames = isMulti ? nodes.map(n => n.data.label) : [];

  // Common tags across the multi-selection (for the bulk chip row).
  const commonTags = isMulti
    ? nodes[0].data.tags.filter(t => nodes.every(n => n.data.tags.includes(t)))
    : [];

  // Reset per-field drafts whenever the selection changes or its data is
  // refreshed by a graph update.
  useEffect(() => {
    setTagDraft('');
    setRenaming(false);
    setDeleteArmed(false);
    const sd = single?.data;
    setGroupDraft(sd?.group ?? '');
    setReachableDraft(sd?.manual_link_source ?? '');
    setSizeWDraft(sd?.size_w != null ? String(Math.round(sd.size_w)) : '');
    setSizeHDraft(sd?.size_h != null ? String(Math.round(sd.size_h)) : '');
    setColorDraft(sd?.metadata_color ?? '');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [single?.id, d?.group, d?.manual_link_source, d?.size_w, d?.size_h, d?.metadata_color, d?.tags, nodes.length]);

  // React to focus signals from the Story Map (context menu / keyboard).
  useEffect(() => {
    if (!focusSignal) return;
    if (focusSignal.field === 'rename' && single) {
      setRenaming(true);
      setRenameDraft(single.data.label);
      requestAnimationFrame(() => renameInputRef.current?.select());
    } else if (focusSignal.field === 'reachable') {
      requestAnimationFrame(() => reachableInputRef.current?.focus());
    } else if (focusSignal.field === 'tags') {
      requestAnimationFrame(() => tagInputRef.current?.focus());
    } else if (focusSignal.field === 'color') {
      colorSectionRef.current?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    } else if (focusSignal.field === 'group') {
      requestAnimationFrame(() => groupInputRef.current?.focus());
    } else if (focusSignal.field === 'delete') {
      armDelete();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusSignal?.ts]);

  const armDelete = useCallback(() => {
    setDeleteArmed(true);
    if (deleteTimerRef.current) clearTimeout(deleteTimerRef.current);
    deleteTimerRef.current = setTimeout(() => setDeleteArmed(false), 3000);
  }, []);

  useEffect(() => () => {
    if (deleteTimerRef.current) clearTimeout(deleteTimerRef.current);
  }, []);

  if (nodes.length === 0) return null;

  // ══════════════════════════════════════════════════════════════════════
  // MULTI-SELECTION MODE — bulk edits
  // ══════════════════════════════════════════════════════════════════════
  if (isMulti) {
    const addTagToAll = (raw: string) => {
      const tag = raw.trim();
      setTagDraft('');
      if (!tag) return;
      if (/\s/.test(tag) || /[[\]{}]/.test(tag)) return;
      for (const n of nodes) {
        if (!n.data.tags.includes(tag)) {
          onUpdateMetadata(n.data.label, { tags: [...n.data.tags, tag] });
        }
      }
    };

    const removeTagFromAll = (tag: string) => {
      for (const n of nodes) {
        if (n.data.tags.includes(tag)) {
          onUpdateMetadata(n.data.label, { tags: n.data.tags.filter(t => t !== tag) });
        }
      }
    };

    const applyColorToAll = (value: string | null) => {
      onBulkMetadata(multiNames, { color: value });
    };

    const commitGroupAll = () => {
      const value = groupDraft.trim();
      if (value === '') {
        onBulkMetadata(multiNames, { group: null });
      } else {
        onBulkMetadata(multiNames, { group: value });
      }
    };

    const resetSizesAll = () => {
      const sized = nodes
        .filter(n => n.data.size_w != null || n.data.size_h != null)
        .map(n => n.data.label);
      if (sized.length > 0) onBulkMetadata(sized, { size: null });
    };

    // Apply one explicit size to every selected passage — the bulk
    // endpoint takes a single uniform patch, so this is one request.
    // No blur-commit here: an explicit Apply (or Enter) only, so focusing
    // an input and clicking away can never accidentally rewrite sizes.
    const commitSizeAll = () => {
      if (sizeWDraft.trim() === '' && sizeHDraft.trim() === '') return;
      const w = parseFloat(sizeWDraft);
      const h = parseFloat(sizeHDraft);
      const width = Number.isFinite(w) && w > 0 ? Math.round(w) : 160;
      const height = Number.isFinite(h) && h > 0 ? Math.round(h) : 36;
      onBulkMetadata(multiNames, { size: { width, height } });
    };

    const deleteAll = () => {
      for (const n of nodes) onDelete(n.data.label, stripLinks);
    };

    const anySized = nodes.some(n => n.data.size_w != null || n.data.size_h != null);
    const sizeApplyDisabled = sizeWDraft.trim() === '' && sizeHDraft.trim() === '';

    return (
      <aside
        className="inspector inspector--multi"
        ref={panelRef}
        style={{ width, ...(height != null ? { height } : {}) }}
      >
        <div
          className="inspector__resizer"
          onPointerDown={startResize}
          onDoubleClick={resetWidth}
          title="Drag to resize width · double-click to reset"
        />
        <div
          className="inspector__resizer inspector__resizer--h"
          onPointerDown={startHeightResize}
          onDoubleClick={resetHeight}
          title="Drag to resize height · double-click for full height"
        />
        <div className="inspector__header">
          <div className="inspector__title-wrap">
            <span className="inspector__title">{nodes.length} passages</span>
            <span className="inspector__badge inspector__badge--multi">multi-select</span>
          </div>
          <div className="inspector__header-actions">
            <button
              className="inspector__icon-btn inspector__icon-btn--close"
              title="Close inspector (Esc)"
              onClick={onClose}
            >
              ✕
            </button>
          </div>
        </div>

        <div className="inspector__scroll">
          <div className="inspector__names">
            {multiNames.slice(0, 12).map(n => <span key={n} className="inspector__name-chip">{n}</span>)}
            {multiNames.length > 12 && (
              <span className="inspector__name-chip inspector__name-chip--more">
                +{multiNames.length - 12} more
              </span>
            )}
          </div>

          {/* ── Bulk tags ─────────────────────────────────────────── */}
          <section className="inspector__section">
            <label className="inspector__label">Tags — all {nodes.length}</label>
            {commonTags.length > 0 && (
              <div className="inspector__chips">
                {commonTags.map(tag => (
                  <span className="inspector__chip" key={tag} title="Shared by every selected passage">
                    {tag}
                    <button
                      className="inspector__chip-x"
                      title={`Remove '${tag}' from all selected passages`}
                      onClick={() => removeTagFromAll(tag)}
                    >
                      ×
                    </button>
                  </span>
                ))}
              </div>
            )}
            <div className="inspector__input-row">
              <input
                ref={tagInputRef}
                className="inspector__input"
                list="inspector-tag-suggestions"
                placeholder="Add tag to all…"
                value={tagDraft}
                onChange={e => setTagDraft(e.target.value)}
                onKeyDown={e => {
                  if (e.key === 'Enter' || e.key === ',') {
                    e.preventDefault();
                    addTagToAll(tagDraft);
                  }
                }}
                onBlur={() => addTagToAll(tagDraft)}
                spellCheck={false}
              />
              <datalist id="inspector-tag-suggestions">
                {allTags.map(t => <option key={t} value={t} />)}
              </datalist>
            </div>
            <div className="inspector__field-hint">
              Chips above are shared by all selected passages; removing one removes it everywhere.
            </div>
          </section>

          {/* ── Bulk color ────────────────────────────────────────── */}
          <section className="inspector__section" ref={colorSectionRef}>
            <label className="inspector__label">Color — all {nodes.length}</label>
            <div className="inspector__swatches">
              {COLOR_PRESETS.map(preset => (
                <button
                  key={preset.value}
                  className="inspector__swatch"
                  style={{ background: preset.value }}
                  title={`${preset.label} — apply to all`}
                  onClick={() => applyColorToAll(preset.value)}
                />
              ))}
              <input
                className="inspector__color-custom"
                type="color"
                title="Custom color — apply to all"
                value={/^#[0-9a-fA-F]{6}$/.test(colorDraft) ? colorDraft : '#2d6a9f'}
                onChange={e => { setColorDraft(e.target.value); applyColorToAll(e.target.value); }}
              />
              <button
                className="inspector__swatch-clear"
                title="Reset all to category colors"
                onClick={() => applyColorToAll(null)}
              >
                auto
              </button>
            </div>
          </section>

          {/* ── Bulk group ────────────────────────────────────────── */}
          <section className="inspector__section">
            <label className="inspector__label">Group — all {nodes.length}</label>
            <div className="inspector__input-row">
              <input
                ref={groupInputRef}
                className="inspector__input"
                list="inspector-group-suggestions"
                placeholder="Set or clear group…"
                value={groupDraft}
                onChange={e => setGroupDraft(e.target.value)}
                onBlur={commitGroupAll}
                onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); commitGroupAll(); } }}
                spellCheck={false}
              />
              <datalist id="inspector-group-suggestions">
                {allGroups.map(g => <option key={g} value={g} />)}
              </datalist>
            </div>
            <div className="inspector__field-hint">
              Empty clears the group for every selected passage.
            </div>
          </section>

          {/* ── Bulk size ──────────────────────────────────────── */}
          <section className="inspector__section">
            <label className="inspector__label">Node size — all {nodes.length}</label>
            <div className="inspector__size-row">
              <input
                className="inspector__input inspector__input--num"
                type="number"
                min="40"
                max="2000"
                placeholder="160"
                title="Width (px) — applied to every selected passage"
                value={sizeWDraft}
                onChange={e => setSizeWDraft(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); commitSizeAll(); } }}
              />
              <span className="inspector__size-x">×</span>
              <input
                className="inspector__input inspector__input--num"
                type="number"
                min="24"
                max="2000"
                placeholder="36"
                title="Height (px) — applied to every selected passage"
                value={sizeHDraft}
                onChange={e => setSizeHDraft(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); commitSizeAll(); } }}
              />
              <button
                className="inspector__btn inspector__btn--apply"
                onClick={commitSizeAll}
                disabled={sizeApplyDisabled}
                title="Set this size on every selected passage"
              >
                Apply
              </button>
            </div>
            {anySized && (
              <button
                className="inspector__swatch-clear"
                style={{ marginTop: 6 }}
                onClick={resetSizesAll}
                title="Remove the size metadata from every selected passage"
              >
                Reset custom sizes
              </button>
            )}
            <div className="inspector__field-hint">
              Sets the same size on every selected passage. Empty boxes fall
              back to the defaults (160 × 36); press Enter or Apply to commit.
            </div>
          </section>
        </div>

        {/* ── Actions ────────────────────────────────────────────── */}
        <div className="inspector__actions">
          {deleteArmed ? (
            <div className="inspector__delete-confirm">
              <label className="inspector__delete-check">
                <input
                  type="checkbox"
                  checked={stripLinks}
                  onChange={e => setStripLinks(e.target.checked)}
                />
                Also remove links
              </label>
              <button
                className="inspector__btn inspector__btn--danger inspector__btn--confirm"
                onClick={deleteAll}
                autoFocus
              >
                Confirm delete ({nodes.length})
              </button>
            </div>
          ) : (
            <button
              className="inspector__btn inspector__btn--danger"
              onClick={armDelete}
              title="Delete all selected passages"
            >
              Delete {nodes.length} passages
            </button>
          )}
        </div>
      </aside>
    );
  }

  // ══════════════════════════════════════════════════════════════════════
  // SINGLE-SELECTION MODE
  // ══════════════════════════════════════════════════════════════════════
  if (!single || !d) return null;

  const name = d.label;

  // ── Tag editing ───────────────────────────────────────────────────────
  const addTag = (raw: string) => {
    const tag = raw.trim();
    if (!tag) return;
    if (d.tags.includes(tag)) {
      setTagDraft('');
      return;
    }
    if (/\s/.test(tag) || /[[\]{}]/.test(tag)) {
      setTagDraft('');
      return;
    }
    onUpdateMetadata(name, { tags: [...d.tags, tag] });
    setTagDraft('');
  };

  const removeTag = (tag: string) => {
    onUpdateMetadata(name, { tags: d.tags.filter(t => t !== tag) });
  };

  const handleTagKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      addTag(tagDraft);
    } else if (e.key === 'Backspace' && tagDraft === '' && d.tags.length > 0) {
      e.preventDefault();
      removeTag(d.tags[d.tags.length - 1]);
    }
  };

  // ── Rename ────────────────────────────────────────────────────────────
  const commitRename = () => {
    const newName = renameDraft.trim();
    setRenaming(false);
    if (newName && newName !== name) {
      onRename(name, newName);
    }
  };

  // ── Group commit ──────────────────────────────────────────────────────
  const commitGroup = () => {
    const value = groupDraft.trim();
    if (value === (d.group ?? '')) return;
    if (value === '') {
      onUpdateMetadata(name, { group: null });
    } else {
      onUpdateMetadata(name, { group: value });
    }
  };

  // ── Manual link (reachable) commit ───────────────────────────────────
  const commitReachable = () => {
    const value = reachableDraft.trim();
    if (value === (d.manual_link_source ?? '')) return;
    if (value === '') {
      onUpdateMetadata(name, { reachable: null });
    } else {
      onUpdateMetadata(name, { reachable: value });
    }
  };

  // ── Size commit ───────────────────────────────────────────────────────
  const commitSize = () => {
    const w = parseFloat(sizeWDraft);
    const h = parseFloat(sizeHDraft);
    const hasDraft = sizeWDraft.trim() !== '' || sizeHDraft.trim() !== '';
    if (!hasDraft) {
      if (d.size_w != null || d.size_h != null) {
        onUpdateMetadata(name, { size: null });
      }
      return;
    }
    const width = Number.isFinite(w) && w > 0 ? Math.round(w) : 160;
    const height = Number.isFinite(h) && h > 0 ? Math.round(h) : 36;
    if (width !== Math.round(d.size_w ?? 0) || height !== Math.round(d.size_h ?? 0)) {
      onUpdateMetadata(name, { size: { width, height } });
    }
  };

  // ── Color ─────────────────────────────────────────────────────────────
  const applyColor = (value: string | null) => {
    onUpdateMetadata(name, { color: value });
  };

  const typeLabel = d.is_start ? 'Start passage' :
    d.is_metadata ? 'Metadata' :
    d.is_special ? 'Special' :
    d.is_entry ? 'Manual link declared' :
    d.is_unreachable ? 'Unreachable' :
    d.is_dead_end ? 'Dead end' :
    'Passage';

  const tagSuggestions = allTags.filter(t => !d.tags.includes(t));

  return (
    <aside
      className="inspector"
      ref={panelRef}
      style={{ width, ...(height != null ? { height } : {}) }}
    >
      <div
        className="inspector__resizer"
        onPointerDown={startResize}
        onDoubleClick={resetWidth}
        title="Drag to resize width · double-click to reset"
      />
      <div
        className="inspector__resizer inspector__resizer--h"
        onPointerDown={startHeightResize}
        onDoubleClick={resetHeight}
        title="Drag to resize height · double-click for full height"
      />
      {/* ── Header ─────────────────────────────────────────────────── */}
      <div className="inspector__header">
        <div className="inspector__title-wrap">
          {renaming ? (
            <input
              ref={renameInputRef}
              className="inspector__rename-input"
              value={renameDraft}
              onChange={e => setRenameDraft(e.target.value)}
              onKeyDown={e => {
                if (e.key === 'Enter') commitRename();
                if (e.key === 'Escape') setRenaming(false);
              }}
              onBlur={commitRename}
              autoFocus
              spellCheck={false}
            />
          ) : (
            <span className="inspector__title" title={name}>{name}</span>
          )}
          <span className={`inspector__badge${d.is_start ? ' inspector__badge--start' : ''}`}>
            {typeLabel}
          </span>
        </div>
        <div className="inspector__header-actions">
          {!renaming && (
            <button
              className="inspector__icon-btn"
              title="Rename passage"
              onClick={() => { setRenaming(true); setRenameDraft(name); }}
            >
              ✎
            </button>
          )}
          <button
            className="inspector__icon-btn inspector__icon-btn--close"
            title="Close inspector (Esc)"
            onClick={onClose}
          >
            ✕
          </button>
        </div>
      </div>

      <div className="inspector__scroll">
        {/* ── Tags ─────────────────────────────────────────────────── */}
        <section className="inspector__section">
          <label className="inspector__label">Tags</label>
          <div
            className="inspector__chips"
            onClick={e => {
              const input = e.currentTarget.querySelector('input');
              if (input && e.target === e.currentTarget) input.focus();
            }}
          >
            {d.tags.map(tag => (
              <span className="inspector__chip" key={tag}>
                {tag}
                <button
                  className="inspector__chip-x"
                  title={`Remove tag '${tag}'`}
                  onClick={() => removeTag(tag)}
                >
                  ×
                </button>
              </span>
            ))}
            <input
              ref={tagInputRef}
              className="inspector__chip-input"
              list="inspector-tag-suggestions"
              placeholder={d.tags.length === 0 ? 'Add tag…' : ''}
              value={tagDraft}
              onChange={e => setTagDraft(e.target.value)}
              onKeyDown={handleTagKeyDown}
              onBlur={() => addTag(tagDraft)}
              spellCheck={false}
            />
            <datalist id="inspector-tag-suggestions">
              {tagSuggestions.map(t => <option key={t} value={t} />)}
            </datalist>
          </div>
          <div className="inspector__field-hint">
            Enter to add · whitespace and brackets are not allowed
          </div>
        </section>

        {/* ── Manual reachability link ─────────────────────────────── */}
        <section className="inspector__section">
          <label className="inspector__label">
            Manual link <span className="inspector__label-key">reachable</span>
          </label>
          <div className="inspector__input-row">
            <input
              ref={reachableInputRef}
              className="inspector__input inspector__input--passage"
              list="inspector-passage-names"
              placeholder="Source passage…"
              value={reachableDraft}
              onChange={e => setReachableDraft(e.target.value)}
              onBlur={commitReachable}
              onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); commitReachable(); } }}
              spellCheck={false}
            />
            {d.manual_link_source && (
              <button
                className="inspector__input-clear"
                title="Remove the manual link"
                onClick={() => { setReachableDraft(''); onUpdateMetadata(name, { reachable: null }); }}
              >
                ✕
              </button>
            )}
            <datalist id="inspector-passage-names">
              {allPassageNames.filter(p => p !== name).map(p => <option key={p} value={p} />)}
            </datalist>
          </div>
          <div className="inspector__field-hint">
            {d.manual_link_source
              ? `Declared reachable from '${d.manual_link_source}' — rendered as a manual-link edge.`
              : 'Declares "this passage is reachable from X" for navigation static analysis can\u2019t see (e.g. variable-aliased links).'}
          </div>
        </section>

        {/* ── Color ────────────────────────────────────────────────── */}
        <section className="inspector__section" ref={colorSectionRef}>
          <label className="inspector__label">Color</label>
          <div className="inspector__swatches">
            {COLOR_PRESETS.map(preset => (
              <button
                key={preset.value}
                className={`inspector__swatch${(d.metadata_color ?? '') === preset.value ? ' inspector__swatch--active' : ''}`}
                style={{ background: preset.value }}
                title={preset.label}
                onClick={() => applyColor(preset.value)}
              />
            ))}
            <input
              className="inspector__color-custom"
              type="color"
              title="Custom color"
              value={/^#[0-9a-fA-F]{6}$/.test(colorDraft) ? colorDraft : '#2d6a9f'}
              onChange={e => { setColorDraft(e.target.value); applyColor(e.target.value); }}
            />
            {d.metadata_color && (
              <button
                className="inspector__swatch-clear"
                title="Use category color"
                onClick={() => { setColorDraft(''); applyColor(null); }}
              >
                auto
              </button>
            )}
          </div>
        </section>

        {/* ── Group ────────────────────────────────────────────────── */}
        <section className="inspector__section">
          <label className="inspector__label">Group</label>
          <div className="inspector__input-row">
            <input
              ref={groupInputRef}
              className="inspector__input"
              list="inspector-group-suggestions"
              placeholder="No group"
              value={groupDraft}
              onChange={e => setGroupDraft(e.target.value)}
              onBlur={commitGroup}
              onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); commitGroup(); } }}
              spellCheck={false}
            />
            <datalist id="inspector-group-suggestions">
              {allGroups.filter(g => g !== d.group).map(g => <option key={g} value={g} />)}
            </datalist>
          </div>
        </section>

        {/* ── Size ─────────────────────────────────────────────────── */}
        <section className="inspector__section">
          <label className="inspector__label">Node size</label>
          <div className="inspector__size-row">
            <input
              className="inspector__input inspector__input--num"
              type="number"
              min="40"
              max="2000"
              placeholder="160"
              title="Width (px)"
              value={sizeWDraft}
              onChange={e => setSizeWDraft(e.target.value)}
              onBlur={commitSize}
              onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); commitSize(); } }}
            />
            <span className="inspector__size-x">×</span>
            <input
              className="inspector__input inspector__input--num"
              type="number"
              min="24"
              max="2000"
              placeholder="36"
              title="Height (px)"
              value={sizeHDraft}
              onChange={e => setSizeHDraft(e.target.value)}
              onBlur={commitSize}
              onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); commitSize(); } }}
            />
            {(d.size_w != null || d.size_h != null) && (
              <button
                className="inspector__swatch-clear"
                title="Reset to default size"
                onClick={() => { setSizeWDraft(''); setSizeHDraft(''); onUpdateMetadata(name, { size: null }); }}
              >
                reset
              </button>
            )}
          </div>
        </section>

        {/* ── Info ─────────────────────────────────────────────────── */}
        <section className="inspector__section inspector__section--info">
          <div className="inspector__info-row"><span>Links</span><span>{d.in_degree} in · {d.out_degree} out</span></div>
          <div className="inspector__info-row"><span>Location</span><span className="inspector__file">{d.file.split('/').pop() || d.file}:{d.line + 1}</span></div>
          {d.var_writes.length > 0 && (
            <div className="inspector__info-row"><span>Writes</span><span className="inspector__vars">{d.var_writes.slice(0, 6).join(', ')}{d.var_writes.length > 6 ? '…' : ''}</span></div>
          )}
          {d.var_reads.length > 0 && (
            <div className="inspector__info-row"><span>Reads</span><span className="inspector__vars">{d.var_reads.slice(0, 6).join(', ')}{d.var_reads.length > 6 ? '…' : ''}</span></div>
          )}
        </section>
      </div>

      {/* ── Actions ────────────────────────────────────────────────── */}
      <div className="inspector__actions">
        {!d.is_start && (
          <button className="inspector__btn" onClick={() => onSetStart(name)} title="Rewrite StoryData's start field">
            Set as start
          </button>
        )}
        {deleteArmed ? (
          <div className="inspector__delete-confirm">
            <label className="inspector__delete-check">
              <input
                type="checkbox"
                checked={stripLinks}
                onChange={e => setStripLinks(e.target.checked)}
              />
              Also remove links
            </label>
            <button
              className="inspector__btn inspector__btn--danger inspector__btn--confirm"
              onClick={() => onDelete(name, stripLinks)}
              autoFocus
            >
              Confirm delete
            </button>
          </div>
        ) : (
          <button
            className="inspector__btn inspector__btn--danger"
            onClick={armDelete}
            title="Delete this passage and its body from the story"
          >
            Delete passage
          </button>
        )}
      </div>
    </aside>
  );
}
