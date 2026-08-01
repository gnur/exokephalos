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
import type { DocumentEntry, RuntimeReport, RuntimeState } from './protocol';
import { XoRuntime } from './runtime';
import './styles.css';

type InstallPrompt = Event & {
  prompt: () => Promise<void>;
  userChoice: Promise<{ outcome: 'accepted' | 'dismissed' }>;
};

const updateServiceWorker = registerSW({ immediate: true });
let scannedWorkspaceTicket = consumeWorkspaceTicket();

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
        if (active) setReport(next);
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
    const onOnline = () => setOnline(true);
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

  async function install() {
    if (!installPrompt) return;
    await installPrompt.prompt();
    await installPrompt.userChoice;
    setInstallPrompt(undefined);
  }

  async function runWorkspace(operation: (runtime: XoRuntime) => Promise<RuntimeReport>) {
    const runtime = runtimeRef.current;
    if (!runtime) return;
    setBusy(true);
    setError('');
    try {
      setReport(await operation(runtime));
    } catch (cause) {
      setError(errorMessage(cause));
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
          <NavItem icon={<NotebookPen />} label="Document" active />
          <NavItem icon={<BookOpen />} label="Books" />
          <NavItem icon={<Inbox />} label="Inbox" />
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
          <div className="search"><Search /><span>{hasWorkspace ? 'Search arrives with note projections' : 'Connect a workspace to begin'}</span><kbd>xo</kbd></div>
          <div className={online ? 'connection online' : 'connection offline'}>
            {online ? <Cloud /> : <CloudOff />}
            <span>{online ? (hasWorkspace ? 'relay sync active' : 'browser online') : 'offline'}</span>
          </div>
        </header>

        <main>
          {hasWorkspace && report ? (
            <WorkspaceView
              report={report}
              busy={busy}
              error={error}
              onRefresh={() => void runWorkspace((runtime) => runtime.refreshSync())}
              onPut={(key, value) => void runWorkspace((runtime) => runtime.putEntry({ key, value }))}
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
            />
          )}
        </main>
      </div>
    </div>
  );
}

function Onboarding({ state, report, error, busy, ticket, onTicket, onCreate, onJoin, installPrompt, onInstall }: {
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
            {installPrompt ? <button className="secondary" onClick={onInstall}><Download /> Install xo</button> : <button className="secondary" onClick={() => void updateServiceWorker(true)}>Check for updates</button>}
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

function WorkspaceView({ report, busy, error, onRefresh, onPut }: {
  report: RuntimeReport;
  busy: boolean;
  error: string;
  onRefresh: () => void;
  onPut: (key: string, value: string) => void;
}) {
  const [key, setKey] = useState('web/demo');
  const [value, setValue] = useState('Hello from xo-web');
  const [ticketVisible, setTicketVisible] = useState(false);
  const statusMessage = error || report.syncError;

  return (
    <>
      <section className="workspace-heading">
        <div><p className="eyebrow"><Radio /> Iroh document connected</p><h1>Workspace <em>online.</em></h1><p className="workspace-id">{report.status.workspaceId}</p></div>
        <button className="secondary" disabled={busy} onClick={onRefresh}><RefreshCw className={busy ? 'spin' : ''} /> Refresh sync</button>
      </section>

      {statusMessage ? <div className="warning"><CloudOff /> <span>{statusMessage}. Cached entries and pending writes remain available offline.</span></div> : null}
      <section className="sync-grid">
        <Metric label="Endpoint" value={short(report.status.endpointId)} />
        <Metric label="Author" value={short(report.status.authorId)} />
        <Metric label="Sync peers" value={String(report.status.peers)} />
        <Metric label="Pending writes" value={String(report.pendingWrites)} />
      </section>

      <section className="document-layout">
        <div className="entries-panel">
          <div className="panel-heading"><div><p className="eyebrow">Document snapshot</p><h2>Document entries</h2></div><span>{report.entries.length}</span></div>
          <div className="entry-list">
            {report.entries.length ? report.entries.map((entry) => <EntryRow key={entry.keyBase64} entry={entry} />) : <div className="empty-state">No entries yet. Publish the first one from this browser.</div>}
          </div>
        </div>
        <aside className="write-panel">
          <p className="eyebrow"><Plus /> Offline-capable write</p>
          <h2>Publish an entry</h2>
          <label>Key<input value={key} onChange={(event) => setKey(event.target.value)} /></label>
          <label>UTF-8 value<textarea value={value} onChange={(event) => setValue(event.target.value)} /></label>
          <button className="primary" disabled={busy || !key.trim()} onClick={() => onPut(key, value)}>{busy ? <LoaderCircle className="spin" /> : <Cloud />} Commit to Iroh Docs</button>
        </aside>
      </section>

      <section className="ticket-panel">
        <div><p className="eyebrow"><KeyRound /> Writable capability</p><h2>Workspace ticket</h2><p>Use this ticket to connect xo-syncd or another peer. Treat it as a secret.</p></div>
        <div className="ticket-actions"><button className="secondary" onClick={() => setTicketVisible((visible) => !visible)}>{ticketVisible ? 'Hide' : 'Reveal'} ticket</button><button className="secondary" onClick={() => void navigator.clipboard.writeText(report.ticket ?? '')}><Copy /> Copy</button></div>
        {ticketVisible ? <textarea className="ticket-output" readOnly value={report.ticket ?? ''} /> : null}
      </section>
    </>
  );
}

function EntryRow({ entry }: { entry: DocumentEntry }) {
  return <article className="entry-row"><div><strong>{entry.key}</strong><span>{entry.value ?? `${entry.contentLen} binary bytes`}</span></div><div className="entry-meta"><code>{short(entry.author)}</code><span className={entry.pending ? 'pending' : 'replicated'}>{entry.pending ? 'pending' : 'replicated'}</span></div></article>;
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="metric"><span>{label}</span><strong>{value}</strong></div>;
}

function NavItem({ icon, label, active = false }: { icon: React.ReactNode; label: string; active?: boolean }) {
  return <button className={active ? 'nav-item active' : 'nav-item'} disabled={!active}>{icon}<span>{label}</span>{!active ? <small>soon</small> : null}</button>;
}

function RuntimeCard({ state, report, error }: { state: RuntimeState; report?: RuntimeReport; error: string }) {
  return <article className={`runtime-card ${state}`}><div className="runtime-header"><span className="window-dots"><i /><i /><i /></span><code>xo-runtime.worker</code></div><div className="runtime-body">{state === 'starting' ? <LoaderCircle className="spin" /> : state === 'ready' ? <Check /> : <CircleAlert />}<div><strong>{state === 'starting' ? 'Starting Iroh runtime…' : state === 'ready' ? 'Runtime ready' : 'Runtime unavailable'}</strong><p>{state === 'ready' ? `xo-web ${report?.runtime.crate_version} · ${short(report?.status.endpointId)}` : state === 'error' ? error : 'Restoring encrypted identity and opening the relay'}</p></div></div><dl><div><dt>application server</dt><dd>none</dd></div><div><dt>persistence</dt><dd>{report?.indexedDb ? 'IndexedDB ready' : 'checking'}</dd></div><div><dt>Iroh transport</dt><dd>{report?.runtime.iroh ? 'relay-only E2EE' : 'starting'}</dd></div><div><dt>previous checkpoint</dt><dd>{report?.restoredAt ? 'restored' : 'new browser'}</dd></div></dl></article>;
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
