//! Syncing the audience copy with prism: edits queued during an outage,
//! local audiences created in prism when it's back, the mapping from their
//! local URIs to prism's, and re-pointing posts that name a mapped local
//! URI.

use crate::error::XrpcResult;
use crate::state::AppState;

/// After a refresh linked create attempts to adopted rows: an attempt that
/// created a local audience in prism maps it (the rows are already there).
pub async fn after_links(s: &AppState, owner: &str, linked: &[(String, String)]) -> XrpcResult<()> {
    for (attempt, prism_rkey) in linked {
        let Some(att) = s.store.audience_attempt(attempt).await? else {
            continue;
        };
        if let Some(local) = &att.local_rkey
            && s.store
                .map_local_audience(owner, local, att.local_row_version, prism_rkey, None)
                .await?
        {
            s.kr.audience_sync_wake.notify_one();
        }
    }
    Ok(())
}

use std::sync::Arc;
use std::time::{Duration, Instant};

use locus_repo::Action;
use locus_repo::records;
use locus_store::kryphocron::AudienceRow;
use proto_blue_lex_data::LexValue;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::audience::{self, Audience, Prism, Refreshed, Trigger};

/// Writes to prism for one account go at most this often (prism's
/// `private` class is 10/s per DID; the account's own client keeps its
/// share).
const WRITE_GAP: Duration = Duration::from_millis(500);

/// What to do with a queued edit, by `updatedAt` (last writer wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    Push,
    TakePrisms { flag: bool },
}

fn sync_rule(row: &AudienceRow, prism_cid: &Option<String>, prism: &Audience) -> Rule {
    if *prism_cid == row.prism_cid {
        // Nothing changed in prism since the edit was queued.
        return Rule::Push;
    }
    let at = locus_core::time::parse_rfc3339_ms;
    match (at(&row.updated_at), at(&prism.updated_at)) {
        (Some(local), Some(theirs)) if local > theirs => Rule::Push,
        (Some(local), Some(theirs)) if local == theirs => Rule::TakePrisms { flag: false },
        _ => Rule::TakePrisms { flag: true },
    }
}

