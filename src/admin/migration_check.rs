//! Signing-key migration check (key-rotation arc Phase C / #376, design §5.3).
//!
//! A read-only operator diagnostic: for every account, confirm the signing key
//! stored locally in `plc_keys.atproto_signing_key` (the per-account ES256K
//! private key) derives to the same public key PLC currently publishes. A
//! divergence means a prior `update_account_signing_key` call wrote one side
//! without the other — the contradiction Phase B's reshape fixed. The set is
//! expected to be empty; any divergence is surfaced to the operator to decide
//! per account (re-rotate, investigate, accept). No remediation is automated.

use crate::context::AppContext;
use crate::crypto::plc::PlcSigner;
use crate::error::{PdsError, PdsResult};
use serde::Serialize;
use sqlx::Row as _;

/// An account whose locally-stored key derives to a different public key than
/// PLC publishes — the anomaly the check hunts for.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DivergentAccount {
    pub did: String,
    /// Public did:key derived from `plc_keys.atproto_signing_key`.
    pub stored_public_did_key: String,
    /// Public did:key PLC currently publishes (the latest op-history entry).
    pub published_public_did_key: String,
}

/// An account the check could not resolve — PLC unreachable / no history, or a
/// malformed/empty stored key. Reported (never silently dropped) so the
/// operator sees the gap honestly.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvableAccount {
    pub did: String,
    pub reason: String,
}

/// The migration-check report. `accounts_checked == aligned +
/// divergences.len() + unresolvable.len()`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationCheckReport {
    pub accounts_checked: usize,
    pub aligned: usize,
    pub divergences: Vec<DivergentAccount>,
    pub unresolvable: Vec<UnresolvableAccount>,
}

/// Normalize a key to its bare multibase form (drop a `did:key:` prefix) so
/// stored-derived and PLC-published forms compare regardless of representation.
fn normalize(key: &str) -> &str {
    key.strip_prefix("did:key:").unwrap_or(key)
}

