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
import type { RuntimeReport, RuntimeState } from './protocol';
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
      if (setupTicket) {
        next = await runtime.joinWorkspace(setupTicket);
        scannedWorkspaceTicket = undefined;
        clearWorkspaceTicket();
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
      if (!ticket || !runtime) return;
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
        onQuery={(input) => runtimeRef.current?.queryNotes(input) ?? Promise.resolve([])}
        onMutate={(input) => runWorkspace((runtime) => runtime.mutateNote(input))}
        onRefresh={() => void runWorkspace((runtime) => runtime.refreshSync())}
        onUpdate={() => void applyUpdate()}
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
            <button onClick={() => void applyUpdate()}><RefreshCw /> Update</button>
          </div>
        ) : null}

        <main>
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
