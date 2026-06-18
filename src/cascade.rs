//! Cascade-token minting and verification — the confinement boundary
//! for cascade write authorization.
//!
//! # Why this is its own module (#282 / rev4 H-5)
//!
//! A `KryphocronWriteAuthorization::Cascade { .. }` write is authorized by a
//! [`CascadeToken`] that only a [`CascadeContext`] can mint. The security
//! property the block-cascade design depends on is that **nothing outside the
//! cascade machinery can mint a token** — if any module could call
//! [`CascadeContext::mint_token`], it could forge cascade authorization and
//! bypass the dedicated-endpoint write gate.
//!
//! Confining the minting surface to a Rust visibility boundary makes that a
//! compile-time guarantee rather than a convention: [`CascadeContext::mint_token`]
//! and [`CascadeContext::mint_secondary_token`] are `pub(in crate::cascade)`, so
//! the cascade orchestration that lives in this module is the *only* production
//! site that can produce a token. The verify side
//! ([`CascadeContext::verify_token`]) stays `pub(crate)` because the consumer —
//! `crate::kryphocron::bind_pipeline` — lives outside this module and must be
//! able to verify-and-consume a token presented on a `Cascade` WriteOp.
//!
//! Construction ([`CascadeContext::new`]) and the read-only accessors are
//! `pub(crate)`: holding an empty context grants no authority, because you still
//! cannot mint into it from outside `crate::cascade`.
//!
//! These types moved here verbatim from `crate::kryphocron` (v0.7 arc 2 steps
//! 2–3); the move is behaviour-preserving. The forgery invariants asserted by
//! the `compile_fail` doctests on [`CascadeToken`] (H-3) were latent before the
//! move and are now made explicit at the type's defining module.

use std::collections::HashMap;

use serde::Serialize;
use uuid::Uuid;

/// Cascade source — identifies the originating cascade operation
/// that produced a `Cascade` or `RecoveryBypass` WriteOp.
///
/// Per v07_DESIGN.md §5 lines 2195-2200. URI fields carry the at-URI
/// of the originating record for audit-emit forensic context;
/// Aurora-Locus stores at-URIs as plain `String` (translation note
/// above).
///
/// The `Cascade` suffix on every variant matches the design's
/// naming (cross-doc consistency for forensic readers). Without
/// the suffix the variants collide semantically with other domain
/// concepts (e.g., `Block` vs. block-cascade); the clippy lint is
/// suppressed here for that reason.
///
/// **Infallibility invariant (v0.8 arc 2 / #183).** Every variant
/// carries forensic identifiers only (at-URI `String`s), and every
/// variant MUST remain infallibly JSON-serializable — `serde_json::to_value`
/// must never return `Err` for any `CascadeSource`. The recovery-write
/// forensic-emit path in `bind_pipeline`'s `RecoveryBypass` arm serializes
/// `cascade_source` with `.expect("infallible")`; a future variant whose
/// `Serialize` impl could fail would panic in the audit-emit path rather
/// than silently corrupt the forensic record. The invariant is
/// structurally enforced by the compile-forced exhaustive test
/// `cascade_source_serialize_is_infallible_for_all_variants` (a new
/// variant without a match arm there fails to compile).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(dead_code, clippy::enum_variant_names)]
pub enum CascadeSource {
    /// Bsky-side delete cascading to kryphocron companion delete (per
    /// v07_DESIGN.md §7e).
    BskyDeleteCascade { bsky_uri: String },
    /// Block-record create/delete cascading to per-audience updates
    /// (per v07_DESIGN.md §7c).
    BlockCascade { block_uri: String },
    /// Threadgate cascading from a `postPrivate` delete (per
    /// v07_DESIGN.md §7d), or as the depth-2 child of a
    /// `BskyDeleteCascade`.
    ThreadgateCascade { post_uri: String },
    /// Audience-delete cascading to per-post reassignment (per
    /// v07_DESIGN.md §7a).
    AudienceDeleteCascade { audience_uri: String },
}

