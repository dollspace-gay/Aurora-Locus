//! Aurora-owned OAuth **client** machinery — the browser-loopback admin login
//! ceremony Aurora drives against its own authorization server, with no
//! proto-blue-oauth dependency for the admin flow (chainlink #439).
//!
//! This is the mirror image of the sibling [`crate::oauth`] module: `oauth` is
//! Aurora's authorization *server* (the AS that issues tokens); `oauth_client`
//! is the client that talks TO that AS on the admin's behalf. Keeping the two
//! apart is the whole point of the arc — Aurora's admin control plane owns both
//! ends of the ceremony and does not inherit any external OAuth client's
//! release cadence or spec drift.
//!
//! - [`dpop`] — RFC 9449 §4.2 DPoP proof construction (Phase 1).
//! - [`admin`] — the PAR + code-exchange + refresh admin login flow (Phase 2).

pub mod admin;
pub mod dpop;
