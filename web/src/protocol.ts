export type RuntimeState = 'starting' | 'ready' | 'error';

export interface RuntimeInfo {
  api_version: number;
  crate_version: string;
  steel: boolean;
  iroh: boolean;
  persistence: string;
}

export interface DocumentEntry {
  key: string;
  keyBase64: string;
  value?: string;
  valueBase64: string;
  author: string;
  contentHash: string;
  contentLen: number;
  pending?: boolean;
}

export interface SyncStatus {
  endpointId: string;
  workspaceId?: string;
  authorId: string;
  peers: number;
  writable: boolean;
}

export interface WorkspaceOutcome {
  workspaceId: string;
  ticket: string;
  syncError?: string;
}

export interface RuntimeReport {
  runtime: RuntimeInfo;
  indexedDb: boolean;
  steelResult: string;
  restoredAt?: string;
  status: SyncStatus;
  entries: DocumentEntry[];
  ticket?: string;
  syncError?: string;
  pendingWrites: number;
}

export type WorkerMethod =
  | 'initialize'
  | 'steel-probe'
  | 'create-workspace'
  | 'join-workspace'
  | 'put-entry'
  | 'refresh-sync'
  | 'share-ticket';

export interface PutEntryInput {
  key: string;
  value: string;
}

export interface WorkerRequest {
  id: number;
  method: WorkerMethod;
  payload?: unknown;
}

export interface WorkerResponse<T = unknown> {
  id: number;
  ok: boolean;
  result?: T;
  error?: string;
}
