import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { installIpcMock } from './mocks/ipc';
import './index.css';

// `vite --mode mock` loads .env.mock and lets the UI run in a plain browser.
if (import.meta.env.VITE_MOCK_IPC === '1') {
  installIpcMock();
}

// Commands are local and deterministic: a failure is worth showing, not retrying.
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </React.StrictMode>,
);
