import React, { useEffect, useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { useLiveQuery } from 'dexie-react-hooks';
import DOMPurify from 'dompurify';
import { marked } from 'marked';
import { Check, CloudOff, Menu, Plus, RefreshCw, Search, Settings, Tags, Trash2, X } from 'lucide-react';
import { parse as parseYAML, stringify as stringifyYAML } from 'yaml';
import { approveSyncClient, changePassword, createAPIKey, importURL, listAPIKeys, listConfigs, listItemActions, listSyncClients, revokeAPIKey, revokeSyncClient, runAction, updateConfig, uploadAsset } from './api';
import { db } from './db';
import { decryptBody, encryptBody, isEncrypted } from './encryption';
import { refreshFromServer, startSyncRuntime, syncOutbox } from './sync';
import type { Action, APIKey, ConfigFile, Frontmatter, Item, OutboxEntry, SyncClient, View } from './types';
import './styles.css';

type Screen = 'items' | 'create' | 'settings';
type Pane = 'tags' | 'items' | 'editor';
type SettingsTab = 'api-keys' | 'password' | 'sync-clients' | 'sync-status' | 'toml-settings';

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

function routeFromState(viewID: string, itemID: string | undefined, pane: Pane, query: string, subviewName: string, tags: string[]) {
  const path = viewID ? `/views/${encodeURIComponent(viewID)}${itemID ? `/${encodeURIComponent(itemID)}` : ''}` : '/';
  const params = new URLSearchParams();
  if (pane !== 'items') params.set('pane', pane);
  if (query.trim()) params.set('q', query.trim());
  if (subviewName) params.set('subview', subviewName);
  if (tags.length) params.set('tags', tags.join(','));
  const qs = params.toString();
  return qs ? `${path}?${qs}` : path;
}

function stateFromLocation(): { viewID: string; itemID?: string; pane: Pane; query: string; subviewName: string; tags: string[] } {
  const parts = window.location.pathname.split('/').filter(Boolean);
  const params = new URLSearchParams(window.location.search);
  const paneParam = params.get('pane');
  const tags = (params.get('tags') ?? '')
    .split(',')
    .map((tag) => tag.trim())
    .filter(Boolean);
  return {
    viewID: parts[0] === 'views' ? decodeURIComponent(parts[1] ?? '') : '',
    itemID: parts[0] === 'views' && parts[2] ? decodeURIComponent(parts[2]) : undefined,
    pane: paneParam === 'tags' || paneParam === 'editor' ? paneParam : 'items',
    query: params.get('q') ?? '',
    subviewName: params.get('subview') ?? '',
    tags,
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
  const [selectedTags, setSelectedTags] = useState<string[]>(initialRoute.tags);
  const [syncStatus, setSyncStatus] = useState<'online' | 'offline' | 'syncing'>(navigator.onLine ? 'online' : 'offline');
  const items = useLiveQuery(() => db.items.filter((item) => !item.deleted).toArray(), [], []);
  const views = useLiveQuery(() => db.views.toArray(), [], []);
  const actions = useLiveQuery(() => db.actions.toArray(), [], []);
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
      setSelectedTags(next.tags);
      setScreen('items');
    };
    window.addEventListener('popstate', onPop);
    return () => window.removeEventListener('popstate', onPop);
  }, []);
  useEffect(() => {
    if (screen !== 'items') return;
    const next = routeFromState(viewID || views[0]?.id || '', selectedID, pane, query, subviewName, selectedTags);
    if (next !== window.location.pathname + window.location.search) {
      window.history.replaceState(null, '', next);
    }
  }, [screen, viewID, selectedID, pane, query, subviewName, selectedTags, views]);

  async function refreshApp() {
    await refreshFromServer();
    if ('serviceWorker' in navigator) {
      const registration = await navigator.serviceWorker.getRegistration();
      await registration?.update();
    }
    window.location.reload();
  }

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
        <img className="brand-logo" src="/icons/logo.svg" alt="" aria-hidden="true" />
      </header>

      <main className="content">
        {screen === 'items' && (
          <ItemsView
            items={items}
            views={views}
            actions={actions}
            selected={selected}
            onSelect={(id) => {
              setSelectedID(id);
              setPane('editor');
              window.history.pushState(null, '', routeFromState(viewID || views[0]?.id || '', id, 'editor', query, subviewName, selectedTags));
            }}
            viewID={viewID}
            subviewName={subviewName}
            selectedTags={selectedTags}
            query={query}
            pane={pane}
            onPane={setPane}
            onTags={setSelectedTags}
          />
        )}
        {screen === 'create' && <CreateView views={views} onCreated={(id) => { setSelectedID(id); setScreen('items'); }} />}
        {screen === 'settings' && <SettingsView entries={outbox} syncStatus={syncStatus} />}
      </main>

      <div className="bottom-search" role="search">
        <button className="icon-button menu-trigger" onClick={() => setMenuOpen((open) => !open)} aria-label="Menu">
          {menuOpen ? <X size={20} /> : <Menu size={20} />}
        </button>
        <Search size={19} />
        <input value={query} onChange={(event) => { setQuery(event.target.value); setScreen('items'); setPane('items'); }} placeholder="Search" />
        <button className="new-button" onClick={() => setScreen('create')} aria-label="New item">
          <Plus size={24} />
        </button>
      </div>
      {menuOpen ? (
        <AppMenu
          views={views}
          activeViewID={viewID}
          activeSubviewName={subviewName}
          onRefresh={() => void refreshApp()}
          onView={(nextViewID, nextSubviewName = '') => {
            setViewID(nextViewID);
            setSubviewName(nextSubviewName);
            setSelectedTags([]);
            setSelectedID(undefined);
            setPane('items');
            setScreen('items');
            setMenuOpen(false);
          }}
          onSettings={() => {
            setScreen('settings');
            setMenuOpen(false);
          }}
        />
      ) : null}
    </div>
  );
}

