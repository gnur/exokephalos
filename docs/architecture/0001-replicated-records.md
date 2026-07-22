# ADR 0001: replicated record contracts

Status: accepted for schema version 1.

The authoritative state is one Iroh Docs namespace. Entry values are canonical
CBOR. Iroh Docs stores those bytes through its attached content-addressed blob
store; exokephalos does not create a second blob containing only another hash.

## Keys

```text
note/<note-id>/revision/<revision-id>
note/<note-id>/head/<author-id>
asset/<asset-id>
config/<path>/revision/<revision-id>
config/<path>/head/<author-id>
tombstone/<target-id>/<author-id>
device/<endpoint-id>
revocation/<author-id>/<admin-author-id>
```

A revision ID is the lowercase BLAKE3 hex digest of the canonical CBOR
`NoteRevision`. A head value is a revision ID. Each author advances only its own
head. The visible revision is the asserted head with the greatest HLC; the actor
ID and then revision ID break ties. Losing heads that are not ancestors of the
winner are conflicts and remain available.

Normal retirement is enforced by conforming clients ignoring author records
after a signed cutoff. Hard revocation rotates the namespace because a device
that retains the namespace write secret cannot otherwise be prevented from
publishing.

