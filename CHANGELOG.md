# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added

### Fixed

### Changed
- Fix rotted integration tests in tests/ (#14)
- Update deps + run clippy/fmt (#13)
- Migrate from embedded Rust-Atproto-SDK to proto-blue crate (#1)
- Delete Rust-Atproto-SDK directory and verify build (#12)
- Adapt cli/rotate_keys.rs to format_resign_commit (#11)
- Update api/repo.rs callers and signer plumbing (#10)
- Rewrite actor_store/repository.rs against proto_blue::repo::Repo (#9)
- Implement Signer wrapper around k256 PLC key (#8)
- Implement RepoStorage for ActorStore (SQLite-backed) (#7)
- Adapt actor_store/car.rs to proto-blue read_car/blocks_to_car (#6)
- Migrate leaf imports: did_doc, identity, oauth, syntax, tid (#5)
- Vendor blob mime/size helpers into src/blob_store/mime.rs (#4)
- Vendor PasswordHasher into src/auth/password.rs (#3)
- Add proto-blue dep, remove path dep on Rust-Atproto-SDK (#2)
