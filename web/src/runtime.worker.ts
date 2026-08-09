/// <reference lib="webworker" />

import init, {
  IrohDocNode,
  prepare_note_mutation,
  query_workspace,
  run_steel,
  runtime_info,
  workspace_snapshot,
} from './generated/xo-web/xo_web.js';
import type {
  DocumentEntry,
  NoteMutationInput,
  NoteQueryInput,
  PutEntryInput,
  RuntimeInfo,
  RuntimeReport,
  SyncStatus,
  WorkerRequest,
  WorkerResponse,
  WorkspaceOutcome,
  WorkspaceSnapshot,
} from './protocol';

const scope = self as DedicatedWorkerGlobalScope;
const DATABASE = 'xo-web';
const DATABASE_VERSION = 3;
const CHECKPOINT_STORE = 'runtime-checkpoints';
const VAULT_STORE = 'vault';
const ENTRY_STORE = 'document-entries';
const PENDING_STORE = 'pending-writes';
const REPLICA_STORE = 'automerge-replicas';
const VAULT_KEY_ID = 'browser-key';
const VAULT_STATE_ID = 'identity';

interface BrowserIdentity {
  peerId?: string;
  endpointSecret: string;
  authorSecret: string;
  ticket?: string;
}

interface PendingWrite {
  id: string;
  key: string;
  valueBase64?: string;
  value?: string;
  author?: string;
  createdAt: string;
}

interface PreparedMutation {
  noteId: string;
  writes: Array<{ key: string; valueBase64: string }>;
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
let workspaceCache: { fingerprint: string; json: string; value: WorkspaceSnapshot } | undefined;

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
      for (const store of [CHECKPOINT_STORE, VAULT_STORE, ENTRY_STORE, PENDING_STORE, REPLICA_STORE]) {
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
  if (!identity.peerId) return;
  node = await IrohDocNode.spawn(
    decodeBase64(identity.endpointSecret),
    decodeBase64(identity.authorSecret),
    identity.peerId,
  );
  if (!identity.ticket) return;
  try {
    const replica = await getRecord<{ id: string; value: string }>(REPLICA_STORE, 'active');
    if (replica?.value) {
      await node.restoreReplica(identity.ticket, replica.value);
      try {
        await node.refreshSync();
        lastSyncError = undefined;
      } catch (cause) {
        lastSyncError = errorMessage(cause);
      }
    } else {
      const outcome = JSON.parse(await node.joinWorkspace(identity.ticket)) as WorkspaceOutcome;
      lastSyncError = outcome.syncError;
    }
    await restoreDurableBrowserEntries();
    await syncPendingWrites();
  } catch (cause) {
    lastSyncError = errorMessage(cause);
  }
}

async function setPeerId(value: string) {
  const peerId = value.trim();
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(peerId)) {
    throw new Error("Peer ID must contain 1–64 letters, digits, '.', '_', or '-' characters");
  }
  if (identity?.peerId && identity.peerId !== peerId) {
    throw new Error('Wipe this browser identity before changing its peer ID');
  }
  await saveIdentity({ ...requireIdentity(), peerId });
  await initializeIroh();
  return report();
}

async function createWorkspace() {
  const outcome = JSON.parse(await requireNode().createWorkspace()) as WorkspaceOutcome;
  await saveIdentity({ ...requireIdentity(), ticket: outcome.ticket });
  lastSyncError = undefined;
  await persistActiveReplica();
  return report();
}

async function joinWorkspace(ticket: string) {
  if (!ticket.trim()) throw new Error('A writable workspace ticket is required');
  const previous = JSON.parse(await requireNode().statusJson()) as SyncStatus;
  const outcome = JSON.parse(await requireNode().joinWorkspace(ticket.trim())) as WorkspaceOutcome;
  if (previous.workspaceId && previous.workspaceId !== outcome.workspaceId) {
    const tx = requireDatabase().transaction([ENTRY_STORE, PENDING_STORE, REPLICA_STORE], 'readwrite');
    tx.objectStore(ENTRY_STORE).clear();
    tx.objectStore(PENDING_STORE).clear();
    tx.objectStore(REPLICA_STORE).clear();
    await transactionComplete(tx);
    workspaceCache = undefined;
  }
  await saveIdentity({ ...requireIdentity(), ticket: outcome.ticket });
  lastSyncError = outcome.syncError;
  await persistActiveReplica();
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
  const status = JSON.parse(await requireNode().statusJson()) as SyncStatus;
  const bytes = new TextEncoder().encode(input.value);
  await enqueuePreparedWrites([{ key, valueBase64: encodeBase64(bytes) }], status.authorId);
  try {
    await publishPendingWrites();
    lastSyncError = undefined;
    await refreshEntryCache();
  } catch (cause) {
    lastSyncError = errorMessage(cause);
  }
  return report();
}

