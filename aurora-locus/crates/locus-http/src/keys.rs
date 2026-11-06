//! Key operations for owners: what keys are held, key backups, and
//! changes to the PDS's PLC rotation key (a new pending key, making it
//! current, discarding it, dropping an old secondary one), plus the PLC key
//! scan that records which held keys each account's document lists.
//!
//! A change to a key nodes hold in memory bumps the install's key
//! generation; every node then restarts its service in place to reload.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use locus_core::key_backup::{Entry, KdfCost, Sealer};
use locus_core::locus_nsid;
use locus_store::identity::PlcKeyRole;
use proto_blue_crypto::{ExportableKeypair, K256Keypair, Keypair};
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::admin_auth::{self, Caller, Reauth, Role, Surface};
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

/// Entries per backup frame.
const FRAME_ENTRIES: i64 = 500;

async fn owner(s: &AppState, headers: &HeaderMap, lxm: &str) -> XrpcResult<Caller> {
    admin_auth::require(s, headers, lxm, Surface::Locus, Role::Owner).await
}

/// Asks every node, this one first, to reload its keys, once the response
/// that caused it has gone out.
pub(crate) fn reload_soon(s: &AppState) {
    let token = s.reload.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        token.cancel();
    });
}

fn backup_cost() -> KdfCost {
    if cfg!(test) {
        KdfCost {
            m_kib: 256,
            t: 1,
            p: 1,
        }
    } else {
        KdfCost::DEFAULT
    }
}

/// `owner.keys.show`: the held keys and how accounts' documents list them.
pub async fn show(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    owner(&s, &headers, locus_nsid!("owner.keys.show")).await?;
    let state = s.store.key_state().await?;
    let rows = s.store.plc_key_rows().await?;
    let current = rows
        .iter()
        .find(|r| r.role == "current")
        .map(|r| r.did_key.clone())
        .unwrap_or_default();
    let counts = s.store.plc_listing_counts(&current).await?;
    let backed_up = |created: i64| state.last_backup_at.is_some_and(|b| b > created);
    Ok(Json(json!({
        "masterKey": {"id": s.master_key.key_id(), "source": s.master_key.source().as_str()},
        "serviceKey": s.secrets.service_key.did(),
        "plcRotationKeys": rows.iter().map(|r| json!({
            "didKey": r.did_key,
            "role": r.role,
            "createdAt": r.created_at,
            "currentSince": r.current_since,
            "secondarySince": r.secondary_since,
            "inLastBackup": backed_up(r.created_at),
        })).collect::<Vec<_>>(),
        "lastBackup": state.last_backup_at.map(|at| json!({"at": at, "by": state.last_backup_by})),
        "accounts": {
            "listingCurrent": counts.current,
            "listingOnlySecondary": counts.secondary_only,
            "listingNoHeldKey": counts.none,
            "notChecked": counts.unchecked,
        },
        "scan": {
            "running": state.scan_cursor.is_some(),
            "startedAt": state.scan_started_at,
            "completedAt": state.scan_completed_at,
        },
    })))
}

#[derive(Deserialize)]
pub struct BackupInput {
    passphrase: String,
}

enum Phase {
    PlcKeys,
    Secrets(String),
    SigningKeys(String),
    Last,
    Record,
    Done,
}

