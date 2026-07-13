import { db, setRevision } from './db';
import { bootstrap, pushChanges } from './api';

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
    const pending = await db.outbox.where('status').anyOf('pending', 'failed').sortBy('created_at');
    if (pending.length === 0) return;
    const now = new Date().toISOString();
    await db.outbox.bulkPut(pending.map((entry) => ({ ...entry, status: 'syncing' as const, updated_at: now })));
    const result = await pushChanges(pending);
    const accepted = new Set(result.accepted ?? []);
    const rejected = new Map((result.rejected ?? []).map((entry) => [entry.id, entry.error]));
    await db.transaction('rw', db.outbox, db.meta, async () => {
      for (const entry of pending) {
        if (accepted.has(entry.id)) {
          await db.outbox.put({ ...entry, status: 'synced', error: undefined, updated_at: new Date().toISOString() });
        } else {
          await db.outbox.put({
            ...entry,
            status: 'failed',
            attempts: entry.attempts + 1,
            error: rejected.get(entry.id) ?? 'change was not accepted',
            updated_at: new Date().toISOString(),
          });
        }
      }
      await setRevision(result.revision);
    });
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