async function mutateNote(input: NoteMutationInput) {
  const entries = await cachedEntries();
  const status = JSON.parse(await requireNode().statusJson()) as SyncStatus;
  const prepared = JSON.parse(prepare_note_mutation(
    JSON.stringify(entries),
    status.authorId,
    JSON.stringify(input),
    BigInt(Date.now()),
    -new Date().getTimezoneOffset() * 60,
  )) as PreparedMutation;
  await enqueuePreparedWrites(prepared.writes, status.authorId);
  try {
    await publishPendingWrites();
    await refreshEntryCache();
    lastSyncError = undefined;
  } catch (cause) {
    lastSyncError = errorMessage(cause);
  }
  return { ...await report(), mutatedNoteId: prepared.noteId };
}

async function enqueuePreparedWrites(writes: PreparedMutation['writes'], author: string) {
  const createdAt = new Date().toISOString();
  for (const [index, write] of writes.entries()) {
    const pending: PendingWrite = {
      id: crypto.randomUUID(),
      key: write.key,
      valueBase64: write.valueBase64,
      author,
      createdAt: `${createdAt}/${String(index).padStart(4, '0')}`,
    };
    await putRecord(PENDING_STORE, pending);
    await putRecord(ENTRY_STORE, optimisticEntry(pending));
  }
}

async function publishPendingWrites() {
  const pending = await allRecords<PendingWrite>(PENDING_STORE);
  for (const write of pending.sort((left, right) => left.createdAt.localeCompare(right.createdAt))) {
    const valueBase64 = write.valueBase64 ?? encodeBase64(new TextEncoder().encode(write.value ?? ''));
    await requireNode().putBase64(write.key, valueBase64);
  }
  return pending;
}

async function confirmPendingWrites(published: PendingWrite[]) {
  for (const write of published) await deleteRecord(PENDING_STORE, write.id);
}

async function syncPendingWrites() {
  const pending = await allRecords<PendingWrite>(PENDING_STORE);
  if (!pending.length) return;
  const published = await publishPendingWrites();
  await requireNode().refreshSync();
  if (await hasRemotePeers()) await confirmPendingWrites(published);
  await refreshEntryCache();
}

async function refreshSync() {
  try {
    const published = await publishPendingWrites();
    await requireNode().refreshSync();
    if (await hasRemotePeers()) await confirmPendingWrites(published);
    await refreshEntryCache();
    lastSyncError = undefined;
  } catch (cause) {
    lastSyncError = errorMessage(cause);
  }
  return report();
}

async function hasRemotePeers() {
  const status = JSON.parse(await requireNode().statusJson()) as SyncStatus;
  return status.peers > 0;
}

async function restoreDurableBrowserEntries() {
  const entries = (await allRecords<DocumentEntry & { id: string }>(ENTRY_STORE))
    .map(({ id: _, ...entry }) => entry);
  if (entries.length) await requireNode().restoreAuthorEntries(JSON.stringify(entries));
}

async function persistActiveReplica() {
  const status = JSON.parse(await requireNode().statusJson()) as SyncStatus;
  if (status.workspaceId && status.writable) {
    await putRecord(REPLICA_STORE, { id: 'active', value: await requireNode().replicaBase64() });
  }
}

async function refreshEntryCache() {
  const entries = JSON.parse(await requireNode().entriesJson()) as DocumentEntry[];
  const tx = requireDatabase().transaction(ENTRY_STORE, 'readwrite');
  const store = tx.objectStore(ENTRY_STORE);
  // xo uses immutable revision/config keys and explicit tombstones. Merging
  // keeps the durable replica usable while remote content is still arriving,
  // instead of erasing it whenever an in-memory Iroh node starts empty.
  for (const entry of entries) store.put({ id: entry.keyBase64, ...entry });
  await transactionComplete(tx);
  await persistActiveReplica();
}

async function cachedEntries() {
  const entries = (await allRecords<DocumentEntry & { id: string }>(ENTRY_STORE))
    .map(({ id: _, ...entry }) => entry);
  const byKey = new Map(entries.map((entry) => [entry.key, entry]));
  const pendingWrites = await allRecords<PendingWrite>(PENDING_STORE);
  pendingWrites.sort((left, right) => left.createdAt.localeCompare(right.createdAt));
  for (const pending of pendingWrites) {
    const { id: _, ...entry } = optimisticEntry(pending);
    byKey.set(entry.key, entry);
  }
  return [...byKey.values()].sort((left, right) => left.key.localeCompare(right.key));
}

async function report(): Promise<RuntimeReport> {
  const status = node
    ? JSON.parse(await node.statusJson()) as SyncStatus
    : { endpointId: '', authorId: '', peers: 0, writable: false };
  const entries = await cachedEntries();
  const workspace = status.workspaceId ? resolvedWorkspace(entries) : undefined;
  const members = status.workspaceId
    ? JSON.parse(await requireNode().membersJson())
    : [];
  const pendingMembers = status.workspaceId
    ? JSON.parse(await requireNode().pendingMembersJson())
    : [];
  return {
    runtime: JSON.parse(runtime_info()) as RuntimeInfo,
    peerId: identity?.peerId,
    indexedDb: true,
    steelResult: run_steel('(+ 20 22)'),
    restoredAt,
    status,
    entries,
    ticket: identity?.ticket,
    syncError: lastSyncError,
    pendingWrites: (await allRecords<PendingWrite>(PENDING_STORE)).length,
    workspace: workspace?.value,
    members,
    pendingMembers,
  };
}

