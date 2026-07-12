//! v0.9 Federation runtime-mutability arc §3.6 (#394) — home of the
//! `bulk_diddoc_update_result` table.
//!
//! The table (migration `0025` sqlite / `0026` postgres) tracks the post-restart
//! bulk did:plc DID-doc update per run, per account. D2's save handler writes the
//! initial `pending` rows in the same outer tx as the `bulk-diddoc-update`
//! marker; Phase E2's background task updates each to a terminal status; E4
//! surfaces the most-recent run (`ORDER BY started_at DESC LIMIT 1` — the recency
//! key is `started_at`, NOT the UUID `run_id`, per R3 H-2).
//!
//! C5 shipped the schema. D2 (#398) added the initial-pending-row writer the
//! save handler composes into its outer tx. E2 (#399) adds the per-account bulk
//! update task itself; the result queries land with E4.

use crate::admin::audit_chain::{self, AppendEntryParams};
use crate::admin::defs::Subject;
use crate::context::AppContext;
use crate::crypto::plc::PlcSigner;
use crate::error::PdsResult;
use crate::identity::did_method::{parse_did, DidMethod};

/// v0.9 Federation runtime-mutability arc §2.2/§3.6 (#398) — write one `pending`
/// row per account for a bulk did:plc update run, into the caller's transaction
/// (so they commit atomically with the `service.public_url` value + markers).
/// `updated_at` starts equal to `started_at`; E2 advances each row to a terminal
/// status. Idempotent re-runs are E2's concern (it upserts); at save-time the
/// rows are fresh for a newly-generated `run_id`.
pub async fn write_initial_pending_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    dids: &[String],
    run_id: &str,
    started_at: &str,
) -> PdsResult<()> {
    for did in dids {
        sqlx::query(
            "INSERT INTO bulk_diddoc_update_result \
             (did, run_id, started_at, status, reason, updated_at) \
             VALUES ($1, $2, $3, 'pending', NULL, $4)",
        )
        .bind(did)
        .bind(run_id)
        .bind(started_at)
        .bind(started_at)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Upsert a result row to a terminal status, keyed on the `(did, run_id)` PK so
/// it advances the `pending` row D2 wrote (or creates one for a revert run E2
/// drives). Not audit-chained — the audit is the per-account
/// `BulkServiceUrlUpdate` entry.
async fn upsert_result_row(
    pool: &sqlx::AnyPool,
    did: &str,
    run_id: &str,
    started_at: &str,
    status: &str,
    reason: Option<&str>,
) -> PdsResult<()> {
    sqlx::query(
        "INSERT INTO bulk_diddoc_update_result \
         (did, run_id, started_at, status, reason, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT(did, run_id) DO UPDATE SET \
         status = excluded.status, reason = excluded.reason, updated_at = excluded.updated_at",
    )
    .bind(did)
    .bind(run_id)
    .bind(started_at)
    .bind(status)
    .bind(reason)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// The run-invariant inputs to a single-account update — built once per run (or
/// per retry) and shared across the per-account calls. Bundled to keep
/// `run_per_account_diddoc_update`'s arity sane.
struct DidDocUpdateRun<'a> {
    new_endpoint: &'a str,
    signer: &'a PlcSigner,
    service_did: &'a str,
    run_id: &'a str,
    started_at: &'a str,
    /// `BulkServiceUrlUpdate` for the automatic run; `RetryBulkServiceUrlUpdate`
    /// for an operator retry.
    audit_action: &'a str,
}

/// v0.9 Federation runtime-mutability arc §2.3 (#399/#400) — update one account's
/// did:plc service endpoint. The single source of truth for the update, shared by
/// E2's per-account loop and E4's retry path.
///
/// Compare-and-skip first (M-3): if the published endpoint already matches,
/// record `aligned` without republishing. Otherwise publish via the PDS-wide
/// signer, audit the change AFTER the PLC call (M-4 — the guard never spans the
/// network), and record `aligned`.
async fn run_per_account_diddoc_update(
    ctx: &AppContext,
    did: &str,
    run: &DidDocUpdateRun<'_>,
) -> PdsResult<()> {
    let current = ctx.plc_client.get_document(did).await?.get_service_endpoint();
    if current.as_deref() == Some(run.new_endpoint) {
        upsert_result_row(
            &ctx.account_db,
            did,
            run.run_id,
            run.started_at,
            "aligned",
            Some("already at target endpoint (no-op)"),
        )
        .await?;
        return Ok(());
    }
    ctx.plc_client
        .update_service_endpoint(did, run.new_endpoint, run.signer)
        .await?;
    // M-4: short guard-held audit tx AFTER the PLC publish, never across it.
    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: run.service_did,
            action: run.audit_action,
            source: "system_diagnostic",
            payload: Some(serde_json::json!({
                "did": did,
                "old_endpoint": current.clone(),
                "new_endpoint": run.new_endpoint,
                "run_id": run.run_id,
            })),
            subject: Some(&Subject::Repo { did: did.to_string() }),
            rationale: &format!(
                "bulk did:plc service-url update: {} → {}",
                current.as_deref().unwrap_or("<none>"),
                run.new_endpoint
            ),
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await?;
    upsert_result_row(&ctx.account_db, did, run.run_id, run.started_at, "aligned", None).await?;
    Ok(())
}

/// v0.9 Federation runtime-mutability arc §2.3 (#399) — the post-restart bulk
/// did:plc DID-document update. Spawned by the boot marker hook (E3) when a
/// `bulk-diddoc-update` marker is present after a `service.public_url` change.
/// Re-points every account's PLC `AtprotoPersonalDataServer` endpoint at the new
/// effective public URL (D2 already baked the override into config at boot).
///
/// Best-effort / surface-and-triage: a per-account failure records a `failed`
/// row and the loop continues. The marker is cleared only on completion, so an
/// interruption re-runs on the next boot — safe because the compare-and-skip
/// short-circuit makes already-aligned accounts a no-op.
pub async fn run_bulk_diddoc_update(
    ctx: &AppContext,
    run_id: &str,
    started_at: &str,
) -> PdsResult<()> {
    use sqlx::Row as _;
    let new_endpoint = ctx.config.service.effective_public_url();
    let signer = PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key)?;
    let service_did = ctx.service_did().to_string();

    // v0.9: all accounts are did:plc (#381 Outcome A); v0.10 filters by method.
    let dids: Vec<String> = sqlx::query("SELECT did FROM actor")
        .fetch_all(&ctx.account_db)
        .await?
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("did").ok())
        .collect();

    let run = DidDocUpdateRun {
        new_endpoint: &new_endpoint,
        signer: &signer,
        service_did: &service_did,
        run_id,
        started_at,
        audit_action: "BulkServiceUrlUpdate",
    };
    let (mut aligned, mut failed, mut skipped) = (0usize, 0usize, 0usize);
    for did in &dids {
        // v0.10 Arc 1 Phase 0 (#414): only did:plc accounts have an external PLC
        // document to republish. did:web identities are served locally (Arc 1) and
        // have no PLC doc — record `skipped_did_web` and continue. A malformed
        // `actor.did` (should never occur) is recorded `failed` rather than
        // crashing the run.
        match parse_did(did) {
            Ok(parsed) if parsed.method() == DidMethod::Plc => {
                match run_per_account_diddoc_update(ctx, did, &run).await {
                    Ok(()) => aligned += 1,
                    Err(e) => {
                        failed += 1;
                        if let Err(e2) = upsert_result_row(
                            &ctx.account_db,
                            did,
                            run_id,
                            started_at,
                            "failed",
                            Some(&e.to_string()),
                        )
                        .await
                        {
                            tracing::error!(did, error = %e2, "bulk-diddoc: failed to record failure row");
                        }
                        tracing::warn!(did, error = %e, "bulk-diddoc: account update failed (surface-and-triage)");
                    }
                }
            }
            Ok(parsed) => {
                // Non-Plc method (did:web): no PLC republish applicable.
                skipped += 1;
                tracing::debug!(
                    did = %parsed.raw,
                    domain = parsed.domain.as_deref().unwrap_or("?"),
                    segment = parsed.segment.as_deref().unwrap_or("-"),
                    "bulk-diddoc: skipping non-did:plc account (no PLC document to republish)"
                );
                if let Err(e2) = upsert_result_row(
                    &ctx.account_db,
                    did,
                    run_id,
                    started_at,
                    "skipped_did_web",
                    Some("did:web account; PLC republish not applicable"),
                )
                .await
                {
                    tracing::error!(did, error = %e2, "bulk-diddoc: failed to record skipped_did_web row");
                }
            }
            Err(e) => {
                // Defensive: actor.did should always parse. If it doesn't, record
                // `failed` rather than crash the run. `skipped_did_web` is reserved
                // for did:web specifically, so an unparseable DID is not skipped.
                failed += 1;
                if let Err(e2) = upsert_result_row(
                    &ctx.account_db,
                    did,
                    run_id,
                    started_at,
                    "failed",
                    Some(&format!("unparseable DID: {e}")),
                )
                .await
                {
                    tracing::error!(did, error = %e2, "bulk-diddoc: failed to record failure row");
                }
                tracing::warn!(did, error = %e, "bulk-diddoc: unparseable actor.did (surface-and-triage)");
            }
        }
    }

    // The run is complete — clear the marker. (A mid-run interruption leaves it
    // set; the next boot re-runs, idempotent via compare-and-skip.)
    crate::api::pending_restart::clear_marker(
        &ctx.account_db,
        crate::api::pending_restart::ACTION_BULK_DIDDOC_UPDATE,
    )
    .await?;
    tracing::info!(run_id, total = dids.len(), aligned, failed, skipped, "bulk did:plc service-url update complete");
    Ok(())
}

