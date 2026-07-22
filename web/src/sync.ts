import { db, ensureSyncDevice, nextSyncVersion, observeSyncVersion, setRevision } from './db';
import { acknowledgeSyncCursor, bootstrap, pullSyncOperations, pushSyncOperations, syncV2Bootstrap } from './api';
import type { Item, SyncOperation } from './types';

let syncing = false;

export async function refreshFromServer() {
  const data = await bootstrap();
  await db.transaction('rw', db.items, db.views, db.actions, db.meta, async () => {
    await db.items.bulkPut(data.items);
    await db.views.clear();
    await db.views.bulkPut(data.views);
    await db.actions.clear();
    await db.actions.bulkPut(data.actions);
    await db.meta.put({ key: 'default_view', value: data.default_view });
    await db.meta.put({ key: 'sync_server_enabled', value: data.sync_server_enabled });
    await setRevision(data.revision);
  });
}

export async function syncOutbox() {
  if (syncing || !navigator.onLine) return;
  syncing = true;
  try {
    const boot = await syncV2Bootstrap();
    const savedEpoch = (await db.meta.get('sync_v2_epoch'))?.value;
    let cursor = Number((await db.meta.get('sync_v2_cursor'))?.value ?? 0);
    if (savedEpoch !== boot.epoch) {
      // Browser migration is server-authoritative, matching the TUI cutover.
      const legacy = await db.outbox.where('status').anyOf('pending', 'failed').toArray();
      await db.outbox.bulkPut(legacy.map((entry) => ({ ...entry, status: 'synced' as const, error: 'retired by sync v2 upgrade', updated_at: new Date().toISOString() })));
      cursor = 0;
      await db.meta.put({ key: 'sync_v2_epoch', value: boot.epoch });
    }
    for (;;) {
      const page = await pullSyncOperations(cursor);
      await applyOperations(page.operations);
      if (page.cursor <= cursor) break;
      cursor = page.cursor;
      await db.meta.put({ key: 'sync_v2_cursor', value: cursor });
      if (page.operations.length < 500) break;
    }
    const device = await ensureSyncDevice();
    const legacy = await db.outbox.where('status').anyOf('pending', 'failed').sortBy('created_at');
    for (const entry of legacy) {
      const version = await nextSyncVersion();
      const operation: SyncOperation = { id: entry.id, epoch: boot.epoch, actor_id: device.id, kind: 'item', target: entry.item_id, delete: entry.op === 'delete_item', path: entry.path, version: { ...version, actor_id: device.id }, frontmatter: entry.frontmatter, body: entry.body };
      await db.syncOps.put({ id: operation.id, source_id: entry.id, operation, status: 'pending', attempts: 0, created_at: entry.created_at, updated_at: new Date().toISOString() });
      await db.outbox.put({ ...entry, status: 'synced', error: undefined, updated_at: new Date().toISOString() });
    }
    const pending = await db.syncOps.where('status').anyOf('pending', 'failed').sortBy('created_at');
    if (pending.length) {
      const result = await pushSyncOperations(pending.map((entry) => entry.operation));
      const byID = new Map(result.results.map((entry) => [entry.id, entry]));
      for (const entry of pending) {
        const outcome = byID.get(entry.id);
        if (outcome?.status === 'applied' || outcome?.status === 'superseded') {
          await db.syncOps.put({ ...entry, status: 'synced', error: undefined, updated_at: new Date().toISOString() });
          cursor = Math.max(cursor, outcome.cursor ?? 0);
        } else await db.syncOps.put({ ...entry, status: 'failed', attempts: entry.attempts + 1, error: outcome?.error ?? 'operation was not accepted', updated_at: new Date().toISOString() });
      }
      await db.meta.put({ key: 'sync_v2_cursor', value: cursor });
    }
    await acknowledgeSyncCursor(cursor);
    await refreshFromServer();
  } catch (error) {
    const syncingEntries = await db.outbox.where('status').equals('syncing').toArray();
    const message = error instanceof Error ? error.message : String(error);
    await db.outbox.bulkPut(
      syncingEntries.map((entry) => ({
        ...entry,
        status: 'failed' as const,
        attempts: entry.attempts + 1,
        error: message,
        updated_at: new Date().toISOString(),
      })),
    );
  } finally {
    syncing = false;
  }
}

async function applyOperations(operations: SyncOperation[]) {
  await db.transaction('rw', db.items, db.meta, async () => {
    for (const op of operations) {
      await observeSyncVersion(op.version.physical_ms, op.version.logical);
      if (op.kind !== 'item') continue;
      if (op.delete) { await db.items.delete(op.target); continue; }
      const fm = op.frontmatter ?? {};
      const item: Item = { id: op.target, path: op.path ?? '', type: String(fm.type ?? ''), title: String(fm.title ?? op.target), subtitle: '', tags: Array.isArray(fm.tags) ? fm.tags.map(String) : [], frontmatter: fm, body: op.body ?? '', raw: '' };
      await db.items.put(item);
    }
  });
}

export function startSyncRuntime(onStatus: (status: 'online' | 'offline' | 'syncing') => void) {
  let events: EventSource | undefined;
  let stopped = false;
  let refreshInFlight = false;

  const syncOnce = async () => {
    if (!navigator.onLine || refreshInFlight) {
      onStatus(navigator.onLine ? 'syncing' : 'offline');
      return;
    }
    refreshInFlight = true;
    onStatus('syncing');
    try {
      await refreshFromServer();
      await syncOutbox();
      onStatus('online');
    } catch {
      onStatus('offline');
    } finally {
      refreshInFlight = false;
    }
  };

  const reconnectEvents = () => {
    if (stopped || !navigator.onLine) return;
    events?.close();
    events = new EventSource('/api/events');
    events.onopen = () => {
      onStatus('online');
      void syncOnce();
    };
    events.onerror = () => onStatus(navigator.onLine ? 'offline' : 'offline');
    events.addEventListener('change', (event) => {
      let detail: { target_kind?: string } = {};
      try {
        detail = JSON.parse((event as MessageEvent).data);
      } catch {
        // Keep the event useful even if a future server sends non-JSON data.
      }
      window.dispatchEvent(new CustomEvent('exo:server-change', { detail }));
      if (detail.target_kind !== 'client') {
        void refreshFromServer().catch(() => undefined);
      }
    });
  };

  const onOnline = () => {
    reconnectEvents();
    void syncOnce();
  };
  const onOffline = () => onStatus('offline');

  window.addEventListener('online', onOnline);
  window.addEventListener('offline', onOffline);
  reconnectEvents();
  void syncOnce();

  return () => {
    stopped = true;
    events?.close();
    window.removeEventListener('online', onOnline);
    window.removeEventListener('offline', onOffline);
  };
}
