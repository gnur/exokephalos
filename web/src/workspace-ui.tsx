import DOMPurify from 'dompurify';
import { marked } from 'marked';
import React, { useEffect, useMemo, useState } from 'react';
import {
  ArrowLeft,
  Check,
  CircleAlert,
  Cloud,
  CloudOff,
  Copy,
  KeyRound,
  Menu,
  Plus,
  Radio,
  RefreshCw,
  Search,
  Settings,
  Tags,
  Trash2,
  X,
} from 'lucide-react';
import './legacy.css';
import type {
  DocumentEntry,
  FrontmatterValue,
  NoteMutationInput,
  NoteQueryInput,
  RuntimeReport,
  WorkspaceNote,
} from './protocol';

type Pane = 'items' | 'tags' | 'detail' | 'settings';

export function WorkspaceExperience({
  report,
  busy,
  error,
  activeView,
  activeSubview,
  search,
  updateAvailable,
  onView,
  onSubview,
  onSearch,
  onQuery,
  onMutate,
  onRefresh,
  onUpdate,
  onApprovePeer,
  onRejectPeer,
  onRemovePeer,
  onWipe,
}: {
  report: RuntimeReport;
  busy: boolean;
  error: string;
  activeView: string;
  activeSubview?: string;
  search: string;
  updateAvailable: boolean;
  onView: (view: string) => void;
  onSubview: (subview?: string) => void;
  onSearch: (search: string) => void;
  onQuery: (input: NoteQueryInput) => Promise<WorkspaceNote[]>;
  onMutate: (input: NoteMutationInput) => Promise<RuntimeReport | undefined>;
  onRefresh: () => void;
  onUpdate: () => void;
  onApprovePeer: (fingerprint: string) => void;
  onRejectPeer: (fingerprint: string) => void;
  onRemovePeer: (fingerprint: string) => void;
  onWipe: () => void;
}) {
  const initialPath = useMemo(() => window.location.pathname.split('/').filter(Boolean), []);
  const initialParameters = useMemo(() => new URLSearchParams(window.location.search), []);
  const [pane, setPane] = useState<Pane>(initialPath[0] === 'views' && initialPath[2] ? 'detail' : 'items');
  const [menuOpen, setMenuOpen] = useState(false);
  const [notes, setNotes] = useState<WorkspaceNote[]>([]);
  const [unfilteredNotes, setUnfilteredNotes] = useState<WorkspaceNote[]>([]);
  const [selectedId, setSelectedId] = useState<string | undefined>(initialPath[0] === 'views' && initialPath[2] ? decodeURIComponent(initialPath[2]) : undefined);
  const [selectedTags, setSelectedTags] = useState<string[]>(() => (initialParameters.get('tags') || '').split(',').map((tag) => tag.trim()).filter(Boolean));
  const [editing, setEditing] = useState(false);
  const [editingId, setEditingId] = useState<string>();
  const [draft, setDraft] = useState('');
  const [createTitle, setCreateTitle] = useState('');
  const [queryError, setQueryError] = useState('');
  const [ticketVisible, setTicketVisible] = useState(false);

  const workspace = report.workspace;
  const view = workspace?.behavior.views.find((candidate) => candidate.id === activeView);
  const selected = notes.find((note) => note.id === selectedId);

  useEffect(() => {
    const routeKeepsDetail = initialPath[0] === 'views'
      && initialPath[2]
      && decodeURIComponent(initialPath[1] || '') === activeView;
    if (!routeKeepsDetail) setPane('items');
  }, [activeView, activeSubview, initialPath]);

  useEffect(() => {
    const onPopState = () => {
      const parts = window.location.pathname.split('/').filter(Boolean);
      const parameters = new URLSearchParams(window.location.search);
      const nextView = parts[0] === 'views' && parts[1] ? decodeURIComponent(parts[1]) : activeView;
      onView(nextView);
      onSubview(parameters.get('subview') || undefined);
      onSearch(parameters.get('q') || '');
      setSelectedTags((parameters.get('tags') || '').split(',').map((tag) => tag.trim()).filter(Boolean));
      if (parts[0] === 'views' && parts[2]) {
        setSelectedId(decodeURIComponent(parts[2]));
        setPane('detail');
      } else {
        setPane('items');
      }
    };
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, [activeView, onSearch, onSubview, onView]);

  useEffect(() => {
    if (!activeView || pane === 'settings' || editing) return;
    const path = pane === 'detail' && selectedId
      ? `/views/${encodeURIComponent(activeView)}/${encodeURIComponent(selectedId)}`
      : `/views/${encodeURIComponent(activeView)}`;
    const parameters = new URLSearchParams();
    if (activeSubview) parameters.set('subview', activeSubview);
    if (search.trim()) parameters.set('q', search.trim());
    if (selectedTags.length) parameters.set('tags', selectedTags.join(','));
    const route = parameters.size ? `${path}?${parameters}` : path;
    if (`${window.location.pathname}${window.location.search}` !== route) {
      window.history.replaceState(null, '', route);
    }
  }, [activeSubview, activeView, editing, pane, search, selectedId, selectedTags]);

  useEffect(() => {
    if (!activeView) return;
    let active = true;
    const base: NoteQueryInput = { view: activeView, subview: activeSubview, search, tags: [] };
    void Promise.all([onQuery({ ...base, tags: selectedTags }), onQuery(base)])
      .then(([next, unfiltered]) => {
        if (!active) return;
        setQueryError('');
        setNotes(next);
        setUnfilteredNotes(unfiltered);
        setSelectedId((current) => next.some((note) => note.id === current) ? current : next[0]?.id);
      })
      .catch((cause: unknown) => {
        if (active) setQueryError(errorMessage(cause));
      });
    return () => { active = false; };
  }, [activeView, activeSubview, search, selectedTags, report.entries, onQuery]);

  const tagCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const note of unfilteredNotes) {
      for (const tag of noteTags(note)) counts.set(tag, (counts.get(tag) ?? 0) + 1);
    }
    for (const tag of selectedTags) if (!counts.has(tag)) counts.set(tag, 0);
    return [...counts.entries()].sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]));
  }, [selectedTags, unfilteredNotes]);

  function selectNote(note: WorkspaceNote) {
    setSelectedId(note.id);
    setPane('detail');
    window.history.pushState(null, '', `/views/${encodeURIComponent(activeView)}/${encodeURIComponent(note.id)}`);
  }

  function startCreate() {
    setEditingId(undefined);
    setCreateTitle('');
    setDraft('---\ntitle: \ntype: \ntags: []\n---\n');
    setEditing(true);
  }

  function startEdit(note: WorkspaceNote) {
    setEditingId(note.id);
    setCreateTitle(noteTitle(note, view?.title_field));
    setDraft(note.markdown);
    setEditing(true);
  }

  async function saveDraft() {
    const saved = await onMutate({
      operation: 'save',
      noteId: editingId,
      title: createTitle,
      markdown: draft,
    });
    if (!saved) return;
    setEditing(false);
    setSelectedId(saved.mutatedNoteId);
    setPane('detail');
  }

  async function remove(note: WorkspaceNote) {
    if (!window.confirm(`Delete “${noteTitle(note, view?.title_field)}”?`)) return;
    const saved = await onMutate({ operation: 'delete', noteId: note.id });
    if (saved) setPane('items');
  }

  function chooseView(id: string, subview?: string) {
    onView(id);
    onSubview(subview);
    setSelectedTags([]);
    setMenuOpen(false);
    setPane('items');
    const path = `/views/${encodeURIComponent(id)}`;
    window.history.pushState(null, '', subview ? `${path}?subview=${encodeURIComponent(subview)}` : path);
  }

  const statusMessage = error || report.syncError || queryError
    || (report.status.restoring ? 'Showing durable notes while Iroh synchronization starts.' : '');
  const screenTitle = pane === 'settings'
    ? 'Settings'
    : pane === 'tags'
      ? 'Tags'
      : pane === 'detail'
        ? noteTitle(selected, view?.title_field)
        : view?.name || 'Items';

  return (
    <div className="legacy-shell">
      <header className="legacy-topbar">
        <div>
          <p className="legacy-eyebrow">exokephalos · {report.runtime.version}</p>
          <h1>{screenTitle}</h1>
        </div>
        <img className="brand-logo" src="/logo.svg" alt="" aria-hidden="true" />
      </header>

      {!navigator.onLine || statusMessage ? (
        <div className="sync-warning" role="status"><CloudOff /> <span>{statusMessage || 'sync offline'}</span></div>
      ) : null}
      {updateAvailable ? (
        <div className="update-banner" role="status">
          <span>A newer xo release is available.</span>
          <button onClick={onUpdate}><RefreshCw /> Update</button>
        </div>
      ) : null}

      <main className="legacy-content">
        {editing ? (
          <EditorPane
            editingId={editingId}
            title={createTitle}
            draft={draft}
            busy={busy}
            error={error}
            onTitle={setCreateTitle}
            onDraft={setDraft}
            onCancel={() => setEditing(false)}
            onSave={() => void saveDraft()}
          />
        ) : pane === 'tags' ? (
          <TagsPane
            tags={tagCounts}
            selected={selectedTags}
            onToggle={(tag) => setSelectedTags((current) => current.includes(tag)
              ? current.filter((value) => value !== tag)
              : [...current, tag])}
            onClear={() => setSelectedTags([])}
            onDone={() => setPane('items')}
          />
        ) : pane === 'detail' ? (
          <DetailPane
            note={selected}
            titleField={view?.title_field}
            onBack={() => setPane('items')}
            onEdit={() => selected && startEdit(selected)}
            onDelete={() => selected && void remove(selected)}
          />
        ) : pane === 'settings' ? (
          <SettingsPane
            report={report}
            busy={busy}
            ticketVisible={ticketVisible}
            onTicketVisible={setTicketVisible}
            onRefresh={onRefresh}
            onApprovePeer={onApprovePeer}
            onRejectPeer={onRejectPeer}
            onRemovePeer={onRemovePeer}
            onRestore={(noteId) => void onMutate({ operation: 'restore', noteId })}
            onWipe={onWipe}
          />
        ) : (
          <ItemsPane
            notes={notes}
            viewName={view?.name || 'Notes'}
            subviewName={activeSubview
              ? view?.subviews.find((subview) => subview.id === activeSubview)?.name || activeSubview
              : undefined}
            showTags={Boolean(view?.show_tags)}
            sortField={view?.sort_field || 'created'}
            selectedTagCount={selectedTags.length}
            deleted={workspace?.deleted ?? []}
            onTags={() => setPane('tags')}
            onSelect={selectNote}
            onRestore={(noteId) => void onMutate({ operation: 'restore', noteId })}
          />
        )}
      </main>

      {menuOpen ? (
        <div className="menu-panel">
          <nav aria-label="Workspace" className="menu-section">
            {workspace?.behavior.views.map((candidate) => (
              <React.Fragment key={candidate.id}>
                <button className={activeView === candidate.id && !activeSubview ? 'menu-item active' : 'menu-item'} onClick={() => chooseView(candidate.id)}>
                  {candidate.name || candidate.id}
                </button>
                {candidate.subviews.map((subview) => (
                  <button key={`${candidate.id}/${subview.id}`} className={activeView === candidate.id && activeSubview === subview.id ? 'menu-item active subview' : 'menu-item subview'} onClick={() => chooseView(candidate.id, subview.id)}>
                    {subview.name || subview.id}
                  </button>
                ))}
              </React.Fragment>
            ))}
          </nav>
          <div className="menu-actions">
            <button className="button" disabled={busy} onClick={onRefresh}><Radio /> Sync now</button>
            <button className="button" onClick={() => { setPane('settings'); setMenuOpen(false); }}><Settings /> Settings</button>
          </div>
        </div>
      ) : null}

      <div className="bottom-search" role="search">
        <button className="icon-button menu-trigger" onClick={() => setMenuOpen((open) => !open)} aria-label={menuOpen ? 'Close navigation' : 'Open navigation'}>
          {menuOpen ? <X /> : <Menu />}
        </button>
        <Search />
        <input value={search} onChange={(event) => { onSearch(event.target.value); setPane('items'); }} placeholder="Search" aria-label="Search notes" />
        <button className="new-button" disabled={busy} onClick={startCreate} aria-label="New note"><Plus /></button>
      </div>
      <footer className="app-footer">xo {report.runtime.version}</footer>
    </div>
  );
}