/// The backup stream: header, frames of entries, and, once the last frame
/// has been produced, the backup recorded (making a pending PLC key that
/// predates it current).
pub(crate) async fn backup_stream(
    s: Arc<AppState>,
    passphrase: &str,
    actor: String,
) -> XrpcResult<Body> {
    let started_at = s.store.db_now_ms().await?;
    let (sealer, head) = Sealer::new(
        passphrase,
        &s.service_did,
        &s.master_key.key_id(),
        started_at,
        backup_cost(),
    )
    .map_err(XrpcError::from)?;
    let first =
        futures::stream::once(async move { Ok::<axum::body::Bytes, std::io::Error>(head.into()) });
    let rest = futures::stream::unfold(
        (s, sealer, Phase::PlcKeys, actor),
        move |(s, mut sealer, phase, actor)| async move {
            let io = |e: locus_core::CoreError| std::io::Error::other(e.to_string());
            let (entries, last, next) = match phase {
                Phase::Done => return None,
                Phase::Record => {
                    match s.store.record_key_backup(started_at, &actor).await {
                        Ok(promoted) => {
                            tracing::info!(by = actor, "key backup made");
                            if let Some(k) = promoted {
                                tracing::info!(
                                    did_key = k,
                                    "the pending PLC rotation key is now current"
                                );
                                reload_soon(&s);
                            }
                        }
                        Err(e) => tracing::warn!("couldn't record the key backup: {e}"),
                    }
                    return None;
                }
                Phase::PlcKeys => match s.store.plc_key_rows().await {
                    Ok(rows) => (
                        rows.into_iter()
                            .map(|r| Entry::PlcKey {
                                did_key: r.did_key,
                                role: r.role,
                            })
                            .collect(),
                        false,
                        Phase::Secrets(String::new()),
                    ),
                    Err(e) => return Some((Err(io(e)), (s, sealer, Phase::Done, actor))),
                },
                Phase::Secrets(after) => {
                    match s
                        .store
                        .secrets_page(&after, FRAME_ENTRIES, &s.master_key)
                        .await
                    {
                        Ok(page) => {
                            let next = match page.last() {
                                Some((name, _)) if page.len() as i64 == FRAME_ENTRIES => {
                                    Phase::Secrets(name.clone())
                                }
                                _ => Phase::SigningKeys(String::new()),
                            };
                            (
                                page.into_iter()
                                    .map(|(name, value)| Entry::Secret { name, value })
                                    .collect(),
                                false,
                                next,
                            )
                        }
                        Err(e) => return Some((Err(io(e)), (s, sealer, Phase::Done, actor))),
                    }
                }
                Phase::SigningKeys(after) => {
                    match s
                        .store
                        .signing_keys_page(&after, FRAME_ENTRIES, &s.master_key)
                        .await
                    {
                        Ok(page) => {
                            let next = match page.last() {
                                Some((did, _, _)) if page.len() as i64 == FRAME_ENTRIES => {
                                    Phase::SigningKeys(did.clone())
                                }
                                _ => Phase::Last,
                            };
                            (
                                page.into_iter()
                                    .map(|(did, key_type, value)| Entry::SigningKey {
                                        did,
                                        key_type,
                                        value,
                                    })
                                    .collect(),
                                false,
                                next,
                            )
                        }
                        Err(e) => return Some((Err(io(e)), (s, sealer, Phase::Done, actor))),
                    }
                }
                Phase::Last => (Vec::new(), true, Phase::Record),
            };
            match sealer.frame(entries, last) {
                Ok(bytes) => Some((Ok(axum::body::Bytes::from(bytes)), (s, sealer, next, actor))),
                Err(e) => Some((Err(io(e)), (s, sealer, Phase::Done, actor))),
            }
        },
    );
    use futures::StreamExt as _;
    Ok(Body::from_stream(first.chain(rest)))
}

/// The file name a backup downloads as.
pub(crate) fn backup_file_name(s: &AppState) -> String {
    let day = locus_core::time::now_ms() / 86_400_000;
    format!("aurora-locus-keys-{}-{day}.backup", s.hostname)
}

