import { useState, useEffect, useRef, type KeyboardEvent } from 'react';
import { useAppStore } from '../store';
import { getMessages, sendMessage } from '../api';
import type { Message, MessageData } from '../types';

function formatTime(timestamp: number): string {
  const date = new Date(timestamp);
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function extractText(data: MessageData): string {
  if ('type' in data) {
    switch (data.type) {
      case 'text':
        return data.content;
      case 'error':
        return `Error [${data.code}]: ${data.message}`;
      case 'progress':
        return `Progress: ${data.percent.toFixed(0)}% (${data.current}/${data.total})`;
      case 'event':
        return `Event: ${data.kind}`;
      default:
        return '';
    }
  }
  
  // Handle raw string data (simplified JSON from backend)
  if (typeof data === 'string') {
    return data;
  }
  
  // Handle object with Text variant
  if (data && typeof data === 'object' && 'Text' in data) {
    return (data as { Text: string }).Text;
  }
  
  return JSON.stringify(data);
}

interface ChatMessageProps {
  message: Message;
}

function ChatMessage({ message }: ChatMessageProps) {
  const content = extractText(message.data);
  if (!content) return null;

  const senderClass = message.origin === 'human' ? 'human' : 
                      message.origin === 'head' ? 'head' : 
                      message.origin === 'system' ? 'system' : '';

  return (
    <div className="chat-message">
      <div className="chat-message-header">
        <span className={`chat-message-sender ${senderClass}`}>
          {message.sender}
        </span>
        <span className="chat-message-time">
          {formatTime(message.timestamp)}
        </span>
      </div>
      <div className="chat-message-content">
        {content}
      </div>
    </div>
  );
}

export function ChatPanel() {
  const messages = useAppStore((s) => s.messages);
  const setMessages = useAppStore((s) => s.setMessages);
  const connected = useAppStore((s) => s.connected);
  
  const [input, setInput] = useState('');
  const [sending, setSending] = useState(false);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    getMessages('main', 100)
      .then(setMessages)
      .catch((err) => console.error('Failed to load messages:', err));
  }, [setMessages]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const handleSend = async () => {
    const trimmed = input.trim();
    if (!trimmed || sending) return;

    setSending(true);
    try {
      await sendMessage(trimmed, 'main');
      setInput('');
    } catch (err) {
      console.error('Failed to send message:', err);
    } finally {
      setSending(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  // Filter to show only Chat messages
  const chatMessages = messages.filter((m) => m.op === 'Chat');

  return (
    <div className="chat-panel-inner">
      {!connected && (
        <div className="chat-disconnected-banner">
          Disconnected from server
        </div>
      )}
      
      <div className="chat-messages">
        {chatMessages.length === 0 ? (
          <div className="empty-state">
            No messages yet
          </div>
        ) : (
          chatMessages.map((msg) => (
            <ChatMessage key={msg.id} message={msg} />
          ))
        )}
        <div ref={messagesEndRef} />
      </div>
      
      <div className="chat-input-container">
        <textarea
          ref={textareaRef}
          className="chat-input"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Type a message... (Enter to send, Shift+Enter for newline)"
          rows={3}
          disabled={sending}
        />
      </div>
    </div>
  );
}
