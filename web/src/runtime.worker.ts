/// <reference lib="webworker" />

import init, {
  IrohDocNode,
  run_steel,
  runtime_info,
} from './generated/xo-web/xo_web.js';
import type {
  DocumentEntry,
  PutEntryInput,
  RuntimeInfo,
  RuntimeReport,
  SyncStatus,
  WorkerRequest,
  WorkerResponse,
  WorkspaceOutcome,
} from './protocol';

const scope = self as DedicatedWorkerGlobalScope;
const DATABASE = 'xo-web';
const DATABASE_VERSION = 2;
const CHECKPOINT_STORE = 'runtime-checkpoints';
const VAULT_STORE = 'vault';
const ENTRY_STORE = 'document-entries';
const PENDING_STORE = 'pending-writes';
const VAULT_KEY_ID = 'browser-key';
const VAULT_STATE_ID = 'identity';

interface BrowserIdentity {
  endpointSecret: string;
  authorSecret: string;
  ticket?: string;
}

interface PendingWrite extends PutEntryInput {
  id: string;
  createdAt: string;
}

interface VaultStateRecord {
  id: string;
  iv: ArrayBuffer;
  ciphertext: ArrayBuffer;
}

let wasmReady: Promise<void> | undefined;
let database: IDBDatabase | undefined;
let node: IrohDocNode | undefined;
let identity: BrowserIdentity | undefined;
let restoredAt: string | undefined;
let lastSyncError: string | undefined;

function initializeWasm() {
  wasmReady ??= init().then(() => undefined);
  return wasmReady;
}

function openDatabase() {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DATABASE, DATABASE_VERSION);
    request.onerror = () => reject(request.error ?? new Error('IndexedDB open failed'));
    request.onupgradeneeded = () => {
      const db = request.result;
      for (const store of [CHECKPOINT_STORE, VAULT_STORE, ENTRY_STORE, PENDING_STORE]) {
        if (!db.objectStoreNames.contains(store)) db.createObjectStore(store, { keyPath: 'id' });
      }
    };
    request.onsuccess = () => resolve(request.result);
  });
}

function transactionComplete(transaction: IDBTransaction) {
  return new Promise<void>((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error ?? new Error('IndexedDB transaction failed'));
    transaction.onabort = () => reject(transaction.error ?? new Error('IndexedDB transaction aborted'));
  });
}

function requestValue<T>(request: IDBRequest<T>) {
  return new Promise<T>((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error('IndexedDB request failed'));
  });
}

async function getRecord<T>(storeName: string, key: IDBValidKey) {
  const tx = requireDatabase().transaction(storeName, 'readonly');
  return requestValue(tx.objectStore(storeName).get(key)) as Promise<T | undefined>;
}

async function putRecord(storeName: string, value: unknown) {
  const tx = requireDatabase().transaction(storeName, 'readwrite');
  tx.objectStore(storeName).put(value);
  await transactionComplete(tx);
}

async function deleteRecord(storeName: string, key: IDBValidKey) {
  const tx = requireDatabase().transaction(storeName, 'readwrite');
  tx.objectStore(storeName).delete(key);
  await transactionComplete(tx);
}

async function allRecords<T>(storeName: string) {
  const tx = requireDatabase().transaction(storeName, 'readonly');
  return requestValue(tx.objectStore(storeName).getAll()) as Promise<T[]>;
}

function requireDatabase() {
  if (!database) throw new Error('IndexedDB is not initialized');
  return database;
}

function requireNode() {
  if (!node) throw new Error('Iroh is not initialized');
  return node;
}

async function initializePersistence() {
  database = await openDatabase();
  const checkpoint = await getRecord<{ id: string; updatedAt?: string }>(CHECKPOINT_STORE, 'runtime');
  restoredAt = checkpoint?.updatedAt;
  await putRecord(CHECKPOINT_STORE, { id: 'runtime', schema: DATABASE_VERSION, updatedAt: new Date().toISOString() });
  identity = await loadIdentity();
}

