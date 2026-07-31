import React, { useEffect, useState } from 'react';
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
  LoaderCircle,
  LockKeyhole,
  Menu,
  NotebookPen,
  Search,
  Settings,
  Sparkles,
  WifiOff,
  X,
} from 'lucide-react';
import { registerSW } from 'virtual:pwa-register';
import type { RuntimeReport, RuntimeState } from './protocol';
import { XoRuntime } from './runtime';
import './styles.css';

type InstallPrompt = Event & {
  prompt: () => Promise<void>;
  userChoice: Promise<{ outcome: 'accepted' | 'dismissed' }>;
};

const updateServiceWorker = registerSW({ immediate: true });

function App() {
  const [state, setState] = useState<RuntimeState>('starting');
  const [report, setReport] = useState<RuntimeReport>();
  const [error, setError] = useState('');
  const [menuOpen, setMenuOpen] = useState(false);
  const [online, setOnline] = useState(navigator.onLine);
  const [installPrompt, setInstallPrompt] = useState<InstallPrompt>();

  useEffect(() => {
    const runtime = new XoRuntime();
    let active = true;
    void runtime.initialize().then(
      (next) => {
        if (!active) return;
        setReport(next);
        setState('ready');
      },
      (cause: unknown) => {
        if (!active) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setState('error');
      },
    );
    return () => {
      active = false;
      runtime.terminate();
    };
  }, []);

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

  return (
    <div className="app-shell">
      <aside className={menuOpen ? 'sidebar open' : 'sidebar'}>
        <div className="brand">
          <img src="/logo.svg" alt="" />
          <div><strong>xo</strong><span>private workspace</span></div>
          <button className="icon-button close-menu" onClick={() => setMenuOpen(false)} aria-label="Close navigation"><X /></button>
        </div>
        <nav aria-label="Workspace">
          <NavItem icon={<NotebookPen />} label="Notes" active />
          <NavItem icon={<BookOpen />} label="Books" />
          <NavItem icon={<Inbox />} label="Inbox" />
        </nav>
        <div className="sidebar-spacer" />
        <nav aria-label="Application">
          <NavItem icon={<Boxes />} label="Steel plugins" />
          <NavItem icon={<Settings />} label="Settings" />
        </nav>
        <div className="privacy-note"><LockKeyhole /><span>Your workspace stays in this browser until you connect a peer.</span></div>
      </aside>
      {menuOpen ? <button className="scrim" onClick={() => setMenuOpen(false)} aria-label="Close navigation" /> : null}

      <div className="workspace">
        <header className="topbar">
          <button className="icon-button menu-button" onClick={() => setMenuOpen(true)} aria-label="Open navigation"><Menu /></button>
          <div className="search"><Search /><span>Search your workspace</span><kbd>⌘ K</kbd></div>
          <div className={online ? 'connection online' : 'connection offline'}>
            {online ? <Cloud /> : <CloudOff />}
            <span>{online ? 'browser online' : 'offline'}</span>
          </div>
        </header>

        <main>
          <section className="hero">
            <div>
              <p className="eyebrow"><Sparkles /> xo web foundation</p>
              <h1>Your knowledge,<br /><em>entirely client-side.</em></h1>
              <p className="lede">A static, installable workspace powered by Rust, WebAssembly, IndexedDB, and sandboxed Steel. No application server required.</p>
              <div className="hero-actions">
                {installPrompt ? <button className="primary" onClick={() => void install()}><Download /> Install xo</button> : null}
                <button className="secondary" onClick={() => void updateServiceWorker(true)}>Check for updates</button>
              </div>
            </div>
            <RuntimeCard state={state} report={report} error={error} />
          </section>

          <section className="status-section" aria-labelledby="foundation-title">
            <div className="section-heading">
              <div><p className="eyebrow">Phase 0</p><h2 id="foundation-title">Browser foundation</h2></div>
              <span className="static-badge">static assets only</span>
            </div>
            <div className="status-grid">
              <StatusCard icon={<Code2 />} title="Rust + WebAssembly" description="A versioned xo-web facade loads inside the dedicated worker." ready={state === 'ready'} />
              <StatusCard icon={<Database />} title="Durable browser state" description="IndexedDB is opened and checkpointed away from the UI thread." ready={Boolean(report?.indexedDb)} />
              <StatusCard icon={<Sparkles />} title="Sandboxed Steel" description={`A fresh Steel VM executes in Wasm${report ? ` and returned ${report.steelResult}` : ''}.`} ready={Boolean(report?.runtime.steel)} />
              <StatusCard icon={<WifiOff />} title="Iroh synchronization" description="Relay connectivity and restart recovery are the next feasibility milestone." ready={Boolean(report?.runtime.iroh)} pending />
            </div>
          </section>

          <section className="next-step">
            <div className="step-number">01</div>
            <div><p className="eyebrow">Next milestone</p><h2>Join the same workspace as xo-syncd</h2><p>Browser-safe Iroh features, identity recovery, and an offline mutation will land behind this worker boundary without introducing an application API.</p></div>
            <div className="architecture"><span>React</span><b>→</b><span>Worker</span><b>→</b><span>Rust / Wasm</span><b>→</b><span>Iroh</span></div>
          </section>
        </main>
      </div>
    </div>
  );
}

function NavItem({ icon, label, active = false }: { icon: React.ReactNode; label: string; active?: boolean }) {
  return <button className={active ? 'nav-item active' : 'nav-item'} disabled={!active}>{icon}<span>{label}</span>{!active ? <small>soon</small> : null}</button>;
}

function RuntimeCard({ state, report, error }: { state: RuntimeState; report?: RuntimeReport; error: string }) {
  return (
    <article className={`runtime-card ${state}`}>
      <div className="runtime-header">
        <span className="window-dots"><i /><i /><i /></span>
        <code>xo-runtime.worker</code>
      </div>
      <div className="runtime-body">
        {state === 'starting' ? <LoaderCircle className="spin" /> : state === 'ready' ? <Check /> : <CircleAlert />}
        <div>
          <strong>{state === 'starting' ? 'Starting private runtime…' : state === 'ready' ? 'Runtime ready' : 'Runtime unavailable'}</strong>
          <p>{state === 'ready' ? `xo-web ${report?.runtime.crate_version} · API ${report?.runtime.api_version}` : state === 'error' ? error : 'Loading static Wasm and restoring IndexedDB'}</p>
        </div>
      </div>
      <dl>
        <div><dt>application server</dt><dd>none</dd></div>
        <div><dt>persistence</dt><dd>{report?.indexedDb ? 'IndexedDB ready' : 'checking'}</dd></div>
        <div><dt>Steel isolation</dt><dd>{report?.runtime.steel ? 'worker sandbox' : 'checking'}</dd></div>
        <div><dt>previous checkpoint</dt><dd>{report?.restoredAt ? 'restored' : 'new browser'}</dd></div>
      </dl>
    </article>
  );
}

function StatusCard({ icon, title, description, ready, pending = false }: { icon: React.ReactNode; title: string; description: string; ready: boolean; pending?: boolean }) {
  return (
    <article className="status-card">
      <div className={ready ? 'status-icon ready' : pending ? 'status-icon pending' : 'status-icon'}>{icon}</div>
      <div><h3>{title}</h3><p>{description}</p></div>
      <span className={ready ? 'status-label ready' : 'status-label'}>{ready ? 'ready' : 'next'}</span>
    </article>
  );
}

createRoot(document.getElementById('root')!).render(<React.StrictMode><App /></React.StrictMode>);
