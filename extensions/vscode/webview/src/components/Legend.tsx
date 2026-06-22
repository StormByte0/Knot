import { useState } from 'react';

export default function Legend() {
  const [collapsed, setCollapsed] = useState(false);

  if (collapsed) {
    return (
      <div id="legend" className="legend-collapsed">
        <button
          className="legend-toggle"
          title="Show legend"
          onClick={() => setCollapsed(false)}
        >
          &#x25B6;
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
        &#x25BC;
      </button>
      {/* ── Passage nodes ─────────────────────────────────────────────── */}
      <div className="legend-item">
        <span className="legend-dot" style={{ background: '#2d6a9f' }} />
        Passage
      </div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{ background: '#2e7d32', border: '2px solid rgba(255,255,255,0.5)' }}
        />
        Start
      </div>
      <div className="legend-item">
        <span className="legend-dot" style={{ background: '#e65100' }} />
        Special
      </div>
      <div className="legend-item">
        <span className="legend-dot" style={{ background: '#6a1b9a' }} />
        Metadata
      </div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{
            background: '#bf6900',
            border: '1.5px dashed #bf6900',
          }}
        />
        Unreachable
      </div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{
            background: 'transparent',
            border: '3px double rgba(255,193,7,0.4)',
          }}
        />
        Dead end
      </div>

      {/* ── Edge types ────────────────────────────────────────────────── */}
      <div className="legend-section">Edges</div>
      <div className="legend-item">
        <span className="legend-edge" style={{ borderColor: '#5a6a7e' }} />
        Navigate
      </div>
      <div className="legend-item">
        <span
          className="legend-edge"
          style={{ borderColor: '#2e86ab', borderStyle: 'dashed' }}
        />
        Include
      </div>
      <div className="legend-item">
        <span
          className="legend-edge"
          style={{ borderColor: '#c62828', borderStyle: 'dashed' }}
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
            border: '1px solid rgba(255,255,255,0.08)',
            borderRadius: 2,
          }}
        />
        Group
      </div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{
            background: 'rgba(230,81,0,0.15)',
            border: '1px solid rgba(230,81,0,0.25)',
            borderRadius: 2,
          }}
        />
        Specials zone
      </div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{
            background: 'rgba(191,105,0,0.15)',
            border: '1px solid rgba(191,105,0,0.25)',
            borderRadius: 2,
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
