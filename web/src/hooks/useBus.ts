// Bus hook - receives full bus feed from backend WebSocket and processes messages.
//
// Replaces polling with real-time updates. Messages are routed to appropriate
// store slices based on their op type and data.

import { useEffect, useRef, useCallback, useState } from 'react';
import { useAppStore } from '../store';
import type { Message, MessageOp, Need, Want, Goal, HeadInfo, HandInfo } from '../types';

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

// Bus processor - routes messages to store
function processBusMessage(
  msg: Message,
  actions: {
    addMessage: (m: Message) => void;
    updateNeed: (needId: string, update: Partial<Need> & { status?: string }) => void;
    removeNeed: (needId: string) => void;
    updateWant: (wantId: string, update: Partial<Want>) => void;
    updateGoal: (goalId: string, update: Partial<Goal>) => void;
    removeGoal: (goalId: string) => void;
    updateHead: (headId: string, update: Partial<HeadInfo>) => void;
    updateHand: (handId: string, update: Partial<HandInfo>) => void;
    updateStatusBar: (update: Partial<StatusBarUpdate>) => void;
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
      break;
  }
}

interface StatusBarUpdate {
  tick: number;
  needs_count: number;
  goals_count: number;
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
    updateGoal: (goalId: string, update: Partial<Goal>) => void;
    removeGoal: (goalId: string) => void;
    updateHand: (handId: string, update: Partial<HandInfo>) => void;
    addMessage: (m: Message) => void;
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
    actions.updateGoal(req.task_id, {
      id: req.task_id,
      head_id: req.head_id,
      goal: req.goal,
      notify_scope: req.notify_scope,
    });
  } else if ('Assigned' in taskData) {
    const asg = taskData.Assigned as { task_id: string; head_id: string; hand_id: string };
    actions.updateHand(asg.hand_id, {
      hand_id: asg.hand_id,
      state: { state: 'running', task_id: asg.task_id, goal_id: asg.task_id, head_id: asg.head_id },
    });
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
    // Remove from active goals
    actions.removeGoal(res.task_id);
    // Set hand back to idle
    actions.updateHand(res.hand_id, {
      hand_id: res.hand_id,
      state: { state: 'idle' },
    });
    // Add result message to chat if on main scope
    if (msg.scope === 'main') {
      actions.addMessage(msg);
    }
  }
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
    case 'conclave_started':
    case 'conclave_finished':
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

  // Store actions
  const setConnected = useAppStore((s) => s.setConnected);
  const addMessage = useAppStore((s) => s.addMessage);
  const updateNeed = useAppStore((s) => s.updateNeed);
  const removeNeed = useAppStore((s) => s.removeNeed);
  const updateWant = useAppStore((s) => s.updateWant);
  const updateGoal = useAppStore((s) => s.updateGoal);
  const removeGoal = useAppStore((s) => s.removeGoal);
  const updateHead = useAppStore((s) => s.updateHead);
  const updateHand = useAppStore((s) => s.updateHand);
  const updateStatusBar = useAppStore((s) => s.updateStatusBar);
  const addRawBusMessage = useAppStore((s) => s.addRawBusMessage);

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
      reconnectTimeoutRef.current = window.setTimeout(() => {
        connect();
      }, 2000);
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
              updateGoal,
              removeGoal,
              updateHead,
              updateHand,
              updateStatusBar,
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
    updateGoal,
    removeGoal,
    updateHead,
    updateHand,
    updateStatusBar,
  ]);

  useEffect(() => {
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
