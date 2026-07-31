export type RuntimeState = 'starting' | 'ready' | 'error';

export interface RuntimeInfo {
  api_version: number;
  crate_version: string;
  steel: boolean;
  iroh: boolean;
  persistence: string;
}

export interface RuntimeReport {
  runtime: RuntimeInfo;
  indexedDb: boolean;
  steelResult: string;
  restoredAt?: string;
}

export type WorkerMethod = 'initialize' | 'steel-probe';

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
