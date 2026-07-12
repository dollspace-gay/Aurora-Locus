# .archived/

Files moved out of the working tree during the v0.10 housekeeping pass
(chainlink #421, recon addendum §3). **Archive-then-retire**: if untouched for a
cycle or two, these can be deleted in a marked cleanup commit. Each was doll-era
(2025-10/11 founding-period) scratch or an orphaned utility with **zero
consumers at HEAD**.

- `test_base32.rs` — scratch base32 experiment; never a workspace target, unreferenced.
- `test_post.json` — scratch XRPC test payload.
- `test_endpoints.sh` / `test_admin_endpoints.sh` / `test_sequencer_endpoints.sh` — manual
  endpoint-poking scripts, superseded by the `tests/` integration suite + the Phase B blocks.
- `oauth-keyset-json.sh` — generator for the legacy `/oauth/*` provider's ES256 keyset; the
  atproto provider (post-Phase-ζ) loads no operator keyset — zero consumers. (Its P-256 `d`
  extraction is the reference model for fixing `install.sh`'s signing-key format bug.)
- `compute_checksums.py` — one-shot util to hand-seed `_sqlx_migrations` rows; superseded by boot auto-migrate.
- `provision_db.py` — one-shot empty-DB creator; superseded by boot auto-migrate.
- `test_data/` — three orphan per-actor `store.sqlite` fixtures (did_plc_test123/456/789) from the founding era; no code or test references them (grep-clean, incl. dynamic-path checks). Binary SQLite blobs.
