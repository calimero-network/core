import { Buffer } from 'buffer';
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './index.css';
import ThemeProvider from './theme/ThemeProvider';

// Minimal polyfills needed for Buffer
if (typeof window !== 'undefined') {
  window.Buffer = Buffer;
  window.Buffer.isBuffer = Buffer.isBuffer;
  // @ts-ignore - just the minimal process properties needed
  window.process = { env: {} };
  // @ts-ignore
  window.global = window;
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  // <React.StrictMode>
    <ThemeProvider>
      <App />
    </ThemeProvider>
  // </React.StrictMode>,
);