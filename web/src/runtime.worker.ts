/// <reference lib="webworker" />

import init, {
  BrowserReplica,
  prepare_note_mutation,
  query_workspace,
  run_steel,
  runtime_info,
  workspace_snapshot,
} from './generated/xo-pwa/xo_pwa.js';
import type {
  DocumentEntry,
  NoteMutationInput,
  NoteQueryInput,
  PutEntryInput,
  RuntimeInfo,
  RuntimeReport,
  WorkerRequest,
  WorkerResponse,
  WorkspaceSnapshot,
} from './protocol';

const scope = self as DedicatedWorkerGlobalScope;
const DATABASE = 'xo-pwa';
const DATABASE_VERSION = 4;
const CHECKPOINT_STORE = 'runtime-checkpoints';
const SETTINGS_STORE = 'central-settings';
const REPLICA_STORE = 'automerge-replicas';
const ACTIVE = 'active';
const PROTOCOL_VERSION = 1;
const MAX_SYNC_BYTES = 8 * 1024 * 1024;

interface Settings {
  id: string;
  clientId?: string;
  actorId: string;
  workspaceId?: string;
  dirty: boolean;
}

interface ReplicaRecord {
  id: string;
  workspaceId: string;
  snapshot: ArrayBuffer;
}

interface PreparedMutation {
  noteId: string;
  writes: Array<{ key: string; valueBase64: string }>;
}

interface ServerHello {
  type: 'server_hello';
  protocol_version: number;
  workspace_id: string;
  clients: string[];
}

interface Presence {
  type: 'presence';
  clients: string[];
}

