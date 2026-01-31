import { useAppStore } from '../store';

export interface Tab {
  id: string;
  title: string;
  type: 'chat' | 'file' | 'self' | 'ltm' | 'conclave';
  path?: string;
}

export function TabBar() {
  const tabs = useAppStore((s) => s.tabs);
  const activeTab = useAppStore((s) => s.activeTab);
  const setActiveTab = useAppStore((s) => s.setActiveTab);
  const closeTab = useAppStore((s) => s.closeTab);

  const handleClose = (e: React.MouseEvent, tabId: string) => {
    e.stopPropagation();
    closeTab(tabId);
  };

  const getTabIcon = (type: Tab['type']) => {
    switch (type) {
      case 'chat':
        return (
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
            <path d="M2.5 2.5C2.5 1.67157 3.17157 1 4 1H12C12.8284 1 13.5 1.67157 13.5 2.5V10.5C13.5 11.3284 12.8284 12 12 12H6.5L3.5 15V12H4C3.17157 12 2.5 11.3284 2.5 10.5V2.5Z"/>
          </svg>
        );
      case 'self':
        return (
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
            <path d="M8 1a3 3 0 100 6 3 3 0 000-6zM4 9a2 2 0 00-2 2v1a2 2 0 002 2h8a2 2 0 002-2v-1a2 2 0 00-2-2H4z"/>
          </svg>
        );
      case 'ltm':
        return (
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
            <path d="M8 1.5a6.5 6.5 0 100 13 6.5 6.5 0 000-13zM8 4a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 018 4zm0 8a1 1 0 100-2 1 1 0 000 2z"/>
          </svg>
        );
      case 'conclave':
        return (
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
            <path d="M5 3a2 2 0 100 4 2 2 0 000-4zM11 3a2 2 0 100 4 2 2 0 000-4zM8 9a2 2 0 100 4 2 2 0 000-4zM3 8a1 1 0 011-1h2a1 1 0 010 2H4a1 1 0 01-1-1zM10 8a1 1 0 011-1h2a1 1 0 010 2h-2a1 1 0 01-1-1zM6.5 12.5a1 1 0 011-1h1a1 1 0 010 2h-1a1 1 0 01-1-1z"/>
          </svg>
        );
      case 'file':
      default:
        return (
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
            <path d="M3.5 1.75C3.5 1.33579 3.83579 1 4.25 1H9.5V4.5C9.5 4.77614 9.72386 5 10 5H13.5V14.25C13.5 14.6642 13.1642 15 12.75 15H4.25C3.83579 15 3.5 14.6642 3.5 14.25V1.75ZM10.5 1.20711L13.2929 4H10.5V1.20711Z"/>
          </svg>
        );
    }
  };

  const isFixedTab = (type: Tab['type']) => type === 'chat' || type === 'self' || type === 'ltm';

  return (
    <div className="tab-bar">
      {tabs.map((tab) => (
        <div
          key={tab.id}
          className={`tab ${activeTab === tab.id ? 'active' : ''} ${tab.type}`}
          onClick={() => setActiveTab(tab.id)}
        >
          <span className="tab-icon">
            {getTabIcon(tab.type)}
          </span>
          <span className="tab-title">{tab.title}</span>
          {!isFixedTab(tab.type) && (
            <button
              className="tab-close"
              onClick={(e) => handleClose(e, tab.id)}
              title="Close"
            >
              <svg width="10" height="10" viewBox="0 0 16 16" fill="currentColor">
                <path d="M4.5 4.5L11.5 11.5M11.5 4.5L4.5 11.5" stroke="currentColor" strokeWidth="1.5" fill="none"/>
              </svg>
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
