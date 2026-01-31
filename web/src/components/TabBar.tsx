import { useAppStore } from '../store';

export interface Tab {
  id: string;
  title: string;
  type: 'chat' | 'file';
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

  return (
    <div className="tab-bar">
      {tabs.map((tab) => (
        <div
          key={tab.id}
          className={`tab ${activeTab === tab.id ? 'active' : ''} ${tab.type}`}
          onClick={() => setActiveTab(tab.id)}
        >
          <span className="tab-icon">
            {tab.type === 'chat' ? (
              <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                <path d="M2.5 2.5C2.5 1.67157 3.17157 1 4 1H12C12.8284 1 13.5 1.67157 13.5 2.5V10.5C13.5 11.3284 12.8284 12 12 12H6.5L3.5 15V12H4C3.17157 12 2.5 11.3284 2.5 10.5V2.5Z"/>
              </svg>
            ) : (
              <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                <path d="M3.5 1.75C3.5 1.33579 3.83579 1 4.25 1H9.5V4.5C9.5 4.77614 9.72386 5 10 5H13.5V14.25C13.5 14.6642 13.1642 15 12.75 15H4.25C3.83579 15 3.5 14.6642 3.5 14.25V1.75ZM10.5 1.20711L13.2929 4H10.5V1.20711Z"/>
              </svg>
            )}
          </span>
          <span className="tab-title">{tab.title}</span>
          {tab.type !== 'chat' && (
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
