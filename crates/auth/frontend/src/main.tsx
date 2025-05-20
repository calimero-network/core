import { Buffer } from 'buffer';
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './index.css';

// Properly set up Buffer and its isBuffer function
if (typeof window !== 'undefined') {
  window.Buffer = Buffer;
  window.Buffer.isBuffer = Buffer.isBuffer;
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);