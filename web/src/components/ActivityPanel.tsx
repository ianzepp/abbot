import { useEffect } from 'react';
import { useAppStore } from '../store';
import { getNeeds, getWants, getGoals, getStatus } from '../api';
import type { Need, Want, Goal, HeadInfo, HandInfo, HeadState, HandState } from '../types';

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

function GoalItem({ goal }: { goal: Goal }) {
  return (
    <div className="activity-item">
      <div className="activity-item-header">
        <span className="activity-item-id">{goal.id.slice(0, 8)}</span>
        <span style={{ fontSize: 10, color: 'var(--text-muted)' }}>
          {goal.head_id}
        </span>
      </div>
      <div className="activity-item-text">{goal.goal}</div>
    </div>
  );
}

function isHeadProcessing(state: HeadState): state is { state: 'processing'; need_id: string } {
  return state.state === 'processing';
}

function isHandRunning(state: HandState): state is { state: 'running'; task_id: string; goal_id: string; head_id: string } {
  return state.state === 'running';
}

function HeadStatusRow({ head }: { head: HeadInfo }) {
  const isActive = head.state.state === 'processing';
  const needId = isActive ? (head.state as { state: 'processing'; need_id: string }).need_id : null;
  return (
    <div className="agent-status-row">
      <span className={`agent-status-indicator ${isActive ? 'active' : 'idle'}`} />
      <span className="agent-status-name">{head.head_id}</span>
      {needId && (
        <span className="agent-status-task">
          {needId.slice(0, 8)}
        </span>
      )}
    </div>
  );
}

function HandStatusRow({ hand }: { hand: HandInfo }) {
  const isActive = hand.state.state === 'running';
  const taskId = isActive ? (hand.state as { state: 'running'; task_id: string }).task_id : null;
  return (
    <div className="agent-status-row">
      <span className={`agent-status-indicator ${isActive ? 'active' : 'idle'}`} />
      <span className="agent-status-name">{hand.hand_id}</span>
      {taskId && (
        <span className="agent-status-task">
          {taskId.slice(0, 8)}
        </span>
      )}
    </div>
  );
}

export function ActivityPanel() {
  const needs = useAppStore((s) => s.needs);
  const wants = useAppStore((s) => s.wants);
  const goals = useAppStore((s) => s.goals);
  const heads = useAppStore((s) => s.heads);
  const hands = useAppStore((s) => s.hands);
  const setNeeds = useAppStore((s) => s.setNeeds);
  const setWants = useAppStore((s) => s.setWants);
  const setGoals = useAppStore((s) => s.setGoals);
  const setHeads = useAppStore((s) => s.setHeads);
  const setHands = useAppStore((s) => s.setHands);
  const collapsedSections = useAppStore((s) => s.collapsedSections);
  const toggleSection = useAppStore((s) => s.toggleSection);

  useEffect(() => {
    const fetchData = async () => {
      try {
        const [needsData, wantsData, goalsData, statusData] = await Promise.all([
          getNeeds(),
          getWants(20),
          getGoals(),
          getStatus(),
        ]);
        setNeeds(needsData);
        setWants(wantsData);
        setGoals(goalsData);
        setHeads(statusData.heads);
        setHands(statusData.hands);
      } catch (err) {
        console.error('Failed to load activity data:', err);
      }
    };

    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, [setNeeds, setWants, setGoals, setHeads, setHands]);

  const activeHeads = heads.filter((h) => isHeadProcessing(h.state)).length;
  const activeHands = hands.filter((h) => isHandRunning(h.state)).length;

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
        {/* Agent Status */}
        <div className="activity-section">
          <SectionHeader
            title="Agents"
            count={activeHeads + activeHands}
            icon={
              <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style={{ marginRight: 4 }}>
                <path d="M8 8C9.65685 8 11 6.65685 11 5C11 3.34315 9.65685 2 8 2C6.34315 2 5 3.34315 5 5C5 6.65685 6.34315 8 8 8ZM8 9C5.23858 9 3 11.2386 3 14H13C13 11.2386 10.7614 9 8 9Z"/>
              </svg>
            }
            collapsed={collapsedSections.has('agents')}
            onToggle={() => toggleSection('agents')}
          />
          {!collapsedSections.has('agents') && (
            <div className="activity-section-content agent-status">
              {heads.length === 0 && hands.length === 0 ? (
                <div className="empty-state">No agents</div>
              ) : (
                <>
                  {heads.map((head) => (
                    <HeadStatusRow key={head.head_id} head={head} />
                  ))}
                  {hands.map((hand) => (
                    <HandStatusRow key={hand.hand_id} hand={hand} />
                  ))}
                </>
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

        {/* Goals Queue */}
        <div className="activity-section">
          <SectionHeader
            title="Goals"
            count={goals.length}
            icon={
              <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" style={{ marginRight: 4 }}>
                <path d="M8 2L2 8L8 14L14 8L8 2ZM8 4L12 8L8 12L4 8L8 4Z"/>
              </svg>
            }
            collapsed={collapsedSections.has('goals')}
            onToggle={() => toggleSection('goals')}
          />
          {!collapsedSections.has('goals') && (
            <div className="activity-section-content">
              {goals.length === 0 ? (
                <div className="empty-state">No active goals</div>
              ) : (
                goals.map((goal) => (
                  <GoalItem key={goal.id} goal={goal} />
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