/// Single-use, transactionally-scoped token authorizing a cascade
/// write. Minted by `CascadeContext::mint_token` (arc 2 step 3);
/// consumed by `validate_write` (arc 2 step 4).
///
/// Tokens are non-clonable, non-serializable, and bound to the
/// originating `CascadeContext` identity. Arc 2 step 3 builds the
/// mint/verify machinery; this step ships the opaque carrier so
/// `KryphocronWriteAuthorization::Cascade { .. }` is constructible
/// as a type but cannot be forged from outside the cascade-context
/// crate path.
///
/// Field layout: a `cascade_context_id` (process-local UUID minted at
/// `CascadeContext::new`) plus a `mint_id` (per-mint nonce). The
/// fields are `pub(crate)` so `CascadeContext::verify_token` can
/// read them across module boundaries within the crate while still
/// keeping the type unforgeable from outside the crate.
///
/// # Type-level forgery invariants (#282 H-3)
///
/// `CascadeToken` is a single-use, in-process authorization *capability*, not a
/// bearer credential. Several derives would silently defeat that and are
/// forbidden — each is asserted absent by a `compile_fail` doctest below, so a
/// future edit re-adding one fails the doc-test gate.
///
/// `Clone` would let a holder duplicate a single-use token and replay it past
/// the spent-marker check:
///
/// ```compile_fail
/// use aurora_locus::cascade::CascadeToken;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<CascadeToken>();
/// ```
///
/// `Copy` would duplicate the token implicitly on every move, defeating the
/// move-consumes-the-capability discipline:
///
/// ```compile_fail
/// use aurora_locus::cascade::CascadeToken;
/// fn requires_copy<T: Copy>() {}
/// requires_copy::<CascadeToken>();
/// ```
///
/// `Serialize` would turn the in-process capability into a wire-reconstructable
/// bearer token, exactly the forgery surface the context-id + mint-registry
/// design exists to deny:
///
/// ```compile_fail
/// use aurora_locus::cascade::CascadeToken;
/// fn requires_serialize<T: serde::Serialize>() {}
/// requires_serialize::<CascadeToken>();
/// ```
///
/// The capability *is* `Debug` (for forensic logging). This positive case must
/// compile — it guards the `compile_fail` doctests above against vacuously
/// passing on a path/visibility error rather than the intended missing-trait
/// error:
///
/// ```
/// use aurora_locus::cascade::CascadeToken;
/// fn requires_debug<T: std::fmt::Debug>() {}
/// requires_debug::<CascadeToken>();
/// ```
#[derive(Debug)]
pub struct CascadeToken {
    /// Identity of the issuing `CascadeContext`. Verified at the
    /// check site to enforce cross-context isolation — a token
    /// minted by context A cannot be verified by context B.
    pub(crate) cascade_context_id: Uuid,
    /// Per-mint nonce — registry key into `CascadeContext::mints`.
    /// Distinguishes depth-1 from depth-2 mints under the same
    /// context per v07_DESIGN.md §5 depth-2 cap; also enables the
    /// single-use spent-marker check on verify.
    pub(crate) mint_id: Uuid,
}

// ---------------------------------------------------------------------------
// Arc 2 step 3 — CascadeContext + token mint/verify
// ---------------------------------------------------------------------------
//
// `CascadeContext` is the producer side of `CascadeToken`. It tracks
// the cascade tree's root operation, the depth-2 one-shot invariant,
// and a per-mint registry the verify side consults at the check site.
// Per v07_DESIGN.md §5, the context's lifetime IS the transaction's
// lifetime — tokens minted during a tx that rolls back are voided
// implicitly because the context is dropped on tx end. Step 3 ships
// the type and its hostile tests; the transaction-lifetime semantics
// wire at step 3.5 (storage tx lending) and step 4 (validate_write
// bind pipeline call), where the context's drop point becomes
// observably tied to txn commit/rollback.

/// Single-context cascade-mint error returned by
/// [`CascadeContext::mint_secondary_token`].
///
/// Per v07_DESIGN.md §5 line 2293 ("preventing depth-3 chains"), each
/// `CascadeContext` may issue at most one depth-2 token. v0.7's
/// cascade inventory (BskyDeleteCascade → ThreadgateCascade is the
/// only depth-2-composing chain) is single-fanout per context, so a
/// one-shot depth-2 mint suffices; multi-fanout cascade trees (if a
/// future cycle introduces one) use one context per sub-tree rather
/// than relaxing this cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("kryphocron cascade depth exceeded — at most one depth-2 mint per context")]
pub struct KryphocronCascadeDepthExceeded;

