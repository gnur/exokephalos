# ADR 0001: replicated record contracts

Status: accepted for schema version 1.

The authoritative state is one durable Automerge document hosted by `xo-syncd`.
Native and browser clients keep durable local copies and exchange opaque Automerge
sync messages through `/api/sync`. Entry values are canonical CBOR; content bytes
are stored directly in the same Automerge record store.

## Keys

```text
note/<note-id>/revision/<revision-id>
note/<note-id>/head/<author-id>
asset/<asset-id>
asset-blob/<asset-id>
config/<path>/<revision-id>
config-blob/<revision-id>
tombstone/<target-id>/<author-id>
```

A revision ID is the lowercase BLAKE3 hex digest of the canonical CBOR
`NoteRevision`. Each author advances only its own head. The visible revision is
the asserted head with the greatest HLC; actor ID and revision ID break ties.
Losing heads that are not ancestors of the winner are conflicts and remain
available.

Client IDs are presence labels only. Authorization is outside the record model:
`xo-syncd` trusts direct clients and relies on an authenticating HTTPS reverse
proxy for browser deployments.
