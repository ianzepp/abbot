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

export interface MemoryContent {
  content: string;
}

export async function getSelf(): Promise<MemoryContent> {
  return fetchJson('/self');
}

export async function getLtm(): Promise<MemoryContent> {
  return fetchJson('/ltm');
}

export interface Conclave {
  id: string;
  status: string;
  created_at: number;
}

export interface ConclaveDetail {
  id: string;
  status: string;
  transcript: string;
  decision: string;
  created_at: number;
}

export async function getConclaves(limit: number = 50): Promise<Conclave[]> {
  return fetchJson(`/conclaves?limit=${limit}`);
}

export async function getConclave(id: string): Promise<ConclaveDetail> {
  return fetchJson(`/conclave?id=${encodeURIComponent(id)}`);
}

export interface StatusBarData {
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
  connected: boolean;
}

export async function getStatusBar(): Promise<StatusBarData> {
  return fetchJson('/statusbar');
}

export async function sendMessage(content: string, scope: string = 'main'): Promise<void> {
  await postJson('/send', { content, scope });
}

export function createWebSocket(): WebSocket {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const wsUrl = `${protocol}//${window.location.host}/ws`;
  return new WebSocket(wsUrl);
}
