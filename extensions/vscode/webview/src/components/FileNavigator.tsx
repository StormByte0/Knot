import { useMemo, useState } from 'react';
import type { WorkspaceFileInfo } from '../types';

// ── File navigator ─────────────────────────────────────────────────────────
//
// A minimizable left rail listing the game source files — the .twee/.tw
// sources, .js scripts, and .css styles Tweego compiles. Clicking a file
// opens it in the editor; the file owning the current primary selection is
// highlighted so the rail doubles as a "where am I?" bridge between the
// graph and the sources.

interface FileNavigatorProps {
  tweeFiles: WorkspaceFileInfo[];
  jsFiles: WorkspaceFileInfo[];
  cssFiles: WorkspaceFileInfo[];
  /** File URI of the primary graph selection (highlight bridge). */
  activeFile?: string;
  onOpenFile: (uri: string) => void;
}

const KIND_LABEL: Record<WorkspaceFileInfo['kind'], string> = {
  twee: 'Twee sources',
  js: 'Scripts',
  css: 'Styles',
};

export default function FileNavigator({
  tweeFiles,
  jsFiles,
  cssFiles,
  activeFile,
  onOpenFile,
}: FileNavigatorProps) {
  const [collapsed, setCollapsed] = useState(false);

  const sections = useMemo(() => ([
    { kind: 'twee' as const, files: tweeFiles },
    { kind: 'js' as const, files: jsFiles },
    { kind: 'css' as const, files: cssFiles },
  ]), [tweeFiles, jsFiles, cssFiles]);

  const total = tweeFiles.length + jsFiles.length + cssFiles.length;

  if (collapsed) {
    return (
      <div className="file-nav file-nav--collapsed">
        <button
          className="file-nav__toggle"
          title="Show game files"
          onClick={() => setCollapsed(false)}
        >
          <svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true">
            <path d="M4 2 L8 6 L4 10" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          <span className="file-nav__toggle-count">{total}</span>
        </button>
      </div>
    );
  }

  return (
    <div className="file-nav">
      <div className="file-nav__header">
        <span className="file-nav__title">Game files</span>
        <button
          className="file-nav__toggle file-nav__toggle--inline"
          title="Hide game files"
          onClick={() => setCollapsed(true)}
        >
          <svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true">
            <path d="M8 2 L4 6 L8 10" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>
      </div>
      {total === 0 ? (
        <div className="file-nav__empty">
          No .twee, .js or .css files found in the workspace.
        </div>
      ) : (
        sections.map(({ kind, files }) =>
          files.length === 0 ? null : (
            <div key={kind} className="file-nav__section">
              <div className="file-nav__section-label">
                {KIND_LABEL[kind]}
                <span className="file-nav__section-count">{files.length}</span>
              </div>
              {files.map(f => (
                <button
                  key={f.uri}
                  className={[
                    'file-nav__item',
                    f.uri === activeFile && 'file-nav__item--active',
                    `file-nav__item--${kind}`,
                  ].filter(Boolean).join(' ')}
                  title={f.name}
                  onClick={() => onOpenFile(f.uri)}
                >
                  <span className={`file-nav__dot file-nav__dot--${kind}`} aria-hidden="true" />
                  <span className="file-nav__name">{f.name}</span>
                  {kind === 'twee' && (
                    <span className="file-nav__count">{f.passageCount}</span>
                  )}
                </button>
              ))}
            </div>
          ),
        )
      )}
    </div>
  );
}
