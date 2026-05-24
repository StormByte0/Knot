import React from 'react';

export default function Legend() {
  return (
    <div id="legend">
      <div className="legend-item">
        <span className="legend-dot" style={{ background: '#2d6a9f' }} />
        Passage
      </div>
      <div className="legend-item">
        <span className="legend-dot" style={{ background: '#2e7d32', border: '2px solid rgba(255,255,255,0.5)' }} />
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
        <span className="legend-dot" style={{ background: '#424242' }} />
        Unreachable
      </div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{
            background: 'transparent',
            border: '1.5px dashed #c62828',
            borderRadius: 2,
          }}
        />
        Broken link
      </div>
      <div className="legend-item">
        <span
          className="legend-dot"
          style={{
            background: 'transparent',
            border: '1px dashed #3a3a5a',
            borderRadius: 2,
          }}
        />
        Group
      </div>
    </div>
  );
}