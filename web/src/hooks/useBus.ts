// Bus hook - receives full bus feed from backend WebSocket and processes messages.
//
// Replaces polling with real-time updates. Messages are routed to appropriate
// store slices based on their op type and data.

import { useEffect, useRef, useCallback, useState } from 'react';
import { useAppStore } from '../store';
import type { Message, MessageOp, Need, Want, Task, HeadInfo, HandInfo } from '../types';

// WebSocket message types from backend
interface WsBusMessage {
  type: 'bus';
  data: Message;
}

interface WsConnectedMessage {
  type: 'connected';
  data: { version: string };
}

interface WsPongMessage {
  type: 'pong';
  data: { timestamp: number };
}

type WsMessage = WsBusMessage | WsConnectedMessage | WsPongMessage;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

// Bus processor - routes messages to store
function processBusMessage(
  msg: Message,
  actions: {
    addMessage: (m: Message) => void;
    updateNeed: (needId: string, update: Partial<Need> & { status?: string }) => void;
    removeNeed: (needId: string) => void;
    updateWant: (wantId: string, update: Partial<Want>) => void;
    removeWant: (wantId: string) => void;
    updateTask: (taskId: string, update: Partial<Task>) => void;
    removeTask: (taskId: string) => void;
    updateHead: (headId: string, update: Partial<HeadInfo>) => void;
    updateHand: (handId: string, update: Partial<HandInfo>) => void;
    updateStatusBar: (update: Partial<StatusBarUpdate>) => void;
    setToolActivity: (activity: { text: string; ts: number } | null) => void;
  }
) {
  const op = msg.op as MessageOp;

  switch (op) {
    case 'Chat':
      // Chat messages go to the message list (for main scope)
      if (msg.scope === 'main') {
        actions.addMessage(msg);
      }
      break;

    case 'Need':
      processNeedMessage(msg, actions);
      break;

    case 'Want':
      processWantMessage(msg, actions);
      break;

    case 'Task':
      processTaskMessage(msg, actions);
      break;

    case 'Ping':
      // System heartbeat - just a tick, no stats
      break;

    case 'Status':
      // Status update from StatService - update statusbar
      if (msg.data && typeof msg.data === 'object' && 'Status' in msg.data) {
        const stats = (msg.data as { Status: StatusBarUpdate }).Status;
        actions.updateStatusBar(stats);
      }
      break;

    case 'Progress':
      // Progress updates - could be task or general progress
      break;

    case 'Event':
      // Events - extensible event handling
      processEventMessage(msg, actions);
      break;

    case 'Sleep':
    case 'Wake':
    case 'Idle':
      // Head lifecycle - update head state
      processHeadLifecycle(msg, actions);
      break;

    case 'Ok':
    case 'Error':
    case 'Done':
    case 'Item':
    case 'Data':
      // Terminal/streaming ops - context-dependent
      if (op === 'Done') {
        actions.setToolActivity(null);
      }
      break;
  }
}

function processWantMessage(
  msg: Message,
  actions: {
    updateWant: (wantId: string, update: Partial<Want>) => void;
    removeWant: (wantId: string) => void;
  }
) {
  if (!isRecord(msg.data)) return;

  const wantData = 'Want' in msg.data ? (msg.data.Want as unknown) : null;

  if (!isRecord(wantData)) return;

  if ('Added' in wantData && isRecord(wantData.Added)) {
    const add = wantData.Added;
    if (
      typeof add.want_id !== 'string' ||
      typeof add.want !== 'string' ||
      typeof add.context !== 'string' ||
      typeof add.priority !== 'string' ||
      typeof add.source !== 'string'
    ) {
      return;
    }
    actions.updateWant(add.want_id, {
      id: add.want_id,
      want: add.want,
      context: add.context,
      priority: add.priority,
      source: add.source,
      created_at: msg.timestamp || Date.now(),
    });
  } else if ('Removed' in wantData && isRecord(wantData.Removed)) {
    const rem = wantData.Removed;
    if (typeof rem.want_id !== 'string') return;
    actions.removeWant(rem.want_id);
  } else if ('Promoted' in wantData && isRecord(wantData.Promoted)) {
    const pro = wantData.Promoted;
    if (typeof pro.want_id !== 'string' || typeof pro.to_priority !== 'string') return;
    // Priority change is informational; wants may also be removed separately when promoted.
    actions.updateWant(pro.want_id, { id: pro.want_id, priority: pro.to_priority });
  }
}