/// Run the signing-key migration check across all accounts.
///
/// Every `plc_keys` row corresponds to an account (the table's `did` is a
/// foreign key into `actor`), so a bare scan of `plc_keys` covers both
/// account-creation origination and federation-entryway origination — the
/// `SELECT ... WHERE did IN (SELECT did FROM accounts)` of design §5.3
/// translated to this codebase's schema (there is no `accounts` table; the
/// `account`/`actor` split means `plc_keys` rows ARE the accounts).
pub async fn run_signing_key_migration_check(
    ctx: &AppContext,
) -> PdsResult<MigrationCheckReport> {
    let rows = sqlx::query("SELECT did, atproto_signing_key FROM plc_keys")
        .fetch_all(&ctx.account_db)
        .await
        .map_err(PdsError::Database)?;

    let mut report = MigrationCheckReport {
        accounts_checked: rows.len(),
        aligned: 0,
        divergences: Vec::new(),
        unresolvable: Vec::new(),
    };

    for row in &rows {
        let did: String = row.try_get("did").map_err(PdsError::Database)?;
        let stored_hex: String = row.try_get("atproto_signing_key").map_err(PdsError::Database)?;

        // Derive the stored public did:key from the private bytes (the same
        // PlcSigner path account creation + validate_operator_keypair use). A
        // malformed/empty stored key can't be checked → unresolvable.
        let stored_public = match PlcSigner::from_hex(&stored_hex) {
            Ok(signer) => signer.public_key_did_key(),
            Err(e) => {
                report.unresolvable.push(UnresolvableAccount {
                    did,
                    reason: format!("stored signing key is unusable: {e}"),
                });
                continue;
            }
        };

        // The currently-published key is the newest op-history entry
        // (oldest-first ordering). A fetch failure / empty history → unresolvable
        // (do not count as aligned or divergent).
        let published = match ctx.plc_client.get_op_history(&did).await {
            Ok(history) => match history.last() {
                Some(entry) => entry.signing_did_key.clone(),
                None => {
                    report.unresolvable.push(UnresolvableAccount {
                        did,
                        reason: "PLC op-history is empty".to_string(),
                    });
                    continue;
                }
            },
            Err(e) => {
                report.unresolvable.push(UnresolvableAccount {
                    did,
                    reason: format!("could not fetch PLC op-history: {e}"),
                });
                continue;
            }
        };

        if normalize(&stored_public) == normalize(&published) {
            report.aligned += 1;
        } else {
            report.divergences.push(DivergentAccount {
                did,
                stored_public_did_key: stored_public,
                published_public_did_key: published,
            });
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::federation_peers::test_support::create_test_context_with;
    use crate::context::AppContext;
    use crate::crypto::plc_client::{mock_op_history, MockPlcClient};
    use std::sync::Arc;

    // Create an account and return (did, its stored key's public did:key).
    async fn seed_and_derive(ctx: &AppContext, handle: &str) -> (String, String) {
        let acc = ctx
            .account_manager
            .create_account(handle.into(), Some(format!("{handle}@example.com")), "password123".into(), None, None)
            .await
            .unwrap();
        let bytes = ctx.account_manager.get_atproto_signing_key_bytes(&acc.did).await.unwrap();
        let pubkey = PlcSigner::from_hex(&hex::encode(&bytes)).unwrap().public_key_did_key();
        (acc.did, pubkey)
    }

    #[tokio::test]
    async fn empty_account_population_reports_all_zero() {
        let ctx = create_test_context_with(|_| {}).await;
        let report = run_signing_key_migration_check(&ctx).await.unwrap();
        assert_eq!(report.accounts_checked, 0);
        assert_eq!(report.aligned, 0);
        assert!(report.divergences.is_empty());
        assert!(report.unresolvable.is_empty());
    }

    #[tokio::test]
    async fn aligned_account_counts_aligned() {
        let mut ctx = create_test_context_with(|_| {}).await;
        let (did, pubkey) = seed_and_derive(&ctx, "migaligned").await;
        // PLC publishes exactly the key the stored private bytes derive to.
        ctx.plc_client = Arc::new(
            MockPlcClient::new()
                .with_op_history(&did, mock_op_history(&[(pubkey.as_str(), "2026-01-01T00:00:00Z")])),
        );
        let report = run_signing_key_migration_check(&ctx).await.unwrap();
        assert_eq!(report.accounts_checked, 1);
        assert_eq!(report.aligned, 1);
        assert!(report.divergences.is_empty(), "got {:?}", report.divergences);
        assert!(report.unresolvable.is_empty());
    }

    #[tokio::test]
    async fn divergent_account_is_reported_with_both_keys() {
        let mut ctx = create_test_context_with(|_| {}).await;
        let (did, pubkey) = seed_and_derive(&ctx, "migdiverge").await;
        let published = "did:key:zQ3shokFTS3brHcDQrn82RUDfCZESWL1ZdCEJwekUDPQiYBme";
        assert_ne!(pubkey, published);
        ctx.plc_client = Arc::new(
            MockPlcClient::new()
                .with_op_history(&did, mock_op_history(&[(published, "2026-01-01T00:00:00Z")])),
        );
        let report = run_signing_key_migration_check(&ctx).await.unwrap();
        assert_eq!(report.accounts_checked, 1);
        assert_eq!(report.aligned, 0);
        assert_eq!(report.divergences.len(), 1);
        let d = &report.divergences[0];
        assert_eq!(d.did, did);
        assert_eq!(d.stored_public_did_key, pubkey);
        assert_eq!(d.published_public_did_key, published);
    }

    #[tokio::test]
    async fn mixed_population_counts_add_up() {
        let mut ctx = create_test_context_with(|_| {}).await;
        let (did_ok, pub_ok) = seed_and_derive(&ctx, "migmixok").await;
        let (did_bad, _pub_bad) = seed_and_derive(&ctx, "migmixbad").await;
        let (did_unres, _pub_unres) = seed_and_derive(&ctx, "migmixunres").await;
        // ok: published == stored; bad: published differs; unres: no op-history.
        ctx.plc_client = Arc::new(
            MockPlcClient::new()
                .with_op_history(&did_ok, mock_op_history(&[(pub_ok.as_str(), "2026-01-01T00:00:00Z")]))
                .with_op_history(
                    &did_bad,
                    mock_op_history(&[("did:key:zDifferentPublishedKey00000", "2026-01-01T00:00:00Z")]),
                ),
            // did_unres intentionally has no configured op-history → fetch errors.
        );
        let report = run_signing_key_migration_check(&ctx).await.unwrap();
        assert_eq!(report.accounts_checked, 3);
        assert_eq!(report.aligned, 1);
        assert_eq!(report.divergences.len(), 1);
        assert_eq!(report.divergences[0].did, did_bad);
        assert_eq!(report.unresolvable.len(), 1);
        assert_eq!(report.unresolvable[0].did, did_unres);
        assert_eq!(
            report.accounts_checked,
            report.aligned + report.divergences.len() + report.unresolvable.len()
        );
    }
}
