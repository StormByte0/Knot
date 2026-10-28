import type { EdgeKind } from '../types';

// Palette shared with StoryMap.tsx (kept in sync by convention).
const C = {
  normal: '#3a7ca5',
  start: '#2f9e6e',
  special: '#d97b3f',
  metadata: '#8e5bc0',
  unreachable: '#a87534',
  ghost: '#f87171',
  edgeNav: '#8494ab',
  edgeJump: '#a78bfa',
  edgeInclude: '#60a5fa',
  edgeUp: '#64748b',
  edgeManual: '#f0b429',
  edgeBroken: '#f87171',
};

interface LegendProps {
  visible?: boolean;
  /** Edge kinds currently hidden via the chips. */
  hiddenEdgeKinds: Set<EdgeKind>;
  onToggleEdgeKind: (kind: EdgeKind) => void;
}

const EDGE_ENTRIES: Array<{ kind: EdgeKind; label: string; color: string; style?: string }> = [
  { kind: 'navigation', label: 'Navigate', color: C.edgeNav },
  { kind: 'jump', label: 'Jump (<<goto>>)', color: C.edgeJump, style: 'dashed' },
  { kind: 'include', label: 'Include', color: C.edgeInclude, style: 'dashed' },
  { kind: 'upstream', label: 'Lifecycle', color: C.edgeUp, style: 'dashed' },
  { kind: 'manual', label: 'Manual link (reachable)', color: C.edgeManual, style: 'dotted' },
  { kind: 'broken', label: 'Broken link', color: C.edgeBroken, style: 'dashed' },
];

export default function Legend({ visible = true, hiddenEdgeKinds, onToggleEdgeKind }: LegendProps) {
  // Hidden entirely when the context-menu toggle is off (the internal
  // collapse chip is the lightweight fold).
  if (!visible) return null;

  return (
    <div id="legend">
      {/* ── Passage nodes ─────────────────────────────────────────────── */}
      <div className="legend-section">Passages</div>
      <div className="legend-item">
        <span className="legend-card" style={{ '--c': C.normal } as React.CSSProperties} />
        Passage
      </div>
      <div className="legend-item">
        <span
          className="legend-card"
          style={{ '--c': C.start, '--ring': 'rgba(255,255,255,0.55)' } as React.CSSProperties}
          title="The story's starting passage"
        />
        Start
      </div>
      <div className="legend-item">
        <span
          className="legend-card"
          style={{ '--c': C.normal, '--outline': 'rgba(46, 204, 113, 0.8)' } as React.CSSProperties}
          title={'Passage declaring a `reachable` manual link in its header metadata — "reachable from the named passage"'}
        />
        Manual link
      </div>
      <div className="legend-item">
        <span className="legend-card" style={{ '--c': C.special } as React.CSSProperties} />
        Special
      </div>
      <div className="legend-item">
        <span className="legend-card" style={{ '--c': C.metadata } as React.CSSProperties} />
        Metadata
      </div>
      <div className="legend-item">
        <span
          className="legend-card legend-card--dashed"
          style={{ '--c': C.unreachable } as React.CSSProperties}
        />
        Unreachable
      </div>
      <div className="legend-item">
        <span
          className="legend-card legend-card--ghost"
          style={{ '--c': C.ghost } as React.CSSProperties}
          title="A link points at a passage that doesn't exist — click the ghost to create it"
        />
        Ghost (missing)
      </div>
      <div className="legend-item">
        <span className="legend-badge legend-badge--warn" title="Dead-end indicator (header badge)" />
        Dead end
      </div>

      {/* ── Edge types (clickable: toggle visibility) ─────────────────── */}
      <div className="legend-section">Edges — click to toggle</div>
      {EDGE_ENTRIES.map(e => (
        <button
          key={e.kind}
          className={`legend-item legend-item--toggle${hiddenEdgeKinds.has(e.kind) ? ' legend-item--off' : ''}`}
          title={hiddenEdgeKinds.has(e.kind)
            ? `Show ${e.label} edges`
            : `Hide ${e.label} edges`}
          onClick={() => onToggleEdgeKind(e.kind)}
        >
          <span
            className="legend-edge"
            style={{
              borderColor: e.color,
              borderStyle: (e.style as React.CSSProperties['borderStyle']) ?? 'solid',
            }}
          />
          {e.label}
          <span className="legend-item__state" aria-hidden="true">
            {hiddenEdgeKinds.has(e.kind) ? 'off' : ''}
          </span>
        </button>
      ))}

      {/* ── Visual indicators ─────────────────────────────────────────── */}
      <div className="legend-section">Indicators</div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{
            background: 'transparent',
            border: '1px dashed rgba(255,255,255,0.14)',
            borderRadius: 3,
          }}
        />
        Group
      </div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{
            background: 'rgba(217,123,63,0.10)',
            border: '1px solid rgba(217,123,63,0.28)',
            borderRadius: 3,
          }}
        />
        Specials zone
      </div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{
            background: 'rgba(168,117,52,0.10)',
            border: '1px solid rgba(168,117,52,0.28)',
            borderRadius: 3,
          }}
        />
        Unreachable zone
      </div>
      <div className="legend-item">
        <span
          className="legend-tag-badge"
          title="Number of tags on this passage"
        >
          3
        </span>
        Tag count
      </div>
    </div>
  );
}
