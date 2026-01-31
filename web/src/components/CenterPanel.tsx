import { useAppStore } from '../store';
import { TabBar } from './TabBar';
import { ChatPanel } from './ChatPanel';
import { FileViewer } from './FileViewer';

export function CenterPanel() {
  const tabs = useAppStore((s) => s.tabs);
  const activeTab = useAppStore((s) => s.activeTab);

  const currentTab = tabs.find((t) => t.id === activeTab);

  return (
    <div className="panel center-panel">
      <TabBar />
      <div className="center-panel-content">
        {currentTab?.type === 'chat' && <ChatPanel />}
        {currentTab?.type === 'file' && currentTab.path && (
          <FileViewer path={currentTab.path} />
        )}
      </div>
    </div>
  );
}
