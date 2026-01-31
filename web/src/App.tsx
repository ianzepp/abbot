import { useEffect } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { FileTree } from './components/FileTree';
import { CenterPanel } from './components/CenterPanel';
import { ActivityPanel } from './components/ActivityPanel';
import { StatusBar } from './components/StatusBar';
import { useBus } from './hooks/useBus';
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
  // Connect to the bus for real-time updates
  useBus();

  useEffect(() => {
    document.title = 'Abbot';
  }, []);

  return (
    <div className="app-container">
      <div className="app-layout">
        <FileTree />
        <CenterPanel />
        <ActivityPanel />
      </div>
      <StatusBar />
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