interface StatusBarUpdate {
  tick: number;
  needs_count: number;
  tasks_count: number;
  wants_count: number;
  hands_running: number;
  hands_total: number;
  heads_busy: number;
  heads_total: number;
  next_conclave_secs: number;
  self_bytes: number;
  ltm_bytes: number;
  conclaves_count: number;
}

function processNeedMessage(
  msg: Message,
  actions: {
    updateNeed: (needId: string, update: Partial<Need> & { status?: string }) => void;
    removeNeed: (needId: string) => void;
    addMessage: (m: Message) => void;
  }
) {
  if (!msg.data || typeof msg.data !== 'object') return;

  // Handle the Rust enum serialization format: { Need: { Request: {...} } }
  const needData = 'Need' in msg.data ? (msg.data as { Need: unknown }).Need : msg.data;
  if (!needData || typeof needData !== 'object') return;

  if ('Request' in needData) {
    const req = needData.Request as {
      need_id: string;
      source: string;
      priority: string;
      need: string;
      context: string;
    };
    actions.updateNeed(req.need_id, {
      id: req.need_id,
      source: req.source,
      priority: req.priority as Need['priority'],
      need: req.need,
      context: req.context,
      created_at: msg.timestamp,
      status: 'pending',
    });
  } else if ('Acknowledged' in needData) {
    const ack = needData.Acknowledged as { need_id: string; head_id: string };
    actions.updateNeed(ack.need_id, { status: 'acknowledged' });
  } else if ('Fulfilled' in needData) {
    const ful = needData.Fulfilled as { need_id: string; head_id: string; summary: string };
    actions.removeNeed(ful.need_id);
    // Optionally add a chat message about fulfillment
    if (msg.scope === 'main') {
      actions.addMessage(msg);
    }
  } else if ('Expired' in needData) {
    const exp = needData.Expired as { need_id: string; reason: string };
    actions.removeNeed(exp.need_id);
  }
}

function processTaskMessage(
  msg: Message,
  actions: {
    updateTask: (taskId: string, update: Partial<Task>) => void;
    removeTask: (taskId: string) => void;
    updateHand: (handId: string, update: Partial<HandInfo>) => void;
    addMessage: (m: Message) => void;
    setToolActivity: (activity: { text: string; ts: number } | null) => void;
  }
) {
  if (!msg.data || typeof msg.data !== 'object') return;

  // Handle the Rust enum serialization format: { Task: { Request: {...} } }
  const taskData = 'Task' in msg.data ? (msg.data as { Task: unknown }).Task : msg.data;
  if (!taskData || typeof taskData !== 'object') return;

  if ('Request' in taskData) {
    const req = taskData.Request as {
      task_id: string;
      head_id: string;
      goal: string;
      input: string;
      notify_scope?: string;
    };
    actions.updateTask(req.task_id, {
      id: req.task_id,
      head_id: req.head_id,
      goal: req.goal,
      notify_scope: req.notify_scope,
    });
  } else if ('Assigned' in taskData) {
    const asg = taskData.Assigned as { task_id: string; head_id: string; hand_id: string };
    actions.updateHand(asg.hand_id, {
      hand_id: asg.hand_id,
      state: { state: 'running', task_id: asg.task_id, head_id: asg.head_id },
    });
  } else if ('ToolCall' in taskData) {
    const call = taskData.ToolCall as {
      task_id: string;
      hand_id: string;
      call_id: string;
      tool: string;
      args: unknown;
    };

    const text = formatToolActivityCall(call.tool, call.args);
    actions.setToolActivity({ text, ts: msg.timestamp || Date.now() });
  } else if ('ToolDone' in taskData) {
    const done = taskData.ToolDone as {
      task_id: string;
      hand_id: string;
      call_id: string;
      tool: string;
      ok: boolean;
      duration_ms: number;
      error_code?: string | null;
    };

    const text = formatToolActivityDone(done.tool, done.ok, done.duration_ms, done.error_code);
    actions.setToolActivity({ text, ts: msg.timestamp || Date.now() });
  } else if ('Echo' in taskData) {
    // Task echo - tool output, could display in UI
    const echo = taskData.Echo as { task_id: string; hand_id: string; tool: string; content: string };
    // For now, just log it; could add to a task detail view
    console.debug(`Task ${echo.task_id} echo from ${echo.tool}`);
  } else if ('Progress' in taskData) {
    const prog = taskData.Progress as { task_id: string; hand_id: string; note: string };
    console.debug(`Task ${prog.task_id} progress: ${prog.note}`);
  } else if ('Result' in taskData) {
    const res = taskData.Result as { task_id: string; hand_id: string; ok: boolean; summary: string };
    // Remove from active tasks
    actions.removeTask(res.task_id);
    // Set hand back to idle (ignore synthetic/unassigned hand ids)
    if (typeof res.hand_id === 'string' && res.hand_id.startsWith('hand-')) {
      actions.updateHand(res.hand_id, {
        hand_id: res.hand_id,
        state: { state: 'idle' },
      });
    }
    // Add result message to chat if on main scope
    if (msg.scope === 'main') {
      actions.addMessage(msg);
    }
  }
}

