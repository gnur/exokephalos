# Iroh-backed PWA implementation plan

## Goal

Build `xo-web` as the primary, greenfield browser application. The old Go/web
codebase is non-normative and requires no API, storage, behavior, or UI
compatibility. Production deployment consists only of versioned static assets;
all workspace logic executes in the browser.

The application should:

- remain installable as a PWA and work offline;
- join the same writable workspace as `xo`, `xo-syncd`, and other peers;
- use the current record, revision, conflict, encryption, and behavior semantics
  from `xo-core` rather than reimplementing them in TypeScript;
- run user-defined Steel actions locally in the browser, with the maximum power
  the browser security model permits; and
- use `xo-syncd` as a continuously available peer, not as a remote action
  executor or required application server.

A PWA cannot provide desktop operating-system privileges. Arbitrary Scheme
computation and browser APIs are possible; arbitrary processes, dynamic
libraries, raw sockets, and unrestricted filesystem access are not. If those
OS capabilities become a requirement, package the same web UI in Tauri or
Electron as an additional desktop target.

## Recommended architecture

```text
xo-web React UI
        |
        | typed message RPC
        v
Dedicated Web Worker
        |
        | wasm-bindgen exports and browser host functions
        v
crates/xo-web (Rust WebAssembly facade)
        |
        +------> Steel VM and xo browser host API
        |
        v
portable xo-core workspace service
        |
        v
Iroh Docs / Blobs / Gossip (browser endpoint, relay-only)
        |
        v
xo-syncd and native xo peers

IndexedDB
  - browser identity and workspace capabilities
  - durable record/blob cache
  - pending local writes and recovery metadata
```

Run Rust, Iroh, and Steel in a dedicated worker. The UI thread should only
render React and issue coarse commands. A service worker remains responsible
for the application shell and static asset cache; it must not own the Iroh
endpoint because browsers may suspend or terminate it at any time.

The browser should be a real Iroh participant while the page is active. Iroh's
browser implementation is relay-only: browser peers cannot use UDP or hole
punching, but connections remain end-to-end encrypted. Keep `xo-syncd` online
as the stable peer that allows short-lived browser sessions to converge.

