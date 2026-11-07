# aurora-locus

An [atproto](https://atproto.com) Personal Data Server (PDS) written in Rust,
built on [proto-blue](https://github.com/skydeval/proto-blue), with built-in
support for kryphocron private posts.

Locus hosts accounts and their repositories, publishes their changes to
relays, and serves apps and other services the way Bluesky's reference PDS
(`@atproto/pds`) does. It reads the reference's `PDS_*` settings, so an
existing `pds.env` boots unchanged, and it can take over a reference
installation in place.

> **Status:** pre-release. 0.1.0 hasn't been tagged yet; expect rough
> edges, and keep backups.

## What it does

- **The reference PDS's behaviour.** Accounts, repositories, blobs, the
  firehose, OAuth, app passwords, account migration, invites, email
  confirmation and sign-in codes, moderation and admin APIs. A differential
  conformance harness runs the same requests against locus and the
  reference and compares the answers.
- **Deliberately better in places.** Revocable sessions, every stored secret
  sealed under one master key, replayed service tokens refused, stricter
  outbound request checks, fewer ways to learn whether an account exists.
  [docs/design/architecture.md](docs/design/architecture.md) lists every
  difference.
- **Private posts with kryphocron.** Posts shared with an audience you
  choose, stored encoded, with audiences kept off the public network, an
  optional gate on the media those posts use, and re-encoding on a schedule.
  Off until you configure a private AppView. The encoding is friction
  against casual reading, not encryption.
- **A web UI.** A setup wizard, an operator console for accounts, invites,
  moderation, settings, keys and jobs, and an account portal. Most
  settings change live, with no restart.
- **One binary, small to large.** One node on SQLite and a local disk, or
  several nodes on Postgres with blobs in S3 and Redis for shared rate
  limits. An entryway mode spreads accounts across shards.
- **Operations built in.** Backups and restore, key backups and rotation,
  moving blobs between storage backends, moving from SQLite to Postgres,
  changing the PDS hostname, Prometheus metrics.

## Quick start

On a fresh Linux server with ports 80 and 443 free and a hostname pointed at
it:

```sh
git clone <this repository>
cd <repository>/aurora-locus
sudo bash deploy/install.sh
```

The installer sets up Docker and Caddy, asks for your hostname, handle
domain and how HTTPS certificates are made (on demand, a DNS-01 wildcard, or
behind Cloudflare), builds the image and starts it. Then open
`https://<your hostname>/setup` and finish in the wizard, using the setup
token it tells you where to find.

[docs/design/operations.md](docs/design/operations.md) covers configuration,
other proxies, running without Docker, multi-node setups and day-to-day
operations.

## Building from source

Locus needs Rust 1.88 (pinned in `aurora-locus/rust-toolchain.toml`) and
`proto-blue-codegen` 0.3.3, which generates the kryphocron lexicon code at
build time:

```sh
cargo install proto-blue-codegen --version 0.3.3 --locked
cd aurora-locus
cargo build --release -p aurora-locus
```

The binary is `target/release/aurora-locus`. `aurora-locus serve` starts a
node, and `aurora-locus --help` lists the operator commands. The Docker image
is built from `aurora-locus/deploy/docker/Dockerfile`.

Tests run with `cargo test`. `aurora-locus/scripts/check.sh` formats, lints
and runs the whole suite; it also runs the Postgres, Redis, S3 and KMS tests
when the matching local test containers are running (the script names the
containers and ports it expects).

## Documentation

| Document | What it covers |
|---|---|
| [Architecture](docs/design/architecture.md) | How a node is built: storage, the repo engine, the firehose, blobs, the HTTP layer, scaling, and every difference from the reference PDS |
| [Identity and accounts](docs/design/identity-and-accounts.md) | did:plc and did:web, handles, sessions, OAuth, service auth, account lifecycle, migration, invites, email, the admin API |
| [The private tier](docs/design/private-tier.md) | kryphocron support: private posts, audiences, the media gate, rotation, behaviour when the private AppView is down, and what the encoding does and doesn't protect |
| [Operations](docs/design/operations.md) | Installing, configuring, deploying and running locus |
| [Security](docs/security.md) | The security measures locus takes and their limits |
| [Changelog](CHANGELOG.md) | Changes, in Keep a Changelog format |

## Security

Please report vulnerabilities privately; see [SECURITY.md](SECURITY.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
