export type RuntimeState = 'starting' | 'ready' | 'error';

export interface RuntimeInfo {
  api_version: number;
  version: string;
  steel: boolean;
  central_sync: boolean;
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

export type FrontmatterValue = null | boolean | number | string | FrontmatterValue[] | { [key: string]: FrontmatterValue };

export interface SubviewDescriptor {
  id: string;
  name: string;
  sort_field?: string;
}

export interface ViewDescriptor {
  id: string;
  name: string;
  key?: string;
  show_tags: boolean;
  title_field: string;
  subtitle_field?: string;
  sort_field?: string;
  descending: boolean;
  subviews: SubviewDescriptor[];
}

export interface WorkspaceBehavior {
  default_view: string;
  views: ViewDescriptor[];
}

export interface NoteConflict {
  note_id: string;
  winning_revision: string;
  concurrent_revisions: string[];
}

export interface HistoryRevision {
  id: string;
  author: string;
  physicalMs: number;
  deleted: boolean;
}

export interface WorkspaceNote {
  id: string;
  frontmatter: Record<string, FrontmatterValue>;
  body: string;
  path: string;
  markdown: string;
  winningRevision: string;
  conflict?: NoteConflict;
  history: HistoryRevision[];
}

export interface WorkspaceSnapshot {
  behavior: WorkspaceBehavior;
  notes: WorkspaceNote[];
  deleted: WorkspaceNote[];
  conflicts: number;
  diagnostics: string[];
}

export interface NoteQueryInput {
  view: string;
  subview?: string;
  search: string;
  tags: string[];
}

export interface NoteMutationInput {
  operation: 'save' | 'delete' | 'restore';
  noteId?: string;
  title?: string;
  markdown?: string;
}

export interface SyncStatus {
  workspaceId?: string;
  authorId: string;
  connection: 'offline' | 'connecting' | 'connected';
  clients: string[];
  writable: boolean;
}

export interface RuntimeReport {
  runtime: RuntimeInfo;
  clientId?: string;
  indexedDb: boolean;
  steelResult: string;
  restoredAt?: string;
  status: SyncStatus;
  entries: DocumentEntry[];
  syncError?: string;
  pendingWrites: number;
  workspace?: WorkspaceSnapshot;
  mutatedNoteId?: string;
}

export type WorkerMethod =
  | 'initialize'
  | 'set-access-token'
  | 'steel-probe'
  | 'set-client-id'
  | 'put-entry'
  | 'query-notes'
  | 'mutate-note'
  | 'refresh-sync'
  | 'wipe-local-data';

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
