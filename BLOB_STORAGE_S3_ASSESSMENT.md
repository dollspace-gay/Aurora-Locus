# Aurora-Locus Blob Storage S3 Backend — Assessment

**Surface:** S3-compatible blob storage backend for parity with bsky-PDS
**Status:** Assessment — scaffold exists, dependencies and module exports disabled, activation and config wiring required
**Reference target:** bsky-PDS 2025-Q1 `PDS_BLOBSTORE_S3_*` env vars (per `atproto/packages/pds/src/config/`)
**Depends on:** Existing `BlobBackend` trait, `BlobBackendType::S3` enum variant, [src/blob_store/s3.rs](src/blob_store/s3.rs) module file
**Date:** 2026-04-30

---

## 1. Where Aurora-Locus stands today

Aurora-Locus's S3 blob storage support is partially scaffolded but actively disabled. The architectural shape is in place: [src/blob_store/mod.rs](src/blob_store/mod.rs) defines a `BlobBackend` trait, `BlobBackendType::S3` enum variant, and the abstraction is consumed by [src/blob_store/store.rs](src/blob_store/store.rs)'s `BlobStore`. The S3 implementation file [src/blob_store/s3.rs](src/blob_store/s3.rs) ships ~320 lines of working AWS SDK integration code. Both module exports and the underlying AWS SDK dependencies are commented out.

Today, an operator who configures Aurora-Locus for production deployment with S3-compatible blob storage gets nothing — the S3 path is unreachable. This is a parity gap relative to bsky-PDS, which exposes S3 blob storage as a first-class deployment option.

### 1.1 What's already there

[src/blob_store/s3.rs](src/blob_store/s3.rs) implements the `BlobBackend` trait against AWS SDK for Rust. The implementation includes:

- `S3BlobBackend` struct holding an `aws_sdk_s3::Client` and bucket name
- `S3Config` with `bucket`, `region`, `endpoint` (for S3-compatible providers), `access_key_id`, `secret_access_key`, and `prefix` for object key namespacing
- `S3BlobBackend::new(config)` async constructor that builds AWS credentials, optionally overrides the endpoint URL for non-AWS providers, and instantiates the S3 client
- `BlobBackend` trait impl: `put`, `get`, `delete`, `exists`, `size` — all working against S3's `PutObject`, `GetObject`, `DeleteObject`, `HeadObject` operations
- Error mapping from `aws_sdk_s3` errors to Aurora's `PdsError` type

[src/blob_store/mod.rs](src/blob_store/mod.rs) defines the abstraction layer:

- `BlobBackend` trait (async, `Send + Sync`) with the five methods listed above
- `BlobBackendType` enum with `Disk { location }` and `S3 { bucket, region, endpoint }` variants — the S3 variant is `#[allow(dead_code)]`
- `BlobStorageConfig` with `backend: BlobBackendType`, `max_blob_size`, `temp_dir`

[src/blob_store/disk.rs](src/blob_store/disk.rs) provides the active disk-backed implementation. The disk path works end-to-end and is the only currently-reachable backend.

### 1.2 What's disabled

Three places hold the S3 path closed:

**Cargo.toml dependencies are commented out.** Lines 56-59 of [Cargo.toml](Cargo.toml) read:
```toml
# Blob storage (S3) - Temporarily disabled due to Windows build issues
# aws-sdk-s3 = "1"
# aws-config = "1"
# aws-credential-types = "1"
```
Without these dependencies, [src/blob_store/s3.rs](src/blob_store/s3.rs) wouldn't compile if it were exported — its `use aws_sdk_s3::Client;` and similar imports would fail at the crate-resolution layer.

**Module export is commented out.** Lines 10-11 and 15 of [src/blob_store/mod.rs](src/blob_store/mod.rs):
```rust
// Temporarily disabled due to AWS SDK build issues on Windows
// pub mod s3;
// ...
// pub use s3::{S3BlobBackend, S3Config};
```
Even if dependencies were uncommented, the module would not be reachable from outside the crate.

**`AppContext` construction always selects disk backend.** [src/context.rs](src/context.rs)'s `BlobStore` initialization passes a `BlobStorageConfig` whose `backend` is hardcoded to `BlobBackendType::Disk`. There's no configuration-driven dispatch between Disk and S3.

### 1.3 The "Windows build issue" framing

The comments in Cargo.toml and mod.rs cite "AWS SDK build issues on Windows" as the reason for disablement. This framing doesn't hold up under scrutiny.

