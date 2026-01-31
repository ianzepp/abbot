import { useAppStore } from '../store';
import { TabBar } from './TabBar';
import { ChatPanel } from './ChatPanel';
import { BusPanel } from './BusPanel';
import { FileViewer } from './FileViewer';
import { SelfPanel } from './SelfPanel';
import { LtmPanel } from './LtmPanel';
import { ConclavePanel } from './ConclavePanel';

export function CenterPanel() {
  const tabs = useAppStore((s) => s.tabs);
  const activeTab = useAppStore((s) => s.activeTab);

  const currentTab = tabs.find((t) => t.id === activeTab);

  return (
    <div className="panel center-panel">
      <TabBar />
      <div className="center-panel-content">
        {currentTab?.type === 'chat' && <ChatPanel />}
        {currentTab?.type === 'bus' && <BusPanel />}
        {currentTab?.type === 'self' && <SelfPanel />}
        {currentTab?.type === 'ltm' && <LtmPanel />}
        {currentTab?.type === 'conclave' && currentTab.path && (
          <ConclavePanel conclaveId={currentTab.path} />
        )}
        {currentTab?.type === 'file' && currentTab.path && (
          <FileViewer path={currentTab.path} />
        )}
      </div>
    </div>
  );
}
