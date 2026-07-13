import React, { useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { useLiveQuery } from 'dexie-react-hooks';
import DOMPurify from 'dompurify';
import { marked } from 'marked';
import { Check, Cloud, CloudOff, Edit3, Menu, Plus, RefreshCw, Search, Settings, Trash2, X } from 'lucide-react';
import { parse as parseYAML, stringify as stringifyYAML } from 'yaml';
import { approveSyncClient, changePassword, importURL, listSyncClients, revokeSyncClient } from './api';
import { db } from './db';
import { refreshFromServer, startSyncRuntime, syncOutbox } from './sync';
import type { Frontmatter, Item, OutboxEntry, SyncClient, View } from './types';
import './styles.css';

type Screen = 'items' | 'create' | 'outbox' | 'settings';
type Pane = 'tags' | 'items' | 'editor';

function itemTitle(item: Item) {
  return item.title || String(item.frontmatter.title ?? item.id);
}

function newID() {
  const alphabet = 'abcdefghijklmnopqrstuvwxyz234567';
  const epoch = Date.UTC(1989, 0, 17);
  const days = Math.max(0, Math.floor((Date.now() - epoch) / 86_400_000));
  const bytes = crypto.getRandomValues(new Uint8Array(4));
  const encodedDays = encodeBase32(days, alphabet);
  const random = Array.from(bytes, (byte) => alphabet[byte % alphabet.length]).join('');
  return `${encodedDays}${random}`.padStart(7, '0');
}

function encodeBase32(value: number, alphabet: string) {
  if (value === 0) return 'a';
  let result = '';
  let n = value;
  while (n > 0) {
    result = alphabet[n % 32] + result;
    n = Math.floor(n / 32);
  }
  return result;
}

function slugify(value: string) {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '')
    .slice(0, 50) || 'untitled';
}

function rawFromParts(frontmatter: Frontmatter, body: string) {
  return `---\n${stringifyYAML(frontmatter).trimEnd()}\n---\n${body}`;
}

function parseRaw(raw: string): { frontmatter: Frontmatter; body: string } {
  if (!raw.startsWith('---')) {
    return { frontmatter: {}, body: raw };
  }
  const end = raw.indexOf('\n---', 3);
  if (end === -1) {
    return { frontmatter: {}, body: raw };
  }
  const frontmatterRaw = raw.slice(3, end).trim();
  const body = raw.slice(end + 4).replace(/^\r?\n/, '');
  const parsed = parseYAML(frontmatterRaw);
  const frontmatter = parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed as Frontmatter : {};
  return { frontmatter, body };
}

function routeFromState(viewID: string, itemID: string | undefined, pane: Pane, query: string, subviewName: string) {
  const path = viewID ? `/views/${encodeURIComponent(viewID)}${itemID ? `/${encodeURIComponent(itemID)}` : ''}` : '/';
  const params = new URLSearchParams();
  if (pane !== 'items') params.set('pane', pane);
  if (query.trim()) params.set('q', query.trim());
  if (subviewName) params.set('subview', subviewName);
  const qs = params.toString();
  return qs ? `${path}?${qs}` : path;
}

function stateFromLocation(): { viewID: string; itemID?: string; pane: Pane; query: string; subviewName: string } {
  const parts = window.location.pathname.split('/').filter(Boolean);
  const params = new URLSearchParams(window.location.search);
  const paneParam = params.get('pane');
  return {
    viewID: parts[0] === 'views' ? decodeURIComponent(parts[1] ?? '') : '',
    itemID: parts[0] === 'views' && parts[2] ? decodeURIComponent(parts[2]) : undefined,
    pane: paneParam === 'tags' || paneParam === 'editor' ? paneParam : 'items',
    query: params.get('q') ?? '',
    subviewName: params.get('subview') ?? '',
  };
}

async function enqueue(entry: Omit<OutboxEntry, 'status' | 'attempts' | 'created_at' | 'updated_at'>) {
  const now = new Date().toISOString();
  await db.outbox.put({ ...entry, status: 'pending', attempts: 0, created_at: now, updated_at: now });
  void syncOutbox();
}

