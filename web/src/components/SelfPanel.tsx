import { getSelf } from '../api';
import { MemoryPanel } from './MemoryPanel';

export function SelfPanel() {
  return (
    <MemoryPanel
      title="Self"
      description="Collective identity of the conclave - values, principles, and character."
      fetchContent={getSelf}
    />
  );
}
