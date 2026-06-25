//! Rotate DID signing keys and update PLC directory
//!
//! This command rotates signing keys for DID:PLC identifiers by:
//! 1. Fetching the current signing key from the repository
//! 2. Comparing with the current key in PLC directory
//! 3. Updating PLC directory if keys don't match
//! 4. Creating a new repository commit with the updated signing key
//! 5. Sequencing identity events (commit events are automatically sequenced)

use crate::{
    account::OperatorSuppliedKeypair,
    actor_store::repository::RepositoryManager,
    admin::{
        audit_chain::{self, AppendEntryParams},
        defs::Subject,
    },
    context::AppContext,
    crypto::{plc::PlcSigner, plc_client::PlcClientApi, proto_blue_signer::RepoSigner},
    error::{PdsError, PdsResult},
    sequencer::events::IdentityEvent,
};
use std::fs;
use std::sync::Arc;

/// Rotate keys for a list of DIDs
///
/// # Arguments
/// * `ctx` - Application context
/// * `dids` - List of DIDs to rotate keys for
/// * `concurrency` - Number of concurrent rotations (default: 10)
pub async fn rotate_keys(
    ctx: &AppContext,
    dids: Vec<String>,
    concurrency: usize,
    rationale: Option<String>,
    operator_keypair: Option<OperatorSuppliedKeypair>,
) -> PdsResult<()> {
    if dids.is_empty() {
        return Err(PdsError::Validation("No DIDs provided".to_string()));
    }

    // An operator-supplied keypair is a single account's key — it cannot rotate
    // a batch. Enforce before ANY state mutation (key-rotation arc B4 / §4.4).
    if operator_keypair.is_some() && dids.len() != 1 {
        return Err(PdsError::Validation(
            "--public-key/--private-key-hex apply to a single account; pass exactly one DID \
             (bulk rotation generates a fresh PDS key per DID and takes no operator keypair)"
                .to_string(),
        ));
    }

    println!(
        "Rotating keys for {} DID(s) with concurrency {}...\n",
        dids.len(),
        concurrency
    );

    // Use the shared PLC client threaded through AppContext (#371 / A3b) rather
    // than constructing a redundant one — same client the admin rotation handler
    // uses, and tests can inject a mock by reassigning ctx.plc_client.
    let plc_client: Arc<dyn PlcClientApi> = ctx.plc_client.clone();

    // Create rotation key signer
    let rotation_signer = PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key)
        .map_err(|e| PdsError::Internal(format!("Invalid PLC rotation key: {}", e)))?;

    // Process DIDs with limited concurrency
    let mut tasks = Vec::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));

    for (idx, did) in dids.iter().enumerate() {
        let did = did.clone();
        let ctx = ctx.clone();
        let plc_client_clone = plc_client.clone();
        let rotation_signer_clone = rotation_signer.clone();
        let sem_clone = semaphore.clone();
        let total = dids.len();
        // Operator-supplied keypair (single-DID case only) + rationale ride into
        // the per-DID rotation. For the batch/PDS path operator_keypair is None
        // and each DID generates its own fresh key.
        let operator_keypair = operator_keypair.clone();
        let rationale = rationale.clone();

        let task = tokio::spawn(async move {
            let _permit = sem_clone.acquire().await.unwrap();

            match rotate_key_for_did(
                &ctx,
                &did,
                plc_client_clone.as_ref(),
                &rotation_signer_clone,
                operator_keypair.as_ref(),
                rationale.as_deref(),
            )
            .await
            {
                Ok(rotated) => {
                    if rotated {
                        println!("[{}/{}] ✓ Rotated key for {}", idx + 1, total, did);
                    } else {
                        println!(
                            "[{}/{}] ○ Key already up-to-date for {}",
                            idx + 1,
                            total,
                            did
                        );
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!(
                        "[{}/{}] ✗ Failed to rotate key for {}: {}",
                        idx + 1,
                        total,
                        did,
                        e
                    );
                    Err(e)
                }
            }
        });

        tasks.push(task);
    }

    // Wait for all tasks to complete
    let mut success_count = 0;
    let already_updated = 0;
    let mut error_count = 0;

    for task in tasks {
        match task.await {
            Ok(Ok(())) => success_count += 1,
            Ok(Err(_)) => error_count += 1,
            Err(e) => {
                error_count += 1;
                eprintln!("Task error: {}", e);
            }
        }
    }

    println!("\n═══════════════════════════════════════");
    println!("Summary:");
    println!("  Total DIDs:      {}", dids.len());
    println!("  ✓ Rotated:       {}", success_count);
    println!("  ○ Already up-to-date: {}", already_updated);
    println!("  ✗ Failed:        {}", error_count);
    println!("═══════════════════════════════════════\n");

    if error_count > 0 {
        println!("⚠️  Warning: {} key rotation(s) failed", error_count);
    } else {
        println!("✓ All key rotations completed successfully");
    }

    Ok(())
}

