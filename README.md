# exokephalos Rust rewrite

This tree contains the staged Rust rewrite described in `exo-rs-plan.md`.
`oldcodebase/` is a read-only behavioral reference for data compatibility.

The first implementation slice establishes deterministic domain contracts,
legacy Markdown/ID/encryption compatibility, and the binary crate boundaries.

```sh
cargo test --workspace
cargo run -p xo -- validate path/to/note.md
cargo run -p xo-admin -- audit-workspace path/to/workspace
```

Iroh dependencies are pinned behind the `xo-core/iroh-sync` feature until the
persistent two-peer adapter is integrated.