function App() {
  const initialRoute = useMemo(() => stateFromLocation(), []);
  const [screen, setScreen] = useState<Screen>('items');
  const [selectedID, setSelectedID] = useState<string | undefined>(initialRoute.itemID);
  const [pane, setPane] = useState<Pane>(initialRoute.pane);
  const [menuOpen, setMenuOpen] = useState(false);
  const [query, setQuery] = useState(initialRoute.query);
  const [viewID, setViewID] = useState(initialRoute.viewID);
  const [subviewName, setSubviewName] = useState(initialRoute.subviewName);
  const [syncStatus, setSyncStatus] = useState<'online' | 'offline' | 'syncing'>(navigator.onLine ? 'online' : 'offline');
  const items = useLiveQuery(() => db.items.filter((item) => !item.deleted).toArray(), [], []);
  const views = useLiveQuery(() => db.views.toArray(), [], []);
  const outbox = useLiveQuery(() => db.outbox.orderBy('created_at').reverse().toArray(), [], []);
  const selected = items.find((item) => item.id === selectedID);

  useEffect(() => startSyncRuntime(setSyncStatus), []);
  useEffect(() => {
    const onPop = () => {
      const next = stateFromLocation();
      setViewID(next.viewID);
      setSelectedID(next.itemID);
      setPane(next.pane);
      setQuery(next.query);
      setSubviewName(next.subviewName);
      setScreen('items');
    };
    window.addEventListener('popstate', onPop);
    return () => window.removeEventListener('popstate', onPop);
  }, []);
  useEffect(() => {
    if (screen !== 'items') return;
    const next = routeFromState(viewID || views[0]?.id || '', selectedID, pane, query, subviewName);
    if (next !== window.location.pathname + window.location.search) {
      window.history.replaceState(null, '', next);
    }
  }, [screen, viewID, selectedID, pane, query, subviewName, views]);

  return (
    <div className="app-shell">
      <header className="topbar">
        {syncStatus !== 'online' ? (
          <div className="sync-warning">
            <CloudOff size={15} />
            sync offline
          </div>
        ) : null}
        <div>
          <p className="eyebrow">exokephalos</p>
          <h1>{labelForScreen(screen)}</h1>
        </div>
        <button className="icon-button menu-trigger" onClick={() => setMenuOpen((open) => !open)} aria-label="Menu">
          {menuOpen ? <X size={20} /> : <Menu size={20} />}
        </button>
        {menuOpen ? (
          <AppMenu
            views={views}
            activeViewID={viewID}
            activeSubviewName={subviewName}
            syncCount={outbox.filter((entry) => entry.status !== 'synced').length}
            onRefresh={() => void refreshFromServer()}
            onView={(nextViewID, nextSubviewName = '') => {
              setViewID(nextViewID);
              setSubviewName(nextSubviewName);
              setScreen('items');
              setMenuOpen(false);
            }}
            onScreen={(next) => {
              setScreen(next);
              setMenuOpen(false);
            }}
          />
        ) : null}
      </header>

      <main className="content">
        {screen === 'items' && (
          <ItemsView
            items={items}
            views={views}
            selected={selected}
            onSelect={(id) => {
              setSelectedID(id);
              setPane('editor');
              window.history.pushState(null, '', routeFromState(viewID || views[0]?.id || '', id, 'editor', query, subviewName));
            }}
            viewID={viewID}
            subviewName={subviewName}
            query={query}
            pane={pane}
            onPane={setPane}
            onView={(nextViewID, nextSubviewName = '') => {
              setViewID(nextViewID);
              setSubviewName(nextSubviewName);
              setSelectedID(undefined);
              setPane('items');
              window.history.pushState(null, '', routeFromState(nextViewID, undefined, 'items', query, nextSubviewName));
            }}
          />
        )}
        {screen === 'create' && <CreateView views={views} onCreated={(id) => { setSelectedID(id); setScreen('items'); }} />}
        {screen === 'outbox' && <OutboxView entries={outbox} />}
        {screen === 'settings' && <SettingsView />}
      </main>

      <div className="bottom-search" role="search">
        <Search size={19} />
        <input value={query} onChange={(event) => { setQuery(event.target.value); setScreen('items'); setPane('items'); }} placeholder="Search" />
        <button className="new-button" onClick={() => setScreen('create')} aria-label="New item">
          <Plus size={24} />
        </button>
      </div>
    </div>
  );
}