function clip(s: string, max: number): string {
  if (s.length <= max) return s;
  return `${s.slice(0, Math.max(0, max - 1))}…`;
}

function formatToolArgs(args: unknown): string {
  if (!args || typeof args !== 'object') return '';
  const rec = args as Record<string, unknown>;

  const preferred: Array<[string, string]> = [
    ['path', 'path'],
    ['pattern', 'pattern'],
    ['query', 'query'],
    ['url', 'url'],
    ['command_preview', 'cmd'],
    ['offset', 'offset'],
    ['limit', 'limit'],
  ];

  const parts: string[] = [];
  for (const [key, label] of preferred) {
    if (!(key in rec)) continue;
    const v = rec[key];
    if (typeof v === 'string') {
      parts.push(`${label}=${clip(v, 64)}`);
    } else if (typeof v === 'number' || typeof v === 'boolean') {
      parts.push(`${label}=${String(v)}`);
    }
    if (parts.length >= 2) break;
  }

  if (parts.length > 0) return parts.join(' ');

  if (Array.isArray(rec.keys)) {
    const keys = rec.keys.filter((k) => typeof k === 'string') as string[];
    if (keys.length > 0) return `keys=${keys.slice(0, 4).join(',')}`;
  }

  return '';
}

function formatToolActivityCall(tool: string, args: unknown): string {
  const detail = formatToolArgs(args);
  return detail ? `${tool} ${detail}` : tool;
}

function formatToolActivityDone(tool: string, ok: boolean, durationMs: number, errorCode?: string | null): string {
  const dur = Number.isFinite(durationMs) ? `${Math.max(0, Math.round(durationMs))}ms` : '';
  if (ok) {
    return dur ? `${tool} done (${dur})` : `${tool} done`;
  }
  const code = errorCode ? ` ${errorCode}` : '';
  return dur ? `${tool} failed${code} (${dur})` : `${tool} failed${code}`;
}

function processEventMessage(
  msg: Message,
  actions: {
    updateStatusBar: (update: Partial<StatusBarUpdate>) => void;
    addMessage: (m: Message) => void;
  }
) {
  if (!msg.data || typeof msg.data !== 'object') return;

  const eventData = 'Event' in msg.data
    ? (msg.data as { Event: { kind: string; payload: unknown } }).Event
    : null;

  if (!eventData) return;

  // Handle specific event kinds
  switch (eventData.kind) {
    case 'status_update':
      // Statusbar metrics update
      if (eventData.payload && typeof eventData.payload === 'object') {
        actions.updateStatusBar(eventData.payload as Partial<StatusBarUpdate>);
      }
      break;
    case 'conclave_call':
    case 'conclave_done':
    case 'autonomy_call':
    case 'autonomy_done':
    case 'slow_idle':
    case 'deep_idle':
      // Conclave lifecycle - could trigger a refresh
      break;
    default:
      // Unknown event - log for debugging
      console.debug(`Unknown event kind: ${eventData.kind}`);
  }
}

