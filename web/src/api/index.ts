import type { Message, FileEntry, Need, Want, Goal, SystemStatus } from '../types';

const API_BASE = '/api';

async function fetchJson<T>(path: string): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`);
  if (!res.ok) {
    throw new Error(`API error: ${res.status} ${res.statusText}`);
  }
  return res.json();
}

async function postJson<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    throw new Error(`API error: ${res.status} ${res.statusText}`);
  }
  return res.json();
}

export async function getFiles(path: string = ''): Promise<FileEntry[]> {
  return fetchJson(`/files?path=${encodeURIComponent(path)}`);
}

export async function getFileContent(path: string): Promise<string> {
  return fetchJson(`/file?path=${encodeURIComponent(path)}`);
}

export async function getMessages(scope: string = 'main', limit: number = 100): Promise<Message[]> {
  return fetchJson(`/messages?scope=${encodeURIComponent(scope)}&limit=${limit}`);
}

export async function getNeeds(): Promise<Need[]> {
  return fetchJson('/needs');
}

export async function getWants(limit: number = 20): Promise<Want[]> {
  return fetchJson(`/wants?limit=${limit}`);
}

export async function getGoals(): Promise<Goal[]> {
  return fetchJson('/goals');
}

export async function getStatus(): Promise<SystemStatus> {
  return fetchJson('/status');
}

export async function sendMessage(content: string, scope: string = 'main'): Promise<void> {
  await postJson('/send', { content, scope });
}

export function createWebSocket(): WebSocket {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const wsUrl = `${protocol}//${window.location.host}/ws`;
  return new WebSocket(wsUrl);
}
