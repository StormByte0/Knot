import React from 'react';

export default function Legend() {
  return (
    <div id="legend">
      <div className="legend-item"><span className="legend-dot" style={{ background: '#3a7ca5' }}></span> Passage</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: '#43a047', border: '2px solid #fff' }}></span> Start</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: '#ef6c00' }}></span> Special</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: '#8e24aa' }}></span> Metadata</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: '#4a4a4a' }}></span> Unreachable</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: 'transparent', border: '1px dashed #666' }}></span> Group</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: 'transparent', border: '2px dashed #e53935' }}></span> Broken link</div>
    </div>
  );
}
