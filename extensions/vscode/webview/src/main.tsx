import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './styles/storymap.css';

const root = ReactDOM.createRoot(document.getElementById('root')!);
// NOTE: React.StrictMode is intentionally NOT used here.
// StrictMode double-mounts components in development (mount → unmount → mount),
// which causes Cytoscape to be initialized, destroyed, and re-initialized.
// The destroy step clears the canvas, causing the "flash then dark" bug
// in VS Code webviews. The production build ignores StrictMode anyway,
// so removing it makes dev behavior match production.
root.render(<App />);
