# React Native application plan

## Goal

Build an iOS-first React Native application that keeps the interaction model of
`oldcodebase/web`, but uses the current Rust/Iroh architecture rather than the
old Go HTTP API, IndexedDB outbox, and SSE synchronization protocol.

The mobile app should be a real xo peer:

- notes remain available and editable offline;
- Rust owns document state, revision creation, conflict resolution, encryption,
  and Iroh synchronization;
- React Native owns navigation, presentation, forms, and short-lived UI state;
- the app joins the same workspace as the TUI and `xo-syncd` with an Iroh
  ticket; and
- the first MVP targets iOS, while keeping the native boundary suitable for an
  Android implementation.

## Recommended architecture

```text
React Native / TypeScript UI
        |
        | typed native commands, DTOs, and events
        v
Swift React Native native module
        |
        | UniFFI-generated Swift bindings
        v
crates/xo-mobile (mobile facade and runtime ownership)
        |
        v
crates/xo-core (records, behavior, encryption, conflicts, Iroh)
        |
        v
Iroh Docs + Iroh Blobs + local persistent stores
```

Use a bare React Native application, or an Expo prebuild/custom development
client. Expo Go cannot load the required Rust native library. Start with a thin
Swift native module over UniFFI rather than exposing low-level Iroh objects to
JavaScript. The same UniFFI contract can later generate Kotlin bindings for
Android.

Do **not** port the old browser synchronization implementation. In particular,
Dexie, the browser outbox, HTTP bootstrap/push/pull, and SSE are obsolete in the
current architecture. There should be one authoritative local store, owned by
Rust, rather than a Rust store plus a second IndexedDB/SQLite cache in
JavaScript.

## What to reuse from the old React web UI

Reuse the product structure and visual behavior, not the DOM implementation.

| Old web feature | Mobile treatment |
| --- | --- |
| View/subview menu | React Navigation drawer or modal driven by `WorkspaceBehavior` |
| Bottom search and create button | Keep as the primary mobile navigation pattern |
| Tag pane and conjunctive tag filters | Rebuild as a tag screen/bottom sheet; query in Rust |
| Item list and detail view | Rebuild with native lists and screens |
| Raw Markdown editor | Rebuild with a multiline native editor; add richer editing later |
| Markdown preview | Use a React Native Markdown renderer with explicit link/image handling |
| Configured actions | Render descriptors from Rust and execute them in Rust |
| Offline/sync warning | Drive from native Iroh lifecycle and sync status events |
| Create/edit/delete | Call the Rust facade; do not enqueue a JavaScript outbox operation |
| Encryption UX | Keep unlock/encrypt flows, but call `xo-core` encryption functions |
| Conflict/device/sync settings | Rebuild against current `xo-core` records and diagnostics |
| API keys, browser password, old sync-client approval | Do not port; these belonged to the old Go web server |
| URL import/Hardcover integration | Defer until equivalent current Rust services exist |
| Fennel/Lua settings editor | Replace with read-only `xo.scm` diagnostics initially |

Pure design tokens, labels, icons, and small TypeScript formatting helpers can
be adapted. `react-dom`, CSS, browser history, Dexie, DOMPurify, `marked`,
WebCrypto, `hash-wasm`, `fetch('/api/...')`, and `EventSource` cannot be reused
as-is.

## Steel on iOS

### Short answer

Yes, the current workspace configuration model can work on iOS. The mobile app
should not need to ship or invoke the full Steel VM for workspace behavior.

`SteelWorkspace::load` currently parses the restricted, declarative
`workspace-config` and `workspace-module` forms with `NativeWorkspaceParser`.
It produces the portable Rust `WorkspaceBehavior` model. Views, predicates,
actions, templates, and capability grants are then evaluated by normal Rust
code in `behavior.rs`. This path does not execute arbitrary downloaded Scheme.

The only current path that creates `Engine::new_sandboxed()` is
`evaluate_xo_config`, which reads the desktop command file
`~/.config/xo/config.scm`. An iOS app has native settings and does not need that
file or VM path.