/// Verify-side failure modes for [`CascadeContext::verify_token`].
///
/// Each variant corresponds to one of the four kickoff-hostile-test
/// failure cases:
/// - `ContextMismatch`: the token was minted by a different context
///   (hostile #4 — cross-context isolation).
/// - `UnknownToken`: the token's `mint_id` is not present in this
///   context's mint registry. Either a forged token, or a token
///   constructed via `#[derive(Default)]`-style code paths that
///   shouldn't exist (none do — CascadeToken has no Default impl).
/// - `SourceMismatch`: the supplied source does not match the source
///   recorded at mint time (hostile #5 — attacker swaps a token
///   minted for one cascade source onto a WriteOp claiming another).
/// - `AlreadySpent`: the token was previously verified successfully
///   and is now consumed (hostile #1 — single-use spent marker
///   prevents replay).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CascadeTokenError {
    /// Token was minted by a different `CascadeContext` (cross-
    /// context isolation breach attempt).
    #[error("cascade token minted by a different context")]
    ContextMismatch,
    /// Token's `mint_id` is not present in this context's mint
    /// registry — forged token or post-drop verify attempt.
    #[error("cascade token not present in mint registry (forged or stale)")]
    UnknownToken,
    /// Supplied source does not match the source the token was
    /// minted for.
    #[error("cascade token source does not match supplied source")]
    SourceMismatch,
    /// Token was previously verified; single-use replay rejected.
    #[error("cascade token already spent")]
    AlreadySpent,
}

/// Per-mint registry entry — stored under `CascadeContext::mints` by
/// `mint_id`. Records the cascade source the token was minted for
/// (used by verify-side source-mismatch check) and the one-shot
/// spent marker (flipped to true on successful verify).
#[derive(Debug)]
struct MintEntry {
    source: CascadeSource,
    /// 1 for depth-1 mints (from `mint_token`), 2 for depth-2
    /// mints (from `mint_secondary_token`). Recorded for forensic
    /// clarity at audit-emit time; the verify path does not gate
    /// on depth (any depth is verifiable as long as the other
    /// invariants hold).
    #[allow(dead_code)]
    depth: u8,
    spent: bool,
}

/// Cascade-tree state plus mint/verify machinery for the duration
/// of a single cascade-initiating transaction.
///
/// Construction marks the cascade-tree root: subsequent depth-1
/// mints (via [`mint_token`](Self::mint_token)) and the one-shot
/// depth-2 mint (via [`mint_secondary_token`](Self::mint_secondary_token))
/// reference this root for forensic correlation. Each mint registers
/// a unique `mint_id` keyed entry that the verify side
/// ([`verify_token`](Self::verify_token)) consumes on first
/// successful match. Single-use (spent-marker) is enforced
/// per-token.
///
/// **Transaction lifetime.** Per v07_DESIGN.md §5, the context's
/// lifetime IS the transaction's lifetime. Arc 2 step 3 ships the
/// type with no tx coupling; arc 2 step 3.5 wires the
/// `SqliteRepoStorage` lent-tx mechanism through which step 4
/// (`validate_write`) reaches the active context. Drop-on-rollback
/// happens implicitly because the context is owned by the
/// dispatcher frame that opens the txn.
///
/// **Thread safety.** All mutating methods take `&mut self`. The
/// context is single-owner per cascade dispatch — concurrent access
/// is not a use case the design supports (concurrent cascades use
/// distinct contexts).
#[derive(Debug)]
pub struct CascadeContext {
    id: Uuid,
    root_source: CascadeSource,
    mints: HashMap<Uuid, MintEntry>,
    /// One-shot flag for the depth-2 mint. Set true on first
    /// successful `mint_secondary_token`; subsequent attempts
    /// return `KryphocronCascadeDepthExceeded`. See the type-level
    /// rationale on [`KryphocronCascadeDepthExceeded`].
    secondary_minted: bool,
}

#[allow(dead_code)]
impl CascadeContext {
    /// Create a new context anchored at the given cascade-tree root
    /// source. `id` is a fresh process-local UUID; subsequent mints
    /// stamp their tokens with this id for the verify-side
    /// cross-context-isolation check.
    ///
    /// `pub(crate)`: holding a context confers no authority on its own —
    /// the authority-granting operation is *minting*, confined below.
    pub(crate) fn new(root_source: CascadeSource) -> Self {
        Self {
            id: Uuid::new_v4(),
            root_source,
            mints: HashMap::new(),
            secondary_minted: false,
        }
    }

    /// Stable process-local identifier for this context. Exposed
    /// for forensic correlation at audit-emit time.
    pub(crate) fn id(&self) -> Uuid {
        self.id
    }

    /// The cascade-tree root operation this context was constructed
    /// for. Read-only after construction.
    pub(crate) fn root_source(&self) -> &CascadeSource {
        &self.root_source
    }