let wasmReady: Promise<void> | undefined;
let database: IDBDatabase | undefined;
let settings: Settings | undefined;
let replica: BrowserReplica | undefined;
let socket: WebSocket | undefined;
let connection: 'offline' | 'connecting' | 'connected' = 'offline';
let accessToken: string | undefined;
let connectedClients: string[] = [];
let reconnectTimer: number | undefined;
let reconnectAttempt = 0;
let socketQueue = Promise.resolve();
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
    request.onupgradeneeded = (event) => {
      const db = request.result;
      for (const obsolete of ['vault', 'document-entries', 'pending-writes']) {
        if (db.objectStoreNames.contains(obsolete)) db.deleteObjectStore(obsolete);
      }
      for (const store of [CHECKPOINT_STORE, SETTINGS_STORE, REPLICA_STORE]) {
        if (!db.objectStoreNames.contains(store)) db.createObjectStore(store, { keyPath: 'id' });
      }
      // Version 3 stored the obsolete transport-specific replica envelope. The centralized
      // transport is intentionally a fresh workspace replica.
      if ((event as IDBVersionChangeEvent).oldVersion < 4 && db.objectStoreNames.contains(REPLICA_STORE)) {
        request.transaction?.objectStore(REPLICA_STORE).clear();
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

function requireDatabase() {
  if (!database) throw new Error('IndexedDB is not initialized');
  return database;
}

function requireSettings() {
  if (!settings) throw new Error('Browser settings are unavailable');
  return settings;
}

function requireReplica() {
  if (!replica) throw new Error('Connect to xo-syncd once before editing offline');
  return replica;
}

async function initializePersistence() {
  database = await openDatabase();
  const checkpoint = await getRecord<{ id: string; updatedAt?: string }>(CHECKPOINT_STORE, 'runtime');
  restoredAt = checkpoint?.updatedAt;
  await putRecord(CHECKPOINT_STORE, { id: 'runtime', schema: DATABASE_VERSION, updatedAt: new Date().toISOString() });
  settings = await getRecord<Settings>(SETTINGS_STORE, ACTIVE) ?? {
    id: ACTIVE,
    actorId: `browser-${crypto.randomUUID()}`,
    dirty: false,
  };
  await saveSettings();
  const saved = await getRecord<ReplicaRecord>(REPLICA_STORE, ACTIVE);
  if (saved) {
    replica = BrowserReplica.restore(new Uint8Array(saved.snapshot), settings.actorId);
    settings.workspaceId = replica.workspaceId();
    await saveSettings();
  }
}

async function saveSettings() {
  await putRecord(SETTINGS_STORE, requireSettings());
}

async function persistReplica() {
  const active = requireReplica();
  const bytes = active.snapshot();
  const copy = new Uint8Array(bytes.length);
  copy.set(bytes);
  await putRecord(REPLICA_STORE, {
    id: ACTIVE,
    workspaceId: active.workspaceId(),
    snapshot: copy.buffer,
  } satisfies ReplicaRecord);
  workspaceCache = undefined;
}

async function setClientId(raw: string) {
  const clientId = raw.trim();
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(clientId)) {
    throw new Error("Client ID must contain 1–64 letters, digits, '.', '_', or '-' characters");
  }
  const current = requireSettings();
  current.clientId = clientId;
  await saveSettings();
  connectNow();
  return report();
}

function socketUrl() {
  const url = new URL(scope.location.href);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  url.pathname = '/api/sync';
  url.search = '';
  url.hash = '';
  return url.toString();
}

function connectNow() {
  const clientId = settings?.clientId;
  if (!clientId || socket?.readyState === WebSocket.OPEN || socket?.readyState === WebSocket.CONNECTING) return;
  if (reconnectTimer !== undefined) scope.clearTimeout(reconnectTimer);
  reconnectTimer = undefined;
  connection = 'connecting';
  if (!accessToken) {
    lastSyncError = 'Sign in is required before synchronization.';
    connection = 'offline';
    return;
  }
  const next = new WebSocket(socketUrl(), ['xo-sync', `xo-bearer.${accessToken}`]);
  next.binaryType = 'arraybuffer';
  socket = next;
  next.addEventListener('open', () => {
    next.send(JSON.stringify({
      type: 'client_hello',
      protocol_version: PROTOCOL_VERSION,
      client_id: clientId,
    }));
  });
  next.addEventListener('message', (event) => {
    socketQueue = socketQueue.then(() => receiveSocketMessage(next, event.data));
  });
  next.addEventListener('close', (event) => {
    if (connection !== 'connected') {
      lastSyncError = `xo-syncd closed the WebSocket (${event.code}${event.reason ? `: ${event.reason}` : ''})`;
    }
    disconnected(next);
  });
  next.addEventListener('error', () => {
    lastSyncError = 'Could not connect to xo-syncd; the local replica remains available.';
  });
}

async function receiveSocketMessage(activeSocket: WebSocket, data: string | ArrayBuffer | Blob) {
  try {
    if (typeof data === 'string') {
      const control = JSON.parse(data) as ServerHello | Presence | { type: 'error'; message: string };
      if (control.type === 'server_hello') {
        if (control.protocol_version !== PROTOCOL_VERSION) throw new Error('Unsupported synchronization protocol');
        await openServerWorkspace(control.workspace_id);
        connectedClients = [...control.clients].sort();
        connection = 'connected';
        reconnectAttempt = 0;
        lastSyncError = undefined;
        pumpSync(activeSocket);
      } else if (control.type === 'presence') {
        connectedClients = [...control.clients].sort();
      } else if (control.type === 'error') {
        throw new Error(control.message);
      }
      return;
    }
    const bytes = data instanceof Blob ? new Uint8Array(await data.arrayBuffer()) : new Uint8Array(data);
    if (!bytes.length || bytes.length > MAX_SYNC_BYTES) throw new Error('Invalid synchronization frame size');
    const changed = requireReplica().receiveSyncMessage(bytes);
    if (changed) await persistReplica();
    const generated = pumpSync(activeSocket);
    if (!generated && settings?.dirty) {
      settings.dirty = false;
      await saveSettings();
    }
  } catch (cause) {
    lastSyncError = errorMessage(cause);
    activeSocket.close();
  }
}

async function openServerWorkspace(workspaceId: string) {
  if (replica?.workspaceId() === workspaceId) {
    replica.resetSync();
    return;
  }
  replica = BrowserReplica.create(workspaceId, requireSettings().actorId);
  settings!.workspaceId = workspaceId;
  settings!.dirty = false;
  await persistReplica();
  await saveSettings();
}

function pumpSync(activeSocket = socket) {
  if (!activeSocket || activeSocket.readyState !== WebSocket.OPEN || !replica) return false;
  const message = replica.generateSyncMessage();
  if (!message) return false;
  activeSocket.send(new Uint8Array(message).buffer);
  return true;
}

function disconnected(closed: WebSocket) {
  if (socket !== closed) return;
  socket = undefined;
  connection = 'offline';
  connectedClients = [];
  scheduleReconnect();
}

function scheduleReconnect() {
  if (!settings?.clientId || reconnectTimer !== undefined) return;
  const delay = Math.min(30_000, 500 * (2 ** Math.min(reconnectAttempt, 6)));
  reconnectAttempt += 1;
  reconnectTimer = scope.setTimeout(() => {
    reconnectTimer = undefined;
    connectNow();
  }, delay + Math.floor(Math.random() * Math.min(delay, 1_000)));
}

async function putWrites(writes: PreparedMutation['writes']) {
  const active = requireReplica();
  for (const write of writes) active.put(write.key, decodeBase64(write.valueBase64));
  settings!.dirty = true;
  await persistReplica();
  await saveSettings();
  pumpSync();
}

async function putEntry(input: PutEntryInput) {
  const key = input.key.trim();
  if (!key) throw new Error('Document key is required');
  await putWrites([{ key, valueBase64: encodeBase64(new TextEncoder().encode(input.value)) }]);
  return report();
}

async function mutateNote(input: NoteMutationInput) {
  const entries = currentEntries();
  const prepared = JSON.parse(prepare_note_mutation(
    JSON.stringify(entries),
    requireSettings().actorId,
    JSON.stringify(input),
    BigInt(Date.now()),
    -new Date().getTimezoneOffset() * 60,
  )) as PreparedMutation;
  await putWrites(prepared.writes);
  return { ...await report(), mutatedNoteId: prepared.noteId };
}

function currentEntries(): DocumentEntry[] {
  if (!replica) return [];
  return JSON.parse(replica.entriesJson()) as DocumentEntry[];
}

async function report(): Promise<RuntimeReport> {
  const entries = currentEntries();
  const workspace = replica ? resolvedWorkspace(entries).value : undefined;
  return {
    runtime: JSON.parse(runtime_info()) as RuntimeInfo,
    clientId: settings?.clientId,
    indexedDb: true,
    steelResult: run_steel('(+ 20 22)'),
    restoredAt,
    status: {
      workspaceId: replica?.workspaceId() ?? settings?.workspaceId,
      authorId: settings?.actorId ?? '',
      connection,
      clients: connectedClients,
      writable: Boolean(replica),
    },
    entries,
    syncError: lastSyncError,
    pendingWrites: settings?.dirty ? 1 : 0,
    workspace,
  };
}

function resolvedWorkspace(entries: DocumentEntry[]) {
  const fingerprint = entries.map((entry) => `${entry.keyBase64}:${entry.contentHash}`).join('|');
  if (workspaceCache?.fingerprint === fingerprint) return workspaceCache;
  const json = workspace_snapshot(JSON.stringify(entries));
  workspaceCache = { fingerprint, json, value: JSON.parse(json) as WorkspaceSnapshot };
  return workspaceCache;
}

async function queryNotes(input: NoteQueryInput) {
  const workspace = resolvedWorkspace(currentEntries());
  return JSON.parse(query_workspace(workspace.json, JSON.stringify(input)));
}

async function refreshSync() {
  connectNow();
  pumpSync();
  return report();
}

async function wipeLocalData() {
  if (reconnectTimer !== undefined) scope.clearTimeout(reconnectTimer);
  reconnectTimer = undefined;
  socket?.close();
  socket = undefined;
  replica = undefined;
  settings = undefined;
  workspaceCache = undefined;
  database?.close();
  database = undefined;
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
    connectNow();
    return report();
  }
  await initializeWasm();
  switch (request.method) {
    case 'set-access-token':
      if (typeof request.payload !== 'string') throw new Error('Access token must be a string');
      accessToken = request.payload || undefined;
      connectNow();
      return undefined;
    case 'steel-probe':
      if (typeof request.payload !== 'string') throw new Error('Steel source must be a string');
      return run_steel(request.payload);
    case 'set-client-id':
      if (typeof request.payload !== 'string') throw new Error('Client ID must be a string');
      return setClientId(request.payload);
    case 'put-entry':
      if (!isPutEntry(request.payload)) throw new Error('Invalid document entry');
      return putEntry(request.payload);
    case 'query-notes':
      if (!isNoteQuery(request.payload)) throw new Error('Invalid note query');
      return queryNotes(request.payload);
    case 'mutate-note':
      if (!isNoteMutation(request.payload)) throw new Error('Invalid note mutation');
      return mutateNote(request.payload);
    case 'refresh-sync':
      return refreshSync();
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

let requestQueue = Promise.resolve();
scope.addEventListener('message', (event: MessageEvent<WorkerRequest>) => {
  if (event.data.method === 'query-notes') void respond(event.data);
  else requestQueue = requestQueue.then(() => respond(event.data));
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