PDS deployments run on Linux. The Bluesky reference deployment, the Aurora-Locus production target, and every realistic operator scenario all assume Unix-like servers. Windows is a developer-environment concern at most, not a deployment concern. Even for Windows-based developers, the standard Rust workflow on Windows is to build through WSL or Docker (which present a Linux build environment), where the AWS SDK builds without issue.

Disabling the entire S3 backend — for everyone, in production — to accommodate a hypothetical Windows-native developer build is the wrong tradeoff. Activation work doesn't need to gate on Windows verification; it just needs to enable the feature for the deployment target Aurora-Locus actually has.

This shifts the Phase 1 framing in §6 from "verify Windows build, decide between unconditional and feature-gated re-enablement" to simply "uncomment the dependencies and exports, ship the feature on the platforms that matter."

---

## 2. The bsky-PDS reference surface

bsky-PDS exposes S3-compatible blob storage as a first-class deployment option. The configuration model is "either S3 or disk, not both" — operators pick at deployment time via env vars. The relevant env vars from `atproto/packages/pds/src/config/env.ts`:

| Env var | Purpose |
|---|---|
| `PDS_BLOBSTORE_S3_BUCKET` | S3 bucket name |
| `PDS_BLOBSTORE_S3_REGION` | AWS region (e.g., `us-east-1`) |
| `PDS_BLOBSTORE_S3_ENDPOINT` | Custom endpoint URL for S3-compatible providers (MinIO, DigitalOcean Spaces, Cloudflare R2, Backblaze B2) |
| `PDS_BLOBSTORE_S3_FORCE_PATH_STYLE` | Use path-style URLs (`endpoint/bucket/key`) instead of virtual-host-style (`bucket.endpoint/key`); required for MinIO and some other providers |
| `PDS_BLOBSTORE_S3_ACCESS_KEY_ID` | S3 access key |
| `PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY` | S3 secret key |
| `PDS_BLOBSTORE_S3_UPLOAD_TIMEOUT_MS` | Upload operation timeout (default 20000ms) |
| `PDS_BLOBSTORE_DISK_LOCATION` | Local filesystem path (mutually exclusive with S3) |
| `PDS_BLOBSTORE_DISK_TMP_LOCATION` | Local filesystem path for temporary uploads (disk-mode only) |

bsky-PDS rejects configurations that set both `PDS_BLOBSTORE_S3_BUCKET` and `PDS_BLOBSTORE_DISK_LOCATION` simultaneously — exactly one backend must be chosen. Aurora-Locus should match this validation.

The deployment patterns operators expect to work:

- **Local disk** for hobbyist deployments — `PDS_BLOBSTORE_DISK_LOCATION=/var/lib/aurora-locus/blobs`
- **AWS S3** for production deployments on AWS — `PDS_BLOBSTORE_S3_BUCKET=my-pds-blobs`, `PDS_BLOBSTORE_S3_REGION=us-east-1`, plus credentials
- **DigitalOcean Spaces** — same as AWS S3 but with `PDS_BLOBSTORE_S3_ENDPOINT=https://nyc3.digitaloceanspaces.com` and Spaces credentials
- **Cloudflare R2** — `PDS_BLOBSTORE_S3_ENDPOINT=https://<account>.r2.cloudflarestorage.com` plus R2 credentials
- **Backblaze B2** — `PDS_BLOBSTORE_S3_ENDPOINT=https://s3.<region>.backblazeb2.com` plus B2 application key
- **MinIO** (self-hosted) — `PDS_BLOBSTORE_S3_ENDPOINT=https://minio.example.com`, `PDS_BLOBSTORE_S3_FORCE_PATH_STYLE=true`, plus MinIO credentials

All of these are S3-protocol-compatible. Aurora-Locus's existing [src/blob_store/s3.rs](src/blob_store/s3.rs) is built on `aws-sdk-s3`, which speaks the S3 wire protocol; once activated, all of these providers work the same way. The `endpoint` field in Aurora's `S3Config` is what enables non-AWS providers — set the endpoint URL, the SDK routes requests there.

---

## 3. Parity gaps

Three gaps separate Aurora-Locus's current S3 support from bsky-PDS's S3 support. Closing them is required for Aurora-Locus to be a credible bsky-PDS alternative for operators using S3-compatible blob storage.

### 3.1 S3 backend not reachable

The headline gap. Aurora-Locus's S3 implementation exists at the file level but is fully disabled at the build and export levels. An operator who tries to use S3 today gets nothing — the path simply isn't there.

