import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import {
  BookOpen,
  Boxes,
  Check,
  CircleAlert,
  Cloud,
  CloudOff,
  Code2,
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
import type { NoteQueryInput, RuntimeReport, RuntimeState } from './protocol';
import { XoRuntime } from './runtime';
import { WorkspaceExperience } from './workspace-ui';
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
let scannedWorkspaceTicket = workspaceTicketFromLocation();

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

async function applyUpdate() {
  try {
    await serviceWorkerRegistration?.update();
    await updateServiceWorker(true);
  } finally {
    window.location.reload();
  }
}

function workspaceRouteState() {
  const parts = window.location.pathname.split('/').filter(Boolean);
  const parameters = new URLSearchParams(window.location.search);
  return {
    view: parts[0] === 'views' && parts[1] ? decodeURIComponent(parts[1]) : '',
    subview: parameters.get('subview') || undefined,
    search: parameters.get('q') || '',
  };
}

function workspaceTicketFromLocation() {
  const parameters = new URLSearchParams(window.location.hash.replace(/^#/, ''));
  return parameters.get('ticket')?.trim() || undefined;
}

function clearWorkspaceTicket() {
  window.history.replaceState(null, '', `${window.location.pathname}${window.location.search}`);
}

const PEER_ADJECTIVES = ['smart', 'clever', 'funny', 'incredible', 'blue', 'green'] as const;
const PEER_SUBJECTS = ['xo', 'exokephalos', 'zettelkasten', 'sandbox', 'browser', 'client'] as const;

function randomPeerId() {
  const random = new Uint32Array(2);
  crypto.getRandomValues(random);
  return `${PEER_ADJECTIVES[random[0] % PEER_ADJECTIVES.length]}-${PEER_SUBJECTS[random[1] % PEER_SUBJECTS.length]}`;
}

function reportRevision(report?: RuntimeReport) {
  if (!report) return '';
  return JSON.stringify({
    entries: report.entries.map((entry) => [entry.keyBase64, entry.contentHash, entry.pending]),
    pendingWrites: report.pendingWrites,
    pendingApproval: report.status.pendingApproval,
    peers: report.status.peers,
    syncError: report.syncError,
    members: report.members,
    pendingMembers: report.pendingMembers,
  });
}

function App() {
  const runtimeRef = useRef<XoRuntime | undefined>(undefined);
  const initialRoute = useRef(workspaceRouteState());
  const [state, setState] = useState<RuntimeState>('starting');
  const [report, setReport] = useState<RuntimeReport>();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [online, setOnline] = useState(navigator.onLine);
  const [installPrompt, setInstallPrompt] = useState<InstallPrompt>();
  const [ticketInput, setTicketInput] = useState('');
  const [peerIdInput, setPeerIdInput] = useState(randomPeerId);
  const [updateAvailable, setUpdateAvailable] = useState(updateIsAvailable);
  const [activeView, setActiveView] = useState(initialRoute.current.view);
  const [activeSubview, setActiveSubview] = useState<string | undefined>(initialRoute.current.subview);
  const [search, setSearch] = useState(initialRoute.current.search);

  useEffect(() => {
    const runtime = new XoRuntime();
    runtimeRef.current = runtime;
    let active = true;
    void (async () => {
      const setupTicket = scannedWorkspaceTicket;
      let next = await runtime.initialize();
      if (setupTicket && next.peerId) {
        next = await runtime.joinWorkspace(setupTicket);
        scannedWorkspaceTicket = undefined;
        clearWorkspaceTicket();
      } else if (setupTicket) {
        setTicketInput(setupTicket);
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
    const joinFromLocation = () => {
      const ticket = workspaceTicketFromLocation();
      const runtime = runtimeRef.current;
      if (!ticket || !runtime || !report?.peerId) return;
      setBusy(true);
      void runtime.joinWorkspace(ticket).then((next) => {
        clearWorkspaceTicket();
        setReport(next);
        setState('ready');
        setError(next.syncError ?? '');
      }).catch((cause: unknown) => {
        setError(errorMessage(cause));
      }).finally(() => setBusy(false));
    };
    window.addEventListener('hashchange', joinFromLocation);
    return () => window.removeEventListener('hashchange', joinFromLocation);
  }, [report?.peerId]);

  useEffect(() => {
    if (state !== 'ready' || (!report?.status.workspaceId && !report?.status.pendingApproval)) return;
    let active = true;
    let running = false;
    const refresh = async () => {
      if (running || !runtimeRef.current) return;
      running = true;
      try {
        const next = await runtimeRef.current.refreshSync();
        if (active) {
          setReport((current) => reportRevision(current) === reportRevision(next) ? current : next);
          if (!next.syncError) setError('');
        }
      } catch (cause) {
        if (active) setError(errorMessage(cause));
      } finally {
        running = false;
      }
    };
    // Admission is polled promptly; routine synchronization is deliberately
    // less frequent so it does not monopolize the serialized Wasm worker.
    const interval = report.status.pendingApproval ? 2_000 : 15_000;
    const timer = window.setInterval(() => void refresh(), interval);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [state, report?.status.workspaceId, report?.status.pendingApproval]);

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

  async function wipeBrowserData() {
    const confirmed = window.confirm(
      'Permanently remove this browser identity, workspace capability, cached notes, pending writes, and offline app files? You will need a new invitation to reconnect.',
    );
    if (!confirmed) return;
    const runtime = runtimeRef.current;
    if (!runtime) return;
    setBusy(true);
    setError('');
    try {
      await runtime.wipeLocalData();
      runtime.terminate();
      runtimeRef.current = undefined;
      localStorage.clear();
      sessionStorage.clear();
      if ('caches' in window) {
        await Promise.all((await caches.keys()).map((name) => caches.delete(name)));
      }
      if ('serviceWorker' in navigator) {
        await Promise.all((await navigator.serviceWorker.getRegistrations()).map((registration) => registration.unregister()));
      }
      window.location.replace('/');
    } catch (cause) {
      setError(errorMessage(cause));
      setBusy(false);
    }
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

  const queryNotes = useCallback(
    (input: NoteQueryInput) => runtimeRef.current?.queryNotes(input) ?? Promise.resolve([]),
    [],
  );

  const hasWorkspace = Boolean(report?.status.workspaceId);
  if (hasWorkspace && report) {
    return (
      <WorkspaceExperience
        report={report}
        busy={busy}
        error={error}
        activeView={activeView}
        activeSubview={activeSubview}
        search={search}
        updateAvailable={updateAvailable}
        onView={(view) => {
          setActiveView(view);
          setActiveSubview(undefined);
        }}
        onSubview={setActiveSubview}
        onSearch={setSearch}
        onQuery={queryNotes}
        onMutate={(input) => runWorkspace((runtime) => runtime.mutateNote(input))}
        onRefresh={() => void runWorkspace((runtime) => runtime.refreshSync())}
        onUpdate={() => void applyUpdate()}
        onApprovePeer={(fingerprint) => void runWorkspace((runtime) => runtime.approvePeer(fingerprint))}
        onRejectPeer={(fingerprint) => void runWorkspace((runtime) => runtime.rejectPeer(fingerprint))}
        onRemovePeer={(fingerprint) => void runWorkspace((runtime) => runtime.removePeer(fingerprint))}
        onWipe={() => void wipeBrowserData()}
      />
    );
  }
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
        <div className="privacy-note"><LockKeyhole /><span>Endpoint and membership identities plus the invitation are encrypted locally in IndexedDB.</span></div>
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
            <button onClick={() => void applyUpdate()}><RefreshCw /> Update</button>
          </div>
        ) : null}

        <main>
          <Onboarding
            state={state}
            report={report}
            error={error}
            busy={busy}
            peerId={peerIdInput}
            onPeerId={setPeerIdInput}
            ticket={ticketInput}
            onTicket={setTicketInput}
            onCreate={() => void runWorkspace(async (runtime) => {
              await runtime.setPeerId(report?.peerId ?? peerIdInput);
              return runtime.createWorkspace();
            })}
            onJoin={() => void runWorkspace(async (runtime) => {
              await runtime.setPeerId(report?.peerId ?? peerIdInput);
              return runtime.joinWorkspace(ticketInput);
            })}
            onRetryApproval={() => void runWorkspace((runtime) => runtime.refreshSync())}
            installPrompt={installPrompt}
            onInstall={() => void install()}
            onCheckForUpdates={() => void checkForUpdates()}
          />
        </main>
        <footer className="app-footer">xo {APP_VERSION}</footer>
      </div>
    </div>
  );
}

function Onboarding({ state, report, error, busy, peerId, onPeerId, ticket, onTicket, onCreate, onJoin, onRetryApproval, installPrompt, onInstall, onCheckForUpdates }: {
  state: RuntimeState;
  report?: RuntimeReport;
  error: string;
  busy: boolean;
  peerId: string;
  onPeerId: (peerId: string) => void;
  ticket: string;
  onTicket: (ticket: string) => void;
  onCreate: () => void;
  onJoin: () => void;
  onRetryApproval: () => void;
  installPrompt?: InstallPrompt;
  onInstall: () => void;
  onCheckForUpdates: () => void;
}) {
  if (state === 'starting') {
    return (
      <section className="hero" aria-live="polite">
        <div>
          <p className="eyebrow"><LoaderCircle className="spin" /> restoring local state</p>
          <h1>Opening your<br /><em>browser workspace.</em></h1>
          <p className="lede">Restoring the encrypted identity and Automerge replica from IndexedDB before connecting to peers.</p>
        </div>
        <RuntimeCard state={state} report={report} error={error} />
      </section>
    );
  }
  return (
    <>
      <section className="hero">
        <div>
          <p className="eyebrow"><Sparkles /> direct browser Iroh</p>
          <h1>Your knowledge,<br /><em>entirely client-side.</em></h1>
          <p className="lede">Create or join an authenticated Automerge workspace. Iroh QUIC, Gossip, Steel, and recovery run in this browser worker.</p>
          {!report?.peerId ? <label className="ticket-form"><span>Peer ID (required)</span><small>A random client name has been generated for this browser. You can change it before creating or joining a workspace.</small><input value={peerId} onChange={(event) => onPeerId(event.target.value)} placeholder="smart-browser" aria-label="Peer ID" required /></label> : <p>Peer ID: <strong>{report.peerId}</strong></p>}
          {report?.syncError?.includes('pending approval') ? <p className="error-message">This peer is waiting for its signed automatic admission.</p> : null}
          <div className="hero-actions">
            {report?.syncError?.includes('pending approval') ? <button className="primary" disabled={busy} onClick={onRetryApproval}><RefreshCw className={busy ? 'spin' : ''} /> Check admission</button> : null}
            <button className="primary" disabled={busy || state !== 'ready' || (!report?.peerId && !peerId.trim())} onClick={onCreate}>{busy ? <LoaderCircle className="spin" /> : <Plus />} Create workspace</button>
            {installPrompt ? <button className="secondary" onClick={onInstall}><Download /> Install xo</button> : <button className="secondary" onClick={onCheckForUpdates}>Check for updates</button>}
          </div>
        </div>
        <RuntimeCard state={state} report={report} error={error} />
      </section>

      <section className="join-section">
        <div><p className="eyebrow"><KeyRound /> Existing workspace</p><h2>Join with an invitation</h2><p><strong>A Peer ID is required before joining.</strong> Enter a unique name for this browser in the Peer ID field above, then paste the invitation. Invitations stay encrypted in this browser.</p></div>
        <div className="ticket-form">
          <textarea value={ticket} onChange={(event) => onTicket(event.target.value)} placeholder="Paste the Automerge workspace invitation from xo or xo-syncd" aria-label="Workspace invitation" />
          <button className="primary" disabled={busy || !ticket.trim() || state !== 'ready' || (!report?.peerId && !peerId.trim())} onClick={onJoin}>{busy ? <LoaderCircle className="spin" /> : <Radio />} Join and synchronize</button>
        </div>
      </section>

      <section className="status-section" aria-labelledby="foundation-title">
        <div className="section-heading"><div><p className="eyebrow">Browser runtime</p><h2 id="foundation-title">No application server</h2></div><span className="static-badge">static assets only</span></div>
        <div className="status-grid">
          <StatusCard icon={<Code2 />} title="Rust + WebAssembly" description="The xo-web facade runs only inside the dedicated worker." ready={state === 'ready'} />
          <StatusCard icon={<Database />} title="Encrypted recovery" description="Identity, capability, records, and pending writes survive in IndexedDB." ready={Boolean(report?.indexedDb)} />
          <StatusCard icon={<Sparkles />} title="Sandboxed Steel" description={`A fresh Steel VM executes in Wasm${report ? ` and returned ${report.steelResult}` : ''}.`} ready={Boolean(report?.runtime.steel)} />
          <StatusCard icon={<WifiOff />} title="Automerge + Iroh" description="Signed Automerge changes synchronize over authenticated, end-to-end encrypted Iroh QUIC." ready={Boolean(report?.runtime.iroh)} />
        </div>
      </section>
    </>
  );
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