/// Settles an account's queued edits and creates its local audiences in
/// prism, after a complete listing. Returns how many prism writes it made.
pub async fn sync_account(s: &Arc<AppState>, owner: &str, listing: &Refreshed) -> XrpcResult<u32> {
    let Some((_, prism_did)) = crate::pipethrough::appview(s) else {
        return Ok(0);
    };
    let mut writes = 0;

    // Queued edits and deletes of prism audiences. (One missing from the
    // complete listing was deleted in prism; the refresh removed it.)
    for row in s.store.pending_audiences(owner).await? {
        if s.kr.prism.is_down() {
            return Ok(writes);
        }
        let u = audience::uri(owner, &row.rkey);
        if row.pending == "delete" {
            // Deletes win, whatever the timestamps.
            let a = audience::call(
                s,
                owner,
                "app.nearhorizon.private.deleteAudience",
                &[],
                Some(json!({"uri": u})),
            )
            .await;
            writes += 1;
            match &a {
                Prism::Answer { status: 200, .. } => {
                    s.store
                        .delete_audience(owner, &row.rkey, Some(row.row_version))
                        .await?;
                }
                _ if stop_for(s, owner, &a).await? => return Ok(writes),
                _ => {}
            }
            tokio::time::sleep(WRITE_GAP).await;
            continue;
        }
        let Some((prism_cid, prism)) = listing.listed.get(&row.rkey) else {
            continue;
        };
        match sync_rule(&row, prism_cid, prism) {
            Rule::TakePrisms { flag } => {
                let theirs = prism.row(owner, &row.rkey, "prism", &prism_did, prism_cid.clone())?;
                if s.store
                    .replace_audience_if(&theirs, row.row_version)
                    .await?
                    && flag
                {
                    s.store
                        .flag_audience(owner, &row.rkey, "dropped: the AppView had a newer version")
                        .await?;
                }
            }
            Rule::Push => {
                let mine = Audience::of_row(&row);
                let a = audience::call(
                    s,
                    owner,
                    "app.nearhorizon.private.updateAudience",
                    &[],
                    Some(json!({"uri": u, "record": mine.to_record(), "swapCid": prism_cid})),
                )
                .await;
                writes += 1;
                match &a {
                    Prism::Answer {
                        status: 200, body, ..
                    } => {
                        let done = mine.row(
                            owner,
                            &row.rkey,
                            "prism",
                            &prism_did,
                            body["cid"].as_str().map(str::to_owned),
                        )?;
                        s.store.write_audience_value(&done, row.row_version).await?;
                    }
                    // A CAS conflict re-runs the comparison next pass.
                    a if a.error_name() == Some("InvalidSwap") => {}
                    a if matches!(a.error_name(), Some("InvalidRequest" | "LimitReached")) => {
                        // A permanent refusal: prism's version stays, flagged.
                        let theirs =
                            prism.row(owner, &row.rkey, "prism", &prism_did, prism_cid.clone())?;
                        s.store
                            .replace_audience_if(&theirs, row.row_version)
                            .await?;
                        s.store
                            .flag_audience(owner, &row.rkey, "refused by the AppView")
                            .await?;
                    }
                    a if stop_for(s, owner, a).await? => return Ok(writes),
                    _ => {}
                }
                tokio::time::sleep(WRITE_GAP).await;
            }
        }
    }

    // Local audiences, created in prism oldest first, while the owner is
    // active.
    let active = s
        .store
        .account_by_did(owner)
        .await?
        .is_some_and(|a| a.status == locus_store::accounts::Status::Active);
    if !active {
        return Ok(writes);
    }
    let cap =
        usize::try_from(s.snapshot().int("kryphocron.audience_max_per_account")).unwrap_or(200);
    let mut in_prism = listing.listed.len();
    if in_prism < cap {
        s.store.clear_limit_flags(owner).await?;
    }
    for row in s.store.local_audiences(owner).await? {
        if s.kr.prism.is_down() {
            break;
        }
        if let Some(att) = s.store.latest_local_attempt(owner, &row.rkey).await? {
            let open = att.outcome == "sending" || att.outcome == "unknown";
            // An expired attempt is retried only after a complete listing
            // that started after it expired lacks a match.
            let unsettled = att.outcome == "expired" && att.expires_at >= listing.started_at;
            if open || unsettled {
                continue;
            }
        }
        if in_prism >= cap {
            s.store.flag_audience(owner, &row.rkey, "limit").await?;
            continue;
        }
        let aud = Audience::of_row(&row);
        let value = aud.to_record();
        let id = hex::encode(rand::random::<[u8; 16]>());
        s.store
            .create_audience_attempt(
                &id,
                owner,
                &value.to_string(),
                &aud.created_at,
                &aud.updated_at,
                Some((&row.rkey, row.row_version)),
            )
            .await?;
        let a = audience::call(
            s,
            owner,
            "app.nearhorizon.private.createAudience",
            &[],
            Some(json!({"record": value})),
        )
        .await;
        writes += 1;
        match &a {
            Prism::Answer {
                status: 200, body, ..
            } => {
                let Some(prism_rkey) = body["uri"]
                    .as_str()
                    .and_then(|u| audience::rkey_of(u, owner))
                else {
                    s.store.delete_attempt(&id).await?;
                    continue;
                };
                crate::faults::at(s, "audience.created").await?;
                let theirs = aud.row(
                    owner,
                    prism_rkey,
                    "prism",
                    &prism_did,
                    body["cid"].as_str().map(str::to_owned),
                )?;
                s.store
                    .map_local_audience(
                        owner,
                        &row.rkey,
                        Some(row.row_version),
                        prism_rkey,
                        Some(&theirs),
                    )
                    .await?;
                s.store.set_attempt_outcome(&id, "adopted").await?;
                in_prism += 1;
            }
            Prism::Timeout => {
                s.store.set_attempt_outcome(&id, "unknown").await?;
                let far = Instant::now() + Duration::from_secs(60);
                let _ = audience::refresh(s, owner, Trigger::Bypass, None, far).await;
            }
            a if a.error_name() == Some("LimitReached") => {
                s.store.delete_attempt(&id).await?;
                s.store.flag_audience(owner, &row.rkey, "limit").await?;
            }
            a if a.error_name() == Some("InvalidRequest") => {
                s.store.delete_attempt(&id).await?;
                s.store.flag_audience(owner, &row.rkey, "refused").await?;
            }
            a => {
                s.store.delete_attempt(&id).await?;
                if stop_for(s, owner, a).await? {
                    return Ok(writes);
                }
            }
        }
        tokio::time::sleep(WRITE_GAP).await;
    }
    Ok(writes)
}

/// `NotEnrolled` and `AccountTakedown` end the account's sync (after their
/// effects on the copy); anything else (NotReady, Reindexing, 429, 5xx,
/// unreachable) leaves the work for a later pass.
async fn stop_for(s: &AppState, owner: &str, a: &Prism) -> XrpcResult<bool> {
    let stop = matches!(a.error_name(), Some("NotEnrolled" | "AccountTakedown"));
    if stop {
        audience::apply_error_answer(s, owner, a).await?;
    }
    Ok(stop)
}