    /// Mint a depth-1 cascade token for `source`. Always succeeds
    /// (depth-1 fanout has no count cap — a single cascade-
    /// initiating operation may produce arbitrarily many depth-1
    /// children, e.g., one block-create may cascade to many
    /// audience updates).
    ///
    /// The minted token is registered in the context's per-mint
    /// registry. The verify side (`verify_token`) consults this
    /// registry on first verify; subsequent verifies of the same
    /// token error with `AlreadySpent`.
    ///
    /// `pub(in crate::cascade)` (#282 H-5): minting is the authority-granting
    /// operation, so it is confined to this module. The cascade orchestration
    /// is the only production minting site.
    pub(in crate::cascade) fn mint_token(&mut self, source: CascadeSource) -> CascadeToken {
        let mint_id = Uuid::new_v4();
        self.mints.insert(
            mint_id,
            MintEntry {
                source,
                depth: 1,
                spent: false,
            },
        );
        CascadeToken {
            cascade_context_id: self.id,
            mint_id,
        }
    }

    /// Mint a depth-2 cascade token for `source`. One-shot per
    /// context: returns `KryphocronCascadeDepthExceeded` on the
    /// second call. See the type-level rationale on
    /// [`KryphocronCascadeDepthExceeded`] for why one-shot is
    /// sufficient for v0.7's cascade inventory.
    ///
    /// `pub(in crate::cascade)` (#282 H-5): same confinement as
    /// [`mint_token`](Self::mint_token).
    pub(in crate::cascade) fn mint_secondary_token(
        &mut self,
        source: CascadeSource,
    ) -> Result<CascadeToken, KryphocronCascadeDepthExceeded> {
        if self.secondary_minted {
            return Err(KryphocronCascadeDepthExceeded);
        }
        self.secondary_minted = true;
        let mint_id = Uuid::new_v4();
        self.mints.insert(
            mint_id,
            MintEntry {
                source,
                depth: 2,
                spent: false,
            },
        );
        Ok(CascadeToken {
            cascade_context_id: self.id,
            mint_id,
        })
    }

    /// Verify and consume `token` against this context for the
    /// supplied `source`. Returns `Ok(())` iff:
    /// 1. `token.cascade_context_id == self.id` (cross-context
    ///    isolation — hostile #4)
    /// 2. `self.mints[token.mint_id]` exists (token is genuine)
    /// 3. `entry.source == source` (no source-swap forge —
    ///    hostile #5)
    /// 4. `!entry.spent` (single-use replay rejected — hostile #1)
    ///
    /// On success, the entry is marked spent; subsequent verifies
    /// of the same token return `AlreadySpent`.
    ///
    /// `pub(crate)`: the consumer (`crate::kryphocron::bind_pipeline`) lives
    /// outside this module and must be able to verify-and-consume a token
    /// presented on a `Cascade` WriteOp.
    pub(crate) fn verify_token(
        &mut self,
        token: &CascadeToken,
        source: &CascadeSource,
    ) -> Result<(), CascadeTokenError> {
        if token.cascade_context_id != self.id {
            return Err(CascadeTokenError::ContextMismatch);
        }
        let entry = self
            .mints
            .get_mut(&token.mint_id)
            .ok_or(CascadeTokenError::UnknownToken)?;
        if &entry.source != source {
            return Err(CascadeTokenError::SourceMismatch);
        }
        if entry.spent {
            return Err(CascadeTokenError::AlreadySpent);
        }
        entry.spent = true;
        Ok(())
    }

    /// Test-only depth-1 mint shim.
    ///
    /// `mint_token` is `pub(in crate::cascade)` so no module outside the
    /// cascade machinery can forge authorization. `crate::kryphocron`'s
    /// `bind_pipeline` tests, however, legitimately need a *genuinely minted*
    /// token (one registered in `self.mints`, so it verifies) to exercise the
    /// happy-path Cascade arm. A directly-constructed `CascadeToken` would miss
    /// the registry and fail verify as `UnknownToken`, which is the forge case,
    /// not the valid case.
    ///
    /// This shim is `#[cfg(test)]` only — it does not exist in the production
    /// binary, so the minting confinement holds at runtime.
    #[cfg(test)]
    pub(crate) fn mint_token_for_test(&mut self, source: CascadeSource) -> CascadeToken {
        self.mint_token(source)
    }
}

#[cfg(test)]
mod cascade_context_tests {
    use super::*;

    fn bsky_root() -> CascadeSource {
        CascadeSource::BskyDeleteCascade {
            bsky_uri: "at://did:plc:test/app.bsky.feed.post/abc".to_string(),
        }
    }

    fn block_source() -> CascadeSource {
        CascadeSource::BlockCascade {
            block_uri: "at://did:plc:test/app.bsky.graph.block/xyz".to_string(),
        }
    }

