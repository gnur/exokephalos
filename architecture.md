# xo architecture

xo is an offline-first personal knowledge system. One Automerge document is the authoritative replica for each workspace. Canonical CBOR revisions, heads, configuration, assets, devices, tombstones, and membership events are stored directly as Automerge byte records. Native clients materialize winning records as Markdown; the PWA resolves the same records in Rust/Wasm.

## Replication and transport

Iroh Endpoint supplies authenticated end-to-end encrypted QUIC connectivity, direct native paths, relay fallback, and Pkarr addressing. Browser peers are relay-only. Iroh Gossip discovers active peers and announces signed Automerge heads; bounded `/xo/automerge/1` streams transfer signed Automerge changes. Gossip is not the record transport.

Every native workspace snapshot is atomically replaced and fsynced before writes are acknowledged. Signed change envelopes are persisted separately and restored with the snapshot. Browser snapshots, signed changes, encrypted identity material, invitations, cached records, and pending writes are durable in IndexedDB.

## Records and conflicts

Notes retain xo's immutable revision graph and per-author heads. HLC ordering selects a deterministic visible revision while concurrent revisions remain available as conflicts. Deletion and restoration are revisions. Workspace Steel configuration is replicated state rather than a projection file. Small assets are stored directly in Automerge; large-blob storage and selective synchronization are out of scope.

## Identity and membership

Each process has:

- a required human-readable peer ID;
- a persistent Ed25519 membership key, whose fingerprint is the canonical actor identity;
- a separate persistent Iroh endpoint key.

The creator writes the self-signed genesis event. Candidates submit signed requests over `/xo/join/1`; an active invitation peer validates the request and endpoint binding and records a signed approval automatically. Active members can permanently remove a key. Removed keys cannot be reactivated. Rejoining requires a fresh membership key and a new signed admission.

Every Automerge change has an Ed25519 sidecar signature binding its workspace, actor, sequence, hash, and raw bytes. Authenticated sync validates the Iroh endpoint binding, membership status, signature, actor, sequence, and revocation cutoff. A removal records accepted heads and actor sequence, rotates the membership epoch and Gossip topic, and causes informed peers to deny further synchronization. Revocation is eventually consistent for offline peers.

Invitations contain a workspace ID, bootstrap endpoint addresses, current Gossip topic, protocol version, and genesis fingerprint. They are discovery material, not bearer write capabilities. Read-only membership and namespace capabilities are intentionally unsupported.

## Components

- `xo-core`: domain records, Automerge persistence, membership, signed changes, protocols, projection, encryption, and Steel behavior.
- `xo`: native TUI, projection reconciliation, peer approval/removal, import/export, capture, and plugins.
- `xo-syncd`: durable user-level replica with health, metrics, setup, invitation, and membership operator endpoints.
- `xo-web`: static React PWA. A dedicated worker owns Rust/Wasm, Iroh, Automerge, encrypted identity, and IndexedDB persistence.
- `xo-admin`: offline import, invitation, peer administration, diagnostics, and verified backup/restore.
- `xo-lsp`: stdio editor diagnostics and completion over a native projection.

TUI and daemon use separate state directories and each mutable native state directory is protected by `.xo-workspace.lock`. The production PWA is static and has no application API or synchronization gateway.

## Security boundaries

Steel executes in a fresh bounded VM. Plugins receive only explicitly granted host capabilities and secrets. Browser identity and invitation data are AES-GCM encrypted with a non-extractable WebCrypto key. Membership keys are independent of endpoint keys, allowing endpoint rebinding without changing logical identity. Network authentication does not replace durable change signatures.

## Testing

Commit CI runs formatting, Clippy, workspace tests, Wasm builds, browser offline/restart tests, native-browser admission and convergence tests, binary matrices, containers, and deployment. Release tags additionally run the ignored 1,000-item rebuild and three-persisted-peer relay/restart/conflict scenarios. Skipped release-only tests are reported separately from successful tests.
