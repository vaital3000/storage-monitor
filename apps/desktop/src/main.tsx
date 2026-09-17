import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { installIpcMock } from './mocks/ipc';
import './index.css';

// `vite --mode mock` loads .env.mock and lets the UI run in a plain browser.
if (import.meta.env.VITE_MOCK_IPC === '1') {
  installIpcMock();
}

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