**What shipping requires:**
- Uncomment AWS SDK dependencies in [Cargo.toml](Cargo.toml): `aws-sdk-s3`, `aws-config`, `aws-credential-types` at version 1.x
- Uncomment `pub mod s3;` and `pub use s3::{S3BlobBackend, S3Config};` in [src/blob_store/mod.rs](src/blob_store/mod.rs)
- Remove the `#[allow(dead_code)]` from `BlobBackendType::S3` in [src/blob_store/mod.rs](src/blob_store/mod.rs)
- Verify the existing `cargo build` and `cargo test` continue to pass on Linux/macOS dev environments and the Linux production target

No feature-gating, no conditional compilation, no Windows-specific accommodation. PDS deployments run on Linux; the activation work is a straightforward uncomment.

### 3.2 Configuration not wired into `AppContext`

[src/context.rs](src/context.rs) currently constructs `BlobStore` with a hardcoded disk backend. There's no env-driven configuration that lets operators select S3 at deployment time.

**What shipping requires:**
- Extend `ServerConfig` (in [src/config.rs](src/config.rs)) with a `blobstore` section that accepts either disk or S3 configuration
- Mirror the bsky-PDS env var names (`PDS_BLOBSTORE_S3_*`, `PDS_BLOBSTORE_DISK_*`) for operator familiarity — operators migrating from bsky-PDS shouldn't need to relearn env var conventions
- Validation: reject configurations that set both `PDS_BLOBSTORE_S3_BUCKET` and `PDS_BLOBSTORE_DISK_LOCATION` (mutually exclusive per bsky-PDS pattern)
- Refactor [src/context.rs](src/context.rs)'s `BlobStore` initialization to dispatch on the configured backend type
- Document the new env vars in `.env.example` or equivalent

### 3.3 Two missing S3 config fields

Aurora's `S3Config` lacks two fields that bsky-PDS exposes:

**`force_path_style: bool`** — Required for MinIO compatibility. MinIO (and some other S3-compatible providers) requires path-style addressing (`endpoint/bucket/key`) rather than the AWS-default virtual-host-style (`bucket.endpoint/key`). The AWS SDK supports this via `Builder::force_path_style(true)`. Default: `false` (matching AWS behavior); operators using MinIO set to `true`.

**`upload_timeout_ms: u64`** — Override for the default upload operation timeout. Useful for operators with slow upload paths or large blob sizes. bsky-PDS defaults to 20000ms (20 seconds); Aurora should match.

**What shipping requires:**
- Add both fields to `S3Config` in [src/blob_store/s3.rs](src/blob_store/s3.rs) with documented defaults
- Wire `force_path_style` into the `aws_sdk_s3::Config::Builder` chain in `S3BlobBackend::new`
- Wire `upload_timeout_ms` into the SDK's request timeout configuration
- Add corresponding env vars: `PDS_BLOBSTORE_S3_FORCE_PATH_STYLE` and `PDS_BLOBSTORE_S3_UPLOAD_TIMEOUT_MS`

---

## 4. Serving-path compatibility

Aurora-Locus's blob serving handler in [src/api/blob.rs](src/api/blob.rs) is already CDN-friendly today, regardless of whether the blob backend is disk or S3. The headers it emits enable downstream caching at any HTTP-level cache (CDN, reverse proxy, browser):

- **`Cache-Control: public, max-age=31536000, immutable`** — Tells caches the response is cacheable by shared caches, valid for one year, and will never change at this URL (true because blobs are content-addressed by CID)
- **`ETag`** based on the CID — Content-addressed, perfect for cache validation
- **`If-None-Match` → 304 Not Modified** — Saves bandwidth on cache revalidation
- **`Accept-Ranges: bytes`** + Range request handling — Enables partial content for video streaming and resumable downloads

Plus the existing CORS layer in [src/server.rs](src/server.rs) (`CorsLayer::new().allow_origin(Any)`) permits cross-origin embedding of blob URLs from any domain — necessary for clients at `bsky.app` (or any other ATProto client) to load images from an Aurora-Locus PDS.

This means CDN deployment is already supported architecturally:

- **Disk backend + reverse proxy CDN**: Cloudflare or similar sits in front of Aurora-Locus's origin URL; blob responses get cached at edge based on the headers Aurora-Locus already emits
- **S3 backend + native CDN**: CloudFront sits in front of S3 (or R2 has built-in CDN; Spaces has built-in CDN); the CDN respects the same headers because the S3 backend stores them as object metadata that S3 returns on subsequent reads

No serving-handler changes are required for CDN parity. The deployment-side CDN configuration is operator concern, not Aurora-Locus concern.

### 4.1 Open question: CDN purge on takedown

There is one moderation-correctness wrinkle worth flagging, even though resolving it isn't part of this assessment's shipping scope.

