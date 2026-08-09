import type {
  NoteMutationInput,
  NoteQueryInput,
  PutEntryInput,
  RuntimeReport,
  WorkspaceNote,
  WorkerMethod,
  WorkerRequest,
  WorkerResponse,
} from './protocol';

interface PendingCall {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
  timeout: number;
}

export class XoRuntime {
  readonly #worker = new Worker(new URL('./runtime.worker.ts', import.meta.url), { type: 'module' });
  readonly #pending = new Map<number, PendingCall>();
  #nextID = 1;

  constructor() {
    this.#worker.addEventListener('message', (event: MessageEvent<WorkerResponse>) => {
      const call = this.#pending.get(event.data.id);
      if (!call) return;
      window.clearTimeout(call.timeout);
      this.#pending.delete(event.data.id);
      if (event.data.ok) call.resolve(event.data.result);
      else call.reject(new Error(event.data.error ?? 'Worker request failed'));
    });
    this.#worker.addEventListener('error', (event) => {
      this.#rejectAll(new Error(event.message || 'xo runtime worker stopped'));
    });
  }

  initialize() {
    return this.#call<RuntimeReport>('initialize');
  }

  setPeerId(peerId: string) {
    return this.#call<RuntimeReport>('set-peer-id', peerId);
  }

  runSteel(source: string) {
    return this.#call<string>('steel-probe', source);
  }

  createWorkspace() {
    return this.#call<RuntimeReport>('create-workspace');
  }

  joinWorkspace(ticket: string) {
    return this.#call<RuntimeReport>('join-workspace', ticket);
  }

  putEntry(input: PutEntryInput) {
    return this.#call<RuntimeReport>('put-entry', input);
  }

  queryNotes(input: NoteQueryInput) {
    return this.#call<WorkspaceNote[]>('query-notes', input);
  }

  mutateNote(input: NoteMutationInput) {
    return this.#call<RuntimeReport>('mutate-note', input);
  }

  refreshSync() {
    return this.#call<RuntimeReport>('refresh-sync');
  }

  shareTicket() {
    return this.#call<string>('share-ticket');
  }

  approvePeer(fingerprint: string) {
    return this.#call<RuntimeReport>('approve-peer', fingerprint);
  }

  rejectPeer(fingerprint: string) {
    return this.#call<RuntimeReport>('reject-peer', fingerprint);
  }

  removePeer(fingerprint: string) {
    return this.#call<RuntimeReport>('remove-peer', fingerprint);
  }

  wipeLocalData() {
    return this.#call<void>('wipe-local-data');
  }

  terminate() {
    this.#worker.terminate();
    this.#rejectAll(new Error('xo runtime worker terminated'));
  }

  #call<T>(method: WorkerMethod, payload?: unknown): Promise<T> {
    const id = this.#nextID++;
    const request: WorkerRequest = { id, method, payload };
    return new Promise<T>((resolve, reject) => {
      const timeout = window.setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, 120_000);
      this.#pending.set(id, {
        resolve: (value) => resolve(value as T),
        reject,
        timeout,
      });
      this.#worker.postMessage(request);
    });
  }

  #rejectAll(error: Error) {
    for (const call of this.#pending.values()) {
      window.clearTimeout(call.timeout);
      call.reject(error);
    }
    this.#pending.clear();
  }
}
