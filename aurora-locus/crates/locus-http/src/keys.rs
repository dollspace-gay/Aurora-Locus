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
    // Entryway role: the token key shards verify with (as did:key and
    // compressed hex, for a reference shard's env).
    let token_key = if s.role == locus_core::bootstrap::Role::Entryway {
        let k = crate::entryway::token_key(&s).await?;
        let did = k.did();
        let hex = proto_blue_crypto::parse_did_key(&did)
            .ok()
            .and_then(|p| k256::PublicKey::from_sec1_bytes(&p.key_bytes).ok())
            .map(|p| {
                hex::encode(
                    k256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&p, true)
                        .as_bytes(),
                )
            });
        Some(json!({"didKey": did, "hex": hex}))
    } else {
        None
    };
    // Shard role: the entryway keys accepted.
    let entryway = s.entryway.as_ref().map(|ew| {
        json!({"url": ew.url, "did": ew.did, "verifyKeys": ew.verify_key_ids(), "plcRotationKeys": ew.plc_rotation_keys})
    });
    Ok(Json(json!({
        "entrywayTokenKey": token_key,
        "entryway": entryway,
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
                                kms_key_id: r.kms_key_id,
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
    // The held keys as stored, not as this node last loaded them: a key
    // stored moments ago (by this node or another) counts at once.
    let held: Vec<String> = s
        .store
        .plc_key_rows()
        .await?
        .into_iter()
        .map(|r| r.did_key)
        .collect();
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
        let listed: Vec<String> = keys.iter().filter(|k| held.contains(k)).cloned().collect();
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
    /// Or this KMS key (secp256k1, ECC_SECG_P256K1), which signs in KMS.
    kms_key_id: Option<String>,
    /// Remove the pending key instead.
    #[serde(default)]
    discard_pending: bool,
    /// Make the pending key current without a key backup.
    #[serde(default)]
    acknowledge_no_backup: bool,
    /// Entryway role: a shard the operator set the pending key on.
    confirm: Option<String>,
    /// Entryway role: make the pending key current although shards are
    /// unconfirmed (they refuse genesis operations signed with it).
    #[serde(default)]
    acknowledge_unconfirmed: bool,
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
    let rows = s.store.plc_key_rows().await?;
    let pending = rows.iter().find(|r| r.role == PlcKeyRole::Pending.as_str());
    // Entryway role: a shard set to the pending key by hand.
    if let Some(name) = &input.confirm {
        if s.role != locus_core::bootstrap::Role::Entryway {
            return Err(XrpcError::invalid_request(
                "--confirm is for the entryway role",
            ));
        }
        let p = pending.ok_or_else(|| XrpcError::invalid_request("No key is pending"))?;
        s.store
            .shard(name)
            .await?
            .ok_or_else(|| XrpcError::invalid_request("No such shard"))?;
        let (left, promoted) = s.store.confirm_pending_shard(name).await?;
        admin_auth::audit(
            &s,
            &who,
            "keys.plcRotationKey.confirm",
            Some(&p.did_key),
            Some(json!({"shard": name})),
        )
        .await;
        if promoted.is_some() {
            reload_soon(&s);
        }
        return Ok(Json(json!({
            "didKey": p.did_key,
            "role": if promoted.is_some() { "current" } else { "pending" },
            "unconfirmed": left,
        })));
    }
    let actions = [
        input.generate,
        input.private_key_hex.is_some(),
        input.kms_key_id.is_some(),
        input.discard_pending,
        input.acknowledge_no_backup || input.acknowledge_unconfirmed,
    ];
    if actions.iter().filter(|a| **a).count() != 1 {
        return Err(XrpcError::invalid_request(
            "Give exactly one of generate, privateKeyHex, kmsKeyId, discardPending, acknowledgeNoBackup or acknowledgeUnconfirmed",
        ));
    }
    let confirmed = s.store.pending_confirmed_shards().await?;
    if !confirmed.is_empty() && (input.discard_pending || actions[..3].iter().any(|a| *a)) {
        return Err(XrpcError::invalid_request(format!(
            "Shards were set to the pending key ({}); complete the change before setting another",
            confirmed.join(", ")
        )));
    }
    if input.acknowledge_no_backup || input.acknowledge_unconfirmed {
        let p = pending.ok_or_else(|| XrpcError::invalid_request("No key is pending"))?;
        let mut tx = s.store.begin().await?;
        let unconfirmed = tx.unconfirmed_shards().await?;
        if !unconfirmed.is_empty() && !input.acknowledge_unconfirmed {
            return Err(XrpcError::invalid_request(format!(
                "These shards aren't confirmed for the pending key: {} (set it there, then --confirm <shard>, or --acknowledge-unconfirmed)",
                unconfirmed.join(", ")
            )));
        }
        let backed_up = s
            .store
            .key_state()
            .await?
            .last_backup_at
            .is_some_and(|b| b > p.created_at);
        if !backed_up && !input.acknowledge_no_backup {
            return Err(XrpcError::invalid_request(
                "No key backup holds the pending key yet (or give --acknowledge-no-backup)",
            ));
        }
        tx.promote_pending_plc_key().await?;
        tx.commit().await?;
        admin_auth::audit(
            &s,
            &who,
            "keys.plcRotationKey.current",
            Some(&p.did_key),
            Some(json!({"withoutBackup": input.acknowledge_no_backup, "unconfirmed": input.acknowledge_unconfirmed})),
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
    let pair =
        match (&input.private_key_hex, &input.kms_key_id) {
            (Some(h), _) => {
                let bytes = Zeroizing::new(
                    hex::decode(h.trim())
                        .map_err(|_| XrpcError::invalid_request("The private key isn't hex"))?,
                );
                Some(K256Keypair::from_private_key(&bytes).map_err(|_| {
                    XrpcError::invalid_request("That isn't a secp256k1 private key")
                })?)
            }
            (None, Some(_)) => None,
            (None, None) => Some(K256Keypair::generate()),
        };
    let did_key = match (&pair, &input.kms_key_id) {
        (Some(p), _) => p.did(),
        (None, Some(id)) => crate::state::kms_did_key(id)
            .await
            .map_err(|e| XrpcError::invalid_request(e.to_string()))?,
        (None, None) => unreachable!("a key is generated"),
    };
    if let Some(p) = pending.filter(|p| p.did_key != did_key) {
        listing_guard(&s, &p.did_key, p.created_at, true).await?;
    }
    let private = pair
        .as_ref()
        .map(|p| Zeroizing::new(p.export_private_key()));
    let source = match (&private, &input.kms_key_id) {
        (Some(p), _) => locus_store::keys::PlcKeySource::Private(p),
        (None, Some(id)) => locus_store::keys::PlcKeySource::Kms(id),
        (None, None) => unreachable!("one of the actions is set"),
    };
    let mut tx = s.store.begin().await?;
    let replaced = tx
        .store_pending_plc_key(&did_key, source, &s.master_key)
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

/// The fingerprint of a secret value, for the audit log (never the value).
fn fingerprint(value: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(&sha2::Sha256::digest(value)[..8])
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSecretInput {
    /// `jwt-secret`, `dpop-secret`, `backup-passphrase`, or in the shard
    /// role `entryway-verify-key`, `entryway-plc-rotation-key` and
    /// `entryway-admin-token`.
    name: String,
    value: String,
    /// The entryway keys: stop accepting the previous ones now (otherwise
    /// a previous verify key stays accepted 2 h, and previous PLC rotation
    /// keys until a later `--revoke-previous`).
    #[serde(default)]
    revoke_previous: bool,
    #[serde(flatten)]
    reauth: Reauth,
}

/// `owner.keys.setSecret`: replaces a secret a changed env value can't
/// (the JWT and DPoP secrets), applied on every node within seconds.
pub async fn set_secret(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<SetSecretInput>,
) -> XrpcResult<Json<Value>> {
    use crate::state::secret_names;
    let who = owner(&s, &headers, locus_nsid!("owner.keys.setSecret")).await?;
    admin_auth::reauth(&s, &who, &input.reauth).await?;
    let value = Zeroizing::new(input.value.trim().to_owned());
    if input.name.starts_with("entryway-") {
        return set_entryway_value(&s, &who, &input, &value).await;
    }
    let (slot, bytes): (&str, Zeroizing<Vec<u8>>) = match input.name.as_str() {
        "jwt-secret" => {
            if value.len() < 32 {
                return Err(XrpcError::invalid_request(
                    "A JWT secret is at least 32 characters",
                ));
            }
            (secret_names::JWT, Zeroizing::new(value.as_bytes().to_vec()))
        }
        "dpop-secret" => {
            let b = hex::decode(value.as_str())
                .ok()
                .filter(|b| b.len() == 32)
                .ok_or_else(|| {
                    XrpcError::invalid_request("A DPoP secret is 32 bytes, as 64 hex characters")
                })?;
            (secret_names::DPOP_SECRET, Zeroizing::new(b))
        }
        "backup-passphrase" => {
            if value.chars().count() < locus_core::key_backup::MIN_PASSPHRASE {
                return Err(XrpcError::invalid_request(
                    "A backup passphrase is at least 12 characters",
                ));
            }
            (
                crate::backups::PASSPHRASE_SECRET,
                Zeroizing::new(value.as_bytes().to_vec()),
            )
        }
        other => {
            return Err(XrpcError::invalid_request(format!(
                "{other} isn't a secret this sets (jwt-secret, dpop-secret, backup-passphrase)"
            )));
        }
    };
    let mut tx = s.store.begin().await?;
    tx.put_secret(slot, &bytes, &s.master_key).await?;
    tx.bump_key_generation().await?;
    tx.commit().await?;
    admin_auth::audit(
        &s,
        &who,
        "keys.setSecret",
        Some(&input.name),
        Some(json!({"fingerprint": fingerprint(&bytes)})),
    )
    .await;
    reload_soon(&s);
    Ok(Json(
        json!({"name": input.name, "fingerprint": fingerprint(&bytes)}),
    ))
}

/// The shard role's entryway values (`keys set entryway-*`). The keys are
/// public: the verify key goes to `shard_verify_key`, the PLC rotation key
/// to its setting; the admin token is a secret.
async fn set_entryway_value(
    s: &AppState,
    who: &admin_auth::Caller,
    input: &SetSecretInput,
    value: &str,
) -> XrpcResult<Json<Value>> {
    let Some(ew) = &s.entryway else {
        return Err(XrpcError::invalid_request(
            "The entryway values are set only in the shard role",
        ));
    };
    let bad = |e: String| XrpcError::invalid_request(e);
    let mut tx = s.store.begin().await?;
    let (fingerprint_of, shown): (Vec<u8>, Value) = match input.name.as_str() {
        "entryway-verify-key" => {
            let k = crate::shard::parse_k256_key(value).map_err(bad)?;
            let keep = (!input.revoke_previous).then_some(crate::shard::VERIFY_KEY_OVERLAP_MS);
            tx.set_shard_verify_key(&k, keep).await?;
            (k.as_bytes().to_vec(), json!(k))
        }
        "entryway-plc-rotation-key" => {
            let k = crate::shard::parse_k256_key(value).map_err(bad)?;
            let mut keys = vec![k.clone()];
            if !input.revoke_previous {
                keys.extend(ew.plc_rotation_keys.iter().filter(|x| **x != k).cloned());
            }
            tx.set_setting("entryway.plc_rotation_keys", json!(keys), &who.actor)
                .await?;
            (k.as_bytes().to_vec(), json!(keys))
        }
        "entryway-admin-token" => {
            if value.is_empty() {
                return Err(XrpcError::invalid_request("The admin token can't be empty"));
            }
            tx.put_secret(
                crate::shard::ADMIN_TOKEN_SECRET,
                value.as_bytes(),
                &s.master_key,
            )
            .await?;
            (value.as_bytes().to_vec(), Value::Null)
        }
        other => {
            return Err(XrpcError::invalid_request(format!(
                "{other} isn't an entryway value (entryway-verify-key, entryway-plc-rotation-key, entryway-admin-token)"
            )));
        }
    };
    tx.bump_key_generation().await?;
    tx.commit().await?;
    let fp = fingerprint(&fingerprint_of);
    admin_auth::audit(
        s,
        who,
        "keys.setSecret",
        Some(&input.name),
        Some(json!({"fingerprint": fp, "revokePrevious": input.revoke_previous})),
    )
    .await;
    reload_soon(s);
    Ok(Json(
        json!({"name": input.name, "fingerprint": fp, "accepted": shown}),
    ))
}

#[derive(Deserialize, Default)]
pub struct ReauthOnly {
    #[serde(flatten)]
    reauth: Reauth,
}

/// `owner.keys.rotateServiceKey`: a new key for the service's own DID
/// document and the tokens it signs.
pub async fn rotate_service_key(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<ReauthOnly>,
) -> XrpcResult<Json<Value>> {
    let who = owner(&s, &headers, locus_nsid!("owner.keys.rotateServiceKey")).await?;
    admin_auth::reauth(&s, &who, &input.reauth).await?;
    let pair = K256Keypair::generate();
    let private = Zeroizing::new(pair.export_private_key());
    let mut tx = s.store.begin().await?;
    tx.put_secret(
        crate::state::secret_names::SERVICE_KEY,
        &private,
        &s.master_key,
    )
    .await?;
    tx.bump_key_generation().await?;
    tx.commit().await?;
    admin_auth::audit(
        &s,
        &who,
        "keys.rotateServiceKey",
        Some(&pair.did()),
        Some(json!({"previous": s.secrets.service_key.did()})),
    )
    .await;
    reload_soon(&s);
    Ok(Json(json!({"didKey": pair.did()})))
}

#[derive(Deserialize)]
pub struct DidsInput {
    #[serde(default)]
    dids: Vec<String>,
    /// Every account `sequencer recover` recovered (the reference's
    /// `rotate-keys-recovery`).
    #[serde(default)]
    recovery: bool,
}

/// `admin.keys.rotateAccountKeys`: the reference's `rotate-keys`. For
/// each account, points its PLC document's signing key at the one stored
/// here if it lists another, then announces `#identity` and a fresh
/// commit's `#sync`.
pub async fn rotate_account_keys(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DidsInput>,
) -> XrpcResult<Json<Value>> {
    let who = admin_auth::require(
        &s,
        &headers,
        locus_nsid!("admin.keys.rotateAccountKeys"),
        Surface::Locus,
        Role::Admin,
    )
    .await?;
    let mut dids = input.dids.clone();
    if input.recovery {
        dids.extend(crate::repair::recovered_dids(&s).await?);
    }
    let mut results = Vec::new();
    for did in &dids {
        let outcome = rotate_one(&s, did).await;
        results.push(match &outcome {
            Ok(changed) => json!({"did": did, "ok": true, "plcUpdated": changed}),
            Err(m) => json!({"did": did, "ok": false, "error": m}),
        });
    }
    admin_auth::audit(
        &s,
        &who,
        "keys.rotateAccountKeys",
        None,
        Some(json!({"dids": dids.len(), "recovery": input.recovery})),
    )
    .await;
    Ok(Json(json!({"results": results})))
}

async fn rotate_one(s: &AppState, did: &str) -> Result<bool, String> {
    let account = s
        .store
        .account_by_did(did)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no such account")?;
    if account.did_method != "plc" {
        return Err("not a did:plc account".into());
    }
    let key = s.signing_key(did).await.map_err(|e| e.message)?;
    let changed = crate::identity::plc_set_signing_key(s, did, &key.did())
        .await
        .map_err(|e| e.message())?;
    crate::identity::emit_identity(s, did)
        .await
        .map_err(|e| e.message)?;
    s.repo
        .resign_with_sync(did, &key)
        .await
        .map_err(|e| format!("couldn't write the new commit: {e}"))?;
    Ok(changed)
}

/// `admin.identity.publish`: the reference's `publish-identity`, an
/// `#identity` for each DID.
pub async fn publish_identity(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DidsInput>,
) -> XrpcResult<Json<Value>> {
    let who = admin_auth::require(
        &s,
        &headers,
        locus_nsid!("admin.identity.publish"),
        Surface::Locus,
        Role::Admin,
    )
    .await?;
    let mut results = Vec::new();
    for did in &input.dids {
        let hosted = s.store.account_by_did(did).await?.is_some();
        let r = if hosted {
            crate::identity::emit_identity(&s, did)
                .await
                .map_err(|e| e.message)
        } else {
            Err("no such account".into())
        };
        results.push(match r {
            Ok(()) => json!({"did": did, "ok": true}),
            Err(m) => json!({"did": did, "ok": false, "error": m}),
        });
    }
    admin_auth::audit(
        &s,
        &who,
        "identity.publish",
        None,
        Some(json!({"dids": input.dids.len()})),
    )
    .await;
    Ok(Json(json!({"results": results})))
}

/// The path key imports are posted to (it takes a large body).
pub const IMPORT_PATH: &str = concat!("/xrpc/", locus_nsid!("owner.keys.import"));
/// The largest key backup an import takes.
pub const MAX_IMPORT_BODY: i64 = 2 * 1024 * 1024 * 1024;

#[derive(Deserialize)]
struct ImportHead {
    passphrase: String,
    #[serde(flatten)]
    reauth: Reauth,
}

/// `owner.keys.import`: reads a key backup (the body is a JSON line with
/// the passphrase and re-auth, then the backup file). Adds the PLC rotation
/// keys this PDS lacks as secondary signers, and replaces an account's
/// signing key with the backup's where the account's live DID document
/// lists the backup's key and not the stored one (a fresh commit and
/// `#sync` follow for active accounts). Nothing else is changed. Reports
/// what it did, and every account whose document lists neither key.
pub async fn import(State(s): St, headers: HeaderMap, body: Body) -> XrpcResult<Json<Value>> {
    use locus_core::key_backup::Entry;
    let who = owner(&s, &headers, locus_nsid!("owner.keys.import")).await?;
    let bytes = axum::body::to_bytes(body, usize::try_from(MAX_IMPORT_BODY).unwrap_or(usize::MAX))
        .await
        .map_err(|_| XrpcError::invalid_request("The key backup couldn't be read"))?;
    let nl = bytes
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(|| XrpcError::invalid_request("Send the passphrase line, then the backup"))?;
    let head: ImportHead = serde_json::from_slice(&bytes[..nl])
        .map_err(|_| XrpcError::invalid_request("Send the passphrase line, then the backup"))?;
    admin_auth::reauth(&s, &who, &head.reauth).await?;
    let passphrase = Zeroizing::new(head.passphrase);
    let (header, entries) =
        locus_core::key_backup::open(&bytes[nl + 1..], &passphrase).map_err(XrpcError::from)?;
    drop(bytes);

    let mut secrets = std::collections::HashMap::new();
    let mut plc_keys = Vec::new();
    let mut signing = Vec::new();
    for e in entries {
        match e {
            Entry::Secret { name, value } => {
                secrets.insert(name, value);
            }
            Entry::PlcKey {
                did_key,
                kms_key_id,
                ..
            } => plc_keys.push((did_key, kms_key_id)),
            Entry::SigningKey {
                did,
                key_type,
                value,
            } => signing.push((did, key_type, value)),
        }
    }

    let mut added = Vec::new();
    for (did_key, kms_key_id) in &plc_keys {
        let private = secrets.get(&locus_store::identity::plc_key_secret_name(did_key));
        let source = match (private, kms_key_id) {
            (Some(p), _) => match K256Keypair::from_private_key(p) {
                Ok(pair) if pair.did() == *did_key => locus_store::keys::PlcKeySource::Private(p),
                _ => continue,
            },
            (None, Some(id)) => locus_store::keys::PlcKeySource::Kms(id),
            (None, None) => continue,
        };
        let mut tx = s.store.begin().await?;
        let new = tx
            .insert_secondary_plc_key(did_key, source, &s.master_key)
            .await?;
        tx.commit().await?;
        if new {
            added.push(did_key.clone());
        }
    }

    let (mut replaced, mut kept, mut neither, mut skipped) = (Vec::new(), 0, Vec::new(), 0);
    for (did, key_type, private) in signing {
        let account = match s.store.account_by_did(&did).await? {
            Some(a) if a.did_method == "plc" && key_type == "k256" => a,
            // Gone, did:web (its restored document is authoritative), or
            // a key type locus doesn't use.
            _ => {
                skipped += 1;
                continue;
            }
        };
        let Ok(backup_key) = K256Keypair::from_private_key(&private) else {
            skipped += 1;
            continue;
        };
        let stored = s.signing_key(&did).await.ok().map(|k| k.did());
        if stored.as_deref() == Some(backup_key.did().as_str()) {
            kept += 1;
            continue;
        }
        let live = match crate::identity::plc_signing_key(&s, &did).await {
            Ok(k) => k,
            Err(e) => {
                neither.push(json!({"did": did, "error": e.message()}));
                continue;
            }
        };
        if live.as_deref() == Some(backup_key.did().as_str()) {
            let mut tx = s.store.begin().await?;
            tx.put_signing_key(&did, "k256", &private, &s.master_key)
                .await?;
            tx.commit().await?;
            if account.status == locus_store::accounts::Status::Active
                && let Err(e) = s.repo.resign_with_sync(&did, &backup_key).await
            {
                tracing::warn!(did, "couldn't write a commit with the restored key: {e}");
            }
            replaced.push(did);
        } else if live.is_some() && live == stored {
            kept += 1;
        } else {
            neither.push(json!({"did": did}));
        }
    }
    admin_auth::audit(
        &s,
        &who,
        "keys.import",
        None,
        Some(json!({
            "backupCreatedAt": header.created_at,
            "plcKeysAdded": added.len(),
            "signingKeysReplaced": replaced.len(),
        })),
    )
    .await;
    if !added.is_empty() {
        reload_soon(&s);
    }
    Ok(Json(json!({
        "plcKeysAdded": added,
        "signingKeysReplaced": replaced,
        "signingKeysKept": kept,
        "skipped": skipped,
        "listNeitherKey": neither,
        "next": if neither.is_empty() { Value::Null } else {
            json!("Accounts whose DID document lists neither key need `aurora-locus identity publish` or `repair identity`.")
        },
    })))
}
