import React from 'react';
import ReactDOM from 'react-dom/client';
import './styles/index.css';
import 'react-tooltip/dist/react-tooltip.css';
import App from './App';
// import { ServerDownProvider } from './context/ServerDownContext';

const root = ReactDOM.createRoot(
  document.getElementById('root') as HTMLElement,
);

root.render(
  <React.StrictMode>
        <App />
  </React.StrictMode>,
);
