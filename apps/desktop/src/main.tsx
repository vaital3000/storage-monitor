import { QueryClientProvider } from '@tanstack/react-query';
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { createQueryClient } from './lib/queryClient';
import './index.css';

// `vite --mode mock` loads .env.mock and lets the UI run in a plain browser. The
// dynamic import keeps the mock and its fixture out of the production bundle.
if (import.meta.env.VITE_MOCK_IPC === '1') {
  const { installIpcMock } = await import('./mocks/ipc');
  installIpcMock();
}

const queryClient = createQueryClient();

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </React.StrictMode>,
);