async function vaultKey() {
  const saved = await getRecord<{ id: string; key: CryptoKey }>(VAULT_STORE, VAULT_KEY_ID);
  if (saved?.key) return saved.key;
  const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
  await putRecord(VAULT_STORE, { id: VAULT_KEY_ID, key });
  return key;
}

async function loadIdentity(): Promise<BrowserIdentity> {
  const key = await vaultKey();
  const saved = await getRecord<VaultStateRecord>(VAULT_STORE, VAULT_STATE_ID);
  if (saved) {
    const plaintext = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: saved.iv }, key, saved.ciphertext);
    return JSON.parse(new TextDecoder().decode(plaintext)) as BrowserIdentity;
  }
  const created: BrowserIdentity = {
    endpointSecret: encodeBase64(crypto.getRandomValues(new Uint8Array(32))),
    authorSecret: encodeBase64(crypto.getRandomValues(new Uint8Array(32))),
  };
  await saveIdentity(created);
  return created;
}

async function saveIdentity(next: BrowserIdentity) {
  const key = await vaultKey();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = new TextEncoder().encode(JSON.stringify(next));
  const ciphertext = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, plaintext);
  await putRecord(VAULT_STORE, {
    id: VAULT_STATE_ID,
    iv: iv.buffer,
    ciphertext,
  } satisfies VaultStateRecord);
  identity = next;
}

async function initializeIroh() {
  if (!identity) throw new Error('browser identity is unavailable');
  node = await IrohDocNode.spawn(
    decodeBase64(identity.endpointSecret),
    decodeBase64(identity.authorSecret),
  );
  if (!identity.ticket) return;
  try {
    const outcome = JSON.parse(await node.joinWorkspace(identity.ticket)) as WorkspaceOutcome;
    lastSyncError = outcome.syncError;
    if (!outcome.syncError) await refreshEntryCache();
    await syncPendingWrites();
  } catch (cause) {
    lastSyncError = errorMessage(cause);
  }
}

async function createWorkspace() {
  const outcome = JSON.parse(await requireNode().createWorkspace()) as WorkspaceOutcome;
  await saveIdentity({ ...requireIdentity(), ticket: outcome.ticket });
  lastSyncError = undefined;
  return report();
}

async function joinWorkspace(ticket: string) {
  if (!ticket.trim()) throw new Error('A writable workspace ticket is required');
  const outcome = JSON.parse(await requireNode().joinWorkspace(ticket.trim())) as WorkspaceOutcome;
  await saveIdentity({ ...requireIdentity(), ticket: outcome.ticket });
  lastSyncError = outcome.syncError;
  if (!outcome.syncError) await refreshEntryCache();
  try {
    await syncPendingWrites();
  } catch (cause) {
    lastSyncError = errorMessage(cause);
  }
  return report();
}

function requireIdentity() {
  if (!identity) throw new Error('browser identity is unavailable');
  return identity;
}

async function enqueueWrite(input: PutEntryInput) {
  const key = input.key.trim();
  if (!key) throw new Error('Document key is required');
  const pending: PendingWrite = {
    id: crypto.randomUUID(),
    key,
    value: input.value,
    createdAt: new Date().toISOString(),
  };
  await putRecord(PENDING_STORE, pending);
  await putRecord(ENTRY_STORE, optimisticEntry(pending));
  try {
    await requireNode().putText(pending.key, pending.value);
    lastSyncError = undefined;
    await refreshEntryCache();
  } catch (cause) {
    lastSyncError = errorMessage(cause);
  }
  return report();
}

async function publishPendingWrites() {
  const pending = await allRecords<PendingWrite>(PENDING_STORE);
  for (const write of pending.sort((left, right) => left.createdAt.localeCompare(right.createdAt))) {
    try {
      await requireNode().putText(write.key, write.value);
    } catch (cause) {
      lastSyncError = errorMessage(cause);
      break;
    }
  }
}

async function confirmPendingWrites() {
  const pending = await allRecords<PendingWrite>(PENDING_STORE);
  for (const write of pending) await deleteRecord(PENDING_STORE, write.id);
}

async function syncPendingWrites() {
  const pending = await allRecords<PendingWrite>(PENDING_STORE);
  if (!pending.length) return;
  await publishPendingWrites();
  await requireNode().refreshSync();
  await confirmPendingWrites();
  await refreshEntryCache();
}