function processHeadLifecycle(
  msg: Message,
  actions: {
    updateHead: (headId: string, update: Partial<HeadInfo>) => void;
  }
) {
  const headId = msg.sender;

  switch (msg.op) {
    case 'Sleep':
      actions.updateHead(headId, {
        head_id: headId,
        state: { state: 'available' },
      });
      break;
    case 'Wake':
      actions.updateHead(headId, {
        head_id: headId,
        state: { state: 'available' },
      });
      break;
    case 'Idle':
      // System idle - all heads available
      break;
  }
}

export function useBus() {
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimeoutRef = useRef<number | null>(null);
  const [serverVersion, setServerVersion] = useState<string | null>(null);
  const connectRef = useRef<(() => void) | null>(null);

  // Store actions
  const setConnected = useAppStore((s) => s.setConnected);
  const addMessage = useAppStore((s) => s.addMessage);
  const updateNeed = useAppStore((s) => s.updateNeed);
  const removeNeed = useAppStore((s) => s.removeNeed);
  const updateWant = useAppStore((s) => s.updateWant);
  const removeWant = useAppStore((s) => s.removeWant);
  const updateTask = useAppStore((s) => s.updateTask);
  const removeTask = useAppStore((s) => s.removeTask);
  const updateHead = useAppStore((s) => s.updateHead);
  const updateHand = useAppStore((s) => s.updateHand);
  const updateStatusBar = useAppStore((s) => s.updateStatusBar);
  const addRawBusMessage = useAppStore((s) => s.addRawBusMessage);
  const setToolActivity = useAppStore((s) => s.setToolActivity);

  const reconnect = useCallback(() => {
    connectRef.current?.();
  }, []);

  function scheduleReconnect(connectFn: () => void) {
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
    }
    reconnectTimeoutRef.current = window.setTimeout(() => {
      connectFn();
    }, 2000);
  }

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/ws`;

    const ws = new WebSocket(wsUrl);
    wsRef.current = ws;

    ws.onopen = () => {
      console.log('Bus connected');
      setConnected(true);
    };

    ws.onclose = () => {
      console.log('Bus disconnected');
      setConnected(false);
      setServerVersion(null);

      // Reconnect after delay
      scheduleReconnect(reconnect);
    };

    ws.onerror = (err) => {
      console.error('Bus error:', err);
    };

    ws.onmessage = (event) => {
      try {
        // Capture raw message for debugging
        addRawBusMessage(event.data);

        const wsMsg: WsMessage = JSON.parse(event.data);

        switch (wsMsg.type) {
          case 'connected':
            setServerVersion(wsMsg.data.version);
            console.log(`Connected to server v${wsMsg.data.version}`);
            break;

          case 'bus':
            processBusMessage(wsMsg.data, {
              addMessage,
              updateNeed,
              removeNeed,
              updateWant,
              removeWant,
              updateTask,
              removeTask,
              updateHead,
              updateHand,
              updateStatusBar,
              setToolActivity,
            });
            break;

          case 'pong':
            // Heartbeat response
            break;
        }
      } catch (err) {
        console.error('Failed to parse bus message:', err);
      }
    };
  }, [
    setConnected,
    addMessage,
    addRawBusMessage,
    updateNeed,
    removeNeed,
    updateWant,
    removeWant,
    updateTask,
    removeTask,
    updateHead,
    updateHand,
    updateStatusBar,
    setToolActivity,
    reconnect,
  ]);

  useEffect(() => {
    connectRef.current = connect;
    connect();

    return () => {
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
      }
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
  }, [connect]);

  // Send a ping to the server
  const ping = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({ type: 'ping' }));
    }
  }, []);

  return { ping, serverVersion };
}