pub(crate) fn backup_response(s: &AppState, body: Body) -> Response {
    let mut res = body.into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) =
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", backup_file_name(s)))
    {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// `owner.keys.backup`: an encrypted backup of every key, streamed.
pub async fn backup(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<BackupInput>,
) -> XrpcResult<Response> {
    let who = owner(&s, &headers, locus_nsid!("owner.keys.backup")).await?;
    admin_auth::audit(&s, &who, "keys.backup", None, None).await;
    let body = backup_stream(Arc::clone(&s), &input.passphrase, who.actor.clone()).await?;
    Ok(backup_response(&s, body))
}

/// Before a held key is removed: re-fetches the documents of every account
/// that may list it, and refuses if one does and lists no other held key
/// (for a pending key, if any lists it at all), or if a fetch fails.
pub(crate) async fn listing_guard(
    s: &AppState,
    did_key: &str,
    since: i64,
    pending: bool,
) -> XrpcResult<()> {
    for did in s.store.plc_guard_candidates(did_key, since).await? {
        let keys = match crate::identity::plc_state(s, &did).await {
            Ok((Some(op), _)) => op.rotation_keys,
            // Tombstoned: lists nothing.
            Ok((None, _)) => continue,
            Err(_) => {
                return Err(XrpcError::invalid_request(format!(
                    "Couldn't check {did}'s DID document, so the key wasn't removed; try again"
                )));
            }
        };
        let listed = s.secrets.plc_keys.listed(&keys);
        s.store.set_plc_key_listing(&did, &listed).await?;
        let names = keys.iter().any(|k| k == did_key);
        if names && (pending || listed.iter().all(|k| k == did_key)) {
            return Err(XrpcError::invalid_request(format!(
                "{did}'s DID document lists {did_key}{}, so it can't be removed",
                if pending {
                    ""
                } else {
                    " and no other key this PDS holds"
                }
            )));
        }
    }
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SetPlcInput {
    /// Generate the new key here.
    #[serde(default)]
    generate: bool,
    /// Or this private key (hex, secp256k1).
    private_key_hex: Option<String>,
    /// Remove the pending key instead.
    #[serde(default)]
    discard_pending: bool,
    /// Make the pending key current without a key backup.
    #[serde(default)]
    acknowledge_no_backup: bool,
    #[serde(flatten)]
    reauth: Reauth,
}

/// `owner.keys.setPlcRotationKey`.
pub async fn set_plc_rotation_key(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<SetPlcInput>,
) -> XrpcResult<Json<Value>> {
    let who = owner(&s, &headers, locus_nsid!("owner.keys.setPlcRotationKey")).await?;
    admin_auth::reauth(&s, &who, &input.reauth).await?;
    let actions = [
        input.generate,
        input.private_key_hex.is_some(),
        input.discard_pending,
        input.acknowledge_no_backup,
    ];
    if actions.iter().filter(|a| **a).count() != 1 {
        return Err(XrpcError::invalid_request(
            "Give exactly one of generate, privateKeyHex, discardPending or acknowledgeNoBackup",
        ));
    }
    let rows = s.store.plc_key_rows().await?;
    let pending = rows.iter().find(|r| r.role == PlcKeyRole::Pending.as_str());
    if input.acknowledge_no_backup {
        let p = pending.ok_or_else(|| XrpcError::invalid_request("No key is pending"))?;
        let mut tx = s.store.begin().await?;
        tx.promote_pending_plc_key().await?;
        tx.commit().await?;
        admin_auth::audit(
            &s,
            &who,
            "keys.plcRotationKey.current",
            Some(&p.did_key),
            Some(json!({"withoutBackup": true})),
        )
        .await;
        reload_soon(&s);
        return Ok(Json(json!({"didKey": p.did_key, "role": "current"})));
    }
    if input.discard_pending {
        let p = pending.ok_or_else(|| XrpcError::invalid_request("No key is pending"))?;
        listing_guard(&s, &p.did_key, p.created_at, true).await?;
        let mut tx = s.store.begin().await?;
        tx.remove_plc_key(&p.did_key).await?;
        tx.commit().await?;
        admin_auth::audit(
            &s,
            &who,
            "keys.plcRotationKey.discard",
            Some(&p.did_key),
            None,
        )
        .await;
        reload_soon(&s);
        return Ok(Json(json!({"didKey": p.did_key, "role": "discarded"})));
    }
    let pair = match &input.private_key_hex {
        Some(h) => {
            let bytes = Zeroizing::new(
                hex::decode(h.trim())
                    .map_err(|_| XrpcError::invalid_request("The private key isn't hex"))?,
            );
            K256Keypair::from_private_key(&bytes)
                .map_err(|_| XrpcError::invalid_request("That isn't a secp256k1 private key"))?
        }
        None => K256Keypair::generate(),
    };
    let did_key = pair.did();
    if let Some(p) = pending.filter(|p| p.did_key != did_key) {
        listing_guard(&s, &p.did_key, p.created_at, true).await?;
    }
    let private = Zeroizing::new(pair.export_private_key());
    let mut tx = s.store.begin().await?;
    let replaced = tx
        .store_pending_plc_key(&did_key, &private, &s.master_key)
        .await?;
    tx.commit().await?;
    admin_auth::audit(
        &s,
        &who,
        "keys.plcRotationKey.pending",
        Some(&did_key),
        Some(json!({"replaced": replaced})),
    )
    .await;
    reload_soon(&s);
    Ok(Json(json!({
        "didKey": did_key,
        "role": "pending",
        "replaced": replaced,
        "next": "Make a key backup (aurora-locus keys backup): the key becomes current once a backup holds it.",
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropInput {
    did_key: String,
    #[serde(flatten)]
    reauth: Reauth,
}

/// `owner.keys.dropPlcKey`: forgets a secondary PLC rotation key no
/// account's document needs any more.
pub async fn drop_plc_key(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DropInput>,
) -> XrpcResult<Json<Value>> {
    let who = owner(&s, &headers, locus_nsid!("owner.keys.dropPlcKey")).await?;
    admin_auth::reauth(&s, &who, &input.reauth).await?;
    let rows = s.store.plc_key_rows().await?;
    let row = rows
        .iter()
        .find(|r| r.did_key == input.did_key)
        .ok_or_else(|| XrpcError::invalid_request("This PDS doesn't hold that key"))?;
    if row.role != PlcKeyRole::Secondary.as_str() {
        return Err(XrpcError::invalid_request(
            "Only a secondary key can be dropped (a pending one is discarded, the current one replaced)",
        ));
    }
    listing_guard(
        &s,
        &row.did_key,
        row.secondary_since.unwrap_or(row.created_at),
        false,
    )
    .await?;
    let mut tx = s.store.begin().await?;
    tx.remove_plc_key(&row.did_key).await?;
    tx.commit().await?;
    admin_auth::audit(
        &s,
        &who,
        "keys.plcRotationKey.drop",
        Some(&row.did_key),
        None,
    )
    .await;
    reload_soon(&s);
    Ok(Json(json!({"didKey": row.did_key, "role": "dropped"})))
}

/// One step of the PLC key scan: starts a pass when one is due (a key
/// became current since the last pass, or a secondary key is held and the
/// last pass is a day old), then checks up to `batch` accounts. Returns
/// how many it checked.
pub async fn plc_scan_step(s: &AppState, batch: i64) -> locus_core::CoreResult<usize> {
    let state = s.store.key_state().await?;
    let cursor = match state.scan_cursor {
        Some(c) => c,
        None => {
            let rows = s.store.plc_key_rows().await?;
            let last_current = rows
                .iter()
                .filter_map(|r| r.current_since)
                .max()
                .unwrap_or(0);
            let secondary = rows
                .iter()
                .any(|r| r.role == PlcKeyRole::Secondary.as_str());
            let done = state.scan_completed_at.unwrap_or(0);
            let now = s.store.db_now_ms().await?;
            let due = done < last_current || (secondary && now - done > 86_400_000);
            if !due {
                return Ok(0);
            }
            s.store.set_plc_scan(Some(""), true, false).await?;
            String::new()
        }
    };
    let dids = s.store.plc_accounts_after(&cursor, batch).await?;
    for did in &dids {
        match crate::identity::plc_state(s, did).await {
            Ok((Some(op), _)) => {
                let listed = s.secrets.plc_keys.listed(&op.rotation_keys);
                s.store.set_plc_key_listing(did, &listed).await?;
            }
            Ok((None, _)) => s.store.set_plc_key_listing(did, &[]).await?,
            Err(_) => {
                // Leave it unchecked for this pass; the guard re-fetches
                // anything unchecked anyway.
                tracing::debug!(did, "PLC key scan couldn't fetch the document");
            }
        }
        // Within the PLC directory's rate limits.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    match dids.last() {
        Some(last) if dids.len() as i64 == batch => {
            s.store.set_plc_scan(Some(last), false, false).await?;
        }
        _ => s.store.set_plc_scan(None, false, true).await?,
    }
    Ok(dids.len())
}