async function refreshSync() {
  try {
    await publishPendingWrites();
    await requireNode().refreshSync();
    await confirmPendingWrites();
    await refreshEntryCache();
    lastSyncError = undefined;
  } catch (cause) {
    lastSyncError = errorMessage(cause);
  }
  return report();
}

async function refreshEntryCache() {
  const entries = JSON.parse(await requireNode().entriesJson()) as DocumentEntry[];
  const tx = requireDatabase().transaction(ENTRY_STORE, 'readwrite');
  const store = tx.objectStore(ENTRY_STORE);
  store.clear();
  for (const entry of entries) store.put({ id: entry.keyBase64, ...entry });
  await transactionComplete(tx);
}

async function cachedEntries() {
  const entries = (await allRecords<DocumentEntry & { id: string }>(ENTRY_STORE))
    .map(({ id: _, ...entry }) => entry);
  const byKey = new Map(entries.map((entry) => [entry.key, entry]));
  for (const pending of await allRecords<PendingWrite>(PENDING_STORE)) {
    const { id: _, ...entry } = optimisticEntry(pending);
    byKey.set(entry.key, entry);
  }
  return [...byKey.values()].sort((left, right) => left.key.localeCompare(right.key));
}

async function report(): Promise<RuntimeReport> {
  const status = JSON.parse(await requireNode().statusJson()) as SyncStatus;
  return {
    runtime: JSON.parse(runtime_info()) as RuntimeInfo,
    indexedDb: true,
    steelResult: run_steel('(+ 20 22)'),
    restoredAt,
    status,
    entries: await cachedEntries(),
    ticket: identity?.ticket,
    syncError: lastSyncError,
    pendingWrites: (await allRecords<PendingWrite>(PENDING_STORE)).length,
  };
}

function optimisticEntry(write: PendingWrite): DocumentEntry & { id: string } {
  const keyBytes = new TextEncoder().encode(write.key);
  const valueBytes = new TextEncoder().encode(write.value);
  const keyBase64 = encodeBase64(keyBytes);
  return {
    id: keyBase64,
    key: write.key,
    keyBase64,
    value: write.value,
    valueBase64: encodeBase64(valueBytes),
    author: 'pending',
    contentHash: 'pending',
    contentLen: valueBytes.length,
    pending: true,
  };
}

async function handle(request: WorkerRequest): Promise<unknown> {
  if (request.method === 'initialize') {
    await initializeWasm();
    await initializePersistence();
    await initializeIroh();
    return report();
  }
  await initializeWasm();
  switch (request.method) {
    case 'steel-probe':
      if (typeof request.payload !== 'string') throw new Error('Steel source must be a string');
      return run_steel(request.payload);
    case 'create-workspace':
      return createWorkspace();
    case 'join-workspace':
      if (typeof request.payload !== 'string') throw new Error('Workspace ticket must be a string');
      return joinWorkspace(request.payload);
    case 'put-entry':
      if (!isPutEntry(request.payload)) throw new Error('Invalid document entry');
      return enqueueWrite(request.payload);
    case 'refresh-sync':
      return refreshSync();
    case 'share-ticket': {
      const ticket = await requireNode().shareTicket();
      await saveIdentity({ ...requireIdentity(), ticket });
      return ticket;
    }
  }
}

function isPutEntry(value: unknown): value is PutEntryInput {
  return typeof value === 'object' && value !== null
    && typeof (value as PutEntryInput).key === 'string'
    && typeof (value as PutEntryInput).value === 'string';
}

function encodeBase64(value: Uint8Array) {
  let binary = '';
  for (const byte of value) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function decodeBase64(value: string) {
  return Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

scope.addEventListener('message', (event: MessageEvent<WorkerRequest>) => {
  const request = event.data;
  void handle(request).then(
    (result) => {
      const response: WorkerResponse = { id: request.id, ok: true, result };
      scope.postMessage(response);
    },
    (cause: unknown) => {
      const response: WorkerResponse = { id: request.id, ok: false, error: errorMessage(cause) };
      scope.postMessage(response);
    },
  );
});
