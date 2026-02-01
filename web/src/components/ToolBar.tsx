import { useAppStore } from '../store';

export function ToolBar() {
  const activity = useAppStore((s) => s.toolActivity);

  return (
    <div className="tool-bar" aria-live="polite" aria-atomic="true">
      <div className="tool-bar-inner">
        <span className={activity ? 'tool-bar-indicator active' : 'tool-bar-indicator'} />
        <span className="tool-bar-text">
          {activity ? activity.text : ''}
        </span>
      </div>
    </div>
  );
}