function ItemsPane({ notes, viewName, subviewName, showTags, sortField, selectedTagCount, deleted, onTags, onSelect, onRestore }: {
  notes: WorkspaceNote[];
  viewName: string;
  subviewName?: string;
  showTags: boolean;
  sortField: string;
  selectedTagCount: number;
  deleted: WorkspaceNote[];
  onTags: () => void;
  onSelect: (note: WorkspaceNote) => void;
  onRestore: (noteId: string) => void;
}) {
  const groups = groupNotesByYear(notes, sortField);
  return (
    <section className="pane-shell">
      <div className="items-header notes-toolbar">
        <div><p className="legacy-eyebrow">Workspace</p><h1>{viewName}</h1><p>{notes.length} item{notes.length === 1 ? '' : 's'}{subviewName ? ` · ${subviewName}` : ''}</p></div>
        {showTags ? <button className="button" onClick={onTags}><Tags /> Tags{selectedTagCount ? ` (${selectedTagCount})` : ''}</button> : null}
      </div>
      <div className="list-pane">
        <div className="item-list note-list" aria-label="Notes">
          {groups.map(([year, groupedNotes], groupIndex) => (
            <React.Fragment key={`${year}-${groupIndex}`}>
              <h2 className="year-heading">{year}</h2>
              {groupedNotes.map((note) => (
                <button key={note.id} className="item-row note-list-item" onClick={() => onSelect(note)}>
                  <strong>{noteTitle(note)}</strong>
                  <span>{noteField(note, 'type') || note.path}</span>
                  <span className="row-tags">{note.conflict ? <i>conflict</i> : null}{noteTags(note).map((tag) => <i key={tag}>{tag}</i>)}</span>
                </button>
              ))}
            </React.Fragment>
          ))}
          {!notes.length ? <div className="empty-state">No matching items.</div> : null}
        </div>
        {deleted.length ? (
          <details className="deleted-panel"><summary>Deleted notes ({deleted.length})</summary>{deleted.map((note) => <div key={note.id}><span><strong>{noteTitle(note)}</strong><small>{note.id}</small></span><button className="button" onClick={() => onRestore(note.id)}>Restore</button></div>)}</details>
        ) : null}
      </div>
    </section>
  );
}

