//! Account status: deactivation, activation (which checks the DID
//! document, as the reference), `checkAccountStatus`, and deletion as a
//! resumable bulk job.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use locus_auth::password;
use locus_auth::session::SCOPE_TAKENDOWN;
use locus_repo::event::{account_body, identity_body, sync_body};
use locus_store::accounts::{SessionFilter, Status};
use locus_store::bulk::BulkJob;
use locus_store::email::TokenPurpose;
use proto_blue_crypto::Keypair;
use proto_blue_lex_data::Cid;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

/// Rows a deletion batch removes at most.
const DELETE_BATCH: i64 = 2_000;

fn internal(e: impl std::fmt::Display) -> XrpcError {
    XrpcError::internal(e.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeactivateInput {
    delete_after: Option<String>,
}

pub async fn deactivate_account(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DeactivateInput>,
) -> XrpcResult<()> {
    let authed = auth::access_with(&s, &headers, auth::FULL, &[SCOPE_TAKENDOWN]).await?;
    let delete_after = input
        .delete_after
        .as_deref()
        .map(|t| {
            parse_datetime_ms(t)
                .ok_or_else(|| XrpcError::invalid_request("deleteAfter must be a datetime"))
        })
        .transpose()?;
    let did = authed.did.as_str();
    let (_lock, mut tx) = s.repo.locked_tx(did).await.map_err(internal)?;
    let status = tx
        .deactivate_account(did, delete_after)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("Account not found"))?;
    let body = account_body(did, false, Some(status.as_str())).map_err(internal)?;
    tx.insert_outbox(did, "account", &body).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn activate_account(State(s): St, headers: HeaderMap) -> XrpcResult<()> {
    let authed = auth::full_access(&s, &headers).await?;
    let did = authed.did.as_str();
    assert_valid_did_doc(&s, did).await?;
    let account = s
        .store
        .account_by_did(did)
        .await?
        .ok_or_else(|| XrpcError::named("AccountNotFound", "user not found"))?;
    let (_lock, mut tx) = s.repo.locked_tx(did).await.map_err(internal)?;
    if !tx.activate_account(did).await? {
        return Err(XrpcError::named("AccountNotFound", "user not found"));
    }
    // As the reference: #account, #identity and #sync, together.
    let root = tx
        .repo_root(did)
        .await?
        .ok_or_else(|| XrpcError::internal("account has no repository"))?;
    let commit_bytes = tx
        .repo_block(did, &root.commit_cid)
        .await?
        .ok_or_else(|| XrpcError::internal("the repository's commit block is missing"))?;
    let commit_cid = Cid::from_str_multibase(&root.commit_cid).map_err(internal)?;
    tx.insert_outbox(
        did,
        "account",
        &account_body(did, true, None).map_err(internal)?,
    )
    .await?;
    tx.insert_outbox(
        did,
        "identity",
        &identity_body(did, Some(&account.handle)).map_err(internal)?,
    )
    .await?;
    tx.insert_outbox(
        did,
        "sync",
        &sync_body(did, &root.rev, &commit_cid, &commit_bytes).map_err(internal)?,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Whether the DID document points here, as the reference's
/// `assertValidDidDocumentForService`: this PDS's endpoint, the account's
/// signing key and, for did:plc, the current PDS rotation key. A passing
/// did:plc check records which held keys the document lists.
pub async fn assert_valid_did_doc(s: &AppState, did: &str) -> XrpcResult<()> {
    let signing = s.signing_key(did).await?.did();
    let (endpoint, key, rotation) = if did.starts_with("did:plc:") {
        let raw = s
            .plc()?
            .last_op_json(did)
            .await
            .map_err(|e| XrpcError::invalid_request(format!("Could not resolve DID: {e}")))?
            .ok_or_else(|| XrpcError::invalid_request("Could not resolve DID"))?;
        let (state, _) = locus_identity::plc::last_op_state(&raw)
            .map_err(|e| XrpcError::invalid_request(e.to_string()))?;
        let op = state.ok_or_else(|| XrpcError::invalid_request("DID is tombstoned"))?;
        (
            op.services.get("atproto_pds").map(|p| p.endpoint.clone()),
            op.verification_methods.get("atproto").cloned(),
            Some(op.rotation_keys),
        )
    } else if let Some(doc) = s.store.hosted_did_web(did).await?.and_then(|h| h.doc) {
        // A document locus serves itself.
        let doc: proto_blue_common::DidDocument = serde_json::from_str(&doc).map_err(internal)?;
        (
            proto_blue_common::get_pds_endpoint(&doc),
            proto_blue_common::get_signing_did_key(&doc),
            None,
        )
    } else {
        let doc = s
            .resolvers()?
            .resolve_did(did, true)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| XrpcError::invalid_request("Could not resolve DID"))?;
        (
            proto_blue_common::get_pds_endpoint(&doc),
            proto_blue_common::get_signing_did_key(&doc),
            None,
        )
    };
    if let Some(rotation) = &rotation {
        let current = s.secrets.plc_keys.current().did();
        if !rotation.contains(&current) {
            let listed = s.secrets.plc_keys.listed(rotation);
            let message = if listed.is_empty() {
                "Server rotation key not included in PLC DID data".to_owned()
            } else {
                "Server rotation key not included in PLC DID data: the document lists only an older \
                 key of this server; add the current one with signPlcOperation"
                    .to_owned()
            };
            return Err(XrpcError::invalid_request(message));
        }
    }
    if endpoint.as_deref() != Some(s.public_url.as_str()) {
        return Err(XrpcError::invalid_request(
            "DID document atproto_pds service endpoint does not match PDS public url",
        ));
    }
    if key.as_deref() != Some(signing.as_str()) {
        return Err(XrpcError::invalid_request(
            "DID document verification method does not match expected signing key",
        ));
    }
    if let Some(rotation) = rotation {
        let _ = s
            .store
            .set_plc_key_listing(did, &s.secrets.plc_keys.listed(&rotation))
            .await;
    }
    Ok(())
}

pub async fn check_account_status(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    let authed = auth::access(&s, &headers).await?;
    let did = authed.did.as_str();
    let account = s
        .store
        .account_by_did(did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("account not found"))?;
    let (commit, rev, blocks, records) = s
        .store
        .repo_stats(did)
        .await?
        .ok_or_else(|| XrpcError::internal("account has no repository"))?;
    let valid_did = assert_valid_did_doc(&s, did).await.is_ok();
    Ok(Json(json!({
        "activated": account.status == Status::Active,
        "validDid": valid_did,
        "repoCommit": commit,
        "repoRev": rev,
        "repoBlocks": blocks,
        "indexedRecords": records,
        "privateStateValues": 0,
        "expectedBlobs": 0,
        "importedBlobs": 0,
    })))
}

pub async fn request_account_delete(State(s): St, headers: HeaderMap) -> XrpcResult<()> {
    let authed = auth::full_access(&s, &headers).await?;
    let account = auth::account_checked(&s, &authed.did, false).await?;
    let email = account
        .email
        .ok_or_else(|| XrpcError::invalid_request("account does not have an email address"))?;
    crate::email::require_email(&s).await?;
    let token = crate::email::issue(&s, &account.did, TokenPurpose::DeleteAccount).await?;
    crate::email::send(
        &s,
        locus_mail::Template::DeleteAccount,
        &email,
        &token,
        &account.handle,
    )
    .await
}

#[derive(Deserialize)]
pub struct DeleteAccountInput {
    did: String,
    password: String,
    token: String,
}

pub async fn delete_account(State(s): St, Json(input): Json<DeleteAccountInput>) -> XrpcResult<()> {
    if input.password.len() > password::MAX_LOGIN_PASSWORD {
        return Err(XrpcError::auth_required(
            "Password too long. Consider resetting your password.",
        ));
    }
    let account = s
        .store
        .account_by_did(&input.did)
        .await?
        .filter(|a| a.status != Status::Deleted)
        .ok_or_else(|| XrpcError::invalid_request("account not found"))?;
    let valid = account
        .password_hash
        .as_deref()
        .is_some_and(|h| password::verify(&input.password, h));
    if !valid {
        return Err(XrpcError::auth_required("Invalid did or password"));
    }
    crate::email::check(&s, &account.did, TokenPurpose::DeleteAccount, &input.token).await?;
    let job = start_deletion(&s, &account.did).await?;
    spawn_deletion(&s, job);
    Ok(())
}

/// The first, forward-only step of a deletion, in one transaction under the
/// repo lock: the bulk job and write block, status `deleted`, every
/// session revoked, and `#account deleted`. Batches then remove the rest.
pub async fn start_deletion(s: &AppState, did: &str) -> XrpcResult<BulkJob> {
    let id = format!("delete-{}", hex::encode(rand::random::<[u8; 12]>()));
    let (_lock, mut tx) = s.repo.locked_tx(did).await.map_err(internal)?;
    let job = tx
        .start_bulk_job(&id, did, "delete", "deleting", &s.node_id)
        .await
        .map_err(|e| match e {
            locus_core::CoreError::Conflict(_) => write_blocked(),
            other => other.into(),
        })?;
    tx.set_account_status(did, Status::Deleted, None).await?;
    tx.revoke_sessions(did, SessionFilter::All).await?;
    tx.delete_all_email_tokens(did).await?;
    tx.insert_outbox(
        did,
        "account",
        &account_body(did, false, Some("deleted")).map_err(internal)?,
    )
    .await?;
    tx.commit().await?;
    Ok(job)
}

/// 503 `RepoWriteBlocked`, retryable.
pub fn write_blocked() -> XrpcError {
    XrpcError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "RepoWriteBlocked",
        "Another operation on this account is running; try again shortly",
    )
}

/// Runs a deletion's batches in the background; the bulk-job resumer
/// finishes it if this node stops first.
pub fn spawn_deletion(s: &AppState, job: BulkJob) {
    let store = s.store.clone();
    tokio::spawn(async move {
        if let Err(e) = store.run_delete_job(&job, DELETE_BATCH).await {
            tracing::warn!(did = job.did, job = job.id, "account deletion paused: {e}");
        }
    });
}

/// One pass of the bulk-job resumer: takes over jobs whose node stopped,
/// and runs them here.
pub async fn resume_bulk_jobs(s: &AppState) -> locus_core::CoreResult<usize> {
    let mut resumed = 0;
    while let Some(job) = s.store.take_over_bulk_job(&s.node_id).await? {
        match job.kind.as_str() {
            "delete" => {
                tracing::info!(did = job.did, job = job.id, "resuming account deletion");
                s.store.run_delete_job(&job, DELETE_BATCH).await?;
            }
            other => tracing::warn!(job = job.id, "no runner for bulk job kind {other}"),
        }
        resumed += 1;
    }
    Ok(resumed)
}

/// One pass of the `deleteAfter` job: deletes deactivated accounts whose
/// time has come.
pub async fn delete_due_accounts(s: &AppState) -> locus_core::CoreResult<usize> {
    let mut n = 0;
    for did in s.store.accounts_due_for_deletion(50).await? {
        match start_deletion(s, &did).await {
            Ok(job) => {
                s.store.run_delete_job(&job, DELETE_BATCH).await?;
                n += 1;
            }
            // Blocked by an import: try again on a later pass.
            Err(e) if e.error == "RepoWriteBlocked" => {}
            Err(e) => tracing::warn!(did, "scheduled deletion failed: {}", e.message),
        }
    }
    Ok(n)
}

/// Parses an RFC 3339 datetime into ms since the epoch (UTC offsets and
/// fractional seconds accepted).
fn parse_datetime_ms(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.splitn(3, '-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let (time, offset_min) =
        if let Some(t) = rest.strip_suffix('Z').or_else(|| rest.strip_suffix('z')) {
            (t, 0)
        } else {
            let i = rest.rfind(['+', '-'])?;
            let (t, off) = rest.split_at(i);
            let sign = if off.starts_with('-') { -1 } else { 1 };
            let (oh, om) = off[1..].split_once(':')?;
            (
                t,
                sign * (oh.parse::<i64>().ok()? * 60 + om.parse::<i64>().ok()?),
            )
        };
    let mut t = time.splitn(3, ':');
    let (hh, mm): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let sec = t.next()?;
    let (ss, frac) = sec.split_once('.').unwrap_or((sec, "0"));
    let ss: i64 = ss.parse().ok()?;
    let ms: i64 = format!("{frac:0<3}")[..3].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + hh * 3600 + mm * 60 + ss) - offset_min * 60) * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::parse_datetime_ms;

    #[test]
    fn datetimes() {
        assert_eq!(parse_datetime_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_datetime_ms("2023-11-14T22:13:20.123Z"),
            Some(1_700_000_000_123)
        );
        assert_eq!(
            parse_datetime_ms("2023-11-14T23:13:20.123+01:00"),
            Some(1_700_000_000_123)
        );
        assert_eq!(
            parse_datetime_ms("2000-02-29T00:00:00.5Z"),
            Some(951_782_400_500)
        );
        assert_eq!(parse_datetime_ms("not a date"), None);
        assert_eq!(parse_datetime_ms("2023-13-01T00:00:00Z"), None);
    }
}
