import React from 'react';

export default function Legend() {
  return (
    <div id="legend">
      <div className="legend-item"><span className="legend-dot" style={{ background: '#4fc3f7' }}></span> Passage</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: '#66bb6a' }}></span> Start</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: '#ffb74d' }}></span> Special</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: '#ce93d8' }}></span> Metadata</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: '#555' }}></span> Unreachable</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: '#ff7043', border: '3px double #ff7043' }}></span> Game Loop</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: 'transparent', border: '2px dashed #f14c4c' }}></span> Broken link</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: 'transparent', border: '2px dashed #888' }}></span> Upstream</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: 'transparent', border: '2px dotted #ab47bc' }}></span> Call</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: 'transparent', border: '2px dotted #26a69a' }}></span> Include</div>
      <div className="legend-item"><span className="legend-dot" style={{ background: 'transparent', border: '2px solid #ffa726' }}></span> Jump</div>
    </div>
  );
}