function labelForScreen(screen: Screen) {
  switch (screen) {
    case 'create': return 'Create';
    case 'settings': return 'Settings';
    default: return 'Items';
  }
}

function AppMenu({ views, activeViewID, activeSubviewName, onRefresh, onView, onSettings }: {
  views: View[];
  activeViewID: string;
  activeSubviewName: string;
  onRefresh: () => void;
  onView: (viewID: string, subviewName?: string) => void;
  onSettings: () => void;
}) {
  const activeView = views.find((view) => view.id === activeViewID) ?? views[0];
  const [expandedViewID, setExpandedViewID] = useState(activeView?.id ?? '');
  const expandedView = views.find((view) => view.id === expandedViewID) ?? activeView;

  function chooseView(view: View) {
    if (view.subviews?.length) {
      setExpandedViewID(view.id);
      return;
    }
    onView(view.id);
  }

  return (
    <div className="menu-panel">
      <div className="menu-section">
        {views.length === 0 ? <div className="empty-state">No views synced yet.</div> : null}
        {views.map((view) => (
          <button key={view.id} className={(expandedView?.id === view.id && !activeSubviewName) ? 'menu-item active' : 'menu-item'} onClick={() => chooseView(view)}>
            {view.config.name || view.id}
          </button>
        ))}
      </div>
      {expandedView?.subviews?.length ? (
        <div className="menu-section subviews">
          {expandedView.subviews.map((subview) => (
            <button key={subview.name} className={expandedView.id === activeViewID && activeSubviewName === subview.name ? 'menu-item active' : 'menu-item'} onClick={() => onView(expandedView.id, subview.name)}>
              {subview.name}
            </button>
          ))}
        </div>
      ) : null}
      <div className="menu-actions">
        <button className="button" onClick={onRefresh}><RefreshCw size={17} /> Refresh</button>
        <button className="button" onClick={onSettings}><Settings size={17} /> Settings</button>
      </div>
    </div>
  );
}

function hasAllTags(item: Item, tags: string[]) {
  return tags.every((tag) => item.tags.includes(tag));
}

