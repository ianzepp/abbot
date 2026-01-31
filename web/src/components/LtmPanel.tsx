import { getLtm } from '../api';
import { MemoryPanel } from './MemoryPanel';

export function LtmPanel() {
  return (
    <MemoryPanel
      title="Long-Term Memory"
      description="Strategic, persistent learnings managed by the conclave."
      fetchContent={getLtm}
    />
  );
}
