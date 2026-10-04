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
      for (const n of nodes) onUpdateMetadata(n.data.label, { color: value });
    };

    const commitGroupAll = () => {
      const value = groupDraft.trim();
      for (const n of nodes) {
        if (value === '') {
          if (n.data.group) onUpdateMetadata(n.data.label, { group: null });
        } else if (value !== n.data.group) {
          onUpdateMetadata(n.data.label, { group: value });
        }
      }
    };

    const resetSizesAll = () => {
      for (const n of nodes) {
        if (n.data.size_w != null || n.data.size_h != null) {
          onUpdateMetadata(n.data.label, { size: null });
        }
      }
    };

    const deleteAll = () => {
      for (const n of nodes) onDelete(n.data.label, stripLinks);
    };

    const anySized = nodes.some(n => n.data.size_w != null || n.data.size_h != null);

    return (
      <aside className="inspector inspector--multi">
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

          {/* ── Bulk size reset ───────────────────────────────────── */}
          {anySized && (
            <section className="inspector__section">
              <label className="inspector__label">Node size</label>
              <button className="inspector__btn" onClick={resetSizesAll}>
                Reset custom sizes
              </button>
            </section>
          )}
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
    <aside className="inspector">
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
