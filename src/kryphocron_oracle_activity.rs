//! Audience-oracle consultation instrumentation (#335 / design §6.4.1).
//!
//! `getOracleActivity` surfaces "recent oracle consultation counts via
//! Aurora-Locus-side wrapping" (§6.4.1 note). The design names the Overview
//! block "block/mute oracle activity", but kryphocron 0.3.0 exposes no
//! block/mute oracle to a standard deployment (those events fire only for
//! operator-supplied oracles, which have no install path here). The real
//! AL-side oracle is the **audience oracle** — consulted on the write path
//! (`participatePrivate`'s `check_participate_audience`) and the read path
//! (`authorize_private_read`'s membership resolution). This is a §-12.x-style
//! translation of the spec's intent into the substrate's actual idiom.
//!
//! The tally is **aggregate counts only** — never per-subject — so it honours
//! the substrate's privacy property while still telling an operator how much
//! the audience oracle is being consulted and how the decisions split. It is
//! process-local and restart-wiped (the design commits "recent consultation
//! counts", not a forensic log); `started_at` bounds the window the counts
//! cover.

use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicU64, Ordering};

/// One audience-oracle consultation outcome, by decision point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleConsultation {
    /// Write path: requester is in the parent's audience.
    WriteAllowed,
    /// Write path: requester is not a member / parent misconfigured.
    WriteDenied,
    /// Write path: parent owner is non-local; the check is deferred (allowed).
    WriteDeferred,
    /// Read path: reader resolved as an audience member (sees plaintext).
    ReadAuthorized,
    /// Read path: reader resolved as a non-member (sees the encoded form).
    ReadDenied,
}

/// Process-local audience-oracle consultation tally. Cheap atomic counters;
/// safe to share behind an `Arc` and increment from any request task.
#[derive(Debug)]
pub struct AudienceOracleActivity {
    started_at: DateTime<Utc>,
    write_allowed: AtomicU64,
    write_denied: AtomicU64,
    write_deferred: AtomicU64,
    read_authorized: AtomicU64,
    read_denied: AtomicU64,
}

impl AudienceOracleActivity {
    /// A fresh, all-zero tally whose window opens at `started_at`.
    pub fn new(started_at: DateTime<Utc>) -> Self {
        Self {
            started_at,
            write_allowed: AtomicU64::new(0),
            write_denied: AtomicU64::new(0),
            write_deferred: AtomicU64::new(0),
            read_authorized: AtomicU64::new(0),
            read_denied: AtomicU64::new(0),
        }
    }

    /// Record one consultation. Relaxed ordering: counters are independent and
    /// read only for display, so no cross-counter ordering is required.
    pub fn record(&self, c: OracleConsultation) {
        let cell = match c {
            OracleConsultation::WriteAllowed => &self.write_allowed,
            OracleConsultation::WriteDenied => &self.write_denied,
            OracleConsultation::WriteDeferred => &self.write_deferred,
            OracleConsultation::ReadAuthorized => &self.read_authorized,
            OracleConsultation::ReadDenied => &self.read_denied,
        };
        cell.fetch_add(1, Ordering::Relaxed);
    }

    /// A consistent-enough point-in-time read of the counters for the endpoint.
    pub fn snapshot(&self) -> AudienceOracleActivitySnapshot {
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let write_allowed = load(&self.write_allowed);
        let write_denied = load(&self.write_denied);
        let write_deferred = load(&self.write_deferred);
        let read_authorized = load(&self.read_authorized);
        let read_denied = load(&self.read_denied);
        AudienceOracleActivitySnapshot {
            started_at: self.started_at,
            write_allowed,
            write_denied,
            write_deferred,
            read_authorized,
            read_denied,
            total: write_allowed
                + write_denied
                + write_deferred
                + read_authorized
                + read_denied,
        }
    }
}

/// A point-in-time read of the consultation tally — the shape the endpoint
/// serialises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudienceOracleActivitySnapshot {
    pub started_at: DateTime<Utc>,
    pub write_allowed: u64,
    pub write_denied: u64,
    pub write_deferred: u64,
    pub read_authorized: u64,
    pub read_denied: u64,
    pub total: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor() -> DateTime<Utc> {
        "2026-06-21T00:00:00Z".parse().unwrap()
    }

    #[test]
    fn fresh_tally_is_all_zero() {
        let snap = AudienceOracleActivity::new(anchor()).snapshot();
        assert_eq!(snap.total, 0);
        assert_eq!(snap.write_allowed, 0);
        assert_eq!(snap.read_authorized, 0);
        assert_eq!(snap.started_at, anchor());
    }

    #[test]
    fn records_each_outcome_into_its_own_counter() {
        let t = AudienceOracleActivity::new(anchor());
        t.record(OracleConsultation::WriteAllowed);
        t.record(OracleConsultation::WriteAllowed);
        t.record(OracleConsultation::WriteDenied);
        t.record(OracleConsultation::WriteDeferred);
        t.record(OracleConsultation::ReadAuthorized);
        t.record(OracleConsultation::ReadDenied);
        t.record(OracleConsultation::ReadDenied);
        let s = t.snapshot();
        assert_eq!(s.write_allowed, 2);
        assert_eq!(s.write_denied, 1);
        assert_eq!(s.write_deferred, 1);
        assert_eq!(s.read_authorized, 1);
        assert_eq!(s.read_denied, 2);
        assert_eq!(s.total, 7, "total sums every decision point");
    }

    #[test]
    fn snapshot_is_a_stable_copy_not_a_live_view() {
        let t = AudienceOracleActivity::new(anchor());
        t.record(OracleConsultation::WriteAllowed);
        let early = t.snapshot();
        t.record(OracleConsultation::WriteAllowed);
        assert_eq!(early.write_allowed, 1, "earlier snapshot is unaffected by later records");
        assert_eq!(t.snapshot().write_allowed, 2);
    }
}