/// Outcome of a single-account retry — the terminal status the row landed on and
/// any failure reason, surfaced to the operator by the retry XRPC.
pub struct BulkRetryOutcome {
    pub status: String,
    pub reason: Option<String>,
}

/// v0.9 Federation runtime-mutability arc §2.3 (#400) — re-run the did:plc update
/// for ONE account (the "Retry" control on a failed result row). Targets the
/// current effective public URL (same as E2's run), audits as
/// `RetryBulkServiceUrlUpdate`, and upserts the row to its new terminal status.
/// `Ok(failed)` (not `Err`) when the PLC operation fails — the failure is the
/// result the operator triages, not an XRPC error.
pub async fn retry_one_account(
    ctx: &AppContext,
    did: &str,
    run_id: &str,
) -> PdsResult<BulkRetryOutcome> {
    use sqlx::Row as _;
    let new_endpoint = ctx.config.service.effective_public_url();
    let signer = PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key)?;
    let service_did = ctx.service_did().to_string();

    // All rows of a run share started_at; reuse it so the upsert keeps the run's
    // recency key. Fall back to now() only if the run has no rows yet.
    let started_at: String = sqlx::query("SELECT started_at FROM bulk_diddoc_update_result WHERE run_id = $1 LIMIT 1")
        .bind(run_id)
        .fetch_optional(&ctx.account_db)
        .await?
        .and_then(|r| r.try_get::<String, _>("started_at").ok())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

    let run = DidDocUpdateRun {
        new_endpoint: &new_endpoint,
        signer: &signer,
        service_did: &service_did,
        run_id,
        started_at: &started_at,
        audit_action: "RetryBulkServiceUrlUpdate",
    };
    match run_per_account_diddoc_update(ctx, did, &run).await {
        Ok(()) => Ok(BulkRetryOutcome { status: "aligned".to_string(), reason: None }),
        Err(e) => {
            let reason = e.to_string();
            upsert_result_row(&ctx.account_db, did, run_id, &started_at, "failed", Some(&reason)).await?;
            Ok(BulkRetryOutcome { status: "failed".to_string(), reason: Some(reason) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Mirror of the shipped migration so the constraints can be exercised on an
    /// in-memory pool without the full migration runner. Kept in lockstep with
    /// `migrations/0025_phase_c_bulk_diddoc_update_result.sql`.
    const SCHEMA: &str = "CREATE TABLE bulk_diddoc_update_result (\
        did TEXT NOT NULL, run_id TEXT NOT NULL, started_at TEXT NOT NULL, \
        status TEXT NOT NULL CHECK(status IN \
        ('pending','aligned','failed','unresolvable','skipped_did_web')), \
        reason TEXT, updated_at TEXT NOT NULL, PRIMARY KEY (did, run_id))";

    async fn pool() -> sqlx::AnyPool {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(SCHEMA).execute(&pool).await.unwrap();
        pool
    }

    async fn insert(pool: &sqlx::AnyPool, did: &str, run_id: &str, status: &str) -> bool {
        sqlx::query(
            "INSERT INTO bulk_diddoc_update_result \
             (did, run_id, started_at, status, reason, updated_at) \
             VALUES ($1, $2, $3, $4, NULL, $5)",
        )
        .bind(did)
        .bind(run_id)
        .bind("2026-06-27T00:00:00Z")
        .bind(status)
        .bind("2026-06-27T00:00:01Z")
        .execute(pool)
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn valid_statuses_insert_ok() {
        let p = pool().await;
        for (i, s) in ["pending", "aligned", "failed", "unresolvable", "skipped_did_web"]
            .iter()
            .enumerate()
        {
            assert!(
                insert(&p, &format!("did:plc:a{i}"), "run-1", s).await,
                "status {s} should be accepted"
            );
        }
    }

    #[tokio::test]
    async fn invalid_status_rejected_by_check() {
        let p = pool().await;
        assert!(
            !insert(&p, "did:plc:x", "run-1", "bogus").await,
            "CHECK constraint must reject an unlisted status"
        );
    }

    #[tokio::test]
    async fn primary_key_did_run_id_unique() {
        let p = pool().await;
        assert!(insert(&p, "did:plc:dup", "run-1", "pending").await);
        // Same (did, run_id) again → PK violation.
        assert!(
            !insert(&p, "did:plc:dup", "run-1", "aligned").await,
            "duplicate (did, run_id) must violate the primary key"
        );
        // Same did, different run_id → allowed (a later run).
        assert!(insert(&p, "did:plc:dup", "run-2", "pending").await);
    }

    // E2 (#399) — the bulk update task end-to-end against the trait double.

    #[tokio::test]
    async fn bulk_update_publishes_aligned_rows_and_clears_marker() {
        use crate::crypto::plc_client::MockPlcClient;
        let mut ctx =
            crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await;

        // Seed two accounts in the actor table.
        for did in ["did:plc:a", "did:plc:b"] {
            sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
                .bind(did)
                .bind(format!("{}.test", &did[8..]))
                .bind("2026-01-01T00:00:00Z")
                .execute(&ctx.account_db)
                .await
                .unwrap();
        }

        // Inject the trait double; configure signing keys so get_document
        // succeeds. The mock returns an empty-service doc, so get_service_endpoint
        // is None → not aligned → the task publishes the new endpoint.
        let mock = Arc::new(
            MockPlcClient::new()
                .with_current_signing_key("did:plc:a", "zKEY")
                .with_current_signing_key("did:plc:b", "zKEY"),
        );
        ctx.plc_client = mock.clone();

        // Set the marker so we can verify the task clears it on completion.
        sqlx::query("INSERT INTO pending_restart_action (action, payload, created_at) VALUES ($1, $2, $3)")
            .bind(crate::api::pending_restart::ACTION_BULK_DIDDOC_UPDATE)
            .bind(r#"{"version":1,"run_id":"run-1"}"#)
            .bind("2026-06-27T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();

        let expected = ctx.config.service.effective_public_url();
        run_bulk_diddoc_update(&ctx, "run-1", "2026-06-27T00:00:00Z").await.unwrap();

        // Both accounts had the new endpoint published.
        assert_eq!(mock.published_service_endpoint("did:plc:a").as_deref(), Some(expected.as_str()));
        assert_eq!(mock.published_service_endpoint("did:plc:b").as_deref(), Some(expected.as_str()));
        // Both result rows are terminal-aligned.
        let aligned: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM bulk_diddoc_update_result WHERE run_id = 'run-1' AND status = 'aligned'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(aligned, 2, "both accounts aligned");
        // A per-account audit entry was emitted for each publish.
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE action = 'BulkServiceUrlUpdate'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(audits, 2, "one audit entry per published account");
        // The marker is cleared now the run is complete.
        let markers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pending_restart_action WHERE action = $1",
        )
        .bind(crate::api::pending_restart::ACTION_BULK_DIDDOC_UPDATE)
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(markers, 0, "marker cleared on completion");
    }

    // v0.10 Arc 1 Phase 0 (#414) — the bulk run skips non-did:plc accounts and
    // writes the reserved `skipped_did_web` status (the producer #408 was missing).

    #[tokio::test]
    async fn bulk_update_skips_did_web_accounts() {
        use crate::crypto::plc_client::MockPlcClient;
        let mut ctx =
            crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await;

        // One did:plc account (republished) and one did:web account (skipped).
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind("did:plc:a")
            .bind("a.test")
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind("did:web:example.com:user:alice")
            .bind("alice.example.com")
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();

        // The mock is only configured for the did:plc account; the did:web DID must
        // never reach the PLC client (it's filtered before run_per_account_*).
        let mock = Arc::new(MockPlcClient::new().with_current_signing_key("did:plc:a", "zKEY"));
        ctx.plc_client = mock.clone();

        let expected = ctx.config.service.effective_public_url();
        run_bulk_diddoc_update(&ctx, "run-1", "2026-06-27T00:00:00Z").await.unwrap();

        // did:plc account republished; did:web account never published.
        assert_eq!(
            mock.published_service_endpoint("did:plc:a").as_deref(),
            Some(expected.as_str())
        );
        assert!(
            mock.published_service_endpoint("did:web:example.com:user:alice").is_none(),
            "did:web account must not have a PLC publish attempted"
        );

        // The did:web account has exactly one `skipped_did_web` row; the did:plc
        // account is `aligned`.
        let web_status: String = sqlx::query_scalar(
            "SELECT status FROM bulk_diddoc_update_result WHERE did = 'did:web:example.com:user:alice'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(web_status, "skipped_did_web");
        let plc_status: String = sqlx::query_scalar(
            "SELECT status FROM bulk_diddoc_update_result WHERE did = 'did:plc:a'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(plc_status, "aligned");
    }
}
