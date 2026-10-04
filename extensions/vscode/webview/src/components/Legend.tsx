import { useState } from 'react';

// Palette shared with StoryMap.tsx (kept in sync by convention).
const C = {
  normal: '#3a7ca5',
  start: '#2f9e6e',
  special: '#d97b3f',
  metadata: '#8e5bc0',
  unreachable: '#a87534',
  edgeNav: '#8494ab',
  edgeJump: '#a78bfa',
  edgeCall: '#34d399',
  edgeInclude: '#60a5fa',
  edgeUp: '#64748b',
  edgeManual: '#f0b429',
  edgeBroken: '#f87171',
};

export default function Legend({ visible = true }: { visible?: boolean }) {
  const [collapsed, setCollapsed] = useState(false);

  // Hidden entirely when the context-menu toggle is off (the internal
  // collapse chip is the lightweight fold).
  if (!visible) return null;

  if (collapsed) {
    return (
      <div id="legend" className="legend-collapsed">
        <button
          className="legend-toggle"
          title="Show legend"
          onClick={() => setCollapsed(false)}
        >
          &#x25B8;
        </button>
      </div>
    );
  }

  return (
    <div id="legend">
      <button
        className="legend-toggle"
        title="Hide legend"
        onClick={() => setCollapsed(true)}
      >
        &#x25BE;
      </button>
      {/* ── Passage nodes ─────────────────────────────────────────────── */}
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
        <span className="legend-badge legend-badge--warn" title="Dead-end indicator (header badge)" />
        Dead end
      </div>

      {/* ── Edge types ────────────────────────────────────────────────── */}
      <div className="legend-section">Edges</div>
      <div className="legend-item">
        <span className="legend-edge" style={{ borderColor: C.edgeNav }} />
        Navigate
      </div>
      <div className="legend-item">
        <span
          className="legend-edge"
          style={{ borderColor: C.edgeJump, borderStyle: 'dashed' }}
        />
        Jump (&lt;&lt;goto&gt;&gt;)
      </div>
      <div className="legend-item">
        <span
          className="legend-edge"
          style={{ borderColor: C.edgeCall, borderStyle: 'dotted' }}
        />
        Call (widget)
      </div>
      <div className="legend-item">
        <span
          className="legend-edge"
          style={{ borderColor: C.edgeInclude, borderStyle: 'dashed' }}
        />
        Include
      </div>
      <div className="legend-item">
        <span
          className="legend-edge"
          style={{ borderColor: C.edgeUp, borderStyle: 'dashed' }}
        />
        Lifecycle
      </div>
      <div className="legend-item">
        <span
          className="legend-edge"
          style={{ borderColor: C.edgeManual, borderStyle: 'dotted' }}
        />
        Manual link (reachable)
      </div>
      <div className="legend-item">
        <span
          className="legend-edge"
          style={{ borderColor: C.edgeBroken, borderStyle: 'dashed' }}
        />
        Broken link
      </div>

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