function ActionFoldout({ item, actions, onEdit, onAction, onImportHardcover }: {
  item?: Item;
  actions: Action[];
  onEdit: () => void;
  onAction: (action: Action, item: Item) => void;
  onImportHardcover: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="action-fab">
      {open ? (
        <div className="action-menu">
          {item ? (
            <>
              <button className="button" onClick={() => { setOpen(false); onEdit(); }}>Edit</button>
              {actions.map((action) => (
                <button key={action.name} className="button" onClick={() => { setOpen(false); onAction(action, item); }}>
                  {action.description || action.name}
                </button>
              ))}
              {actions.length === 0 ? <div className="empty-state">No matching actions.</div> : null}
            </>
          ) : (
            <button className="button" onClick={() => { setOpen(false); onImportHardcover(); }}>Import from hardcover</button>
          )}
        </div>
      ) : null}
      <button className="fab" onClick={() => setOpen((value) => !value)} aria-label="Actions">
        {open ? <X size={19} /> : <span className="lambda-icon">λ</span>}
      </button>
    </div>
  );
}

function ItemsView({ items, views, actions, selected, onSelect, viewID, subviewName, selectedTags, query, pane, onPane, onTags }: {
  items: Item[];
  views: View[];
  actions: Action[];
  selected?: Item;
  onSelect: (id: string) => void;
  viewID: string;
  subviewName: string;
  selectedTags: string[];
  query: string;
  pane: Pane;
  onPane: (pane: Pane) => void;
  onTags: (tags: string[]) => void;
}) {
  const activeView = views.find((view) => view.id === viewID) ?? views[0];
  const [editRequest, setEditRequest] = useState(0);
  const [applicableActions, setApplicableActions] = useState<Action[]>([]);
  const baseVisible = useMemo(() => {
    const typeHint = activeView?.id.endsWith('s') ? activeView.id.slice(0, -1) : '';
    const subviewIDs = activeView?.subviews?.find((subview) => subview.name === subviewName)?.item_ids;
    const ids = new Set(subviewIDs ?? activeView?.item_ids ?? []);
    const q = query.trim().toLowerCase();
    return [...items]
      .filter((item) => !activeView || ids.has(item.id) || (!ids.size && (!typeHint || item.type === typeHint || activeView.id.includes(item.type))))
      .filter((item) => !q || `${itemTitle(item)} ${item.raw || item.body} ${item.tags.join(' ')}`.toLowerCase().includes(q))
      .sort((a, b) => String(b.frontmatter.created ?? '').localeCompare(String(a.frontmatter.created ?? '')));
  }, [items, activeView, subviewName, query]);
  const visible = useMemo(() => {
    if (!selectedTags.length) return baseVisible;
    return baseVisible.filter((item) => hasAllTags(item, selectedTags));
  }, [baseVisible, selectedTags]);
  const current = selected && visible.some((item) => item.id === selected.id) ? selected : visible[0];
  const tags = useMemo(() => {
    const counts = new Map<string, number>();
    for (const item of visible) {
      for (const tag of item.tags) counts.set(tag, (counts.get(tag) ?? 0) + 1);
    }
    for (const tag of selectedTags) {
      if (!counts.has(tag)) counts.set(tag, 0);
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  }, [visible, selectedTags]);
  const showTags = Boolean(activeView?.config.show_tags);
  const title = activeView?.config.name || activeView?.id || 'Items';

  function toggleTag(tag: string) {
    onTags(selectedTags.includes(tag) ? selectedTags.filter((value) => value !== tag) : [...selectedTags, tag]);
  }

  async function applyAction(action: Action, item: Item) {
    const updated = await runAction(action.name, item.id);
    await db.items.put(updated);
    const result = await listItemActions(updated.id);
    setApplicableActions(result.actions ?? []);
  }

  async function importFromHardcover() {
    const url = window.prompt('Hardcover URL');
    if (!url?.trim()) return;
    const result = await importURL(url.trim());
    await refreshFromServer();
    const id = String(result.frontmatter.id ?? result.id);
    onSelect(id);
    onPane('editor');
  }

  useEffect(() => {
    if (pane === 'editor' && !selected && visible[0]) onSelect(visible[0].id);
  }, [pane, selected, visible, onSelect]);
  useEffect(() => {
    if (pane === 'tags' && !showTags) onPane('items');
  }, [pane, showTags, onPane]);
  useEffect(() => {
    let cancelled = false;
    if (!selected) {
      setApplicableActions([]);
      return () => {
        cancelled = true;
      };
    }
    void listItemActions(selected.id)
      .then((result) => {
        if (!cancelled) setApplicableActions(result.actions ?? []);
      })
      .catch(() => {
        if (!cancelled) setApplicableActions([]);
      });
    return () => {
      cancelled = true;
    };
  }, [selected?.id, actions]);

  return (
    <section className="pane-shell">
      {pane !== 'editor' ? (
        <div className="items-header">
          <div>
            <h2>{title}</h2>
            <p>{visible.length} item{visible.length === 1 ? '' : 's'}{subviewName ? ` · ${subviewName}` : ''}</p>
          </div>
          {showTags ? (
            <button className="button" onClick={() => onPane('tags')}><Tags size={17} /> Tags{selectedTags.length ? ` (${selectedTags.length})` : ''}</button>
          ) : null}
        </div>
      ) : null}
      {pane === 'tags' ? (
        <div className="tags-pane">
          {selectedTags.length ? (
            <div className="chips">
              {selectedTags.map((tag) => (
                <button key={tag} className="chip active" onClick={() => toggleTag(tag)}>{tag}</button>
              ))}
            </div>
          ) : null}
          <div className="tag-list">
            {tags.length === 0 ? <div className="empty-state">No tags in this result set.</div> : null}
            {tags.map(([tag, count]) => (
              <button key={tag} className={selectedTags.includes(tag) ? 'tag-row active' : 'tag-row'} onClick={() => toggleTag(tag)}>
                <span>{tag}</span>
                <strong>{count}</strong>
              </button>
            ))}
          </div>
          <div className="tag-actions">
            <button className="button" disabled={!selectedTags.length} onClick={() => onTags([])}>Clear</button>
            <button className="button primary" onClick={() => onPane('items')}>View results</button>
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
            {visible.length === 0 ? <div className="empty-state">No matching items.</div> : null}
          </div>
        </div>
      ) : null}
      {pane === 'editor' ? <ItemDetail item={current} editRequest={editRequest} /> : null}
      <ActionFoldout
        item={selected}
        actions={applicableActions}
        onEdit={() => {
          if (!selected) return;
          onPane('editor');
          setEditRequest((value) => value + 1);
        }}
        onAction={(action, item) => void applyAction(action, item)}
        onImportHardcover={() => void importFromHardcover()}
      />
    </section>
  );
}

function ItemDetail({ item, editRequest }: { item?: Item; editRequest: number }) {
  const [editing, setEditing] = useState(false);
  const [raw, setRaw] = useState('');
  const [decryptedBody, setDecryptedBody] = useState<string | undefined>();
  const [uploadError, setUploadError] = useState('');
  const editorRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    setRaw(item ? item.raw || rawFromParts(item.frontmatter, item.body) : '');
    setDecryptedBody(undefined);
    setEditing(false);
  }, [item?.id]);
  useEffect(() => {
    if (!item || editRequest <= 0) return;
    if (item.frontmatter.encrypted === true && decryptedBody === undefined) {
      void unlock();
      return;
    }
    setEditing(true);
  }, [editRequest, item, decryptedBody]);

  if (!item) return <div className="empty-state">No items cached yet.</div>;

  async function save() {
    if (!item) return;
    const { frontmatter } = parseRaw(raw);
    let { body } = parseRaw(raw);
    const title = String(frontmatter.title ?? item.id);
    if (frontmatter.encrypted === true && !isEncrypted(body)) {
      const passphrase = window.prompt('Passphrase for this note');
      if (!passphrase) return;
      body = await encryptBody(item.id, passphrase, body);
    }
    const updated = { ...item, title, frontmatter, body, raw: rawFromParts(frontmatter, body), updated_at: new Date().toISOString() };
    await db.items.put(updated);
    await enqueue({ id: crypto.randomUUID(), op: 'upsert_item', item_id: item.id, path: item.path, frontmatter, body });
    setEditing(false);
    setDecryptedBody(undefined);
  }

  async function unlock() {
    const passphrase = window.prompt('Passphrase for this note');
    if (!passphrase) return;
    try {
      const plain = await decryptBody(item.id, passphrase, item.body);
      setDecryptedBody(plain);
      setRaw(rawFromParts(item.frontmatter, plain));
    } catch {
      setUploadError('Unable to decrypt note: incorrect passphrase or damaged body.');
    }
  }

  async function remove() {
    if (!item) return;
    await db.items.put({ ...item, deleted: true, updated_at: new Date().toISOString() });
    await enqueue({ id: crypto.randomUUID(), op: 'delete_item', item_id: item.id, path: item.path });
  }

  async function attach(files?: FileList | null) {
    const file = files?.[0];
    if (!file) return;
    try {
      setUploadError('');
      const uploaded = await uploadAsset(file);
      const editor = editorRef.current;
      const start = editor?.selectionStart ?? raw.length;
      const end = editor?.selectionEnd ?? start;
      setRaw((value) => `${value.slice(0, start)}${uploaded.markdown}${value.slice(end)}`);
      requestAnimationFrame(() => {
        if (editor) {
          const pos = start + uploaded.markdown.length;
          editor.focus(); editor.setSelectionRange(pos, pos);
        }
      });
    } catch (err) {
      setUploadError(err instanceof Error ? err.message : 'Image upload failed');
    }
  }

  const parsed = parseRaw(item.raw || rawFromParts(item.frontmatter, item.body));
  const frontmatterText = stringifyYAML(parsed.frontmatter).trimEnd();
  // Asset references are workspace-relative Markdown paths. Make rendered
  // images root-relative so a note opened under /views/... still resolves it.
  const html = DOMPurify.sanitize((marked.parse(parsed.body, { async: false }) as string).replaceAll('src="assets/', 'src="/assets/'));

  return (
    <article className="detail-pane">
      {editing ? (
        <div className="editor" onDrop={(event) => { event.preventDefault(); void attach(event.dataTransfer.files); }} onDragOver={(event) => event.preventDefault()}>
          <textarea ref={editorRef} className="raw-editor" value={raw} onChange={(event) => setRaw(event.target.value)} onPaste={(event) => { if (event.clipboardData.files?.length) { event.preventDefault(); void attach(event.clipboardData.files); } }} aria-label="Raw markdown" />
          {uploadError ? <p className="error-message" role="alert">{uploadError}</p> : null}
          <div className="button-row">
            <label className="button">Upload image<input hidden type="file" accept="image/jpeg,image/png,image/gif,image/webp" onChange={(event) => void attach(event.target.files)} /></label>
            <button className="button" onClick={() => { setEditing(false); setDecryptedBody(undefined); }}>Cancel</button>
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
          {item.frontmatter.encrypted === true && decryptedBody === undefined ? <div className="empty-state"><p>This note body is encrypted.</p><button className="button primary" onClick={() => void unlock()}>Unlock</button>{uploadError ? <p className="error-message" role="alert">{uploadError}</p> : null}</div> : <div className="markdown-view">
            <pre className="frontmatter-view">---{'\n'}{frontmatterText}{'\n'}---</pre>
            <div className="markdown-body" dangerouslySetInnerHTML={{ __html: DOMPurify.sanitize((marked.parse(decryptedBody ?? parsed.body, { async: false }) as string).replaceAll('src="assets/', 'src="/assets/')) }} />
          </div>}
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
    <div className="sync-status-pane">
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
    </div>
  );
}

function SettingsView({ entries, syncStatus }: { entries: OutboxEntry[]; syncStatus: 'online' | 'offline' | 'syncing' }) {
  const [tab, setTab] = useState<SettingsTab>('api-keys');
  const [clients, setClients] = useState<SyncClient[]>([]);
  const [apiKeys, setAPIKeys] = useState<APIKey[]>([]);
  const [configs, setConfigs] = useState<ConfigFile[]>([]);
  const [selectedConfigPath, setSelectedConfigPath] = useState('');
  const [configContent, setConfigContent] = useState('');
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [apiKeyAppName, setAPIKeyAppName] = useState('');
  const [apiKeyFilter, setAPIKeyFilter] = useState('type == "note"');
  const [apiKeyExpiresAt, setAPIKeyExpiresAt] = useState(defaultAPIKeyExpiry());
  const [newAPIKey, setNewAPIKey] = useState('');
  const [message, setMessage] = useState('');

  async function loadClients() {
    try {
      const result = await listSyncClients();
      setClients(Array.isArray(result.clients) ? result.clients : []);
    } catch {
      setClients([]);
    }
  }

  async function loadAPIKeys() {
    try {
      const result = await listAPIKeys();
      setAPIKeys(Array.isArray(result.keys) ? result.keys : []);
    } catch {
      setAPIKeys([]);
    }
  }

  async function loadConfigs() {
    try {
      const result = await listConfigs();
      const next = Array.isArray(result.configs) ? result.configs : [];
      setConfigs(next);
      const path = selectedConfigPath || next[0]?.path || '';
      setSelectedConfigPath(path);
      setConfigContent(next.find((cfg) => cfg.path === path)?.content ?? '');
    } catch {
      setConfigs([]);
      setSelectedConfigPath('');
      setConfigContent('');
    }
  }

  useEffect(() => {
    void loadClients();
    void loadAPIKeys();
    void loadConfigs();
    const onServerChange = (event: Event) => {
      const detail = (event as CustomEvent<{ target_kind?: string }>).detail;
      if (!detail?.target_kind || detail.target_kind === 'client') void loadClients();
      if (!detail?.target_kind || detail.target_kind === 'config') void loadConfigs();
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

  async function addAPIKey() {
    const result = await createAPIKey(apiKeyAppName, apiKeyFilter, apiKeyExpiresAt);
    setNewAPIKey(result.key);
    setAPIKeyAppName('');
    setAPIKeyFilter('type == "note"');
    setAPIKeyExpiresAt(defaultAPIKeyExpiry());
    await loadAPIKeys();
  }

  async function copyAPIKey() {
    if (!newAPIKey) return;
    await navigator.clipboard.writeText(newAPIKey);
    setMessage('API key copied');
  }

  async function saveConfig() {
    if (!selectedConfigPath) return;
    await updateConfig(selectedConfigPath, configContent);
    setMessage('TOML settings saved');
    await refreshFromServer();
    await loadConfigs();
  }

  function selectConfig(path: string) {
    setSelectedConfigPath(path);
    setConfigContent(configs.find((cfg) => cfg.path === path)?.content ?? '');
  }

  return (
    <section className="single-pane settings-pane">
      <div className="settings-tabs" role="tablist" aria-label="Settings">
        <button className={tab === 'api-keys' ? 'settings-tab active' : 'settings-tab'} onClick={() => setTab('api-keys')}>API keys</button>
        <button className={tab === 'password' ? 'settings-tab active' : 'settings-tab'} onClick={() => setTab('password')}>Password</button>
        <button className={tab === 'sync-clients' ? 'settings-tab active' : 'settings-tab'} onClick={() => setTab('sync-clients')}>Sync clients</button>
        <button className={tab === 'sync-status' ? 'settings-tab active' : 'settings-tab'} onClick={() => setTab('sync-status')}>Sync status</button>
        <button className={tab === 'toml-settings' ? 'settings-tab active' : 'settings-tab'} onClick={() => setTab('toml-settings')}>TOML settings</button>
      </div>

      {tab === 'api-keys' ? (
        <div className="settings-section">
          <h2>API keys</h2>
          <label>App name<input value={apiKeyAppName} onChange={(event) => setAPIKeyAppName(event.target.value)} /></label>
          <label>Expiration<input type="date" value={apiKeyExpiresAt} min={todayDate()} max={maxAPIKeyExpiry()} onChange={(event) => setAPIKeyExpiresAt(event.target.value)} /></label>
          <label>CEL filter<textarea value={apiKeyFilter} onChange={(event) => setAPIKeyFilter(event.target.value)} /></label>
          <div className="chips">
            <button className="chip" onClick={() => setAPIKeyFilter('type == "secret" && "acceptance" in tags')}>Secrets acceptance</button>
            <button className="chip" onClick={() => setAPIKeyFilter('type == "note"')}>Notes</button>
          </div>
          <button className="button primary" disabled={!apiKeyAppName.trim() || !apiKeyFilter.trim() || !apiKeyExpiresAt} onClick={() => void addAPIKey()}>Create API key</button>
          {newAPIKey ? (
            <div className="notice">
              <strong>New API key</strong>
              <code>{newAPIKey}</code>
              <button className="button" onClick={() => void copyAPIKey()}>Copy</button>
            </div>
          ) : null}
          <div className="outbox-list">
            {apiKeys.length === 0 ? <div className="empty-state">No API keys.</div> : null}
            {apiKeys.map((key) => (
              <div className="outbox-row" key={key.id}>
                <div>
                  <strong>{key.app_name}</strong>
                  <span>...{key.key_suffix} · expires {formatDate(key.expires_at)} · last used {formatDate(key.last_used_at) || 'never'}</span>
                  <em>{key.filter}</em>
                  {key.revoked_at ? <em>revoked {formatDate(key.revoked_at)}</em> : null}
                </div>
                {!key.revoked_at ? <button className="button" onClick={() => void revokeAPIKey(key.id).then(loadAPIKeys)}>Revoke</button> : null}
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {tab === 'password' ? (
        <div className="settings-section">
          <h2>Password</h2>
          <input type="password" placeholder="Current password" value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} />
          <input type="password" placeholder="New password" value={newPassword} onChange={(event) => setNewPassword(event.target.value)} />
          <button className="button primary" onClick={() => void updatePassword()}>Change password</button>
          {message ? <p className="notice">{message}</p> : null}
        </div>
      ) : null}

      {tab === 'sync-clients' ? (
        <div className="settings-section">
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
        </div>
      ) : null}

      {tab === 'sync-status' ? (
        <div className="settings-section">
          <h2>Sync status</h2>
          <div className="notice">Status: {syncStatus}</div>
          <OutboxView entries={entries} />
        </div>
      ) : null}

      {tab === 'toml-settings' ? (
        <div className="settings-section">
          <h2>TOML settings</h2>
          {configs.length === 0 ? <div className="empty-state">No synced TOML settings.</div> : null}
          {configs.length ? (
            <>
              <label>Config file<select value={selectedConfigPath} onChange={(event) => selectConfig(event.target.value)}>
                {configs.map((cfg) => <option key={cfg.path} value={cfg.path}>{cfg.path}</option>)}
              </select></label>
              <textarea className="raw-editor" value={configContent} onChange={(event) => setConfigContent(event.target.value)} aria-label="TOML settings" />
              <button className="button primary" onClick={() => void saveConfig()}>Save TOML settings</button>
              {message ? <p className="notice">{message}</p> : null}
            </>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

function todayDate() {
  return new Date().toISOString().slice(0, 10);
}

function maxAPIKeyExpiry() {
  const date = new Date();
  date.setFullYear(date.getFullYear() + 1);
  return date.toISOString().slice(0, 10);
}

function defaultAPIKeyExpiry() {
  const date = new Date();
  date.setMonth(date.getMonth() + 1);
  return date.toISOString().slice(0, 10);
}

function formatDate(value: string) {
  if (!value) return '';
  return value.slice(0, 10);
}

createRoot(document.getElementById('root')!).render(<App />);