function labelForScreen(screen: Screen) {
  switch (screen) {
    case 'create': return 'Create';
    case 'outbox': return 'Sync outbox';
    case 'settings': return 'Settings';
    default: return 'Items';
  }
}

function AppMenu({ views, activeViewID, activeSubviewName, syncCount, onRefresh, onView, onScreen }: {
  views: View[];
  activeViewID: string;
  activeSubviewName: string;
  syncCount: number;
  onRefresh: () => void;
  onView: (viewID: string, subviewName?: string) => void;
  onScreen: (screen: Screen) => void;
}) {
  const activeView = views.find((view) => view.id === activeViewID) ?? views[0];
  return (
    <div className="menu-panel">
      <div className="menu-actions">
        <button className="button" onClick={onRefresh}><RefreshCw size={17} /> Refresh</button>
        <button className="button" onClick={() => onScreen('outbox')}><Cloud size={17} /> Sync{syncCount ? ` (${syncCount})` : ''}</button>
        <button className="button" onClick={() => onScreen('settings')}><Settings size={17} /> Settings</button>
      </div>
      <div className="menu-section">
        {views.map((view) => (
          <button key={view.id} className={(activeView?.id === view.id && !activeSubviewName) ? 'menu-item active' : 'menu-item'} onClick={() => onView(view.id)}>
            {view.config.name || view.id}
          </button>
        ))}
      </div>
      {activeView?.subviews?.length ? (
        <div className="menu-section subviews">
          {activeView.subviews.map((subview) => (
            <button key={subview.name} className={activeSubviewName === subview.name ? 'menu-item active' : 'menu-item'} onClick={() => onView(activeView.id, subview.name)}>
              {subview.name}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function ItemsView({ items, views, selected, onSelect, viewID, subviewName, query, pane, onPane, onView }: {
  items: Item[];
  views: View[];
  selected?: Item;
  onSelect: (id: string) => void;
  viewID: string;
  subviewName: string;
  query: string;
  pane: Pane;
  onPane: (pane: Pane) => void;
  onView: (viewID: string, subviewName?: string) => void;
}) {
  const activeView = views.find((view) => view.id === viewID) ?? views[0];
  const visible = useMemo(() => {
    const typeHint = activeView?.id.endsWith('s') ? activeView.id.slice(0, -1) : '';
    const subviewIDs = activeView?.subviews?.find((subview) => subview.name === subviewName)?.item_ids;
    const ids = new Set(subviewIDs ?? activeView?.item_ids ?? []);
    const q = query.trim().toLowerCase();
    return [...items]
      .filter((item) => !activeView || ids.has(item.id) || (!ids.size && (!typeHint || item.type === typeHint || activeView.id.includes(item.type))))
      .filter((item) => !q || `${itemTitle(item)} ${item.raw || item.body} ${item.tags.join(' ')}`.toLowerCase().includes(q))
      .sort((a, b) => String(b.frontmatter.created ?? '').localeCompare(String(a.frontmatter.created ?? '')));
  }, [items, activeView, subviewName, query]);
  const current = selected ?? visible[0];
  const tags = useMemo(() => {
    const counts = new Map<string, number>();
    for (const item of visible) {
      for (const tag of item.tags) counts.set(tag, (counts.get(tag) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  }, [visible]);

  useEffect(() => {
    if (pane === 'editor' && !selected && visible[0]) onSelect(visible[0].id);
  }, [pane, selected, visible, onSelect]);

  return (
    <section className="pane-shell">
      <div className="pane-tabs" role="tablist">
        <button className={pane === 'tags' ? 'pane-tab active' : 'pane-tab'} onClick={() => onPane('tags')}>Tags</button>
        <button className={pane === 'items' ? 'pane-tab active' : 'pane-tab'} onClick={() => onPane('items')}>Items</button>
        <button className={pane === 'editor' ? 'pane-tab active' : 'pane-tab'} onClick={() => onPane('editor')}>Editor</button>
      </div>
      {pane === 'tags' ? (
        <div className="tags-pane">
          <div className="menu-section inline">
            {views.map((view) => (
              <button key={view.id} className={(activeView?.id === view.id && !subviewName) ? 'menu-item active' : 'menu-item'} onClick={() => onView(view.id)}>
                {view.config.name || view.id}
              </button>
            ))}
          </div>
          {activeView?.subviews?.length ? (
            <div className="menu-section inline">
              {activeView.subviews.map((subview) => (
                <button key={subview.name} className={subviewName === subview.name ? 'menu-item active' : 'menu-item'} onClick={() => onView(activeView.id, subview.name)}>
                  {subview.name}
                </button>
              ))}
            </div>
          ) : null}
          <div className="tag-list">
            {tags.map(([tag, count]) => (
              <button key={tag} className="tag-row" onClick={() => onPane('items')}>
                <span>{tag}</span>
                <strong>{count}</strong>
              </button>
            ))}
          </div>
        </div>
      ) : null}
      {pane === 'items' ? (
        <div className="list-pane">
          <div className="item-list">
            {visible.map((item) => (
              <button key={item.id} className={current?.id === item.id ? 'item-row active' : 'item-row'} onClick={() => onSelect(item.id)}>
              <strong>{itemTitle(item)}</strong>
              <span>{item.subtitle || item.type || item.path}</span>
            </button>
            ))}
          </div>
        </div>
      ) : null}
      {pane === 'editor' ? <ItemDetail item={current} /> : null}
    </section>
  );
}

function ItemDetail({ item }: { item?: Item }) {
  const [editing, setEditing] = useState(false);
  const [raw, setRaw] = useState('');

  useEffect(() => {
    setRaw(item ? item.raw || rawFromParts(item.frontmatter, item.body) : '');
    setEditing(false);
  }, [item?.id]);

  if (!item) return <div className="empty-state">No items cached yet.</div>;

  async function save() {
    if (!item) return;
    const { frontmatter, body } = parseRaw(raw);
    const title = String(frontmatter.title ?? item.id);
    const updated = { ...item, title, frontmatter, body, raw, updated_at: new Date().toISOString() };
    await db.items.put(updated);
    await enqueue({ id: crypto.randomUUID(), op: 'upsert_item', item_id: item.id, path: item.path, frontmatter, body });
    setEditing(false);
  }

  async function remove() {
    if (!item) return;
    await db.items.put({ ...item, deleted: true, updated_at: new Date().toISOString() });
    await enqueue({ id: crypto.randomUUID(), op: 'delete_item', item_id: item.id, path: item.path });
  }

  const parsed = parseRaw(item.raw || rawFromParts(item.frontmatter, item.body));
  const frontmatterText = stringifyYAML(parsed.frontmatter).trimEnd();
  const html = DOMPurify.sanitize(marked.parse(parsed.body, { async: false }) as string);

  return (
    <article className="detail-pane">
      {editing ? (
        <div className="editor">
          <textarea className="raw-editor" value={raw} onChange={(event) => setRaw(event.target.value)} aria-label="Raw markdown" />
          <div className="button-row">
            <button className="button" onClick={() => setEditing(false)}>Cancel</button>
            <button className="button primary" onClick={() => void save()}>Save</button>
          </div>
        </div>
      ) : (
        <>
          <div className="detail-heading">
            <div>
              <h2>{itemTitle(item)}</h2>
              <p>{item.path}</p>
            </div>
            <button className="icon-button danger" onClick={() => void remove()} aria-label="Delete"><Trash2 size={19} /></button>
          </div>
          <div className="markdown-view">
            <pre className="frontmatter-view">---{'\n'}{frontmatterText}{'\n'}---</pre>
            <div className="markdown-body" dangerouslySetInnerHTML={{ __html: html }} />
          </div>
          <button className="fab" onClick={() => setEditing(true)}>Edit</button>
        </>
      )}
    </article>
  );
}

function CreateView({ views, onCreated }: { views: View[]; onCreated: (id: string) => void }) {
  const [type, setType] = useState('note');
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [url, setURL] = useState('');
  const types = Array.from(new Set(['note', ...views.map((view) => view.id.endsWith('s') ? view.id.slice(0, -1) : view.id)]));

  async function create() {
    const id = newID();
    const created = new Date().toISOString();
    const path = `${type}/${created.slice(0, 4)}/${created.slice(5, 7)}/${slugify(title)}.md`;
    const frontmatter = { id, type, title, tags: [], created };
    const item: Item = { id, type, path, title, subtitle: '', tags: [], frontmatter, body, raw: rawFromParts(frontmatter, body), updated_at: created };
    await db.items.put(item);
    await enqueue({ id: crypto.randomUUID(), op: 'upsert_item', item_id: id, path, frontmatter, body });
    setTitle('');
    setBody('');
    onCreated(id);
  }

  async function createFromURL() {
    const result = await importURL(url);
    const id = String(result.frontmatter.id ?? result.id);
    await refreshFromServer();
    setURL('');
    onCreated(id);
  }

  return (
    <section className="single-pane form-pane">
      <label>Type<select value={type} onChange={(event) => setType(event.target.value)}>{types.map((value) => <option key={value}>{value}</option>)}</select></label>
      <label>Title<input value={title} onChange={(event) => setTitle(event.target.value)} /></label>
      <label>Body<textarea value={body} onChange={(event) => setBody(event.target.value)} /></label>
      <button className="button primary" disabled={!title.trim()} onClick={() => void create()}>Create</button>
      <div className="divider" />
      <label>Import URL<input value={url} onChange={(event) => setURL(event.target.value)} placeholder="https://..." /></label>
      <button className="button" disabled={!url.trim()} onClick={() => void createFromURL()}>Import URL</button>
    </section>
  );
}

function OutboxView({ entries }: { entries: OutboxEntry[] }) {
  const [status, setStatus] = useState('all');
  const filtered = entries.filter((entry) => status === 'all' || entry.status === status);
  async function retry(entry: OutboxEntry) {
    await db.outbox.put({ ...entry, status: 'pending', error: undefined, updated_at: new Date().toISOString() });
    await syncOutbox();
  }
  return (
    <section className="single-pane">
      <div className="chips">
        {['all', 'pending', 'failed', 'synced'].map((value) => <button key={value} className={status === value ? 'chip active' : 'chip'} onClick={() => setStatus(value)}>{value}</button>)}
      </div>
      <div className="outbox-list">
        {filtered.map((entry) => (
          <div className="outbox-row" key={entry.id}>
            <div><strong>{entry.op}</strong><span>{entry.item_id} · {entry.status}</span>{entry.error ? <em>{entry.error}</em> : null}</div>
            {entry.status === 'failed' ? <button className="icon-button" onClick={() => void retry(entry)} aria-label="Retry"><RefreshCw size={18} /></button> : <Check size={18} />}
          </div>
        ))}
      </div>
    </section>
  );
}

function SettingsView() {
  const [clients, setClients] = useState<SyncClient[]>([]);
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [message, setMessage] = useState('');

  async function loadClients() {
    try {
      const result = await listSyncClients();
      setClients(Array.isArray(result.clients) ? result.clients : []);
    } catch {
      setClients([]);
    }
  }

  useEffect(() => {
    void loadClients();
    const onServerChange = (event: Event) => {
      const detail = (event as CustomEvent<{ target_kind?: string }>).detail;
      if (!detail?.target_kind || detail.target_kind === 'client') void loadClients();
    };
    window.addEventListener('exo:server-change', onServerChange);
    return () => window.removeEventListener('exo:server-change', onServerChange);
  }, []);

  async function updatePassword() {
    await changePassword(currentPassword, newPassword);
    setCurrentPassword('');
    setNewPassword('');
    setMessage('Password changed');
  }

  return (
    <section className="single-pane settings-pane">
      <h2>Password</h2>
      <input type="password" placeholder="Current password" value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} />
      <input type="password" placeholder="New password" value={newPassword} onChange={(event) => setNewPassword(event.target.value)} />
      <button className="button primary" onClick={() => void updatePassword()}>Change password</button>
      {message ? <p className="notice">{message}</p> : null}
      <h2>Sync clients</h2>
      <div className="outbox-list">
        {clients.length === 0 ? <div className="empty-state">No sync clients.</div> : null}
        {clients.map((client) => (
          <div className="outbox-row" key={client.id}>
            <div><strong>{client.label}</strong><span>{client.id} · {client.status}</span></div>
            {client.status === 'pending' ? <button className="button" onClick={() => void approveSyncClient(client.id).then(loadClients)}>Approve</button> : null}
            {client.status === 'approved' ? <button className="button" onClick={() => void revokeSyncClient(client.id).then(loadClients)}>Revoke</button> : null}
          </div>
        ))}
      </div>
    </section>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