/// The leased audience sync job: polls each account's copy in the
/// background (refreshing, then syncing), on an interval stretched to stay
/// within the call budget, and soonest for accounts with work queued during
/// an outage.
pub async fn run(s: Arc<AppState>, cancel: CancellationToken) -> locus_core::CoreResult<()> {
    loop {
        if let Err(e) = poll_once(&s, &cancel).await {
            tracing::warn!("audience sync pass failed: {}", e.message);
        }
        let _ = s.store.record_job_run("kr-audience-sync", None, None).await;
        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            () = s.kr.audience_sync_wake.notified() => {}
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
}

/// One pass over the accounts due a poll.
pub async fn poll_once(s: &Arc<AppState>, cancel: &CancellationToken) -> XrpcResult<u32> {
    if crate::pipethrough::appview(s).is_none() || s.kr.prism.is_down() {
        return Ok(0);
    }
    let snap = s.snapshot();
    let budget = snap.int("kryphocron.prism_call_budget").max(1);
    let floor = snap.int("kryphocron.audience_poll_interval_s");
    let accounts = s.store.count_audience_syncs().await?;
    // Every account once per interval, within the budget.
    let interval_ms = floor.max(accounts / budget) * 1000;
    let mut polled = 0;
    for did in s
        .store
        .audience_polls_due(s.store.db_now().await?, 50)
        .await?
    {
        if cancel.is_cancelled() || s.kr.prism.is_down() {
            break;
        }
        let deadline = Instant::now() + Duration::from_secs(300);
        let r = audience::refresh(s, &did, Trigger::Background, None, deadline).await?;
        let mut calls = r.pages;
        if r.complete && r.error.is_none() {
            calls += sync_account(s, &did, &r).await?;
        }
        s.store
            .set_next_poll(&did, s.store.db_now().await? + interval_ms)
            .await?;
        polled += 1;
        let pace =
            Duration::from_millis(u64::from(calls) * 1000 / u64::try_from(budget).unwrap_or(1));
        tokio::time::sleep(pace).await;
    }
    // Tombstones outlive every listing that could still be running.
    let keep = (snap.int("kryphocron.audience_listing_deadline_s") * 1000).max(600_000);
    s.store
        .prune_audience_tombstones(s.store.db_now().await? - keep)
        .await?;
    Ok(polled)
}

// ---------------------------------------------------------------- re-pointing

/// Most ops and record bytes in one re-point commit (§6.5's limits, with
/// headroom).
const REPOINT_OPS: usize = 200;
const REPOINT_BYTES: usize = 1_500_000;

/// One pass of the re-point job: every private post in a repo here that
/// names a mapped local audience URI is rewritten to name the prism URI.
/// Only `audienceList` changes (the content's seed excludes the audience,
/// so it stays decodable); one CAS-guarded commit per repo batch, so a
/// record changed meanwhile is skipped and found again next pass.
pub async fn repoint_pass(
    s: &Arc<AppState>,
    cancel: &CancellationToken,
) -> locus_core::CoreResult<u64> {
    let per_minute = s
        .snapshot()
        .int("kryphocron.repoint_max_commits_per_minute")
        .max(1);
    let pause = Duration::from_millis(u64::try_from(60_000 / per_minute).unwrap_or(500));
    let rows = s.store.repoint_candidates(5_000).await?;
    let mut by_repo: std::collections::BTreeMap<String, Vec<_>> = std::collections::BTreeMap::new();
    for r in rows {
        by_repo.entry(r.did.clone()).or_default().push(r);
    }
    let mut total = 0;
    for (did, recs) in by_repo {
        if cancel.is_cancelled() {
            break;
        }
        let active = s
            .store
            .account_by_did(&did)
            .await?
            .is_some_and(|a| a.status == locus_store::accounts::Status::Active);
        if !active {
            continue;
        }
        for chunk in recs.chunks(REPOINT_OPS) {
            let mut writes = Vec::new();
            let mut bytes = 0;
            for r in chunk {
                let Some((cid, _)) = s.store.record_entry(&did, &r.collection, &r.rkey).await?
                else {
                    continue;
                };
                let Some(block) = s.store.block(&did, &cid).await? else {
                    continue;
                };
                let Ok(LexValue::Map(mut map)) = proto_blue_lex_cbor::decode(&block) else {
                    continue;
                };
                map.insert(
                    "audienceList".into(),
                    LexValue::String(audience::uri(&r.audience_did, &r.prism_rkey)),
                );
                let Ok(rec) = records::prepare_record(
                    &r.collection,
                    &r.rkey,
                    LexValue::Map(map),
                    None,
                    usize::MAX,
                ) else {
                    continue;
                };
                bytes += rec.bytes.len();
                if !writes.is_empty() && bytes > REPOINT_BYTES {
                    break;
                }
                let read = proto_blue_lex_data::Cid::from_str_multibase(&cid).map_err(|_| {
                    locus_core::CoreError::Storage("a stored CID doesn't parse".into())
                })?;
                writes.push(crate::repo::prepared(
                    Action::Update,
                    &r.collection,
                    &r.rkey,
                    Some(&rec),
                    Some(Some(read)),
                ));
            }
            if writes.is_empty() {
                continue;
            }
            let n = writes.len() as u64;
            let key = s
                .signing_key(&did)
                .await
                .map_err(|e| locus_core::CoreError::Storage(e.message))?;
            crate::faults::at(s, "repoint.commit").await?;
            match s
                .repo
                .commit(locus_repo::CommitRequest {
                    did: &did,
                    writes,
                    swap_commit: None,
                    signer: &key,
                    dedupe_backlinks: false,
                })
                .await
            {
                Ok(o) => {
                    crate::repo::finish_moves(s, &o.moves).await;
                    total += n;
                }
                Err(e) => {
                    tracing::info!(did, "re-point commit deferred: {e}");
                    break;
                }
            }
            tokio::time::sleep(pause).await;
        }
    }
    if total > 0 {
        s.kr.audit
            .record_locus("audience.repoint", format!("records={total}"));
    }
    Ok(total)
}