    /// Hostile #1 — mint depth-1, verify ok, second verify fails
    /// with `AlreadySpent`. Single-use replay protection.
    #[test]
    fn hostile_1_single_use_spent_marker_blocks_replay() {
        let mut ctx = CascadeContext::new(bsky_root());
        let source = bsky_root();
        let token = ctx.mint_token(source.clone());

        assert!(ctx.verify_token(&token, &source).is_ok(), "first verify");
        assert_eq!(
            ctx.verify_token(&token, &source),
            Err(CascadeTokenError::AlreadySpent),
            "second verify must fail with AlreadySpent",
        );
    }

    /// Hostile #2 — mint depth-1 and depth-2 in the same context,
    /// both verify successfully.
    #[test]
    fn hostile_2_depth_1_and_depth_2_both_verify() {
        let mut ctx = CascadeContext::new(bsky_root());
        let source = bsky_root();

        let primary = ctx.mint_token(source.clone());
        let secondary = ctx
            .mint_secondary_token(source.clone())
            .expect("first secondary mint must succeed");

        assert!(
            ctx.verify_token(&primary, &source).is_ok(),
            "depth-1 token verifies",
        );
        assert!(
            ctx.verify_token(&secondary, &source).is_ok(),
            "depth-2 token verifies",
        );
    }

    /// Hostile #3 — after one successful depth-2 mint, a second
    /// `mint_secondary_token` call returns
    /// `KryphocronCascadeDepthExceeded`. The one-shot-per-context
    /// rule per v07_DESIGN.md §5 line 2293.
    #[test]
    fn hostile_3_second_secondary_mint_returns_depth_exceeded() {
        let mut ctx = CascadeContext::new(bsky_root());
        let source = bsky_root();

        let _first = ctx
            .mint_secondary_token(source.clone())
            .expect("first secondary mint must succeed");

        let second = ctx.mint_secondary_token(source);
        assert_eq!(
            second.unwrap_err(),
            KryphocronCascadeDepthExceeded,
            "second secondary mint must fail with depth-exceeded",
        );
    }

    /// Hostile #4 — a token minted by context A cannot be verified
    /// by context B. Cross-context isolation: each context's
    /// process-local UUID gates verify.
    #[test]
    fn hostile_4_cross_context_verify_rejected() {
        let mut ctx_a = CascadeContext::new(bsky_root());
        let mut ctx_b = CascadeContext::new(bsky_root());
        let source = bsky_root();

        let token_from_a = ctx_a.mint_token(source.clone());
        assert_eq!(
            ctx_b.verify_token(&token_from_a, &source),
            Err(CascadeTokenError::ContextMismatch),
            "ctx_b must reject token minted by ctx_a",
        );
    }

    /// Hostile #5 — a token minted for `BskyDeleteCascade` does
    /// not verify against `BlockCascade`. Source-mismatch forge
    /// detection: an attacker who swaps the source on a Cascade
    /// WriteOp cannot reuse a legitimate token for a different
    /// cascade type.
    #[test]
    fn hostile_5_source_mismatch_rejected() {
        let mut ctx = CascadeContext::new(bsky_root());
        let mint_source = bsky_root();
        let attack_source = block_source();

        let token = ctx.mint_token(mint_source);
        assert_eq!(
            ctx.verify_token(&token, &attack_source),
            Err(CascadeTokenError::SourceMismatch),
            "verify with mismatched source must fail",
        );
    }

    /// Defense-in-depth — a forged token (random UUIDs not minted
    /// by any context) is rejected with `UnknownToken` when
    /// presented to a context whose id happens to match. Covers
    /// the `mints` HashMap lookup miss path in `verify_token`.
    #[test]
    fn forged_token_with_matching_context_id_rejected_as_unknown() {
        let mut ctx = CascadeContext::new(bsky_root());
        let source = bsky_root();

        let forged = CascadeToken {
            cascade_context_id: ctx.id(),
            mint_id: Uuid::new_v4(),
        };
        assert_eq!(
            ctx.verify_token(&forged, &source),
            Err(CascadeTokenError::UnknownToken),
            "forged token must be rejected even with matching ctx id",
        );
    }

    /// Sanity — context exposes its root source via the public
    /// accessor; the field is stable across mints.
    #[test]
    fn root_source_accessor_stable_across_mints() {
        let root = bsky_root();
        let mut ctx = CascadeContext::new(root.clone());
        assert_eq!(ctx.root_source(), &root, "before mint");

        let _ = ctx.mint_token(root.clone());
        assert_eq!(ctx.root_source(), &root, "after depth-1 mint");

        let _ = ctx.mint_secondary_token(root.clone());
        assert_eq!(ctx.root_source(), &root, "after depth-2 mint");
    }
}