When a blob is taken down via Aurora-Locus's quarantine system, the CDN cache (with `max-age=31536000, immutable`) won't reflect the takedown until the cache TTL expires — potentially up to a year. For deployments that rely on takedown for moderation correctness, this is a real gap.

Three handling options:

- **Defer.** Document the limitation in operator-facing docs; takedowns are functionally weak for as long as the CDN holds the cached blob
- **Manual purge guidance.** Document operator-side procedures for purging the CDN cache when takedowns happen, vendor by vendor (CloudFront, Cloudflare, R2, Spaces, etc.) — operators wire up the purge themselves based on takedown events from the moderation surface
- **Aurora-driven purge.** Aurora-Locus calls a configurable webhook (or vendor-specific API) when a takedown occurs; the webhook is responsible for purging the relevant CDN

Option 2 is the pragmatic middle. Option 3 is the right answer if the moderation story needs to be airtight by default, but it's substantial work because it means CDN-vendor-specific integration code (or operator-side webhook plumbing).

**Recommendation for v0.2:** Option 2. Document the operator concern in deployment docs, no code changes required, frame Option 3 as future work tied to the admin/moderation extension surface (the `subscribeModEvents` endpoint from the admin assessment naturally provides the takedown event stream that a CDN-purge webhook would consume).

---

## 5. Out of scope

These are explicitly excluded from this assessment to keep scope bounded.

**Signed URL generation.** If blobs ever need access control (private posts, takedown grace periods, member-only content), the deployment model would need signed URL generation that the CDN can validate. Aurora-Locus's current model treats all blobs as public-readable; signed URLs would be a significant architectural addition. Out of scope for v0.2.

**Aurora-driven CDN purge.** Per §4.1, this is deferred to future work tied to the admin/moderation extension surface.

**Migration tooling for live disk → S3 data movement.** Operators with existing disk-backed Aurora-Locus deployments who want to move blobs to S3 need a one-shot migration script. That's separate work, not part of v0.2's scope. Operators who need it can use generic S3 sync tools (`aws s3 sync` or `mc mirror` for MinIO) in the meantime, with the caveat that Aurora-Locus's blob CIDs need to map cleanly to S3 object keys (the `prefix` field in `S3Config` plus the CID makes this straightforward).

**Multi-region / multi-bucket configurations.** Aurora-Locus operates against a single S3 bucket; multi-region replication or multi-bucket sharding is operator-side concern (S3 cross-region replication, CloudFront multi-origin, etc.) and not Aurora-Locus's responsibility.

**Blob storage backends beyond Disk and S3.** Other potential backends — IPFS, GCS, Azure Blob Storage — are out of scope. Operators wanting non-S3-compatible backends would either contribute the implementation as a new `BlobBackend` impl or use an S3-compatible gateway in front of their preferred storage.

---

## 6. Implementation phases

The work splits into three phases. Phase 1 is the activation work; Phase 2 is the configuration wiring; Phase 3 is the parity polish.

### Phase 1 — Activate the S3 backend

**Goal:** Make the S3 path reachable.

**Deliverables:**
1. Uncomment AWS SDK dependencies in [Cargo.toml](Cargo.toml): `aws-sdk-s3 = "1"`, `aws-config = "1"`, `aws-credential-types = "1"`
2. Uncomment `pub mod s3;` and `pub use s3::{S3BlobBackend, S3Config};` in [src/blob_store/mod.rs](src/blob_store/mod.rs)
3. Remove `#[allow(dead_code)]` from `BlobBackendType::S3`
4. Verify `cargo build` and `cargo test` pass on Linux and macOS — the deployment-relevant targets

The "Windows build issues" comment originally cited as the reason for disablement is not a coherent justification for a server-side product. PDS deployments run on Linux; Windows-based developers use WSL or Docker. No feature-gating or Windows-specific accommodation is required.

**Risk:** Low. The hard work is already in [src/blob_store/s3.rs](src/blob_store/s3.rs); activation is uncomment-and-verify.

### Phase 2 — Configuration wiring

**Goal:** Make S3 selectable at deployment time via env vars matching bsky-PDS conventions.

**Deliverables:**
1. Extend `ServerConfig` in [src/config.rs](src/config.rs) with a `blobstore` section accepting either disk or S3 configuration
2. Implement env-var loading: `PDS_BLOBSTORE_S3_BUCKET`, `PDS_BLOBSTORE_S3_REGION`, `PDS_BLOBSTORE_S3_ENDPOINT`, `PDS_BLOBSTORE_S3_FORCE_PATH_STYLE`, `PDS_BLOBSTORE_S3_ACCESS_KEY_ID`, `PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY`, `PDS_BLOBSTORE_S3_UPLOAD_TIMEOUT_MS`, `PDS_BLOBSTORE_DISK_LOCATION`
3. Configuration validation: reject configurations setting both S3 bucket and disk location
4. Refactor [src/context.rs](src/context.rs)'s `BlobStore` construction to dispatch on the configured backend
5. Update `.env.example` with the new env var documentation
6. Verify the existing test suite still passes (disk path is unchanged); add tests for the new dispatch logic