function TagsPane({ tags, selected, onToggle, onClear, onDone }: {
  tags: [string, number][];
  selected: string[];
  onToggle: (tag: string) => void;
  onClear: () => void;
  onDone: () => void;
}) {
  return <section className="tags-pane"><div className="tag-list">{tags.map(([tag, count]) => <button key={tag} className={selected.includes(tag) ? 'tag-row active' : 'tag-row'} onClick={() => onToggle(tag)}><span>{tag}</span><strong>{count}</strong></button>)}{!tags.length ? <div className="empty-state">No tags in this result set.</div> : null}</div><div className="tag-actions"><button className="button" disabled={!selected.length} onClick={onClear}>Clear</button><button className="button primary" onClick={onDone}>View results</button></div></section>;
}

function DetailPane({ note, titleField, onBack, onEdit, onDelete }: {
  note?: WorkspaceNote;
  titleField?: string;
  onBack: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  if (!note) return <div className="empty-state">No item selected.</div>;
  const html = DOMPurify.sanitize(marked.parse(note.body || '', { async: false }) as string);
  return (
    <article className="detail-pane note-preview">
      <div className="detail-toolbar"><button className="button back-button" onClick={onBack}><ArrowLeft /> Items</button><div><button className="button" onClick={onEdit}>Edit</button><button className="icon-button danger" onClick={onDelete} aria-label="Delete"><Trash2 /></button></div></div>
      <div className="detail-heading"><div><p className="legacy-eyebrow">{note.id}</p><h2>{noteTitle(note, titleField)}</h2><p>{note.path}</p></div></div>
      {note.conflict ? <div className="conflict-callout"><CircleAlert /><span>This item has {note.conflict.concurrent_revisions.length} concurrent revision(s). Saving joins all current heads.</span></div> : null}
      <dl className="frontmatter-grid">{Object.entries(note.frontmatter).map(([key, value]) => <div key={key}><dt>{key}</dt><dd>{displayFrontmatter(value)}</dd></div>)}</dl>
      <div className="markdown-body markdown-preview" dangerouslySetInnerHTML={{ __html: html }} />
      <details className="history-panel"><summary>Revision history ({note.history.length})</summary>{note.history.slice().reverse().map((revision) => <div key={revision.id}><code>{short(revision.id)}</code><span>{localTimestamp(revision.physicalMs)} · {short(revision.author)}{revision.deleted ? ' · deleted' : ''}</span></div>)}</details>
    </article>
  );
}

function EditorPane({ editingId, title, draft, busy, error, onTitle, onDraft, onCancel, onSave }: {
  editingId?: string;
  title: string;
  draft: string;
  busy: boolean;
  error: string;
  onTitle: (title: string) => void;
  onDraft: (draft: string) => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  return <section className="single-pane editor"><div className="detail-heading"><div><p className="legacy-eyebrow">{editingId ? `Edit ${editingId}` : 'Create item'}</p><h2>Markdown editor</h2></div><button className="icon-button" onClick={onCancel} aria-label="Close editor"><X /></button></div>{error ? <p className="error-message">{error}</p> : null}{!editingId ? <label>Title<input autoFocus value={title} onChange={(event) => onTitle(event.target.value)} /></label> : null}<label>Frontmatter and Markdown<textarea className="raw-editor" value={draft} onChange={(event) => onDraft(event.target.value)} spellCheck="true" /></label><div className="button-row"><button className="button" onClick={onCancel}>Cancel</button><button className="button primary" disabled={busy || (!editingId && !title.trim())} onClick={onSave}>{busy ? <RefreshCw className="spin" /> : <Check />} Save note</button></div></section>;
}

function SettingsPane({ report, busy, ticketVisible, onTicketVisible, onRefresh, onApprovePeer, onRejectPeer, onRemovePeer, onRestore, onWipe }: {
  report: RuntimeReport;
  busy: boolean;
  ticketVisible: boolean;
  onTicketVisible: (visible: boolean) => void;
  onRefresh: () => void;
  onApprovePeer: (fingerprint: string) => void;
  onRejectPeer: (fingerprint: string) => void;
  onRemovePeer: (fingerprint: string) => void;
  onRestore: (noteId: string) => void;
  onWipe: () => void;
}) {
  return <section className="single-pane settings-pane">
    <div className="settings-section"><p className="legacy-eyebrow">Automerge workspace</p><h2>Synchronization</h2><div className="status-grid compact"><Status icon={<Cloud />} label="Transport" value="relay-only E2EE" /><Status icon={<Radio />} label="Peers" value={String(report.status.peers)} /><Status icon={<Check />} label="Pending writes" value={String(report.pendingWrites)} /><Status icon={<KeyRound />} label="Workspace" value={short(report.status.workspaceId)} /></div><button className="button" disabled={busy} onClick={onRefresh}><RefreshCw className={busy ? 'spin' : ''} /> Sync now</button></div>
    <div className="settings-section"><p className="legacy-eyebrow">Authenticated membership</p><h2>Peers</h2>{report.pendingMembers.map((peer) => <div className="entry-row" key={peer.fingerprint}><div><strong>{peer.peerId}</strong><code>{short(peer.fingerprint)}</code></div><div className="button-row"><button className="button" disabled={busy} onClick={() => onApprovePeer(peer.fingerprint)}>Approve</button><button className="button danger-button" disabled={busy} onClick={() => onRejectPeer(peer.fingerprint)}>Reject</button></div></div>)}{report.members.map((peer) => <div className="entry-row" key={peer.fingerprint}><div><strong>{peer.peerId}</strong><span>{peer.status} · {short(peer.fingerprint)}</span></div>{peer.fingerprint !== report.status.authorId && peer.status === 'active' ? <button className="button danger-button" disabled={busy} onClick={() => onRemovePeer(peer.fingerprint)}>Remove</button> : null}</div>)}</div>
    <div className="settings-section ticket-panel"><div><p className="legacy-eyebrow">Workspace invitation</p><h2>Invitation</h2><p>New peers remain quarantined until an active member approves them.</p></div><div className="button-row"><button className="button" onClick={() => onTicketVisible(!ticketVisible)}>{ticketVisible ? 'Hide' : 'Reveal'} ticket</button><button className="button" onClick={() => void navigator.clipboard.writeText(report.ticket ?? '')}><Copy /> Copy</button></div>{ticketVisible ? <textarea className="ticket-output" readOnly value={report.ticket ?? ''} /> : null}</div>
    {report.workspace?.deleted.length ? <details className="deleted-panel"><summary>Deleted notes ({report.workspace.deleted.length})</summary>{report.workspace.deleted.map((note) => <div key={note.id}><span><strong>{noteTitle(note)}</strong><small>{note.id}</small></span><button className="button" onClick={() => onRestore(note.id)}>Restore</button></div>)}</details> : null}
    {report.workspace?.diagnostics.map((diagnostic) => <p className="error-message" key={diagnostic}>{diagnostic}</p>)}
    <details className="raw-panel"><summary>Raw Automerge records ({report.entries.length})</summary><div className="entry-list">{report.entries.map((entry) => <EntryRow key={entry.keyBase64} entry={entry} />)}</div></details>
    <div className="settings-section danger-zone"><p className="legacy-eyebrow">Local browser data</p><h2>Reset this client</h2><p>Remove the encrypted identity, workspace invitation, durable Automerge replica, pending writes, and offline application files from this browser.</p><button className="button danger-button" disabled={busy} onClick={onWipe}><Trash2 /> Wipe all browser data</button></div>
  </section>;
}

function Status({ icon, label, value }: { icon: React.ReactNode; label: string; value: string }) {
  return <div className="status-card"><span>{icon}</span><div><small>{label}</small><strong>{value}</strong></div></div>;
}

function EntryRow({ entry }: { entry: DocumentEntry }) {
  return <article className="entry-row"><div><strong>{entry.key}</strong><span>{entry.value ?? `${entry.contentLen} binary bytes`}</span></div><div className="entry-meta"><code>{short(entry.author)}</code><span>{entry.pending ? 'pending' : 'replicated'}</span></div></article>;
}

function groupNotesByYear(notes: WorkspaceNote[], sortField: string): Array<[string, WorkspaceNote[]]> {
  const groups: Array<[string, WorkspaceNote[]]> = [];
  for (const note of notes) {
    const value = noteField(note, sortField);
    const year = /^\d{4}/.exec(value)?.[0] ?? 'No year';
    const current = groups.at(-1);
    if (current?.[0] === year) current[1].push(note);
    else groups.push([year, [note]]);
  }
  return groups;
}

function noteField(note: WorkspaceNote, field?: string) {
  if (!field) return '';
  const value = note.frontmatter[field];
  return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ? String(value) : '';
}

function noteTitle(note?: WorkspaceNote, field = 'title') {
  if (!note) return 'Items';
  return noteField(note, field) || noteField(note, 'title') || 'Untitled';
}

function noteTags(note: WorkspaceNote) {
  const tags = note.frontmatter.tags;
  if (Array.isArray(tags)) return tags.filter((tag): tag is string => typeof tag === 'string');
  if (typeof tags === 'string') return tags.split(',').map((tag) => tag.trim()).filter(Boolean);
  return [];
}

function displayFrontmatter(value: FrontmatterValue) {
  const displayed = withoutDisplayedTimezone(value);
  if (displayed === null) return 'null';
  if (typeof displayed === 'object') return JSON.stringify(displayed);
  return String(displayed);
}

function withoutDisplayedTimezone(value: FrontmatterValue): FrontmatterValue {
  if (typeof value === 'string' && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(value)) {
    const milliseconds = Date.parse(value);
    if (!Number.isNaN(milliseconds)) return localTimestamp(milliseconds);
  }
  if (Array.isArray(value)) return value.map(withoutDisplayedTimezone);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, nested]) => [key, withoutDisplayedTimezone(nested)]));
  return value;
}

function localTimestamp(milliseconds: number) {
  const instant = new Date(milliseconds);
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${instant.getFullYear()}-${pad(instant.getMonth() + 1)}-${pad(instant.getDate())}T${pad(instant.getHours())}:${pad(instant.getMinutes())}:${pad(instant.getSeconds())}`;
}

function short(value?: string) {
  if (!value) return 'not connected';
  return value.length > 18 ? `${value.slice(0, 9)}…${value.slice(-7)}` : value;
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}
