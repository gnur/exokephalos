import type { Bootstrap, Item, OutboxEntry, SyncClient } from './types';

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

export function runAction(actionName: string, itemID: string) {
  return api<Item>(`/api/app/actions/${encodeURIComponent(actionName)}`, {
    method: 'POST',
    body: JSON.stringify({ item_id: itemID }),
  });
}

export function importURL(url: string) {
  return api<{ id: string; frontmatter: Record<string, unknown>; body: string }>('/api/items', {
    method: 'POST',
    body: JSON.stringify({ url }),
  });
}