**Risk:** Low. Configuration plumbing follows the same pattern as Postgres backend selection in the Postgres assessment.

### Phase 3 — Parity polish

**Goal:** Close the two missing S3 config fields and verify cross-provider compatibility.

**Deliverables:**
1. Add `force_path_style: bool` to `S3Config` (default `false`); wire into `aws_sdk_s3::Config::Builder::force_path_style`
2. Add `upload_timeout_ms: u64` to `S3Config` (default `20000`); wire into the SDK's request timeout
3. Add corresponding env vars: `PDS_BLOBSTORE_S3_FORCE_PATH_STYLE`, `PDS_BLOBSTORE_S3_UPLOAD_TIMEOUT_MS`
4. Integration tests against multiple S3-compatible providers — at minimum: AWS S3 (or LocalStack for cost-free testing), MinIO (self-hosted via Docker), and one other provider (DigitalOcean Spaces or Cloudflare R2)
5. Document deployment patterns for each tested provider in operator-facing docs (env var examples, common gotchas, expected behavior)

**Risk:** Low to medium. Integration tests against real providers are the slowest piece — provider accounts, test credentials, cleanup automation. LocalStack covers AWS S3 cheaply; MinIO covers the path-style and self-hosted scenario; one cloud provider (Spaces or R2) covers the cloud-S3-compatible scenario.

---

## 7. Pre-implementation checks

Items to verify before chainlink issue creation.

| Assumption | How to verify |
|---|---|
| `BlobBackend` trait abstraction is sufficient for S3 — no protocol-specific extensions needed | Audit the existing `S3BlobBackend` impl for any methods or behaviors not expressible through the trait |
| The `force_path_style` and `upload_timeout_ms` AWS SDK APIs match bsky-PDS's behavior expectations | Test against MinIO with `force_path_style=true` to confirm path-style URLs are emitted; test upload with deliberately-slow network to confirm timeout fires |
| The serving handler in [src/api/blob.rs](src/api/blob.rs) works identically with disk and S3 backends | The trait abstraction means yes by construction, but verify with integration tests that the response shape is byte-identical for the same blob via either backend |
| CDN-side caching with the existing headers is observed in practice (not just promised by the spec) | Deploy a test instance behind Cloudflare or similar; verify that subsequent requests for the same blob are served from the CDN edge without hitting Aurora-Locus |

---

## 8. Open questions

One genuine open question; recommendation stated where the doc is leaning.

### 8.1 Should `prefix` field stay or be removed for parity?

Aurora's `S3Config` has a `prefix` field (default `"blobs/"`) that bsky-PDS doesn't expose. This is Aurora-specific — it lets operators namespace blob keys within a bucket they're sharing with other applications.

**Recommendation:** Keep it. The field is additive — operators who don't set it get the default `"blobs/"` prefix which is reasonable behavior. Operators who need it (sharing a bucket across multiple Aurora-Locus instances, or sharing with other applications) get a useful capability bsky-PDS doesn't offer. This is one of the few places where Aurora-Locus's S3 surface exceeds bsky-PDS's, and it's harmless if unused.

---

## 9. Closing

Aurora-Locus's S3 blob storage support exists at the file and trait-impl levels but is currently inaccessible due to commented-out dependencies and module exports. The work to close this parity gap is bounded:

- Verify and re-enable Cargo dependencies and module exports
- Wire S3 configuration into `ServerConfig` and `AppContext`
- Add two missing config fields (`force_path_style`, `upload_timeout_ms`) for parity with bsky-PDS
- Validate cross-provider compatibility through integration tests

The serving-side CDN compatibility is already in place — Aurora-Locus's existing blob handler emits the correct cache headers and CORS policy for downstream CDN caching, regardless of whether the underlying backend is disk or S3. CDN deployment is therefore an operator-side concern, not an Aurora-Locus development concern.

The CDN-purge-on-takedown moderation correctness gap is real but deferred to future work tied to the admin/moderation extension surface; v0.2 ships with operator documentation noting the limitation.

Status as of this assessment: **scope and parity gaps identified, ready for chainlink issue creation against Phases 1, 2, and 3.** Phase 1 (activation) is gating; Phase 2 and 3 can sequence after Phase 1 lands.
