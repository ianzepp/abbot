import { useState, useEffect, useRef } from 'react';
import { useAppStore, type StatusBarData } from '../store';

interface SectionConfig {
  id: string;
  label: string;
  render: (data: StatusBarData) => string;
}

const SECTIONS: SectionConfig[] = [
  {
    id: 'tick',
    label: 'Tick',
    render: (d) => `Tick: ${d.tick}`,
  },
  {
    id: 'queues',
    label: 'Queues (N/T/W)',
    render: (d) => `N:${d.needs_count} T:${d.tasks_count} W:${d.wants_count}`,
  },
  {
    id: 'hands',
    label: 'Hands',
    render: (d) => `Hands: ${d.hands_running}/${d.hands_total}`,
  },
  {
    id: 'heads',
    label: 'Heads',
    render: (d) => `Heads: ${d.heads_busy}/${d.heads_total}`,
  },
  {
    id: 'conclave',
    label: 'Conclave',
    render: (d) => `Next: ${d.next_conclave_secs}s (${d.conclaves_count} total)`,
  },
  {
    id: 'memory',
    label: 'Memory (Self/LTM)',
    render: (d) => `Self: ${formatBytes(d.self_bytes)} | LTM: ${formatBytes(d.ltm_bytes)}`,
  },
];

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}K`;
  return `${(bytes / (1024 * 1024)).toFixed(1)}M`;
}

export function StatusBar() {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  // All data now comes from the bus via store - no polling needed
  const data = useAppStore((s) => s.statusBar);
  const visibleSections = useAppStore((s) => s.statusbarSections);
  const toggleSection = useAppStore((s) => s.toggleStatusbarSection);
  const connected = useAppStore((s) => s.connected);

  // Close menu when clicking outside
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (
        menuRef.current &&
        !menuRef.current.contains(e.target as Node) &&
        buttonRef.current &&
        !buttonRef.current.contains(e.target as Node)
      ) {
        setMenuOpen(false);
      }
    };

    if (menuOpen) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [menuOpen]);

  return (
    <div className="statusbar">
      <div className="statusbar-left">
        <button
          ref={buttonRef}
          className="statusbar-menu-button"
          onClick={() => setMenuOpen(!menuOpen)}
          title="Configure statusbar"
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
            <path d="M2 4h12v1.5H2V4zm0 4h12v1.5H2V8zm0 4h12v1.5H2V12z"/>
          </svg>
        </button>

        {menuOpen && (
          <div ref={menuRef} className="statusbar-menu">
            <div className="statusbar-menu-header">Statusbar Sections</div>
            {SECTIONS.map((section) => (
              <label key={section.id} className="statusbar-menu-item">
                <input
                  type="checkbox"
                  checked={visibleSections.has(section.id)}
                  onChange={() => toggleSection(section.id)}
                />
                <span>{section.label}</span>
              </label>
            ))}
          </div>
        )}
      </div>

      <div className="statusbar-sections">
        {SECTIONS.filter((s) => visibleSections.has(s.id)).map((section) => (
          <div key={section.id} className="statusbar-section">
            {section.render(data)}
          </div>
        ))}
      </div>

      <div className="statusbar-right">
        <div className={`statusbar-connection ${connected ? 'connected' : 'disconnected'}`}>
          <span className="statusbar-connection-dot" />
          {connected ? 'Connected' : 'Disconnected'}
        </div>
      </div>
    </div>
  );
}
