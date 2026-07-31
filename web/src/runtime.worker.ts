/// <reference lib="webworker" />

import init, { run_steel, runtime_info } from './generated/xo-web/xo_web.js';
import type { RuntimeReport, WorkerRequest, WorkerResponse } from './protocol';

const scope = self as DedicatedWorkerGlobalScope;
const DATABASE = 'xo-web';
const DATABASE_VERSION = 1;
const CHECKPOINT_STORE = 'runtime-checkpoints';

let wasmReady: Promise<void> | undefined;

function initializeWasm() {
  wasmReady ??= init().then(() => undefined);
  return wasmReady;
}

function openDatabase() {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DATABASE, DATABASE_VERSION);
    request.onerror = () => reject(request.error ?? new Error('IndexedDB open failed'));
    request.onupgradeneeded = () => {
      const database = request.result;
      if (!database.objectStoreNames.contains(CHECKPOINT_STORE)) {
        database.createObjectStore(CHECKPOINT_STORE, { keyPath: 'id' });
      }
    };
    request.onsuccess = () => resolve(request.result);
  });
}

async function initializePersistence() {
  const database = await openDatabase();
  try {
    const restoredAt = await new Promise<string | undefined>((resolve, reject) => {
      const transaction = database.transaction(CHECKPOINT_STORE, 'readwrite');
      const store = transaction.objectStore(CHECKPOINT_STORE);
      const read = store.get('runtime');
      let previous: string | undefined;
      read.onsuccess = () => {
        previous = (read.result as { updatedAt?: string } | undefined)?.updatedAt;
        store.put({ id: 'runtime', schema: 1, updatedAt: new Date().toISOString() });
      };
      read.onerror = () => reject(read.error ?? new Error('IndexedDB read failed'));
      transaction.oncomplete = () => resolve(previous);
      transaction.onerror = () => reject(transaction.error ?? new Error('IndexedDB write failed'));
      transaction.onabort = () => reject(transaction.error ?? new Error('IndexedDB transaction aborted'));
    });
    return restoredAt;
  } finally {
    database.close();
  }
}

async function handle(request: WorkerRequest): Promise<unknown> {
  await initializeWasm();
  switch (request.method) {
    case 'initialize': {
      const restoredAt = await initializePersistence();
      const report: RuntimeReport = {
        runtime: JSON.parse(runtime_info()) as RuntimeReport['runtime'],
        indexedDb: true,
        steelResult: run_steel('(+ 20 22)'),
        restoredAt,
      };
      return report;
    }
    case 'steel-probe': {
      if (typeof request.payload !== 'string') throw new Error('Steel source must be a string');
      return run_steel(request.payload);
    }
  }
}

scope.addEventListener('message', (event: MessageEvent<WorkerRequest>) => {
  const request = event.data;
  void handle(request).then(
    (result) => {
      const response: WorkerResponse = { id: request.id, ok: true, result };
      scope.postMessage(response);
    },
    (cause: unknown) => {
      const response: WorkerResponse = {
        id: request.id,
        ok: false,
        error: cause instanceof Error ? cause.message : String(cause),
      };
      scope.postMessage(response);
    },
  );
});
