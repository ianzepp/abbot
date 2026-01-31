import { useState, useEffect } from 'react';
import { getConclave, type ConclaveDetail } from '../api';

interface RoomMessage {
  mind: string;
  content: string;
  round: number;
}

interface RoomDecision {
  needs: { need: string; priority: string; votes: string[] }[];
  wants: { want: string; priority: string; proposer: string }[];
  ltm_ops: { kind: string; content: string; pattern: string; proposer: string }[];
  self_ops: { kind: string; content: string; pattern: string; proposer: string }[];
}

function formatTranscript(transcript: RoomMessage[]): string {
  if (!transcript || transcript.length === 0) {
    return '*No transcript recorded*';
  }

  let currentRound = -1;
  const lines: string[] = [];

  for (const msg of transcript) {
    if (msg.round !== currentRound) {
      currentRound = msg.round;
      lines.push(`\n## Round ${currentRound + 1}\n`);
    }
    lines.push(`### ${msg.mind}\n`);
    lines.push(`${msg.content}\n`);
  }

  return lines.join('\n');
}

function formatDecision(decision: RoomDecision): string {
  const sections: string[] = [];

  if (decision.needs && decision.needs.length > 0) {
    sections.push('## Needs Created\n');
    for (const n of decision.needs) {
      sections.push(`- **[${n.priority}]** ${n.need}`);
      if (n.votes && n.votes.length > 0) {
        sections.push(`  - Votes: ${n.votes.join(', ')}`);
      }
    }
    sections.push('');
  }

  if (decision.wants && decision.wants.length > 0) {
    sections.push('## Wants Added\n');
    for (const w of decision.wants) {
      sections.push(`- **[${w.priority}]** ${w.want} (by ${w.proposer})`);
    }
    sections.push('');
  }

  if (decision.ltm_ops && decision.ltm_ops.length > 0) {
    sections.push('## LTM Updates\n');
    for (const op of decision.ltm_ops) {
      sections.push(`- **${op.kind}**: ${op.content || op.pattern} (by ${op.proposer})`);
    }
    sections.push('');
  }

  if (decision.self_ops && decision.self_ops.length > 0) {
    sections.push('## Self Updates\n');
    for (const op of decision.self_ops) {
      sections.push(`- **${op.kind}**: ${op.content || op.pattern} (by ${op.proposer})`);
    }
    sections.push('');
  }

  if (sections.length === 0) {
    return '*No decisions made*';
  }

  return sections.join('\n');
}

interface ConclavePanelProps {
  conclaveId: string;
}

export function ConclavePanel({ conclaveId }: ConclavePanelProps) {
  const [data, setData] = useState<ConclaveDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setLoading(true);
    setError(null);
    getConclave(conclaveId)
      .then(setData)
      .catch((err) => setError(err.message))
      .finally(() => setLoading(false));
  }, [conclaveId]);

  if (loading) {
    return <div className="conclave-panel loading">Loading...</div>;
  }

  if (error) {
    return <div className="conclave-panel error">{error}</div>;
  }

  if (!data) {
    return <div className="conclave-panel error">Conclave not found</div>;
  }

  let transcript: RoomMessage[] = [];
  let decision: RoomDecision = { needs: [], wants: [], ltm_ops: [], self_ops: [] };

  try {
    transcript = JSON.parse(data.transcript);
  } catch {
    // ignore parse error
  }

  try {
    decision = JSON.parse(data.decision);
  } catch {
    // ignore parse error
  }

  const date = new Date(data.created_at);
  const formattedDate = date.toLocaleString();

  const markdown = `# Conclave: ${data.id}

**Status:** ${data.status}
**Time:** ${formattedDate}

---

# Transcript

${formatTranscript(transcript)}

---

# Decisions

${formatDecision(decision)}
`;

  return (
    <div className="conclave-panel">
      <pre className="conclave-panel-content">{markdown}</pre>
    </div>
  );
}