/// Rotate keys from a file containing DIDs (one per line)
///
/// # Arguments
/// * `ctx` - Application context
/// * `file_path` - Path to file containing DIDs
/// * `concurrency` - Number of concurrent rotations
pub async fn rotate_keys_from_file(
    ctx: &AppContext,
    file_path: &str,
    concurrency: usize,
) -> PdsResult<()> {
    println!("Reading DIDs from file: {}\n", file_path);

    let content = fs::read_to_string(file_path)
        .map_err(|e| PdsError::Internal(format!("Failed to read file: {}", e)))?;

    let dids: Vec<String> = content
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| line.starts_with("did:plc:"))
        .collect();

    if dids.is_empty() {
        return Err(PdsError::Validation(
            "No valid did:plc DIDs found in file".to_string(),
        ));
    }

    // Bulk file rotation is always PDS-generated (a file of DIDs can't carry
    // per-account operator keypairs); no operator keypair, no shared rationale.
    rotate_keys(ctx, dids, concurrency, None, None).await
}

/// Rotate the signing key for a single DID (key-rotation arc B4 / §4.2-§4.4).
///
/// Mirrors the admin rotation handler (admin.rs / #374): pick the new keypair
/// (Path A PDS-generated, or Path B operator-supplied — validated-then-gated),
/// no-op if it already matches the published key, else publish to PLC → write
/// `plc_keys` → advance the repo with an empty commit signed by the NEW
/// per-account key → sequence an identity event → emit the rotation audit.
/// Returns true if a rotation occurred, false on the idempotent no-op.
async fn rotate_key_for_did(
    ctx: &AppContext,
    did: &str,
    plc_client: &dyn PlcClientApi,
    rotation_signer: &PlcSigner,
    operator_keypair: Option<&OperatorSuppliedKeypair>,
    rationale: Option<&str>,
) -> PdsResult<bool> {
    // Validate DID format
    if !did.starts_with("did:plc:") {
        return Err(PdsError::Validation(format!(
            "Not a did:plc identifier: {}",
            did
        )));
    }

    // §4.2.1 — pick the new per-account keypair. Path A (no operator keypair):
    // PDS generation. Path B (operator-supplied): validate FIRST so a mismatch
    // surfaces regardless of gate state, THEN enforce the operator-supplied-keys
    // gate (the same fail-closed runtime read the admin handler + dry-run use).
    let (rotation_keypair, generation_source) = match operator_keypair {
        None => (ctx.account_manager.generate_rotation_keypair()?, "pds"),
        Some(supplied) => {
            let validated = ctx.account_manager.validate_operator_keypair(supplied)?;
            let gate_on = crate::api::aurora_admin::resolve_runtime_setting(
                ctx,
                crate::api::aurora_admin::KEY_ROTATION_OPERATOR_SUPPLIED_KEYS_ENABLED_KEY,
            )
            .await
            .as_bool()
            .unwrap_or(false);
            if !gate_on {
                return Err(PdsError::Validation(
                    "operator-supplied rotation keys are disabled on this deployment \
                     (set key_rotation.operator_supplied_keys_enabled to enable)"
                        .to_string(),
                ));
            }
            (validated, "operator_supplied")
        }
    };

    // §4.2.2 — no-op short-circuit: skip if the would-be key already matches the
    // published key. Vestigial on Path A (a fresh key never matches), the
    // genuine idempotent no-op on Path B when the operator supplies the
    // currently-published key.
    let current_doc = plc_client.get_document(did).await?;
    let current_multibase = plc_client.get_signing_key(&current_doc)?;
    let new_multibase = rotation_keypair
        .public_did_key
        .strip_prefix("did:key:")
        .unwrap_or(&rotation_keypair.public_did_key);
    if plc_client.keys_match(&current_multibase, new_multibase) {
        tracing::debug!(did = %did, "Signing key already up to date; skipping rotation (no-op)");
        return Ok(false);
    }

    // §4.2.3 — publish the new public key to PLC with the PDS-wide rotation key.
    plc_client
        .update_signing_key(did, &rotation_keypair.public_did_key, rotation_signer)
        .await?;

    // §4.2.4 — write the new private key to plc_keys; capture the old keys for
    // the audit.
    let old_keys = ctx
        .account_manager
        .update_atproto_signing_key(did, &rotation_keypair.private_key_bytes)
        .await?;

    // §4.2.5 — advance the repo with an empty commit signed by the NEW
    // per-account private key (the R2.1 contradiction fix: was the PDS-wide
    // repo_signing_key, at the two rotate_keys.rs sites). Empty write set →
    // proto-blue produces a fresh signed commit over the unchanged MST,
    // advancing the chain so sync observers see the rotation.
    let repo_mgr = RepositoryManager::with_sequencer(
        did.to_string(),
        (*ctx.actor_store).clone(),
        ctx.sequencer.clone(),
    );
    let repo_signer_pb: std::sync::Arc<dyn proto_blue::crypto::Signer> = {
        let s = RepoSigner::from_bytes(&rotation_keypair.private_key_bytes).map_err(|e| {
            PdsError::Internal(format!(
                "Failed to build repo signer from new per-account key: {}",
                e
            ))
        })?;
        std::sync::Arc::new(s)
    };

    let (commit_cid, rev) = repo_mgr
        .apply_writes(
            vec![],
            repo_signer_pb,
            std::sync::Arc::new(crate::blob_store::StrictPromoter),
        )
        .await
        .map_err(|e| {
            tracing::warn!("Failed to create commit for {}: {}", did, e);
            e
        })?;

    tracing::info!(
        did = %did,
        commit_cid = %commit_cid,
        rev = %rev,
        "Created empty commit for signing key rotation"
    );

    // Sequence identity event (commit event already sequenced by apply_writes).
    let account = ctx.account_manager.get_account(did).await?;
    let identity_evt = IdentityEvent::new(did.to_string(), account.handle);
    ctx.sequencer
        .sequence_identity(identity_evt)
        .await
        .map_err(|e| {
            tracing::warn!("Failed to sequence identity event for {}: {}", did, e);
            e
        })?;
    tracing::info!(did = %did, "Sequenced identity event");

    // §4.2.6 — emit the rotation audit. CLI rotations were previously
    // un-audited; the design requires an audit regardless of entry point, and
    // the CLI is the path most needing one (no XRPC handler trace). There is no
    // operator-auth identity on the CLI, so the actor is the deployment's
    // service DID; `entry_point: "cli"` in the payload marks the origin without
    // polluting the actor or the provenance `source` column. Private key
    // material never appears — only public did:keys.
    let subject = Subject::Repo {
        did: did.to_string(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject).await?;
    let rationale_str = rationale.unwrap_or("rotate account signing key (CLI)");
    let payload = serde_json::json!({
        "old_atproto_signing_key": old_keys.old_public_did_key,
        "new_atproto_signing_key": rotation_keypair.public_did_key,
        "generation_source": generation_source,
        "empty_commit_cid": commit_cid.to_string(),
        "entry_point": "cli",
    });
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(PdsError::Database)?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: Some(payload),
            actor_did: &ctx.config.service.service_did,
            action: "account.update_signing_key",
            subject: Some(&subject),
            rationale: rationale_str,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await?;
    tx.commit().await.map_err(PdsError::Database)?;
    tracing::info!(did = %did, "Rotated account signing key via CLI");

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::federation_peers::test_support::create_test_context_with;
    use crate::crypto::plc_client::MockPlcClient;
    use clap::Parser;

    // ---- flag parsing (key-rotation arc B4 / §4.4) ----

    #[test]
    fn rotate_keys_flags_require_both_operator_key_halves() {
        use crate::cli::{Cli, Commands};
        // Neither half → parses (PDS-generated path).
        let cli = Cli::try_parse_from(["aurora", "rotate-keys", "did:plc:a"]).unwrap();
        match cli.command {
            Some(Commands::RotateKeys { public_key, private_key_hex, dids, .. }) => {
                assert!(public_key.is_none() && private_key_hex.is_none());
                assert_eq!(dids, vec!["did:plc:a".to_string()]);
            }
            _ => panic!("expected RotateKeys"),
        }
        // Both halves → parses.
        assert!(Cli::try_parse_from([
            "aurora", "rotate-keys", "did:plc:a",
            "--public-key", "did:key:zP", "--private-key-hex", "abcd",
        ])
        .is_ok());
        // Only public → parse error (clap `requires`).
        assert!(Cli::try_parse_from([
            "aurora", "rotate-keys", "did:plc:a", "--public-key", "did:key:zP",
        ])
        .is_err());
        // Only private → parse error.
        assert!(Cli::try_parse_from([
            "aurora", "rotate-keys", "did:plc:a", "--private-key-hex", "abcd",
        ])
        .is_err());
        // Rationale parses.
        assert!(Cli::try_parse_from([
            "aurora", "rotate-keys", "did:plc:a", "--rationale", "scheduled",
        ])
        .is_ok());
    }

    // ---- rotation flow (mirrors the B3 admin handler tests) ----

    // Seed an account AND its actor-store repo (genesis), so the empty-commit
    // advance has a repo to append to — same as the B3 handler tests.
    async fn seed_account(ctx: &AppContext, handle: &str) -> String {
        let acc = ctx
            .account_manager
            .create_account(handle.into(), Some(format!("{handle}@example.com")), "password123".into(), None, None)
            .await
            .unwrap();
        let repo_mgr = RepositoryManager::with_validation_mode(
            acc.did.clone(),
            (*ctx.actor_store).clone(),
            ctx.config.validation_mode,
        );
        repo_mgr.initialize().await.expect("init actor repo");
        let h = acc.handle.clone().unwrap_or_else(|| handle.to_string());
        crate::api::account_emit::create_account_emit_sequence(ctx, &acc.did, &h)
            .await
            .expect("seed genesis repo");
        acc.did
    }

    async fn set_gate(ctx: &AppContext, enabled: bool) {
        sqlx::query(
            "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(crate::api::aurora_admin::KEY_ROTATION_OPERATOR_SUPPLIED_KEYS_ENABLED_KEY)
        .bind(if enabled { "true" } else { "false" })
        .bind("2026-06-25T00:00:00Z")
        .bind("did:plc:super")
        .execute(&ctx.account_db)
        .await
        .expect("seed gate row");
    }

    fn signer(ctx: &AppContext) -> PlcSigner {
        PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key).unwrap()
    }

    #[tokio::test]
    async fn cli_rotation_pds_generated_rotates_and_audits() {
        let ctx = create_test_context_with(|_| {}).await;
        let did = seed_account(&ctx, "clipds").await;
        let before = ctx.account_manager.get_atproto_signing_key_bytes(&did).await.unwrap();
        // Current published key differs from any fresh key → not a no-op.
        let mock = MockPlcClient::new().with_current_signing_key(&did, "zCurrentDifferentKey");
        let rotated = rotate_key_for_did(&ctx, &did, &mock, &signer(&ctx), None, Some("scheduled")).await.unwrap();
        assert!(rotated, "a fresh key is always a real rotation");
        let after = ctx.account_manager.get_atproto_signing_key_bytes(&did).await.unwrap();
        assert_ne!(before, after, "plc_keys rotated");
        let payload: String = sqlx::query_scalar(
            "SELECT payload FROM audit_chain_entry WHERE action = $1",
        )
        .bind("account.update_signing_key")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert!(payload.contains("\"generation_source\":\"pds\""), "got {payload}");
        assert!(payload.contains("\"entry_point\":\"cli\""), "CLI origin recorded");
        assert!(!payload.contains(&hex::encode(&after)), "no private key in audit");
    }

    #[tokio::test]
    async fn cli_rotation_operator_supplied_gate_off_rejects() {
        let ctx = create_test_context_with(|_| {}).await; // gate defaults OFF
        let did = seed_account(&ctx, "cligateoff").await;
        let before = ctx.account_manager.get_atproto_signing_key_bytes(&did).await.unwrap();
        let kp = ctx.account_manager.generate_rotation_keypair().unwrap();
        let supplied = OperatorSuppliedKeypair {
            public_did_key: kp.public_did_key.clone(),
            private_key_hex: hex::encode(&kp.private_key_bytes),
        };
        let mock = MockPlcClient::new().with_current_signing_key(&did, "zCurrentDifferentKey");
        let err = rotate_key_for_did(&ctx, &did, &mock, &signer(&ctx), Some(&supplied), None)
            .await
            .expect_err("gate off rejects operator-supplied");
        assert!(matches!(err, PdsError::Validation(_)), "got {err:?}");
        let after = ctx.account_manager.get_atproto_signing_key_bytes(&did).await.unwrap();
        assert_eq!(before, after, "gate-off rejection must not rotate");
    }

    #[tokio::test]
    async fn cli_rotation_operator_supplied_gate_on_rotates() {
        let ctx = create_test_context_with(|_| {}).await;
        set_gate(&ctx, true).await;
        let did = seed_account(&ctx, "cligateon").await;
        let kp = ctx.account_manager.generate_rotation_keypair().unwrap();
        let supplied = OperatorSuppliedKeypair {
            public_did_key: kp.public_did_key.clone(),
            private_key_hex: hex::encode(&kp.private_key_bytes),
        };
        let mock = MockPlcClient::new().with_current_signing_key(&did, "zCurrentDifferentKey");
        let rotated = rotate_key_for_did(&ctx, &did, &mock, &signer(&ctx), Some(&supplied), None).await.unwrap();
        assert!(rotated);
        let after = ctx.account_manager.get_atproto_signing_key_bytes(&did).await.unwrap();
        assert_eq!(after, kp.private_key_bytes, "stored key is the operator-supplied one");
    }

    #[tokio::test]
    async fn cli_rotation_operator_supplied_mismatch_rejects_before_gate() {
        let ctx = create_test_context_with(|_| {}).await; // gate OFF — mismatch still wins
        let did = seed_account(&ctx, "climis").await;
        let gen = ctx.account_manager.generate_rotation_keypair().unwrap();
        let other = ctx.account_manager.generate_rotation_keypair().unwrap();
        let supplied = OperatorSuppliedKeypair {
            public_did_key: other.public_did_key.clone(),
            private_key_hex: hex::encode(&gen.private_key_bytes),
        };
        let mock = MockPlcClient::new().with_current_signing_key(&did, "zCurrentDifferentKey");
        let err = rotate_key_for_did(&ctx, &did, &mock, &signer(&ctx), Some(&supplied), None)
            .await
            .expect_err("mismatch rejected");
        let msg = err.to_string();
        assert!(msg.contains("mismatch"), "validate-first surfaces mismatch, not gate: {msg}");
    }

    #[tokio::test]
    async fn cli_rotation_no_op_when_key_already_current() {
        let ctx = create_test_context_with(|_| {}).await;
        set_gate(&ctx, true).await;
        let did = seed_account(&ctx, "clinoop").await;
        let before = ctx.account_manager.get_atproto_signing_key_bytes(&did).await.unwrap();
        let kp = ctx.account_manager.generate_rotation_keypair().unwrap();
        let supplied = OperatorSuppliedKeypair {
            public_did_key: kp.public_did_key.clone(),
            private_key_hex: hex::encode(&kp.private_key_bytes),
        };
        // Published key already equals the operator-supplied key → no-op.
        let mock = MockPlcClient::new().with_current_signing_key(&did, &kp.public_did_key);
        let rotated = rotate_key_for_did(&ctx, &did, &mock, &signer(&ctx), Some(&supplied), None).await.unwrap();
        assert!(!rotated, "operator key already current → idempotent no-op");
        let after = ctx.account_manager.get_atproto_signing_key_bytes(&did).await.unwrap();
        assert_eq!(before, after, "no-op must not mutate the stored key");
    }

    #[tokio::test]
    async fn cli_rotate_keys_operator_key_requires_single_did() {
        let ctx = create_test_context_with(|_| {}).await;
        let kp = ctx.account_manager.generate_rotation_keypair().unwrap();
        let supplied = OperatorSuppliedKeypair {
            public_did_key: kp.public_did_key.clone(),
            private_key_hex: hex::encode(&kp.private_key_bytes),
        };
        // Two DIDs + an operator keypair → rejected before any mutation.
        let err = rotate_keys(
            &ctx,
            vec!["did:plc:a".into(), "did:plc:b".into()],
            10,
            None,
            Some(supplied),
        )
        .await
        .expect_err("operator-supplied keypair with >1 DID is rejected");
        assert!(matches!(err, PdsError::Validation(_)), "got {err:?}");
    }
}