Iroh does not currently publish an npm browser package. Build an
application-specific Rust wrapper with `wasm-bindgen`, as recommended by the
[Iroh browser documentation](https://docs.iroh.computer/languages/wasm-browser).

## Feasibility findings and hard gates

### Iroh

Iroh officially supports browser WebAssembly, and `iroh`, `iroh-docs`,
`iroh-blobs`, and `iroh-gossip` contain browser-specific code paths. This does
not mean the current `xo-core` feature graph is browser-ready.

A direct check of the current tree with:

```text
cargo check -p xo-core --target wasm32-unknown-unknown \
  --features iroh-sync --locked
```

fails because the workspace enables native defaults and Tokio's `net`,
`signal`, and multithreaded runtime features, which pull in unsupported `mio`
networking. Iroh's documentation specifically requires disabling its default
features for browser builds. The Docs and Blobs defaults also enable native
filesystem/RPC features.

The initial spike must establish a browser-specific dependency graph with:

- Iroh default features disabled;
- Docs/Blobs filesystem and RPC features disabled;
- browser-compatible task spawning and timers;
- an in-memory protocol store; and
- explicit IndexedDB persistence around browser identity, capabilities,
  records, blobs, and pending writes.

Native `IrohNode` uses files, Redb, and the filesystem blob store, so the
browser composes an in-memory Docs/Blobs/Gossip node and surrounds it with an
IndexedDB recovery layer. The implemented Phase 0 persists the endpoint and
author keys, writable ticket, document entry cache, and pending writes; it
restores the same identity, reimports the capability, replays unsent writes,
and resumes relay synchronization after reload.

The direct-Wasm spike now passes: two isolated browser contexts converge through
a native Iroh peer, and an offline reload restores cached data and pending
writes. `xo-syncd` remains an ordinary native peer. No browser sync gateway or
server-side workspace API is required.

### Steel

A minimal crate that constructs `steel::steel_vm::engine::Engine::new_sandboxed()`
successfully passes `cargo check` with `steel-core 0.8.2` for
`wasm32-unknown-unknown`. This proves that the interpreter can be included in a
browser Wasm build; it does not yet prove bundle size, runtime performance, or
all host integrations.

The browser can support:

- arbitrary Scheme control flow, functions, recursion, collections, and
  computation supported by Steel;
- loading synced action source into a fresh interpreter;
- reading the selected note and workspace query results;
- adding/removing tags and reading/writing recursive frontmatter;
- editing note bodies and producing multiple note mutations;
- calling explicitly registered browser host functions;
- browser `fetch` through a permissioned asynchronous host adapter;
- browser storage through a permissioned IndexedDB adapter; and
- UI requests such as notifications or prompts through typed messages.

The browser cannot provide:

- shell commands or child processes;
- native dynamic libraries or Steel dylib loading;
- unrestricted local filesystem paths;
- arbitrary TCP/UDP sockets or listening ports;
- direct access to environment variables or host secrets; or
- reliable execution after the browser suspends or closes the page.

This is effectively full Steel language power inside the browser sandbox, not
full desktop-machine power. Browser APIs should be exposed as named Steel host
modules rather than by allowing scripts to call arbitrary JavaScript.

## Steel action runtime

Replace the current action-only declarative parser with two supported modes:

1. **Declarative actions** remain fast, deterministic, and compatible with
   every client.
2. **Scripted actions** contain arbitrary Steel source and execute in the local
   Steel VM.

A scripted action descriptor should include at least:

```text
id
name/description
predicate
entry function
source or module path
requested capabilities
optional execution limits
```

The worker creates a fresh engine for each invocation, loads the pinned source,
registers only granted host modules, invokes the entry function with an
immutable note/workspace value, and receives a transaction result. All writes
must then pass through the shared Rust workspace service.

Suggested host capability groups:

- `note:read`
- `note:write`
- `workspace:query`
- `workspace:write`
- `browser:fetch` with origin rules
- `browser:storage` with a namespaced quota
- `browser:clipboard`
- `browser:notification`
- `browser:prompt`

Capabilities should communicate intent and drive user consent. They are not a
strong security boundary if a bug gives Steel or Wasm access to an unintended
host import, so keep the import surface small.

For safety and recoverability:

- execute Steel off the UI thread;
- pin each invocation to the action source hash and base record revisions;
- collect mutations into a transaction before committing;
- reject stale writes or surface normal xo conflicts;
- record action ID, source hash, input revisions, and output revision IDs;
- terminate and recreate the worker to stop runaway scripts;
- enforce input/output and mutation-count limits even if CPU limits are
  initially coarse;
- never allow Steel to mutate IndexedDB or Iroh internals directly; and
- show a clear trust warning before executing newly synced or changed scripts.

A Web Worker can be terminated, but Wasm currently offers no universal safe
preemption mechanism inside a long-running synchronous call. Phase 1 must test
Steel fuel/interrupt facilities; if unavailable, run scripted actions in a
separate disposable worker so cancellation can terminate the entire worker.

## Greenfield UI scope

Useful design ideas may be reimplemented without retaining old APIs or data
contracts:

- React screen structure and responsive layout;
- view/subview navigation;
- item list, editor, Markdown preview, search, and tag filtering;
- create/edit/delete interactions;
- action menus and action error display;
- PWA manifest, installability, and application-shell caching;
- typography, icons, and Playwright coverage; and
- IndexedDB as browser durability infrastructure.

Do not carry forward:

| Old implementation | Replacement |
| --- | --- |
| Go `/api/app/*` CRUD | `xo-web` Wasm commands |
| HTTP sync v2 push/pull | Browser Iroh endpoint |
| `EventSource('/api/events')` | Worker events derived from Iroh subscriptions |
| TypeScript HLC/operation generation | Shared Rust record/revision logic |
| Dexie `syncOps` protocol | Rust record recovery queue |
| Server `runAction` API | Local Steel Wasm execution |
| Server login/password | Local workspace unlock |
| Fennel/Lua config screens | Steel editor and diagnostics |
| API-key/client admin | Current peer diagnostics |

Browser libraries such as `react-dom`, Dexie, DOMPurify, `marked`, WebCrypto,
and normal `fetch` remain useful. Keep Markdown sanitization and apply a
restrictive Content Security Policy.

## Rust/Wasm facade

Create `crates/xo-web` as a `cdylib`/`rlib` with `wasm-bindgen` exports. It should
own the browser endpoint and workspace service. Prefer coarse asynchronous
operations:

```text
initialize(persisted_state)
create_workspace()
join_workspace(ticket)
resume_workspace(workspace_id)
close_workspace()
export_recovery_state()
query_notes(query)
get_note(note_id)
create_note(draft)
save_note(draft, expected_revision)
delete_note(note_id)
restore_note(note_id)
list_actions(note_id)
run_action(action_id, note_id, parameters)
cancel_action(invocation_id)
list_conflicts()
resolve_conflict(resolution)
revision_history(note_id)
sync_status()
refresh_sync()
```

Use serializable DTOs at the Wasm boundary and preserve recursive frontmatter
types. Return stable error codes in addition to messages. Worker events should
invalidate React queries rather than stream every low-level Iroh event.

Extract a projection-independent workspace service from the current desktop
`WorkspaceSession`; browser clients do not have a Markdown directory
projection. The service must own snapshot loading, record commits, revisions,
conflicts, behavior, encryption, and Iroh event application.

## Browser persistence and security

Store in IndexedDB:

- endpoint secret and author identity;
- workspace ID and encrypted read/write capability;
- latest verified records and required blobs;
- pending Rust-generated writes and tombstones;
- recovery/checkpoint version; and
- trusted action source hashes and capability decisions.

Use a non-extractable WebCrypto key where practical, with passphrase wrapping
for portable recovery. Never place writable tickets, endpoint secrets, note
keys, or plaintext encrypted-note keys in `localStorage`, URLs, analytics, or
logs.

The cache schema needs explicit migrations and crash tests. A local mutation is
successful only after its record and recovery metadata are durably committed to
IndexedDB. Network publication may happen afterward.

Additional controls:

- strict CSP with no `unsafe-eval` and narrowly scoped Wasm allowance;
- DOMPurify for rendered Markdown;
- fetch capability origin allowlists and visible network consent;
- encrypted export/import for browser recovery state;
- action-source hash shown in trust prompts and audit history; and
- graceful handling when storage persistence is denied or evicted.

## Delivery phases

### Phase 0 — browser feasibility gate (complete)

- [x] Add a minimal `xo-web` Wasm crate and browser-specific dependency features.
- [x] Compile Iroh Docs/Blobs/Gossip and Steel for Wasm in CI.
- [x] Start an Iroh browser endpoint and connect through a relay.
- [x] Join a workspace hosted by `xo-syncd` and synchronize browser writes across two browser identities.
- [x] Persist encrypted identity and capability plus the document cache and pending writes in IndexedDB.
- [x] Close/reload offline and prove cached state and unsent writes survive.
- [x] Execute Steel in the dedicated Wasm worker.
- [x] Record production bundle size in CI output; the combined Iroh/Steel Wasm is currently about 11.1 MiB raw and 3.9 MiB compressed.

### Phase 1 — shared browser workspace core

- Extract the projection-independent workspace service.
- Add target-specific dependency features and browser task abstractions.
- Implement Rust-generated note mutations, tombstones, revisions, and conflict
  handling against the IndexedDB recovery layer.
- Add worker RPC, structured errors, cancellation, and sync-status events.
- Add native/Wasm parity tests for record encoding and conflict resolution.

### Phase 2 — read-only PWA

- Point views, subviews, search, tags, list, detail, and history at Wasm queries.
- Load `xo.scm` and display configuration diagnostics.
- Verify offline reload and installability on desktop and mobile browsers.

### Phase 3 — editing and synchronization

- Route create/edit/delete/restore through the Rust facade.
- Add optimistic revision checks and conflict UI.
- Expose Iroh status and peer diagnostics through worker events.

### Phase 4 — full Steel actions

- Add scripted action descriptors and source/module loading.
- Implement immutable input values and transactional mutation output.
- Add browser host modules, capability prompts, trust hashes, and audit records.
- Add disposable action workers, cancellation, and resource limits.
- Build an in-app Steel editor with parse/runtime diagnostics.
- Test actions across reloads, offline execution, concurrent edits, and changed
  source hashes.

### Phase 5 — hardening and release

- Add multi-browser Playwright coverage against a real `xo-syncd` peer.
- Test Chromium, Firefox, and WebKit, including installed PWA behavior.
- Test IndexedDB migration, quota exhaustion, eviction, crash recovery, and
  corrupt checkpoints.
- Audit CSP, Markdown rendering, secret handling, fetch permissions, and Steel
  host imports.
- Verify the production artifact contains only static assets and needs no application server.

## CI matrix

Add jobs for:

- `cargo check`/Clippy for the native workspace;
- `cargo check --target wasm32-unknown-unknown` for `xo-web`;
- `wasm-bindgen`/`wasm-pack` release build and bundle-size budget;
- TypeScript typecheck, ESLint, and production Vite build;
- unit tests for worker RPC and IndexedDB migrations;
- Playwright Chromium/Firefox/WebKit offline tests; and
- a networked browser ↔ `xo-syncd` ↔ native `xo-core` convergence test.

Pin Rust, Node, browser, and Wasm tooling versions. Browser protocol tests must
use the same Iroh versions as native peers.

## Acceptance criteria

The implementation is complete when:

- the PWA creates or joins a current writable workspace using a ticket;
- browser, TUI, and `xo-syncd` converge on notes, tombstones, and conflicts;
- notes can be created and edited offline, survive a full browser restart, and
  synchronize later;
- no HTTP CRUD API, SSE dependency, or server-side action executor is introduced;
- arbitrary Steel action code runs locally in the browser worker;
- Steel can query notes and transactionally update tags, frontmatter, and body;
- browser API access is explicit and permissioned;
- changed synced action code requires renewed trust;
- runaway action execution can be cancelled without freezing the UI;
- no workspace capability or key leaks into URL/localStorage/logs; and
- supported-browser end-to-end and native/Wasm parity tests pass in CI.

## Initial pull-request sequence

1. **Wasm feasibility:** minimal `xo-web`, browser dependency graph, Steel VM
   probe, and browser ↔ `xo-syncd` Iroh test.
2. **IndexedDB recovery:** identity/capability/cache persistence and offline
   reload convergence test.
3. **Workspace service extraction:** projection-independent queries and commits.
4. **Worker facade:** typed RPC, events, cancellation, and sync state.
5. **Read UI:** views, list, detail, search, tags, and diagnostics.
6. **Write UI:** CRUD, revisions, conflicts, blobs, and offline writes.
7. **Steel actions:** arbitrary scripts, transactional host API, permissions,
   audit history, and disposable workers.
8. **Release hardening:** verify static-only deployment, complete browser
   security review, and pass supported-browser release tests.
