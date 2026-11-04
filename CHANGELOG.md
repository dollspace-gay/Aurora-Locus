# Changelog

All notable changes to aurora-locus are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Work toward 0.1: a production Rust atproto PDS built on proto-blue, with
kryphocron built in.

### Added

- Cargo workspace in `aurora-locus/` with the crate layout for 0.1, pinned to
  proto-blue (`skydeval/proto-blue` at `7a3fb57`), kryphocron 0.4.5 and
  kryphocron-lexicons 0.3.5. Rust 1.88.
- `aurora-locus version` command.
- Bootstrap configuration from `locus.toml`, `LOCUS_*` variables and the
  reference PDS's `PDS_*` variables, including its SSRF-protection and
  dev-mode rules.
- Storage with SQLite and Postgres backends: embedded migrations, and a node
  refuses to start against a database schema newer than it knows.
- Runtime settings stored in the database, validated, audited and applied
  live on every node (Postgres `NOTIFY` plus a 5-second poll).
- Env values for runtime settings seed them at first boot and apply again
  when they change, once every live node agrees.
- Fenced leases for singleton jobs: a node that loses its lease can't commit.
- `aurora-locus serve` starts a node: opens and migrates the database, keeps a
  persistent node id, claims a TID clock id, applies env values, follows live
  settings (including the log level) and shuts down cleanly on SIGTERM.
- Job run bookkeeping (last run, last success, next run, last error).
- Repo engine: every record write is one transaction covering the MST
  blocks, the repository root, the record index and the `#commit` outbox
  event. The MST loads lazily from stored blocks, so a write reads only the
  paths it changes. Writes are checked in order against the record index
  (swapCommit and swapRecord, and create/update/delete conflicts return 400
  rather than 500). Every commit verifies, from the changed nodes alone, that
  the tree changed exactly as the writes say before anything is stored.
- Commits are signed v3 commits with `prev: null`; revs strictly increase per
  repository, even across clock steps.
- `#commit` events carry `since`, `prevData`, per-op `prev`, and a CAR with
  the commit, new MST nodes, written records and covering proofs, so
  consumers can check every op; writes whose event would exceed 2 MB are
  refused.
- Reference-counted repo blocks, collected by a leased GC job once
  unreferenced for 10 minutes.
- Encrypted key store: every stored secret and account signing key is sealed
  with XChaCha20-Poly1305 under one master key per install (from a file or
  `LOCUS_MASTER_KEY`). The key's id and a MAC are recorded in the database;
  a node with a different key refuses to start, and every secret write checks
  the key again. A file key is generated only on a fresh single-node SQLite
  install with a persistent data dir.
- did:plc operations: build, sign and verify operations and tombstones, derive
  a DID from its genesis operation, and read and write the PLC directory's
  JSON form, checked against real directory data. Operations locus signs are
  validated first (did:key rotation keys and verification methods, at://
  `alsoKnownAs`, http(s) service endpoints).
- Handle rules as the reference PDS: normalization and syntax, disallowed
  TLDs, the first-label length and no-dot rules on the PDS's own handle
  domains, the reference's reserved names plus operator-entered ones, and the
  reference's slur filter.

### Changed

- proto-blue is pinned to `skydeval/proto-blue` `feat/lazy-mst` (`a88e52e`),
  which adds the lazily loaded MST.
