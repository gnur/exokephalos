# Automerge migration

Status: implemented. Iroh Docs and Iroh Blobs were removed; native and browser replicas now use the protocols and persistence model below.

## Target

Replace Iroh Docs and Iroh Blobs with one Automerge document per workspace. Keep Iroh Endpoint, relay transport, Pkarr lookup, and Gossip. Gossip discovers peers and announces document heads; all document synchronization uses authenticated Iroh QUIC streams.

Existing canonical CBOR revision, head, configuration, device, tombstone, and workspace records remain the Rust-authoritative application model and are stored directly as byte values in Automerge. Large blob storage, selective synchronization, read-only membership, and compatibility with existing Iroh Docs state are out of scope.

## Identity and admission

Every process has a required human-readable peer ID. Native clients default it to the host name; browser users enter it before creating or joining a Gossip swarm. The cryptographic identity is a separately generated Ed25519 membership key. The persistent Iroh endpoint identity is bound to that membership key.

The workspace creator writes a self-signed genesis membership event. A candidate uses `/xo/join/1` to submit its peer ID, public key, and endpoint binding. An active invitation peer automatically records a signed approval after validating the request and endpoint binding. Signed immutable approval, rejection, endpoint-binding, and removal events replicate in Automerge and are announced through Gossip.

Every `/xo/automerge/1` connection performs mutual nonce challenge-response authentication. Automerge changes have Ed25519 sidecar signatures and an actor ID derived from the membership-key fingerprint. Removal records the accepted causal frontier; informed peers close the removed peer's streams and reject later or unknown changes from that key. Revocation is necessarily eventually consistent while peers are offline.

## Persistence

Native workspaces use atomically replaced, fsynced Automerge snapshots plus a replayable incremental change log. Browser workspaces persist the real snapshot and incremental changes in IndexedDB. Writes, remote changes, import finalization, and shutdown are not acknowledged until the applicable durable write completes.

## Delivery order

1. Peer ID, membership keys, signed membership events, and Automerge record store.
2. Native snapshot/change-log persistence and a backend-independent record repository.
3. Join invitations, admission protocol, authenticated sync protocol, and signed changes.
4. Gossip discovery, membership-first reconciliation, peer state, and revocation.
5. TUI and daemon conversion, including approval and removal controls.
6. PWA worker/IndexedDB conversion and onboarding/peer controls.
7. Backup, restore, doctor, installer, and operator-flow conversion.
8. Remove Iroh Docs/Blobs and run native/browser full-mesh, restart, revocation, and 1,000-item release tests.
