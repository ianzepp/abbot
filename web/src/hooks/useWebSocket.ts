import { useEffect, useRef, useCallback } from 'react';
import { useAppStore } from '../store';
import type { Message } from '../types';

interface WsMessage {
  type: 'message' | 'needs' | 'wants' | 'goals' | 'status';
  data: unknown;
}

export function useWebSocket() {
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimeoutRef = useRef<number | null>(null);
  
  const setConnected = useAppStore((s) => s.setConnected);
  const addMessage = useAppStore((s) => s.addMessage);
  const setNeeds = useAppStore((s) => s.setNeeds);
  const setWants = useAppStore((s) => s.setWants);
  const setGoals = useAppStore((s) => s.setGoals);
  const setHeads = useAppStore((s) => s.setHeads);
  const setHands = useAppStore((s) => s.setHands);

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/ws`;
    
    const ws = new WebSocket(wsUrl);
    wsRef.current = ws;

    ws.onopen = () => {
      console.log('WebSocket connected');
      setConnected(true);
    };

    ws.onclose = () => {
      console.log('WebSocket disconnected');
      setConnected(false);
      
      // Reconnect after delay
      reconnectTimeoutRef.current = window.setTimeout(() => {
        connect();
      }, 2000);
    };

    ws.onerror = (err) => {
      console.error('WebSocket error:', err);
    };

    ws.onmessage = (event) => {
      try {
        const msg: WsMessage = JSON.parse(event.data);
        
        switch (msg.type) {
          case 'message':
            addMessage(msg.data as Message);
            break;
          case 'needs':
            setNeeds(msg.data as never[]);
            break;
          case 'wants':
            setWants(msg.data as never[]);
            break;
          case 'goals':
            setGoals(msg.data as never[]);
            break;
          case 'status':
            const status = msg.data as { heads: never[]; hands: never[] };
            setHeads(status.heads);
            setHands(status.hands);
            break;
        }
      } catch (err) {
        console.error('Failed to parse WebSocket message:', err);
      }
    };
  }, [setConnected, addMessage, setNeeds, setWants, setGoals, setHeads, setHands]);

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

  const send = useCallback((data: unknown) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(data));
    }
  }, []);

  return { send };
}
