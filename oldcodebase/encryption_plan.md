# Per-note encrypted bodies

## Summary

Encrypt only the Markdown body for notes with plain frontmatter
`encrypted: true`. Each encrypted note has its own passphrase; clients prompt
on every unlock or encryption and discard the passphrase, derived key, and
decrypted body immediately after rendering or saving. There is no recovery: a
forgotten passphrase makes the body unrecoverable. Assets remain plaintext.

Use Argon2id to derive a 32-byte key and AES-256-GCM to encrypt and
authenticate the body. Store a versioned envelope in the body:

```
exo-encrypted:v1:<base64url JSON>
```

The JSON includes KDF parameters, a 16-byte salt, a 12-byte nonce, and the
ciphertext including the 128-bit GCM tag. Bind encryption to
`exo-encrypted:v1` plus the immutable note ID as AES-GCM associated data, so
ciphertext cannot be copied to another note. Default Argon2id parameters:
64 MiB memory, 3 iterations, 4 lanes, and a 32-byte key.

## Implementation changes

### Shared encryption contract

- Add a versioned encryption contract and cross-language test vectors.
- Add a Go helper package for envelope parsing, Argon2id derivation,
  AES-GCM encryption/decryption, malformed-envelope errors, and best-effort
  byte-buffer clearing.
- Add the equivalent PWA implementation, using Web Crypto for AES-GCM and an
  Argon2id implementation compatible with the Go parameters and base64url
  encoding.
- Treat `encrypted: true` bodies without a valid envelope as locked/corrupt;
  never fall back to interpreting them as plaintext.

### Opaque server and sync handling

- Keep the sync service, caches, SQLite server records, HTTP APIs, events,
  snapshots, and filesystem writes opaque: they transport and persist the
  ciphertext body exactly as supplied.
- Do not add decryption keys, crypto operations, plaintext indexes, or server
  validation that requires decryption.
- Preserve encrypted bodies unchanged for exports, imports, API updates, and
  metadata-only CEL actions. Legacy server-rendered views display ciphertext
  rather than trying to render it.

### PWA experience

- Add an "Encrypt note" action for existing plaintext notes and an encrypted
  option during creation. Both set `encrypted: true` and encrypt the body
  before it reaches IndexedDB, the outbox, or the server.
- While locked, note list/detail views expose only frontmatter. Opening or
  editing prompts for the note passphrase, decrypts only in component memory,
  and renders/edits plaintext there.
- Saving encrypts with a fresh salt and nonce, then clears plaintext state and
  passphrase references. Cancel, close, and route changes also clear them.
- Never place decrypted body text, passphrases, or derived keys in Dexie,
  localStorage, sessionStorage, service-worker caches, logs, or error
  messages.
- Exclude locked encrypted bodies from PWA text search; title, tags, and other
  frontmatter remain searchable and CEL-queryable.

### TUI experience

- Add action-menu commands to encrypt a selected plaintext note and to
  unlock/view or edit an encrypted note.
- Prompt for the passphrase on every encrypted preview or edit. Preview
  plaintext exists only in memory. For editing, create a uniquely named
  plaintext temporary file with mode `0600`, pass that path to `$EDITOR`, then
  read it back after the editor exits, encrypt the edited body with a fresh
  salt and nonce, atomically write the ciphertext note, and remove the
  temporary file on every success, cancel, and error path.
- Create the temporary file under the operating system temporary directory;
  never use the workspace, `.exo`, or the cache directory. Do not preserve it
  for recovery. Best-effort overwrite its contents before removal where the
  platform permits, while documenting that filesystem snapshots, backups, and
  editor swap/backup files can still retain plaintext.
- Locked previews show metadata plus a clear locked-body placeholder. Render
  Markdown only after successful decryption.
- Continue storing the encrypted envelope in the local markdown file/cache and
  syncing it unchanged.

## Test plan

- Cross-language fixtures prove PWA-produced envelopes decrypt in Go and
  Go-produced envelopes decrypt in the PWA. Cover wrong passphrases, modified
  ciphertext/tag, changed note ID, malformed fields, and unsupported versions.
- PWA tests cover encryption on create/convert, lock/unlock, cancel, edit/save
  re-encryption, no plaintext persistence in Dexie/outbox, and
  frontmatter-only search.
- TUI tests cover passphrase prompts, locked fallback, decrypted preview,
  external-editor temporary-file permissions/location/cleanup, edited-body
  re-encryption, and transient-state cleanup.
- Sync integration tests assert server records, snapshots, events, and a
  second workspace contain the exact ciphertext envelope and never plaintext.
- Existing CEL/action tests verify frontmatter queries and metadata-only
  mutations continue to work for encrypted notes. Asset behavior is unchanged.

## Assumptions

- `encrypted: true` is the plaintext marker. Titles, tags, types, dates, and
  all other frontmatter deliberately remain visible to the server and clients.
- Per-note passphrases are neither remembered nor recoverable. Changing a
  passphrase requires unlocking and re-encrypting the note.
- This protects bodies at rest and from the sync server, but cannot protect
  plaintext while a user actively views or edits it, including the temporary
  file used by the external editor, or from a compromised client/browser.

## Algorithm rationale

- AES-256-GCM is selected because both the PWA (Web Crypto) and TUI (Go
  standard library) support it without implementing a block cipher. Generate
  a unique, cryptographically random 96-bit nonce for every encryption using
  a key; never reuse a nonce with the same key.
- Argon2id is selected for password-based key derivation. The 64 MiB, three
  iteration, four-lane profile is suitable for memory-constrained clients and
  must be embedded in each envelope for future migration.