async function queryNotes(input: NoteQueryInput) {
  const workspace = resolvedWorkspace(await cachedEntries());
  return JSON.parse(query_workspace(workspace.json, JSON.stringify(input)));
}

function resolvedWorkspace(entries: DocumentEntry[]) {
  const fingerprint = entries.map((entry) =>
    `${entry.keyBase64}:${entry.contentHash}:${entry.pending ? entry.valueBase64 : ''}`
  ).join('|');
  if (workspaceCache?.fingerprint === fingerprint) return workspaceCache;
  const json = workspace_snapshot(JSON.stringify(entries));
  workspaceCache = {
    fingerprint,
    json,
    value: JSON.parse(json) as WorkspaceSnapshot,
  };
  return workspaceCache;
}

function optimisticEntry(write: PendingWrite): DocumentEntry & { id: string } {
  const keyBytes = new TextEncoder().encode(write.key);
  const valueBase64 = write.valueBase64 ?? encodeBase64(new TextEncoder().encode(write.value ?? ''));
  const valueBytes = decodeBase64(valueBase64);
  const keyBase64 = encodeBase64(keyBytes);
  let value: string | undefined;
  try {
    value = new TextDecoder('utf-8', { fatal: true }).decode(valueBytes);
  } catch {
    value = undefined;
  }
  return {
    id: keyBase64,
    key: write.key,
    keyBase64,
    value,
    valueBase64,
    author: write.author ?? 'pending',
    contentHash: 'pending',
    contentLen: valueBytes.length,
    pending: true,
  };
}

async function wipeLocalData() {
  database?.close();
  database = undefined;
  node = undefined;
  identity = undefined;
  workspaceCache = undefined;
  await new Promise<void>((resolve, reject) => {
    const request = indexedDB.deleteDatabase(DATABASE);
    request.onsuccess = () => resolve();
    request.onerror = () => reject(request.error ?? new Error('Could not delete xo IndexedDB data'));
    request.onblocked = () => reject(new Error('Close other xo tabs before wiping browser data'));
  });
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
    case 'set-peer-id':
      if (typeof request.payload !== 'string') throw new Error('Peer ID must be a string');
      return setPeerId(request.payload);
    case 'create-workspace':
      return createWorkspace();
    case 'join-workspace':
      if (typeof request.payload !== 'string') throw new Error('Workspace ticket must be a string');
      return joinWorkspace(request.payload);
    case 'put-entry':
      if (!isPutEntry(request.payload)) throw new Error('Invalid document entry');
      return enqueueWrite(request.payload);
    case 'query-notes':
      if (!isNoteQuery(request.payload)) throw new Error('Invalid note query');
      return queryNotes(request.payload);
    case 'mutate-note':
      if (!isNoteMutation(request.payload)) throw new Error('Invalid note mutation');
      return mutateNote(request.payload);
    case 'refresh-sync':
      return refreshSync();
    case 'share-ticket': {
      const ticket = await requireNode().shareTicket();
      await saveIdentity({ ...requireIdentity(), ticket });
      return ticket;
    }
    case 'approve-peer':
      await requireNode().approvePeer(request.payload as string);
      await persistActiveReplica();
      return report();
    case 'reject-peer':
      await requireNode().rejectPeer(request.payload as string);
      await persistActiveReplica();
      return report();
    case 'remove-peer': {
      await requireNode().removePeer(request.payload as string);
      const ticket = await requireNode().shareTicket();
      await saveIdentity({ ...requireIdentity(), ticket });
      await persistActiveReplica();
      return report();
    }
    case 'wipe-local-data':
      await wipeLocalData();
      return undefined;
  }
}

function isPutEntry(value: unknown): value is PutEntryInput {
  return typeof value === 'object' && value !== null
    && typeof (value as PutEntryInput).key === 'string'
    && typeof (value as PutEntryInput).value === 'string';
}

function isNoteQuery(value: unknown): value is NoteQueryInput {
  return typeof value === 'object' && value !== null
    && typeof (value as NoteQueryInput).view === 'string'
    && typeof (value as NoteQueryInput).search === 'string'
    && Array.isArray((value as NoteQueryInput).tags);
}

function isNoteMutation(value: unknown): value is NoteMutationInput {
  return typeof value === 'object' && value !== null
    && ['save', 'delete', 'restore'].includes((value as NoteMutationInput).operation);
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
  void respond(event.data);
});

async function respond(request: WorkerRequest) {
  try {
    const result = await handle(request);
    const response: WorkerResponse = { id: request.id, ok: true, result };
    scope.postMessage(response);
  } catch (cause) {
    const response: WorkerResponse = { id: request.id, ok: false, error: errorMessage(cause) };
    scope.postMessage(response);
  }
}
