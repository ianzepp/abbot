// Types matching the Rust backend structures

export type Origin = 'head' | 'hand' | 'human' | 'system';

export type MessageOp = 
  | 'Ok' | 'Error' | 'Done' 
  | 'Item' | 'Data'
  | 'Event' | 'Progress'
  | 'Chat' | 'Ping' | 'Status' | 'Task' | 'Need' | 'Want' | 'Sleep' | 'Wake' | 'Idle';

export type NeedPriority = 'low' | 'normal' | 'high' | 'urgent';

export interface Message {
  id: string;
  op: MessageOp;
  origin: Origin;
  sender: string;
  scope: string;
  data: MessageData;
  reply_to?: string;
  timestamp: number;
}

export type MessageData = 
  | { type: 'text'; content: string }
  | { type: 'error'; code: string; message: string }
  | { type: 'progress'; percent: number; current: number; total: number }
  | { type: 'event'; kind: string; payload: unknown }
  | { type: 'task'; task: TaskMsg }
  | { type: 'need'; need: NeedMsg }
  | { type: 'empty' };

export interface TaskMsg {
  variant: 'request' | 'assigned' | 'echo' | 'progress' | 'result';
  task_id: string;
  head_id?: string;
  hand_id?: string;
  goal?: string;
  input?: string;
  tool?: string;
  content?: string;
  note?: string;
  ok?: boolean;
  summary?: string;
}

export interface NeedMsg {
  variant: 'request' | 'acknowledged' | 'fulfilled' | 'expired';
  need_id: string;
  source?: string;
  priority?: NeedPriority;
  need?: string;
  context?: string;
  head_id?: string;
  summary?: string;
  reason?: string;
}

export interface Need {
  id: string;
  source: string;
  priority: NeedPriority;
  need: string;
  context: string;
  created_at: number;
}

export interface Want {
  id: string;
  want: string;
  context: string;
  priority: string;
  source: string;
  created_at: number;
}

export interface Task {
  id: string;
  head_id: string;
  goal: string;
  notify_scope?: string;
}

export type HeadState = 
  | { state: 'available' }
  | { state: 'processing'; need_id: string };

export interface HeadInfo {
  head_id: string;
  state: HeadState;
}

export type HandState =
  | { state: 'idle' }
  | { state: 'running'; task_id: string; head_id: string };

export interface HandInfo {
  hand_id: string;
  state: HandState;
}

export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  children?: FileEntry[];
}

export interface Conclave {
  id: string;
  status: string;
  created_at: number;
}
