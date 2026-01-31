import { useEffect, useRef } from 'react';
import { useAppStore } from '../store';

export function BusPanel() {
  const rawBusMessages = useAppStore((s) => s.rawBusMessages);
  const clearRawBusMessages = useAppStore((s) => s.clearRawBusMessages);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [rawBusMessages]);

  return (
    <div className="bus-panel">
      <div className="bus-panel-header">
        <span className="bus-panel-count">{rawBusMessages.length} messages</span>
        <button className="bus-panel-clear" onClick={clearRawBusMessages}>
          Clear
        </button>
      </div>
      <div className="bus-panel-messages">
        {rawBusMessages.length === 0 ? (
          <div className="empty-state">No bus messages yet</div>
        ) : (
          rawBusMessages.map((msg, idx) => (
            <div key={idx} className="bus-message">
              {msg}
            </div>
          ))
        )}
        <div ref={messagesEndRef} />
      </div>
    </div>
  );
}
