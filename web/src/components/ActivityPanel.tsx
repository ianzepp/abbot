import { useAppStore } from '../store';
import type { Need, Want, Task, Conclave } from '../types';

function SectionHeader({ 
  title, 
  count, 
  icon,
  collapsed, 
  onToggle 
}: { 
  title: string; 
  count: number;
  icon: React.ReactNode;
  collapsed: boolean; 
  onToggle: () => void;
}) {
  return (
    <div className="activity-section-header" onClick={onToggle}>
      <span className="activity-section-title">
        <span style={{ 
          display: 'inline-flex', 
          transition: 'transform 0.1s',
          transform: collapsed ? 'rotate(-90deg)' : 'rotate(0deg)'
        }}>
          <svg width="10" height="10" viewBox="0 0 16 16" fill="var(--text-muted)">
            <path d="M4.5 5.5L8 9L11.5 5.5" stroke="currentColor" strokeWidth="1.5" fill="none"/>
          </svg>
        </span>
        {icon}
        {title}
      </span>
      {count > 0 && <span className="activity-section-count">{count}</span>}
    </div>
  );
}

function NeedItem({ need }: { need: Need }) {
  return (
    <div className="activity-item">
      <div className="activity-item-header">
        <span className={`activity-item-priority ${need.priority}`} />
        <span className="activity-item-id">{need.id.slice(0, 8)}</span>
        <span className="activity-item-status pending">pending</span>
      </div>
      <div className="activity-item-text">{need.need}</div>
    </div>
  );
}

function WantItem({ want }: { want: Want }) {
  return (
    <div className="activity-item">
      <div className="activity-item-header">
        <span className={`activity-item-priority ${want.priority}`} />
        <span className="activity-item-id">{want.id.slice(0, 8)}</span>
      </div>
      <div className="activity-item-text">{want.want}</div>
    </div>
  );
}

function TaskItem({ task }: { task: Task }) {
  return (
    <div className="activity-item">
      <div className="activity-item-header">
        <span className="activity-item-id">{task.id.slice(0, 8)}</span>
        <span style={{ fontSize: 10, color: 'var(--text-muted)' }}>
          {task.head_id}
        </span>
      </div>
      <div className="activity-item-text">{task.goal}</div>
    </div>
  );
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp);
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function ConclaveItem({ conclave, onClick }: { conclave: Conclave; onClick: () => void }) {
  const shortId = conclave.id.replace('conclave:', '');
  return (
    <div className="activity-item clickable" onClick={onClick}>
      <div className="activity-item-header">
        <span className={`activity-item-status ${conclave.status}`}>
          {conclave.status}
        </span>
        <span className="activity-item-time">
          {formatTime(conclave.created_at)}
        </span>
      </div>
      <div className="activity-item-text">{shortId}</div>
    </div>
  );
}

export function ActivityPanel() {
  // All data now comes from the bus via store - no polling needed
  const needs = useAppStore((s) => s.needs);
  const wants = useAppStore((s) => s.wants);
  const tasks = useAppStore((s) => s.tasks);
  const conclaves = useAppStore((s) => s.conclaves);
  const openConclave = useAppStore((s) => s.openConclave);
  const collapsedSections = useAppStore((s) => s.collapsedSections);
  const toggleSection = useAppStore((s) => s.toggleSection);

  return (
    <div className="panel activity-panel">
      <div className="panel-header">
        <span className="panel-header-title">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
            <path d="M8 2C4.68629 2 2 4.68629 2 8C2 11.3137 4.68629 14 8 14C11.3137 14 14 11.3137 14 8C14 4.68629 11.3137 2 8 2ZM8 4V8L11 10"/>
          </svg>
          Activity
        </span>
      </div>
      
      <div className="panel-content">
        {/* Conclaves */}
        <div className="activity-section">
          <SectionHeader
            title="Conclaves"
            count={conclaves.length}
            icon={
              <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style={{ marginRight: 4 }}>
                <path d="M5 3a2 2 0 100 4 2 2 0 000-4zM11 3a2 2 0 100 4 2 2 0 000-4zM8 9a2 2 0 100 4 2 2 0 000-4z"/>
              </svg>
            }
            collapsed={collapsedSections.has('conclaves')}
            onToggle={() => toggleSection('conclaves')}
          />
          {!collapsedSections.has('conclaves') && (
            <div className="activity-section-content">
              {conclaves.length === 0 ? (
                <div className="empty-state">No conclaves yet</div>
              ) : (
                conclaves.map((conclave) => (
                  <ConclaveItem
                    key={conclave.id}
                    conclave={conclave}
                    onClick={() => openConclave(conclave.id)}
                  />
                ))
              )}
            </div>
          )}
        </div>

        {/* Needs Queue */}
        <div className="activity-section">
          <SectionHeader
            title="Needs"
            count={needs.length}
            icon={
              <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style={{ marginRight: 4 }}>
                <path d="M8 1L10.5 6H14L11 9.5L12.5 15L8 11.5L3.5 15L5 9.5L2 6H5.5L8 1Z"/>
              </svg>
            }
            collapsed={collapsedSections.has('needs')}
            onToggle={() => toggleSection('needs')}
          />
          {!collapsedSections.has('needs') && (
            <div className="activity-section-content">
              {needs.length === 0 ? (
                <div className="empty-state">No pending needs</div>
              ) : (
                needs.map((need) => (
                  <NeedItem key={need.id} need={need} />
                ))
              )}
            </div>
          )}
        </div>

        {/* Tasks Queue */}
        <div className="activity-section">
          <SectionHeader
            title="Tasks"
            count={tasks.length}
            icon={
              <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style={{ marginRight: 4 }}>
                <path d="M8 2L2 8L8 14L14 8L8 2ZM8 4L12 8L8 12L4 8L8 4Z"/>
              </svg>
            }
            collapsed={collapsedSections.has('tasks')}
            onToggle={() => toggleSection('tasks')}
          />
          {!collapsedSections.has('tasks') && (
            <div className="activity-section-content">
              {tasks.length === 0 ? (
                <div className="empty-state">No active tasks</div>
              ) : (
                tasks.map((task) => (
                  <TaskItem key={task.id} task={task} />
                ))
              )}
            </div>
          )}
        </div>

        {/* Wants Pool */}
        <div className="activity-section">
          <SectionHeader
            title="Wants"
            count={wants.length}
            icon={
              <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style={{ marginRight: 4 }}>
                <path d="M8 14C4.68629 14 2 11.3137 2 8C2 4.68629 4.68629 2 8 2C11.3137 2 14 4.68629 14 8C14 11.3137 11.3137 14 8 14ZM8 12C10.2091 12 12 10.2091 12 8C12 5.79086 10.2091 4 8 4C5.79086 4 4 5.79086 4 8C4 10.2091 5.79086 12 8 12ZM8 10C6.89543 10 6 9.10457 6 8C6 6.89543 6.89543 6 8 6C9.10457 6 10 6.89543 10 8C10 9.10457 9.10457 10 8 10Z"/>
              </svg>
            }
            collapsed={collapsedSections.has('wants')}
            onToggle={() => toggleSection('wants')}
          />
          {!collapsedSections.has('wants') && (
            <div className="activity-section-content">
              {wants.length === 0 ? (
                <div className="empty-state">No wants in pool</div>
              ) : (
                wants.map((want) => (
                  <WantItem key={want.id} want={want} />
                ))
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
