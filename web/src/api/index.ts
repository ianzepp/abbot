import type { Message, FileEntry } from '../types';

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

export interface MemoryContent {
  content: string;
}

export async function getSelf(): Promise<MemoryContent> {
  return fetchJson('/self');
}

export async function getLtm(): Promise<MemoryContent> {
  return fetchJson('/ltm');
}

export interface ConclaveDetail {
  id: string;
  status: string;
  transcript: string;
  decision: string;
  created_at: number;
}

export async function getConclave(id: string): Promise<ConclaveDetail> {
  return fetchJson(`/conclave?id=${encodeURIComponent(id)}`);
}

export async function sendMessage(content: string, scope: string = 'main'): Promise<void> {
  await postJson('/send', { content, scope });
}
