import { useEffect } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { FileTree } from './components/FileTree';
import { CenterPanel } from './components/CenterPanel';
import { ActivityPanel } from './components/ActivityPanel';
import { useWebSocket } from './hooks/useWebSocket';
import './index.css';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      staleTime: 5000,
    },
  },
});

function AppContent() {
  useWebSocket();

  useEffect(() => {
    document.title = 'Abbot';
  }, []);

  return (
    <div className="app-layout">
      <FileTree />
      <CenterPanel />
      <ActivityPanel />
    </div>
  );
}

function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <AppContent />
    </QueryClientProvider>
  );
}

export default App;