A direct `cargo check` of `steel-core 0.8.2` for `aarch64-apple-ios` succeeds
with its current default features; JIT and dynamic-library features are not
enabled. A complete `xo-core` + Iroh device build still needs to be proven on a
macOS/Xcode runner because an iOS SDK and `xcrun` are unavailable in the current
Linux development environment. Iroh's official
[compatibility matrix](https://docs.iroh.computer/compatibility) lists iOS as a
supported platform.

### Recommended refactor before the mobile build

Split the current `steel_runtime.rs` responsibilities:

1. Move the native workspace parser, encoder, size limits, and
   `SteelWorkspace::load` behavior into an always-portable module such as
   `workspace_config`.
2. Keep `evaluate_xo_config` and its `steel-core` dependency behind a separate
   `steel-vm` or `desktop-config` feature.
3. Build `xo-mobile` with `workspace-config` and without `steel-vm`.
4. Add parity tests proving the desktop/TUI and mobile feature sets decode the
   same `xo.scm` and produce identical serialized `WorkspaceBehavior` values.

This reduces binary size and App Store risk. Apple's
[App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/)
restrict downloading and executing code that changes application
functionality. xo's current
allow-listed declarative forms are data interpreted by fixed, reviewed Rust
logic, which is a much safer model than exposing general Steel evaluation. Do
not enable Steel JIT, dynamic libraries, filesystem/process primitives, or
arbitrary synced Scheme evaluation in the iOS target.

## Native Rust facade

Create `crates/xo-mobile` with `staticlib`/`cdylib` output and a UniFFI API. It
should own a Tokio runtime and keep all Iroh handles off the JavaScript thread.
Expose coarse operations rather than one FFI call per record.

Initial API surface:

```text
initialize(app_support_directory)
create_workspace()
join_workspace(writable_or_read_only_ticket)
resume_workspace()
close_workspace()
workspace_summary()
query_notes(query)
get_note(note_id)
create_note(draft)
save_note(note)
delete_note(note_id)
restore_note(note_id)
run_action(action_id, note_id)
list_conflicts()
revision_history(note_id)
list_devices()
sync_status()
refresh_sync()
subscribe_events(listener)
```

Use typed DTOs for stable fields and serialized JSON only where UniFFI types
would make recursive frontmatter unnecessarily awkward. Version every DTO/API
contract. Map Rust errors to stable error codes plus a human-readable message.
Never expose Rust panics over the boundary.

The facade should emit coalesced events such as:

- `workspace-changed`;
- `sync-status-changed`;
- `conflicts-changed`;
- `configuration-changed`; and
- `fatal-storage-error`.

React Native should respond by invalidating/reloading a screen query. Avoid
sending every low-level Iroh event over the bridge.

## Core refactors required

`WorkspaceSession` currently combines replicated document behavior with the
desktop Markdown projection. Mobile has no workspace-wide filesystem
projection, so extract a projection-independent service into `xo-core`:

- workspace selection/create/import/resume;
- snapshot and behavior loading;
- note create/save/delete/restore;
- predecessor collection and conflict-closing commits;
- action execution;
- device/history/conflict queries; and
- sync lifecycle/status.

The TUI session should compose that service with `ProjectionState`; the mobile
facade should use it without a projection. This prevents the mobile app from
reimplementing revision, HLC, signing, capability, or conflict semantics.

Also add a subscription API around Iroh Docs events so mobile snapshots can be
refreshed after remote changes without polling aggressively.

## iOS lifecycle and security

- Store Iroh state under the app's Application Support directory, not in the
  React Native bundle or Documents directory.
- Pass the directory from Swift into Rust; Rust must not infer desktop paths.
- Apply an appropriate iOS file-protection class to endpoint keys, Docs state,
  blob data, and local indexes.
- Treat writable tickets as capabilities. Accept paste/QR input directly into
  the native call, redact it from logs, never place it in AsyncStorage, and
  zeroize temporary Rust buffers where practical.
- Keep note passphrases in memory by default. If “remember passphrase” is added,
  use Keychain and make it an explicit opt-in.
- Keep decrypted note bodies out of analytics, crash reports, logs, navigation
  state, and persistent JavaScript stores.
- Validate links before opening them and explicitly resolve workspace asset
  references through the Rust facade.
- Run synchronization while the app is foregrounded. On backgrounding, flush
  durable state and allow iOS to suspend the process. iOS does not permit a
  permanent user-space daemon; `xo-syncd` remains the always-on peer.
- Add best-effort `BGAppRefreshTask` support only after foreground sync is
  reliable. It must be treated as opportunistic, not as a delivery guarantee.

## Product scope

### MVP

- Join an existing workspace by pasted ticket or QR code.
- Resume the workspace without asking for the ticket again.
- View configured views and subviews.
- Search titles and filter by tags using Rust behavior queries.
- List, preview, create, edit, and delete Markdown notes.
- Work offline and synchronize when foregrounded.
- Show sync state and actionable errors.
- Show conflicts and let a save create a revision descending from every branch.
- Unlock and edit encrypted note bodies with the existing xo format.

### After MVP

- Create the first workspace on mobile and pair it with `xo-syncd`.
- Asset/image picker, blob upload, and rendered workspace images.
- Restore deleted notes and richer revision-history inspection.
- Configured templates and all declarative action effects.
- Device retirement and diagnostics.
- Background refresh and optional push-assisted wakeups.
- Android packaging through the same UniFFI facade.
- Rich Markdown editing, share extension, deep links, and document import.

### Explicitly deferred

- Continuous background synchronization on iOS.
- Recreating the old Go server API, API-key, and password screens.
- Editing or executing arbitrary Steel code on-device.
- A second JavaScript-owned persistent copy of the workspace.
- General filesystem projection semantics on iOS.

## Delivery phases

### Phase 0: platform proof

- [ ] Add a macOS CI job with the Rust iOS simulator and device targets.
- [ ] Build `steel-core`, `xo-core` with Iroh, and a minimal static library for
      `aarch64-apple-ios` and `aarch64-apple-ios-sim`.
- [ ] Create a minimal React Native app that calls `xo_mobile_version()` through
      Swift/UniFFI on a simulator and physical device.
- [ ] Start and stop an Iroh endpoint repeatedly across foreground/background
      transitions.
- [ ] Measure clean binary size, startup time, idle memory, and battery/network
      behavior.

**Exit criterion:** an iOS device can create persistent Iroh state, restart the
app, and reopen the same endpoint identity without crashes or data loss.

### Phase 1: portable core boundary

- [ ] Split native workspace parsing from the optional Steel VM.
- [ ] Extract projection-independent workspace/session operations from the TUI.
- [ ] Add the `xo-mobile` crate and versioned UniFFI DTOs.
- [ ] Add create/join/resume, snapshot, query, save, delete, and shutdown calls.
- [ ] Add Rust contract tests that run the same workflow through the direct core
      API and mobile facade.

**Exit criterion:** a Rust integration test can create a workspace, join a
second mobile facade, edit offline, reconnect, and converge with TUI-compatible
records.

### Phase 2: React Native shell based on the old web UI

- [ ] Create `apps/mobile` as a TypeScript React Native application.
- [ ] Implement onboarding, ticket paste, QR scanning, and workspace loading.
- [ ] Build the header, bottom search/create controls, view menu, list, detail,
      and tag-filter screens.
- [ ] Establish design tokens from `oldcodebase/web/src/styles.css` instead of
      copying CSS declarations.
- [ ] Add loading, empty, offline, syncing, and fatal-storage states.
- [ ] Add accessibility labels, Dynamic Type support, safe-area handling, dark
      mode, and iPad layouts from the start.

**Exit criterion:** the app can browse an existing workspace completely offline
and its navigation covers the old web UI's core list/detail flow.

### Phase 3: mutations and behavior parity

- [ ] Implement create, raw Markdown edit, delete, and restore.
- [ ] Execute view predicates, sorting, tag facets, and actions in Rust.
- [ ] Add encrypted-note unlock/edit/save through `xo-core`.
- [ ] Add optimistic UI only where rollback is deterministic; otherwise reload
      from the committed Rust snapshot.
- [ ] Add configuration diagnostics and an unsupported-schema screen.

**Exit criterion:** TUI and mobile produce equivalent notes and query results
for shared fixture workspaces, including encrypted notes.

### Phase 4: synchronization and conflicts

- [ ] Connect Iroh Docs subscriptions to coalesced native events.
- [ ] Implement foreground connect/resume/retry and app lifecycle handling.
- [ ] Add sync status, missing-blob, and peer/error diagnostics.
- [ ] Build a conflict screen showing the deterministic winner and concurrent
      revisions.
- [ ] Test edit/edit, edit/delete, rename/edit, three-peer, offline restart, and
      stale-ticket scenarios against a real `xo-syncd`.

**Exit criterion:** a TUI, mobile device, and `xo-syncd` pass an automated
three-peer offline-conflict convergence scenario.

### Phase 5: assets and mobile polish

- [ ] Add image selection/camera permissions only when the feature is used.
- [ ] Store assets through Iroh Blobs and render them through a controlled native
      URL/data bridge.
- [ ] Add revision history, deleted-note recovery, sharing, and deep links.
- [ ] Add optional background refresh and verify graceful suspension at every
      point in a sync.
- [ ] Profile large workspaces and virtualize lists; avoid loading all bodies
      for list screens.

**Exit criterion:** representative large workspaces meet documented startup,
scroll, memory, and sync targets on the oldest supported iPhone.

### Phase 6: release hardening

- [ ] Add unit tests for TypeScript selectors/components and Rust facade logic.
- [ ] Add simulator end-to-end tests plus physical-device smoke tests.
- [ ] Run migration, corruption, low-disk, no-network, relay-only, process-kill,
      and interrupted-blob tests.
- [ ] Produce privacy manifests, permission strings, export-compliance answers,
      App Store review notes, and a demo workspace/ticket.
- [ ] Confirm no secrets or decrypted bodies enter logs/crash reporting.
- [ ] Distribute internal builds, then TestFlight, before App Store submission.

**Exit criterion:** all native, Rust, end-to-end, security, and recovery gates
pass from a clean checkout and a fresh install.

## CI additions

Add separate jobs rather than slowing every existing Rust build:

1. Linux: Rust facade/unit tests without Apple packaging.
2. macOS: `cargo check`/build for iOS device and simulator targets.
3. macOS: generate UniFFI bindings and fail on an uncommitted binding diff.
4. macOS: `xcodebuild` the React Native iOS application.
5. Simulator: launch, create/import a fixture workspace, edit, restart, and
   verify persistence.
6. Nightly or release-gated: real `xo-syncd` network/convergence tests and a
   physical-device smoke suite.

Cache Rust dependencies and Xcode/Pods carefully, but never cache generated
endpoint keys or test workspace capabilities across jobs.

## Key risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Iroh behaves differently under iOS suspension | Prove lifecycle behavior in Phase 0; foreground-first sync model |
| Rust/React Native bridge becomes chatty | Coarse commands, snapshot queries, and coalesced events |
| Divergent TUI/mobile conflict behavior | Keep commits, HLC, resolution, and actions in shared Rust code |
| Steel increases size or review risk | Ship native declarative parser only; no VM/JIT/dylibs on mobile |
| Old web UI encourages obsolete server assumptions | Reuse UX only; delete API/Dexie/SSE concepts from mobile design |
| Large workspaces overwhelm the JS bridge | Rust-side filtering/paging and summary DTOs without note bodies |
| Tickets or plaintext leak into JS persistence | Native ingestion, redacted logs, Keychain only by explicit opt-in |
| Background sync is unreliable | Treat `xo-syncd` as always-on peer and background tasks as best effort |
| iOS native build is discovered too late | Device/simulator compile and lifecycle spike before UI work |

## First implementation pull requests

1. **Mobile target spike:** macOS CI, iOS targets, minimal `xo-mobile` static
   library, UniFFI Swift call, and simulator app.
2. **Portable config split:** remove `steel-core` from the mobile feature graph
   while preserving `xo.scm` behavior parity.
3. **Workspace service extraction:** projection-independent create/join/query/
   mutate API shared by TUI and mobile.
4. **Mobile facade:** versioned DTOs, lifecycle, event listener, and Rust tests.
5. **React Native shell:** onboarding plus old-web-inspired view/list/detail
   navigation using fixture DTOs.
6. **Live workspace integration:** replace fixtures with the native facade and
   add ticket join/resume.
7. **Mutation/conflict/encryption:** complete the MVP write path and end-to-end
   three-peer tests.
