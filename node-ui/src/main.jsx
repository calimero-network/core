import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App.jsx'
import './index.css'
import { Buffer as BufferPolyfill } from 'buffer'

// Make Buffer to be available globally
globalThis.Buffer = BufferPolyfill

ReactDOM.createRoot(document.getElementById('root')).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
