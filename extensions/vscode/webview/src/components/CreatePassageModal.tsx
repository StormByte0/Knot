import { useEffect, useRef, useState } from 'react';
import type { WorkspaceFileInfo } from '../types';

export interface CreatePassageRequest {
  /** Graph-canvas coordinates where the passage should be created. */
  x: number;
  y: number;
  /** Prefilled name (broken-link ghost affordance). */
  prefillName?: string;
}

interface CreatePassageModalProps {
  request: CreatePassageRequest;
  existingNames: Set<string>;
  allTags: string[];
  /** Every .twee/.tw file in the workspace (the target-file picker). */
  tweeFiles: WorkspaceFileInfo[];
  /** The heuristic default: StoryData's file, else the largest twee file. */
  defaultTargetFile?: string;
  /** `knot.build.sourceDir` (workspace-relative, '' = root) — the base
   *  directory for "New file…" paths (extension host resolves against it). */
  sourceDir?: string;
  onCancel: () => void;
  onCreate: (
    name: string,
    tags: string[],
    position: { x: number; y: number },
    target: { file?: string; newFile?: string },
  ) => void;
}

export default function CreatePassageModal({
  request,
  existingNames,
  allTags,
  tweeFiles,
  defaultTargetFile,
  sourceDir,
  onCancel,
  onCreate,
}: CreatePassageModalProps) {
  const [name, setName] = useState(request.prefillName ?? '');
  const [tagDraft, setTagDraft] = useState('');
  const [tags, setTags] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Target file: an existing URI, or '__new__' to create a new file.
  const [target, setTarget] = useState<string>(defaultTargetFile ?? tweeFiles[0]?.uri ?? '__new__');
  const [newFileDraft, setNewFileDraft] = useState('');
  const nameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    nameInputRef.current?.focus();
    if (request.prefillName) nameInputRef.current?.select();
  }, [request.prefillName]);

  const addTag = (raw: string) => {
    const tag = raw.trim();
    setTagDraft('');
    if (!tag || tags.includes(tag)) return;
    if (/\s/.test(tag) || /[[\]{}]/.test(tag)) return;
    setTags([...tags, tag]);
  };

  const validateNewFile = (raw: string): string | null => {
    const rel = raw.trim().replace(/\\/g, '/');
    if (!rel) return 'File name is required';
    if (rel.includes('..')) return 'Path cannot contain ".."';
    if (!/^[A-Za-z0-9_\-./ ]+$/.test(rel)) {
      return 'Use letters, digits, "-", "_", "/", spaces and "." only';
    }
    if (!/\.(twee|tw)$/i.test(rel)) return "File must end in '.twee' or '.tw'";
    return null;
  };

  const validateAndCreate = () => {
    const trimmed = name.trim();
    if (!trimmed) {
      setError('Passage name is required');
      return;
    }
    if (/::/.test(trimmed) || /[[\]{}]/.test(trimmed)) {
      setError("Name cannot contain '::', '[', ']', '{' or '}'");
      return;
    }
    if (trimmed !== name) {
      setError('Name cannot start or end with whitespace');
      return;
    }
    if (existingNames.has(trimmed)) {
      setError(`A passage named '${trimmed}' already exists`);
      return;
    }
    if (target === '__new__') {
      const fileErr = validateNewFile(newFileDraft);
      if (fileErr) {
        setError(fileErr);
        return;
      }
      onCreate(trimmed, tags, { x: request.x, y: request.y }, {
        newFile: newFileDraft.trim().replace(/\\/g, '/'),
      });
      return;
    }
    onCreate(trimmed, tags, { x: request.x, y: request.y }, { file: target });
  };

  const newFileSelected = target === '__new__';

  return (
    <div
      className="modal-backdrop"
      onMouseDown={e => { if (e.target === e.currentTarget) onCancel(); }}
    >
      <div
        className="modal"
        onKeyDown={e => {
          if (e.key === 'Escape') onCancel();
          if (e.key === 'Enter') { e.preventDefault(); validateAndCreate(); }
        }}
      >
        <div className="modal__title">New passage</div>
        <div className="modal__subtitle">
          Created at {Math.round(request.x)}, {Math.round(request.y)}
        </div>

        <label className="modal__label">Name</label>
        <input
          ref={nameInputRef}
          className="modal__input"
          value={name}
          onChange={e => { setName(e.target.value); setError(null); }}
          placeholder="Passage name"
          spellCheck={false}
        />

        <label className="modal__label">Tags</label>
        <div className="inspector__chips modal__chips">
          {tags.map(tag => (
            <span className="inspector__chip" key={tag}>
              {tag}
              <button
                className="inspector__chip-x"
                onClick={() => setTags(tags.filter(t => t !== tag))}
              >
                ×
              </button>
            </span>
          ))}
          <input
            className="inspector__chip-input"
            list="modal-tag-suggestions"
            placeholder={tags.length === 0 ? 'Add tag…' : ''}
            value={tagDraft}
            onChange={e => setTagDraft(e.target.value)}
            onKeyDown={e => {
              if (e.key === 'Enter' || e.key === ',') {
                e.preventDefault();
                addTag(tagDraft);
              } else if (e.key === 'Backspace' && tagDraft === '' && tags.length > 0) {
                e.preventDefault();
                setTags(tags.slice(0, -1));
              }
            }}
            onBlur={() => addTag(tagDraft)}
            spellCheck={false}
          />
          <datalist id="modal-tag-suggestions">
            {allTags.filter(t => !tags.includes(t)).map(t => <option key={t} value={t} />)}
          </datalist>
        </div>

        <label className="modal__label">Target file</label>
        <select
          className="modal__input modal__select"
          value={target}
          onChange={e => { setTarget(e.target.value); setError(null); }}
        >
          {tweeFiles.length === 0 && <option value="__new__">No .twee files — create one</option>}
          {tweeFiles.map(f => (
            <option key={f.uri} value={f.uri}>
              {f.name} ({f.passageCount} passages)
            </option>
          ))}
          <option value="__new__">＋ New file…</option>
        </select>
        {newFileSelected && (
          <>
            <label className="modal__label">
              New file path (relative to {sourceDir ? sourceDir : 'workspace root'})
            </label>
            <input
              className="modal__input"
              value={newFileDraft}
              onChange={e => { setNewFileDraft(e.target.value); setError(null); }}
              placeholder={sourceDir ? `${sourceDir}/chapter1.twee` : 'chapter1.twee'}
              spellCheck={false}
            />
            <div className="modal__hint">
              {sourceDir
                ? `Created inside the source directory ${sourceDir}/ (knot.build.sourceDir).`
                : 'Created in the workspace root. Set knot.build.sourceDir to keep sources in a subfolder.'}
            </div>
          </>
        )}

        {error && <div className="modal__error">{error}</div>}

        <div className="modal__actions">
          <button className="inspector__btn" onClick={onCancel}>Cancel</button>
          <button className="inspector__btn inspector__btn--primary" onClick={validateAndCreate}>
            Create passage
          </button>
        </div>
      </div>
    </div>
  );
}
