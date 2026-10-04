import { useEffect, useRef, useState } from 'react';

export interface CreatePassageRequest {
  /** Graph-canvas coordinates where the passage should be created. */
  x: number;
  y: number;
}

interface CreatePassageModalProps {
  request: CreatePassageRequest;
  existingNames: Set<string>;
  allTags: string[];
  onCancel: () => void;
  onCreate: (name: string, tags: string[], position: { x: number; y: number }) => void;
}

export default function CreatePassageModal({
  request,
  existingNames,
  allTags,
  onCancel,
  onCreate,
}: CreatePassageModalProps) {
  const [name, setName] = useState('');
  const [tagDraft, setTagDraft] = useState('');
  const [tags, setTags] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    nameInputRef.current?.focus();
  }, []);

  const addTag = (raw: string) => {
    const tag = raw.trim();
    setTagDraft('');
    if (!tag || tags.includes(tag)) return;
    if (/\s/.test(tag) || /[[\]{}]/.test(tag)) return;
    setTags([...tags, tag]);
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
    onCreate(trimmed, tags, { x: request.x, y: request.y });
  };

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
