import type { Action, APIKey, Bootstrap, ConfigFile, Item, OutboxEntry, SyncClient, SyncOperation } from './types';

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: {
      Accept: 'application/json',
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...init?.headers,
    },
  });
  if (res.status === 401) {
    window.location.assign(`/login?next=${encodeURIComponent(window.location.pathname + window.location.search)}`);
    throw new Error('authentication required');
  }
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try {
      const body = await res.json();
      if (body.error) message = body.error;
    } catch {
      // Keep HTTP status text.
    }
    throw new Error(message);
  }
	if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

export function bootstrap() {
  return api<Bootstrap>('/api/app/bootstrap');
}

export function pushChanges(changes: OutboxEntry[]) {
  return api<{ revision: number; accepted: string[]; rejected: Array<{ id: string; error: string }> }>('/api/app/changes', {
    method: 'POST',
    body: JSON.stringify({
      changes: changes.map((entry) => ({
        client_mutation_id: entry.id,
        op: entry.op,
        target_kind: 'item',
        id: entry.item_id,
        path: entry.path,
        frontmatter: entry.frontmatter,
        body: entry.body ?? '',
      })),
    }),
  });
}

// v2 is used by new offline clients. It is separate from the legacy app
// mutation route so a partially upgraded install cannot silently mix clocks.
export function pushSyncOperations(operations: SyncOperation[]) {
  return import('./db').then(async ({ ensureSyncDevice }) => {
    const device = await ensureSyncDevice();
    return api<{ results: Array<{ id: string; status: string; cursor?: number; error?: string }> }>('/api/app/sync/v2/push', {
      method: 'POST', body: JSON.stringify({ operations }), headers: { 'X-Exo-Device-ID': device.id },
    });
  });
}

export function syncV2Bootstrap() {
  return import('./db').then(async ({ ensureSyncDevice }) => {
    const device = await ensureSyncDevice();
    return api<{ epoch: string; cursor: number; operations: SyncOperation[] }>('/api/app/sync/v2/bootstrap', { headers: { 'X-Exo-Device-ID': device.id } });
  });
}

export function pullSyncOperations(cursor: number) {
  return import('./db').then(async ({ ensureSyncDevice }) => {
    const device = await ensureSyncDevice();
    return api<{ cursor: number; operations: SyncOperation[] }>(`/api/app/sync/v2/pull?cursor=${cursor}`, { headers: { 'X-Exo-Device-ID': device.id } });
  });
}

export function acknowledgeSyncCursor(cursor: number) {
  return import('./db').then(async ({ ensureSyncDevice }) => {
    const device = await ensureSyncDevice();
    await api<unknown>('/api/app/sync/v2/ack', { method: 'POST', headers: { 'X-Exo-Device-ID': device.id }, body: JSON.stringify({ cursor }) });
  });
}

export function listSyncV2Devices() { return api<{ devices: Array<{ id: string; label: string; kind: string; created_at: string; retired_at: string }> }>('/api/app/sync/v2/devices'); }
export function retireSyncV2Device(id: string) { return api<{ ok: true }>(`/api/app/sync/v2/devices/${encodeURIComponent(id)}/retire`, { method: 'POST' }); }
export function compactSyncV2Tombstones() { return api<{ compacted: number }>('/api/app/sync/v2/compact', { method: 'POST' }); }

export function listSyncClients() {
  return api<{ clients: SyncClient[] }>('/api/app/sync-clients');
}

export function approveSyncClient(id: string) {
  return api<{ ok: true }>(`/api/app/sync-clients/${encodeURIComponent(id)}/approve`, { method: 'POST' });
}

export function revokeSyncClient(id: string) {
  return api<{ ok: true }>(`/api/app/sync-clients/${encodeURIComponent(id)}/revoke`, { method: 'POST' });
}

export function changePassword(currentPassword: string, newPassword: string) {
  return api<{ ok: true }>('/api/app/password', {
    method: 'POST',
    body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }),
  });
}

export function listAPIKeys() {
  return api<{ keys: APIKey[] }>('/api/app/api-keys');
}

export function createAPIKey(appName: string, filter: string, expiresAt: string) {
  return api<{ key: string; record: APIKey }>('/api/app/api-keys', {
    method: 'POST',
    body: JSON.stringify({ app_name: appName, filter, expires_at: expiresAt }),
  });
}

export function revokeAPIKey(id: number) {
  return api<{ ok: true }>(`/api/app/api-keys/${encodeURIComponent(String(id))}/revoke`, { method: 'POST' });
}

export function runAction(actionName: string, itemID: string) {
  return api<Item>(`/api/app/actions/${encodeURIComponent(actionName)}`, {
    method: 'POST',
    body: JSON.stringify({ item_id: itemID }),
  });
}

export function listItemActions(itemID: string) {
  return api<{ actions: Action[] }>(`/api/app/items/${encodeURIComponent(itemID)}/actions`);
}

export function listConfigs() {
  return api<{ configs: ConfigFile[] }>('/api/app/configs');
}

export function updateConfig(path: string, content: string) {
  return api<{ ok: true }>(`/api/app/configs/${encodeURIComponent(path)}`, {
    method: 'PUT',
    body: JSON.stringify({ content }),
  });
}

export function importURL(url: string) {
  return api<{ id: string; frontmatter: Record<string, unknown>; body: string }>('/api/items', {
    method: 'POST',
    body: JSON.stringify({ url }),
  });
}

export async function uploadAsset(file: File) {
  const body = new FormData();
  body.append('image', file);
  const res = await fetch('/api/app/assets', { method: 'POST', body, headers: { Accept: 'application/json' } });
  if (!res.ok) throw new Error(await res.text());
  return res.json() as Promise<{ path: string; markdown: string }>;
}
