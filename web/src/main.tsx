import React, { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import {
  BookOpen,
  Boxes,
  Check,
  CircleAlert,
  Cloud,
  CloudOff,
  Code2,
  Copy,
  Database,
  Download,
  Inbox,
  KeyRound,
  LoaderCircle,
  LockKeyhole,
  Menu,
  NotebookPen,
  Plus,
  Radio,
  RefreshCw,
  Search,
  Settings,
  Sparkles,
  WifiOff,
  X,
} from 'lucide-react';
import { registerSW } from 'virtual:pwa-register';
import type {
  DocumentEntry,
  NoteMutationInput,
  NoteQueryInput,
  RuntimeReport,
  RuntimeState,
  WorkspaceNote,
} from './protocol';
import { XoRuntime } from './runtime';
import './styles.css';

type InstallPrompt = Event & {
  prompt: () => Promise<void>;
  userChoice: Promise<{ outcome: 'accepted' | 'dismissed' }>;
};

const APP_VERSION = __XO_VERSION__;
const UPDATE_INTERVAL_MS = 10 * 60 * 1_000;
const UPDATE_EVENT = 'xo-update-available';
let updateIsAvailable = false;
let serviceWorkerRegistration: ServiceWorkerRegistration | undefined;

const updateServiceWorker = registerSW({
  immediate: true,
  onNeedRefresh() {
    announceUpdate();
  },
  onRegisteredSW(_serviceWorkerUrl, registration) {
    serviceWorkerRegistration = registration;
  },
});
let scannedWorkspaceTicket = consumeWorkspaceTicket();

function announceUpdate() {
  updateIsAvailable = true;
  window.dispatchEvent(new Event(UPDATE_EVENT));
}

async function checkForUpdates() {
  try {
    const registration = serviceWorkerRegistration ?? await navigator.serviceWorker.getRegistration();
    if (registration) {
      serviceWorkerRegistration = registration;
      await registration.update();
    }
  } catch {
    // The service worker can be unavailable during first load or while offline.
  }
  try {
    const response = await fetch(`/version.json?checked=${Date.now()}`, {
      cache: 'no-store',
      headers: { Accept: 'application/json' },
    });
    if (!response.ok) return;
    const manifest = await response.json() as { version?: string };
    if (manifest.version && manifest.version !== APP_VERSION) announceUpdate();
  } catch {
    // Keep running the cached application while the server is unavailable.
  }
}

async function refreshFullApp() {
  try {
    await serviceWorkerRegistration?.update();
    await updateServiceWorker(true);
  } finally {
    window.location.reload();
  }
}

function consumeWorkspaceTicket() {
  const parameters = new URLSearchParams(window.location.hash.replace(/^#/, ''));
  const ticket = parameters.get('ticket')?.trim();
  if (!ticket) return undefined;
  window.history.replaceState(null, '', `${window.location.pathname}${window.location.search}`);
  return ticket;
}

function App() {
  const runtimeRef = useRef<XoRuntime | undefined>(undefined);
  const [state, setState] = useState<RuntimeState>('starting');
  const [report, setReport] = useState<RuntimeReport>();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [online, setOnline] = useState(navigator.onLine);
  const [installPrompt, setInstallPrompt] = useState<InstallPrompt>();
  const [ticketInput, setTicketInput] = useState('');
  const [updateAvailable, setUpdateAvailable] = useState(updateIsAvailable);
  const [activeView, setActiveView] = useState('');
  const [activeSubview, setActiveSubview] = useState<string>();
  const [search, setSearch] = useState('');

  useEffect(() => {
    const runtime = new XoRuntime();
    runtimeRef.current = runtime;
    let active = true;
    void (async () => {
      const setupTicket = scannedWorkspaceTicket;
      let next = await runtime.initialize();
      if (setupTicket) {
        try {
          next = await runtime.joinWorkspace(setupTicket);
        } finally {
          scannedWorkspaceTicket = undefined;
        }
      }
      if (!active) return;
      setReport(next);
      setState('ready');
    })().catch((cause: unknown) => {
      if (!active) return;
      setError(errorMessage(cause));
      setState('error');
    });
    return () => {
      active = false;
      if (runtimeRef.current === runtime) runtimeRef.current = undefined;
      runtime.terminate();
    };
  }, []);

  useEffect(() => {
    if (state !== 'ready' || !report?.status.workspaceId) return;
    let active = true;
    let running = false;
    const refresh = async () => {
      if (running || !runtimeRef.current) return;
      running = true;
      try {
        const next = await runtimeRef.current.refreshSync();
        if (active) {
          setReport(next);
          if (!next.syncError) setError('');
        }
      } catch (cause) {
        if (active) setError(errorMessage(cause));
      } finally {
        running = false;
      }
    };
    const timer = window.setInterval(() => void refresh(), 3_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [state, report?.status.workspaceId]);

  useEffect(() => {
    const onUpdate = () => setUpdateAvailable(true);
    const onPageShow = () => void checkForUpdates();
    window.addEventListener(UPDATE_EVENT, onUpdate);
    window.addEventListener('pageshow', onPageShow);
    void checkForUpdates();
    const timer = window.setInterval(() => void checkForUpdates(), UPDATE_INTERVAL_MS);
    return () => {
      window.removeEventListener(UPDATE_EVENT, onUpdate);
      window.removeEventListener('pageshow', onPageShow);
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    const onOnline = () => {
      setOnline(true);
      void checkForUpdates();
    };
    const onOffline = () => setOnline(false);
    const onInstall = (event: Event) => {
      event.preventDefault();
      setInstallPrompt(event as InstallPrompt);
    };
    window.addEventListener('online', onOnline);
    window.addEventListener('offline', onOffline);
    window.addEventListener('beforeinstallprompt', onInstall);
    return () => {
      window.removeEventListener('online', onOnline);
      window.removeEventListener('offline', onOffline);
      window.removeEventListener('beforeinstallprompt', onInstall);
    };
  }, []);

  useEffect(() => {
    const behavior = report?.workspace?.behavior;
    if (!behavior) return;
    const view = behavior.views.find((candidate) => candidate.id === activeView);
    if (!activeView || !view) {
      setActiveView(behavior.default_view || behavior.views[0]?.id || 'all');
      setActiveSubview(undefined);
    } else if (activeSubview && !view.subviews.some((subview) => subview.id === activeSubview)) {
      setActiveSubview(undefined);
    }
  }, [activeView, report?.workspace?.behavior]);

  async function install() {
    if (!installPrompt) return;
    await installPrompt.prompt();
    await installPrompt.userChoice;
    setInstallPrompt(undefined);
  }

  async function runWorkspace(operation: (runtime: XoRuntime) => Promise<RuntimeReport>) {
    const runtime = runtimeRef.current;
    if (!runtime) return undefined;
    setBusy(true);
    setError('');
    try {
      const next = await operation(runtime);
      setReport(next);
      return next;
    } catch (cause) {
      setError(errorMessage(cause));
      return undefined;
    } finally {
      setBusy(false);
    }
  }

  const hasWorkspace = Boolean(report?.status.workspaceId);
  return (
    <div className="app-shell">
      <aside className={menuOpen ? 'sidebar open' : 'sidebar'}>
        <div className="brand">
          <img src="/logo.svg" alt="" />
          <div><strong>xo</strong><span>private workspace</span></div>
          <button className="icon-button close-menu" onClick={() => setMenuOpen(false)} aria-label="Close navigation"><X /></button>
        </div>
        <nav aria-label="Workspace">
          {report?.workspace?.behavior.views.map((view) => (
            <React.Fragment key={view.id}>
              <NavItem
                icon={view.id === 'books' ? <BookOpen /> : view.id === 'inbox' ? <Inbox /> : <NotebookPen />}
                label={view.name || view.id}
                active={activeView === view.id && !activeSubview}
                onClick={() => {
                  setActiveView(view.id);
                  setActiveSubview(undefined);
                  setMenuOpen(false);
                }}
              />
              {view.subviews.map((subview) => (
                <NavItem
                  key={`${view.id}/${subview.id}`}
                  icon={<span className="subview-mark" />}
                  label={subview.name || subview.id}
                  active={activeView === view.id && activeSubview === subview.id}
                  nested
                  onClick={() => {
                    setActiveView(view.id);
                    setActiveSubview(subview.id);
                    setMenuOpen(false);
                  }}
                />
              ))}
            </React.Fragment>
          )) ?? <NavItem icon={<NotebookPen />} label="Workspace" active />}
        </nav>
        <div className="sidebar-spacer" />
        <nav aria-label="Application">
          <NavItem icon={<Boxes />} label="Steel plugins" />
          <NavItem icon={<Settings />} label="Settings" />
        </nav>
        <div className="privacy-note"><LockKeyhole /><span>Identity and writable capability are encrypted locally in IndexedDB.</span></div>
      </aside>
      {menuOpen ? <button className="scrim" onClick={() => setMenuOpen(false)} aria-label="Close navigation" /> : null}

      <div className="workspace">
        <header className="topbar">
          <button className="icon-button menu-button" onClick={() => setMenuOpen(true)} aria-label="Open navigation"><Menu /></button>
          <div className="search"><Search />{hasWorkspace ? <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search notes" aria-label="Search notes" /> : <span>Connect a workspace to begin</span>}<kbd>xo</kbd></div>
          <div className={online ? 'connection online' : 'connection offline'}>
            {online ? <Cloud /> : <CloudOff />}
            <span>{online ? (hasWorkspace ? (report?.status.peers ? 'relay sync active' : 'local workspace') : 'browser online') : 'offline'}</span>
          </div>
        </header>

        {updateAvailable ? (
          <div className="update-banner" role="status">
            <span>A newer xo release is available.</span>
            <button onClick={() => void refreshFullApp()}><RefreshCw /> Refresh full app</button>
          </div>
        ) : null}

        <main>
          {hasWorkspace && report ? (
            <WorkspaceView
              report={report}
              busy={busy}
              error={error}
              activeView={activeView}
              activeSubview={activeSubview}
              search={search}
              onSubview={setActiveSubview}
              onQuery={(input) => runtimeRef.current?.queryNotes(input) ?? Promise.resolve([])}
              onMutate={(input) => runWorkspace((runtime) => runtime.mutateNote(input))}
              onRefresh={() => void runWorkspace((runtime) => runtime.refreshSync())}
            />
          ) : (
            <Onboarding
              state={state}
              report={report}
              error={error}
              busy={busy}
              ticket={ticketInput}
              onTicket={setTicketInput}
              onCreate={() => void runWorkspace((runtime) => runtime.createWorkspace())}
              onJoin={() => void runWorkspace((runtime) => runtime.joinWorkspace(ticketInput))}
              installPrompt={installPrompt}
              onInstall={() => void install()}
              onCheckForUpdates={() => void checkForUpdates()}
            />
          )}
        </main>
        <footer className="app-footer">xo {APP_VERSION}</footer>
      </div>
    </div>
  );
}

function Onboarding({ state, report, error, busy, ticket, onTicket, onCreate, onJoin, installPrompt, onInstall, onCheckForUpdates }: {
  state: RuntimeState;
  report?: RuntimeReport;
  error: string;
  busy: boolean;
  ticket: string;
  onTicket: (ticket: string) => void;
  onCreate: () => void;
  onJoin: () => void;
  installPrompt?: InstallPrompt;
  onInstall: () => void;
  onCheckForUpdates: () => void;
}) {
  return (
    <>
      <section className="hero">
        <div>
          <p className="eyebrow"><Sparkles /> direct browser Iroh</p>
          <h1>Your knowledge,<br /><em>entirely client-side.</em></h1>
          <p className="lede">Create a new Iroh document or join xo-syncd with a writable ticket. Docs, Blobs, Gossip, Steel, and recovery all run in this browser worker.</p>
          <div className="hero-actions">
            <button className="primary" disabled={busy || state !== 'ready'} onClick={onCreate}>{busy ? <LoaderCircle className="spin" /> : <Plus />} Create workspace</button>
            {installPrompt ? <button className="secondary" onClick={onInstall}><Download /> Install xo</button> : <button className="secondary" onClick={onCheckForUpdates}>Check for updates</button>}
          </div>
        </div>
        <RuntimeCard state={state} report={report} error={error} />
      </section>

      <section className="join-section">
        <div><p className="eyebrow"><KeyRound /> Existing workspace</p><h2>Join with a writable ticket</h2><p>Tickets stay encrypted in this browser. Network traffic is relay-only and end-to-end encrypted by Iroh.</p></div>
        <div className="ticket-form">
          <textarea value={ticket} onChange={(event) => onTicket(event.target.value)} placeholder="Paste the writable Iroh Docs ticket from xo or xo-syncd" aria-label="Writable workspace ticket" />
          <button className="primary" disabled={busy || !ticket.trim() || state !== 'ready'} onClick={onJoin}>{busy ? <LoaderCircle className="spin" /> : <Radio />} Join and synchronize</button>
        </div>
      </section>

      <section className="status-section" aria-labelledby="foundation-title">
        <div className="section-heading"><div><p className="eyebrow">Browser runtime</p><h2 id="foundation-title">No application server</h2></div><span className="static-badge">static assets only</span></div>
        <div className="status-grid">
          <StatusCard icon={<Code2 />} title="Rust + WebAssembly" description="The xo-web facade runs only inside the dedicated worker." ready={state === 'ready'} />
          <StatusCard icon={<Database />} title="Encrypted recovery" description="Identity, capability, records, and pending writes survive in IndexedDB." ready={Boolean(report?.indexedDb)} />
          <StatusCard icon={<Sparkles />} title="Sandboxed Steel" description={`A fresh Steel VM executes in Wasm${report ? ` and returned ${report.steelResult}` : ''}.`} ready={Boolean(report?.runtime.steel)} />
          <StatusCard icon={<WifiOff />} title="Iroh Docs" description="Browser Docs, Blobs, and Gossip connect through an end-to-end encrypted relay." ready={Boolean(report?.runtime.iroh)} />
        </div>
      </section>
    </>
  );
}

function WorkspaceView({ report, busy, error, activeView, activeSubview, search, onSubview, onQuery, onMutate, onRefresh }: {
  report: RuntimeReport;
  busy: boolean;
  error: string;
  activeView: string;
  activeSubview?: string;
  search: string;
  onSubview: (subview?: string) => void;
  onQuery: (input: NoteQueryInput) => Promise<WorkspaceNote[]>;
  onMutate: (input: NoteMutationInput) => Promise<RuntimeReport | undefined>;
  onRefresh: () => void;
}) {
  const [notes, setNotes] = useState<WorkspaceNote[]>([]);
  const [unfilteredNotes, setUnfilteredNotes] = useState<WorkspaceNote[]>([]);
  const [selectedId, setSelectedId] = useState<string>();
  const [selectedTags, setSelectedTags] = useState<string[]>([]);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingId, setEditingId] = useState<string>();
  const [draft, setDraft] = useState('');
  const [createTitle, setCreateTitle] = useState('');
  const [queryError, setQueryError] = useState('');
  const [ticketVisible, setTicketVisible] = useState(false);
  const statusMessage = error || report.syncError;
  const workspace = report.workspace;
  const view = workspace?.behavior.views.find((candidate) => candidate.id === activeView);

  useEffect(() => {
    setSelectedTags([]);
  }, [activeView, activeSubview]);

  useEffect(() => {
    if (!activeView) return;
    let active = true;
    const base: NoteQueryInput = { view: activeView, subview: activeSubview, search, tags: [] };
    void Promise.all([
      onQuery({ ...base, tags: selectedTags }),
      onQuery(base),
    ]).then(([next, unfiltered]) => {
      if (!active) return;
      setQueryError('');
      setNotes(next);
      setUnfilteredNotes(unfiltered);
      setSelectedId((current) => next.some((note) => note.id === current) ? current : next[0]?.id);
    }).catch((cause: unknown) => {
      if (active) setQueryError(errorMessage(cause));
    });
    return () => { active = false; };
  }, [activeView, activeSubview, search, selectedTags, report.entries, onQuery]);

  const selected = notes.find((note) => note.id === selectedId);
  const availableTags = [...new Set(unfilteredNotes.flatMap(noteTags))].sort();

  function startCreate() {
    setEditingId(undefined);
    setCreateTitle('');
    setDraft('---\ntitle: \ntype: \ntags: []\n---\n');
    setEditorOpen(true);
  }

  function startEdit(note: WorkspaceNote) {
    setEditingId(note.id);
    setCreateTitle(noteTitle(note, view?.title_field));
    setDraft(note.markdown);
    setEditorOpen(true);
  }

  async function saveDraft() {
    const saved = await onMutate({
      operation: 'save',
      noteId: editingId,
      title: createTitle,
      markdown: draft,
    });
    if (saved) {
      setEditorOpen(false);
      setSelectedId(saved.mutatedNoteId);
    }
  }

  async function remove(note: WorkspaceNote) {
    if (!window.confirm(`Delete “${noteTitle(note, view?.title_field)}”?`)) return;
    await onMutate({ operation: 'delete', noteId: note.id });
  }

  return (
    <>
      <section className="notes-toolbar">
        <div>
          <p className="eyebrow"><Radio /> {view?.name || activeView || 'Workspace'}</p>
          <h1>{view?.name || 'Notes'}</h1>
          <p className="workspace-id">{report.status.workspaceId}</p>
        </div>
        <div className="toolbar-actions">
          <button className="secondary" disabled={busy} onClick={onRefresh}><Radio className={busy ? 'spin' : ''} /> Sync</button>
          <button className="secondary" onClick={() => void refreshFullApp()}><RefreshCw /> Refresh app</button>
          <button className="primary" disabled={busy} onClick={startCreate}><Plus /> New note</button>
        </div>
      </section>

      {view?.subviews.length ? (
        <div className="subview-tabs" aria-label="Subviews">
          <button className={!activeSubview ? 'active' : ''} onClick={() => onSubview(undefined)}>All</button>
          {view.subviews.map((subview) => <button key={subview.id} className={activeSubview === subview.id ? 'active' : ''} onClick={() => onSubview(subview.id)}>{subview.name || subview.id}</button>)}
        </div>
      ) : null}

      {statusMessage ? <div className="warning"><CloudOff /> <span>{statusMessage}. Cached notes and pending edits remain available offline.</span></div> : null}
      {queryError ? <div className="warning"><CircleAlert /> <span>{queryError}</span></div> : null}
      {workspace?.diagnostics.map((diagnostic) => <div className="warning" key={diagnostic}><CircleAlert /> <span>{diagnostic}</span></div>)}

      <section className="sync-strip">
        <Metric label="Notes" value={String(workspace?.notes.length ?? 0)} />
        <Metric label="Conflicts" value={String(workspace?.conflicts ?? 0)} />
        <Metric label="Peers" value={String(report.status.peers)} />
        <Metric label="Pending" value={String(report.pendingWrites)} />
      </section>

      {view?.show_tags && availableTags.length ? (
        <div className="tag-filter" aria-label="Filter by tags">
          <span>Tags</span>
          {availableTags.map((tag) => <button key={tag} className={selectedTags.includes(tag) ? 'active' : ''} onClick={() => setSelectedTags((current) => current.includes(tag) ? current.filter((value) => value !== tag) : [...current, tag])}>{tag}</button>)}
          {selectedTags.length ? <button onClick={() => setSelectedTags([])}>Clear</button> : null}
        </div>
      ) : null}

      <section className="note-layout">
        <div className="note-list" aria-label="Notes">
          <div className="panel-heading"><div><p className="eyebrow">Results</p><h2>{notes.length} notes</h2></div></div>
          {notes.length ? notes.map((note) => (
            <button key={note.id} className={selectedId === note.id ? 'note-list-item selected' : 'note-list-item'} onClick={() => setSelectedId(note.id)}>
              <span><strong>{noteTitle(note, view?.title_field)}</strong><small>{noteField(note, view?.subtitle_field) || note.path}</small></span>
              <span className="note-badges">{note.conflict ? <i>conflict</i> : null}{noteTags(note).map((tag) => <i key={tag}>{tag}</i>)}</span>
            </button>
          )) : <div className="empty-state">No notes match this view, subview, search, and tag filter.</div>}
        </div>

        <article className="note-preview">
          {selected ? (
            <>
              <header>
                <div><p className="eyebrow">{selected.id}</p><h2>{noteTitle(selected, view?.title_field)}</h2><p>{selected.path}</p></div>
                <div className="preview-actions"><button className="secondary" onClick={() => startEdit(selected)}>Edit</button><button className="danger" onClick={() => void remove(selected)}>Delete</button></div>
              </header>
              {selected.conflict ? <div className="conflict-callout"><CircleAlert /><span>This note has {selected.conflict.concurrent_revisions.length} concurrent revision(s). Saving merges all current heads.</span></div> : null}
              <dl className="frontmatter-grid">{Object.entries(selected.frontmatter).map(([key, value]) => <div key={key}><dt>{key}</dt><dd>{displayFrontmatter(value)}</dd></div>)}</dl>
              <pre className="markdown-preview">{selected.body || 'This note has no body.'}</pre>
              <details className="history-panel"><summary>Revision history ({selected.history.length})</summary>{selected.history.slice().reverse().map((revision) => <div key={revision.id}><code>{short(revision.id)}</code><span>{localTimestamp(revision.physicalMs)} · {short(revision.author)}{revision.deleted ? ' · deleted' : ''}</span></div>)}</details>
            </>
          ) : <div className="empty-state preview-empty">Select a note to read or edit it.</div>}
        </article>
      </section>

      {workspace?.deleted.length ? (
        <details className="deleted-panel"><summary>Deleted notes ({workspace.deleted.length})</summary>{workspace.deleted.map((note) => <div key={note.id}><span><strong>{noteTitle(note)}</strong><small>{note.id}</small></span><button className="secondary" onClick={() => void onMutate({ operation: 'restore', noteId: note.id })}>Restore</button></div>)}</details>
      ) : null}

      <details className="raw-panel">
        <summary>Raw document entries ({report.entries.length})</summary>
        <div className="entry-list">{report.entries.map((entry) => <EntryRow key={entry.keyBase64} entry={entry} />)}</div>
      </details>

      <section className="ticket-panel">
        <div><p className="eyebrow"><KeyRound /> Writable capability</p><h2>Workspace ticket</h2><p>Use this ticket to connect xo-syncd or another peer. Treat it as a secret.</p></div>
        <div className="ticket-actions"><button className="secondary" onClick={() => setTicketVisible((visible) => !visible)}>{ticketVisible ? 'Hide' : 'Reveal'} ticket</button><button className="secondary" onClick={() => void navigator.clipboard.writeText(report.ticket ?? '')}><Copy /> Copy</button></div>
        {ticketVisible ? <textarea className="ticket-output" readOnly value={report.ticket ?? ''} /> : null}
      </section>

      {editorOpen ? (
        <div className="editor-backdrop" role="presentation">
          <section className="note-editor" role="dialog" aria-modal="true" aria-labelledby="editor-title">
            <header><div><p className="eyebrow">{editingId ? `Edit ${editingId}` : 'Create note'}</p><h2 id="editor-title">Markdown editor</h2></div><button className="icon-button" onClick={() => setEditorOpen(false)} aria-label="Close editor"><X /></button></header>
            {error ? <div className="warning"><CircleAlert /><span>{error}</span></div> : null}
            {!editingId ? <label>Title<input autoFocus value={createTitle} onChange={(event) => setCreateTitle(event.target.value)} placeholder="Note title" /></label> : null}
            <label>Frontmatter and Markdown<textarea value={draft} onChange={(event) => setDraft(event.target.value)} spellCheck="true" /></label>
            <footer><span>Edits create immutable xo revisions and synchronize through Iroh.</span><div><button className="secondary" onClick={() => setEditorOpen(false)}>Cancel</button><button className="primary" disabled={busy || (!editingId && !createTitle.trim())} onClick={() => void saveDraft()}>{busy ? <LoaderCircle className="spin" /> : <Cloud />} Save note</button></div></footer>
          </section>
        </div>
      ) : null}
    </>
  );
}

function localTimestamp(milliseconds: number) {
  const instant = new Date(milliseconds);
  const offsetMinutes = -instant.getTimezoneOffset();
  const sign = offsetMinutes >= 0 ? '+' : '-';
  const absoluteOffset = Math.abs(offsetMinutes);
  const pad = (value: number, width = 2) => String(value).padStart(width, '0');
  return `${instant.getFullYear()}-${pad(instant.getMonth() + 1)}-${pad(instant.getDate())}`
    + `T${pad(instant.getHours())}:${pad(instant.getMinutes())}:${pad(instant.getSeconds())}`
    + `${sign}${pad(Math.floor(absoluteOffset / 60))}:${pad(absoluteOffset % 60)}`;
}

function noteField(note: WorkspaceNote, field?: string) {
  if (!field) return '';
  const value = note.frontmatter[field];
  return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ? String(value) : '';
}

function noteTitle(note: WorkspaceNote, field = 'title') {
  return noteField(note, field) || noteField(note, 'title') || 'Untitled';
}

function noteTags(note: WorkspaceNote) {
  const tags = note.frontmatter.tags;
  if (Array.isArray(tags)) return tags.filter((tag): tag is string => typeof tag === 'string');
  if (typeof tags === 'string') return tags.split(',').map((tag) => tag.trim()).filter(Boolean);
  return [];
}

function displayFrontmatter(value: unknown) {
  if (value === null) return 'null';
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

function EntryRow({ entry }: { entry: DocumentEntry }) {
  return <article className="entry-row"><div><strong>{entry.key}</strong><span>{entry.value ?? `${entry.contentLen} binary bytes`}</span></div><div className="entry-meta"><code>{short(entry.author)}</code><span className={entry.pending ? 'pending' : 'replicated'}>{entry.pending ? 'pending' : 'replicated'}</span></div></article>;
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="metric"><span>{label}</span><strong>{value}</strong></div>;
}

function NavItem({ icon, label, active = false, nested = false, onClick }: { icon: React.ReactNode; label: string; active?: boolean; nested?: boolean; onClick?: () => void }) {
  return <button className={`${active ? 'nav-item active' : 'nav-item'}${nested ? ' nested' : ''}`} disabled={!onClick} onClick={onClick}>{icon}<span>{label}</span>{!onClick && !active ? <small>soon</small> : null}</button>;
}

function RuntimeCard({ state, report, error }: { state: RuntimeState; report?: RuntimeReport; error: string }) {
  return <article className={`runtime-card ${state}`}><div className="runtime-header"><span className="window-dots"><i /><i /><i /></span><code>xo-runtime.worker</code></div><div className="runtime-body">{state === 'starting' ? <LoaderCircle className="spin" /> : state === 'ready' ? <Check /> : <CircleAlert />}<div><strong>{state === 'starting' ? 'Starting Iroh runtime…' : state === 'ready' ? 'Runtime ready' : 'Runtime unavailable'}</strong><p>{state === 'ready' ? `xo-web ${report?.runtime.version} · ${short(report?.status.endpointId)}` : state === 'error' ? error : 'Restoring encrypted identity and opening the relay'}</p></div></div><dl><div><dt>application server</dt><dd>none</dd></div><div><dt>persistence</dt><dd>{report?.indexedDb ? 'IndexedDB ready' : 'checking'}</dd></div><div><dt>Iroh transport</dt><dd>{report?.runtime.iroh ? 'relay-only E2EE' : 'starting'}</dd></div><div><dt>previous checkpoint</dt><dd>{report?.restoredAt ? 'restored' : 'new browser'}</dd></div></dl></article>;
}

function StatusCard({ icon, title, description, ready }: { icon: React.ReactNode; title: string; description: string; ready: boolean }) {
  return <article className="status-card"><div className={ready ? 'status-icon ready' : 'status-icon'}>{icon}</div><div><h3>{title}</h3><p>{description}</p></div><span className={ready ? 'status-label ready' : 'status-label'}>{ready ? 'ready' : 'starting'}</span></article>;
}

function short(value?: string) {
  if (!value) return 'not connected';
  return value.length > 18 ? `${value.slice(0, 9)}…${value.slice(-7)}` : value;
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

createRoot(document.getElementById('root')!).render(<React.StrictMode><App /></React.StrictMode>);
