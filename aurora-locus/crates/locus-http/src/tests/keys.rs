//! PLC rotation keys over time: the drop guard, what signs after a key
//! change, and key imports.

use super::*;
use proto_blue_crypto::{ExportableKeypair as _, K256Keypair};

fn ns(m: &str) -> String {
    format!("/xrpc/{}.{m}", locus_core::LOCUS_NS)
}

fn pool(h: &Harness) -> sqlx::SqlitePool {
    match &h.state.store {
        Store::Sqlite(p) => p.clone(),
        Store::Postgres(_) => unreachable!("the harness is on SQLite"),
    }
}

/// The node after a restart on the same database: keys reloaded.
async fn reload(h: Harness) -> Harness {
    let Harness {
        state, plc, _dir, ..
    } = h;
    let store = state.store.clone();
    let master_key = Arc::clone(&state.master_key);
    let secrets = ServerSecrets::load_or_create(&store, &master_key)
        .await
        .unwrap();
    let settings = Settings::load(store.clone()).await.unwrap();
    let sessions = LegacySessions::new(
        store.clone(),
        secrets.jwt.clone(),
        state.service_did.clone(),
    );
    let (mailer, mail) = locus_mail::Mailer::capturing(store.clone(), Arc::clone(&master_key));
    let (kr, audit_rx) = crate::kryphocron::Kr::new(secrets.correlation_key);
    tokio::spawn(crate::kryphocron::run_audit_writer(
        store.clone(),
        audit_rx,
        tokio_util::sync::CancellationToken::new(),
    ));
    let blob_epoch = store.blob_backend().await.unwrap().epoch;
    let state = Arc::new(AppState {
        repo: RepoEngine::new(store.clone(), Arc::new(TidClock::new(0))),
        firehose: locus_sync::Firehose::new(store.clone(), 500),
        store,
        settings,
        master_key,
        secrets,
        sessions,
        mailer,
        blobs: test_blob_store(_dir.path()).with_epoch(blob_epoch),
        scratch: locus_blob::DiskStore::new(_dir.path()),
        public_url: state.public_url.clone(),
        hostname: state.hostname.clone(),
        service_did: state.service_did.clone(),
        node_id: "test-node".into(),
        ssrf_protection: true,
        resolvers: crate::state::ResolverCache::default(),
        limits: crate::ratelimit::Limiter::default(),
        kv: crate::scratch::Scratch::default(),
        oauth_clients: crate::oauth::ClientCache::default(),
        kr,
        setup: Arc::new(crate::setup::Gate::finished()),
        logs: Arc::new(locus_core::logs::LogRing::new(100)),
        branding: crate::branding::Cache::default(),
        socket_secret: crate::socket::new_secret(),
        reload: tokio_util::sync::CancellationToken::new(),
        hosts: arc_swap::ArcSwap::from_pointee(crate::hostnames::Hosts::default()),
        role: state.role,
        dev_mode: false,
        entryway: state.entryway.clone(),
    });
    crate::kryphocron::refresh_oracle(&state).await.unwrap();
    Harness {
        app: crate::router(state.clone()),
        state,
        plc,
        mail,
        _dir,
    }
}

/// Pushes an op setting `did`'s rotation keys straight into the test PLC
/// (a change locus didn't make or see).
async fn set_rotation_keys(h: &Harness, did: &str, keys: &[&str]) {
    let last = h.plc.lock().unwrap().last(did).unwrap();
    let prev = SignedOp::from_json(&last).unwrap();
    let SignedOp::Operation { op, .. } = &prev else {
        panic!("{last}")
    };
    let mut op = op.clone();
    op.rotation_keys = keys.iter().map(|k| (*k).to_owned()).collect();
    op.prev = Some(prev.cid().unwrap().to_string_base32());
    let signed = crate::state::PlcSigner::Local(K256Keypair::generate())
        .sign_op(&op)
        .await
        .unwrap();
    h.plc
        .lock()
        .unwrap()
        .log
        .get_mut(did)
        .unwrap()
        .push(signed.to_json());
}

/// Which of `keys` signed `did`'s latest PLC op.
fn last_op_signer(h: &Harness, did: &str, keys: &[&str]) -> Option<String> {
    let last = h.plc.lock().unwrap().last(did).unwrap();
    let op = SignedOp::from_json(&last).unwrap();
    keys.iter().find(|k| op.verify(k)).map(|k| (*k).to_owned())
}

async fn held(h: &Harness) -> Vec<(String, String)> {
    rows(h)
        .await
        .into_iter()
        .map(|r| (r.did_key, r.role))
        .collect()
}

async fn rows(h: &Harness) -> Vec<locus_store::keys::PlcKeyRow> {
    h.state.store.plc_key_rows().await.unwrap()
}

impl Harness {
    async fn owner_call(&self, owner: &str, method: &str, body: Value) -> (StatusCode, Value) {
        self.admin_call(
            "POST",
            &ns(method),
            owner,
            Some(merge(body, &json!({"password": "olive-password-12"}))),
        )
        .await
    }

    async fn plc_secret(&self, did_key: &str) -> Option<Vec<u8>> {
        self.state
            .store
            .get_secret(
                &format!("plc_rotation_key/{did_key}"),
                &self.state.master_key,
            )
            .await
            .unwrap()
            .map(|s| s.to_vec())
    }
}

#[tokio::test]
async fn the_drop_guard_refetches_what_it_can_t_trust_and_refuses_on_doubt() {
    let h = harness(false).await;
    let (a, _) = h.user("ann.pds.example.com").await;
    let (b, _) = h.user("ben.pds.example.com").await;
    let (c, _) = h.user("cal.pds.example.com").await;
    let owner = h.owner_with_password("olive", "olive-password-12").await;
    let k1 = h.state.secrets.plc_keys.current_did().unwrap();

    // K2 becomes current; K1 secondary. Every node reloads its keys.
    let (st, v) = h
        .owner_call(
            &owner,
            "owner.keys.setPlcRotationKey",
            json!({"generate": true}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let k2 = v["didKey"].as_str().unwrap().to_owned();
    let (st, v) = h
        .owner_call(
            &owner,
            "owner.keys.setPlcRotationKey",
            json!({"acknowledgeNoBackup": true}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let h = reload(h).await;
    let since = rows(&h)
        .await
        .into_iter()
        .find(|r| r.did_key == k1)
        .unwrap()
        .secondary_since
        .unwrap();

    // C moved to both keys; its listing says so.
    set_rotation_keys(&h, &c, &[&k2, &k1]).await;
    h.state
        .store
        .set_plc_key_listing(&c, &[k2.clone(), k1.clone()])
        .await
        .unwrap();
    // A's and B's documents list only K1, through ops locus never saw. A
    // has no listing row; B's says K2 but was checked before K1's demotion.
    let db = pool(&h);
    for did in [&a, &b] {
        set_rotation_keys(&h, did, &[&k1]).await;
    }
    sqlx::query("DELETE FROM plc_key_listing WHERE did = ?1")
        .bind(&a)
        .execute(&db)
        .await
        .unwrap();
    h.state
        .store
        .set_plc_key_listing(&b, std::slice::from_ref(&k2))
        .await
        .unwrap();
    sqlx::query("UPDATE plc_key_listing SET checked_at = ?2 WHERE did = ?1")
        .bind(&b)
        .bind(since - 1000)
        .execute(&db)
        .await
        .unwrap();

    let drop = |key: &str| {
        let body = json!({"didKey": key});
        let (h, owner) = (&h, owner.clone());
        async move { h.owner_call(&owner, "owner.keys.dropPlcKey", body).await }
    };
    // A, with no row, is re-fetched and refuses the drop.
    let (st, v) = drop(&k1).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["message"].as_str().unwrap().contains(&a), "{v}");
    assert_eq!(
        h.state.store.plc_key_listing(&a).await.unwrap(),
        Some(vec![k1.clone()]),
        "the guard records what it saw"
    );
    // Fixed; now B's stale row is re-fetched and refuses it.
    set_rotation_keys(&h, &a, &[&k2, &k1]).await;
    let (st, v) = drop(&k1).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["message"].as_str().unwrap().contains(&b), "{v}");
    set_rotation_keys(&h, &b, &[&k1, &k2]).await;

    // A PLC directory that can't be reached refuses too.
    let plc_url = h.state.snapshot().text("identity.plc_url").to_owned();
    h.state
        .store
        .set_setting("identity.plc_url", json!("http://127.0.0.1:9"), "test")
        .await
        .unwrap();
    h.state.settings.refresh().await.unwrap();
    let (st, v) = drop(&k1).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(
        v["message"].as_str().unwrap().contains("Couldn't check"),
        "{v}"
    );
    assert!(held(&h).await.contains(&(k1.clone(), "secondary".into())));
    h.state
        .store
        .set_setting("identity.plc_url", json!(plc_url), "test")
        .await
        .unwrap();
    h.state.settings.refresh().await.unwrap();

    // Neither the current nor a pending key can be dropped.
    let (st, v) = h
        .owner_call(
            &owner,
            "owner.keys.setPlcRotationKey",
            json!({"generate": true}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let k3 = v["didKey"].as_str().unwrap().to_owned();
    for key in [&k2, &k3] {
        let (st, v) = drop(key).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
        assert!(
            v["message"].as_str().unwrap().contains("Only a secondary"),
            "{v}"
        );
    }

    // Every document lists another held key: dropped, and gone from the
    // key store.
    assert!(h.plc_secret(&k1).await.is_some());
    let (st, v) = drop(&k1).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(
        held(&h).await.iter().all(|(k, _)| *k != k1),
        "{:?}",
        held(&h).await
    );
    assert!(
        h.plc_secret(&k1).await.is_none(),
        "its private half is gone"
    );
    assert!(
        h.state
            .store
            .audit_entries(None, 50)
            .await
            .unwrap()
            .iter()
            .any(|e| e.action == "keys.plcRotationKey.drop"),
        "audited"
    );
}

#[tokio::test]
async fn a_key_import_is_owner_only_dates_new_keys_and_resigns_only_what_changed() {
    let h = harness(false).await;
    let (a, _) = h.user("amy.pds.example.com").await;
    let (b, b_access) = h.user("bea.pds.example.com").await;
    let owner = h.owner_with_password("olive", "olive-password-12").await;
    let admin = format!("Bearer {}", h.operator("adam", "admin").await);
    // A held PLC key and A's rotated signing key are in the backup; the
    // install then loses both (as a restore from an older snapshot does).
    let extra = K256Keypair::generate();
    let mut tx = h.state.store.begin().await.unwrap();
    tx.insert_secondary_plc_key(
        &extra.did(),
        locus_store::keys::PlcKeySource::Private(&extra.export_private_key()),
        &h.state.master_key,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let k1 = h.state.signing_key(&a).await.unwrap();
    let k2 = h.swap_signing_key(&a).await;
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.keys.rotateAccountKeys"),
            &owner,
            Some(json!({"dids": [a]})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, backup) = h.backup_bytes(&owner, "import passphrase").await;
    assert_eq!(st, StatusCode::OK);
    let mut tx = h.state.store.begin().await.unwrap();
    tx.put_signing_key(&a, "k256", &k1.export_private_key(), &h.state.master_key)
        .await
        .unwrap();
    tx.remove_plc_key(&extra.did()).await.unwrap();
    tx.commit().await.unwrap();

    let import = |auth: String, head: Value| {
        let mut bytes = head.to_string().into_bytes();
        bytes.push(b'\n');
        bytes.extend_from_slice(&backup);
        let h = &h;
        async move {
            let (st, _, out) = h
                .raw(
                    "POST",
                    &ns("owner.keys.import"),
                    None,
                    bytes,
                    &[
                        ("authorization", &auth),
                        ("content-type", "application/octet-stream"),
                    ],
                )
                .await;
            (
                st,
                serde_json::from_slice::<Value>(&out).unwrap_or(Value::Null),
            )
        }
    };
    let good = json!({"passphrase": "import passphrase", "password": "olive-password-12"});
    // Not an owner, or the wrong passphrase: refused, nothing changed.
    let (st, v) = import(admin, good.clone()).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    let (st, v) = import(
        owner.clone(),
        json!({"passphrase": "the wrong passphrase", "password": "olive-password-12"}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(held(&h).await.iter().all(|(k, _)| *k != extra.did()));
    assert_eq!(h.state.signing_key(&a).await.unwrap().did(), k1.did());

    let b_root = h.state.store.repo_root_of(&b).await.unwrap().unwrap();
    let b_events = h.outbox_kinds(&b).await.len();
    let b_key = h.state.signing_key(&b).await.unwrap().did();
    let a_root = h.state.store.repo_root_of(&a).await.unwrap().unwrap();
    let before = h.state.store.db_now_ms().await.unwrap();
    let (st, v) = import(owner.clone(), good).await;
    let after = h.state.store.db_now_ms().await.unwrap();
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["plcKeysAdded"], json!([extra.did()]), "{v}");
    assert_eq!(v["signingKeysReplaced"], json!([a]), "{v}");

    // The added key is secondary since the import, not since the backup.
    let row = rows(&h)
        .await
        .into_iter()
        .find(|r| r.did_key == extra.did())
        .unwrap();
    assert_eq!(row.role, "secondary");
    let since = row.secondary_since.unwrap();
    assert!(
        (before..=after).contains(&since),
        "{before} {since} {after}"
    );

    // B's document lists its stored key: untouched.
    assert_eq!(h.state.signing_key(&b).await.unwrap().did(), b_key);
    let b_now = h.state.store.repo_root_of(&b).await.unwrap().unwrap();
    assert_eq!(b_now.commit_cid, b_root.commit_cid, "no new commit for B");
    assert_eq!(
        h.outbox_kinds(&b).await.len(),
        b_events,
        "nothing emitted for B"
    );

    // A: a fresh commit over the same tree, signed with K2, and a #sync
    // carrying exactly that commit.
    assert_eq!(h.state.signing_key(&a).await.unwrap().did(), k2.did());
    let root = h.state.store.repo_root_of(&a).await.unwrap().unwrap();
    assert_ne!(root.commit_cid, a_root.commit_cid);
    assert_eq!(root.data_cid, a_root.data_cid);
    assert!(root.rev > a_root.rev, "a later rev");
    let commit_bytes = h
        .state
        .store
        .block(&a, &root.commit_cid)
        .await
        .unwrap()
        .unwrap();
    let commit = proto_blue_repo::commit::SignedCommit::from_lex_value(
        &proto_blue_lex_cbor::decode(&commit_bytes).unwrap(),
    )
    .unwrap();
    assert!(proto_blue_repo::commit::verify_commit_sig(&commit, &k2.did()).unwrap());
    assert!(!proto_blue_repo::commit::verify_commit_sig(&commit, &k1.did()).unwrap());
    let (kind, payload) = h.state.store.outbox_for(&a).await.unwrap().pop().unwrap();
    assert_eq!(kind, "sync");
    let body: proto_blue_lex_data::LexValue = proto_blue_lex_cbor::decode(&payload).unwrap();
    let m = body.as_map().unwrap();
    assert_eq!(m["rev"].as_str(), Some(root.rev.as_str()));
    let Some(proto_blue_lex_data::LexValue::Bytes(car)) = m.get("blocks") else {
        panic!("no blocks")
    };
    let (car_root, blocks) = proto_blue_repo::car::read_car_with_root(car).unwrap();
    assert_eq!(car_root.to_string_base32(), root.commit_cid);
    let synced = proto_blue_repo::commit::SignedCommit::from_lex_value(
        &proto_blue_lex_cbor::decode(blocks.get(&car_root).unwrap()).unwrap(),
    )
    .unwrap();
    assert!(proto_blue_repo::commit::verify_commit_sig(&synced, &k2.did()).unwrap());

    // Reloaded, the node signs with the imported key for a document that
    // lists only it.
    let h = reload(h).await;
    set_rotation_keys(&h, &b, &[&extra.did()]).await;
    let (st, v) = h.update_handle(&b_access, "bea2.pds.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let current = h.state.secrets.plc_keys.current_did().unwrap();
    assert_eq!(
        last_op_signer(&h, &b, &[&current, &extra.did()]),
        Some(extra.did())
    );
}

/// Requests and signs a PLC op for the account with `rotation_keys`.
async fn sign_plc_op(h: &Harness, access: &str, email: &str, rotation_keys: &[&str]) -> Value {
    let (st, v) = h
        .post(
            "com.atproto.identity.requestPlcOperationSignature",
            Some(access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, code) = h.last_mail(email);
    let (st, v) = h
        .post(
            "com.atproto.identity.signPlcOperation",
            Some(access),
            json!({"token": code, "rotationKeys": rotation_keys}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    v["operation"].clone()
}

#[tokio::test]
async fn after_a_key_change_new_identities_name_it_and_old_documents_keep_their_signer() {
    let h = harness(false).await;
    let (old, old_access) = h.user("oli.pds.example.com").await;
    let (both, both_access) = h.user("bob.pds.example.com").await;
    let (mig, mig_access) = h.user("mia.pds.example.com").await;
    let owner = h.owner_with_password("olive", "olive-password-12").await;
    let k1 = h.state.secrets.plc_keys.current_did().unwrap();
    let (st, v) = h
        .owner_call(
            &owner,
            "owner.keys.setPlcRotationKey",
            json!({"generate": true}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let k2 = v["didKey"].as_str().unwrap().to_owned();
    let (st, v) = h
        .owner_call(
            &owner,
            "owner.keys.setPlcRotationKey",
            json!({"acknowledgeNoBackup": true}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let h = reload(h).await;
    let keys = [k1.as_str(), k2.as_str()];

    // New: recommendations and a new account's genesis name K2 only.
    let (st, rec) = h
        .call(
            "GET",
            "/xrpc/com.atproto.identity.getRecommendedDidCredentials",
            Some(&old_access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{rec}");
    assert_eq!(rec["rotationKeys"], json!([k2]));
    let (new, _) = h.user("neo.pds.example.com").await;
    let genesis = h.plc.lock().unwrap().last(&new).unwrap();
    assert_eq!(genesis["rotationKeys"], json!([k2]), "{genesis}");
    assert_eq!(last_op_signer(&h, &new, &keys), Some(k2.clone()));

    // An account whose document lists only K1: a handle change, a signed
    // op and a hostname-migration op are all signed with K1.
    let (st, v) = h.update_handle(&old_access, "oli2.pds.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let last = h.plc.lock().unwrap().last(&old).unwrap();
    assert_eq!(last["alsoKnownAs"], json!(["at://oli2.pds.example.com"]));
    assert_eq!(last_op_signer(&h, &old, &keys), Some(k1.clone()));
    let op = sign_plc_op(&h, &old_access, "oli@example.com", &[&k2, &k1]).await;
    let signed = SignedOp::from_json(&op).unwrap();
    assert!(signed.verify(&k1) && !signed.verify(&k2), "{op}");
    assert!(matches!(
        crate::identity::plc_set_endpoint(&h.state, &old, "https://pds2.example.com").await,
        Ok(true)
    ));
    let last = h.plc.lock().unwrap().last(&old).unwrap();
    assert_eq!(
        last["services"]["atproto_pds"]["endpoint"],
        "https://pds2.example.com"
    );
    assert_eq!(last_op_signer(&h, &old, &keys), Some(k1.clone()));

    // A document listing both signs with whichever it lists first.
    set_rotation_keys(&h, &both, &[&k2, &k1]).await;
    let (st, v) = h.update_handle(&both_access, "bob2.pds.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(last_op_signer(&h, &both, &keys), Some(k2.clone()));
    set_rotation_keys(&h, &both, &[&k1, &k2]).await;
    let (st, v) = h.update_handle(&both_access, "bob3.pds.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(last_op_signer(&h, &both, &keys), Some(k1.clone()));

    // Activation needs the current key: a deactivated account whose
    // document lists only K1 (and has no listing row) is refused, as is
    // validDid, and no row is written.
    let (st, v) = h
        .post(
            "com.atproto.server.deactivateAccount",
            Some(&mig_access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    sqlx::query("DELETE FROM plc_key_listing WHERE did = ?1")
        .bind(&mig)
        .execute(&pool(&h))
        .await
        .unwrap();
    let (st, status) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.checkAccountStatus",
            Some(&mig_access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{status}");
    assert_eq!(status["validDid"], json!(false), "{status}");
    let (st, v) = h
        .post(
            "com.atproto.server.activateAccount",
            Some(&mig_access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(
        v["message"].as_str().unwrap().contains("signPlcOperation"),
        "{v}"
    );
    assert_eq!(h.state.store.plc_key_listing(&mig).await.unwrap(), None);
    // Still deactivated, it signs (with K1) and submits an op adding K2;
    // then it reactivates.
    let op = sign_plc_op(&h, &mig_access, "mia@example.com", &[&k2, &k1]).await;
    let (st, v) = h
        .post(
            "com.atproto.identity.submitPlcOperation",
            Some(&mig_access),
            json!({"operation": op}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(last_op_signer(&h, &mig, &keys), Some(k1.clone()));
    assert_eq!(
        h.state.store.plc_key_listing(&mig).await.unwrap(),
        Some(vec![k2.clone(), k1.clone()])
    );
    let (st, v) = h
        .post(
            "com.atproto.server.activateAccount",
            Some(&mig_access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");

    // The env value set back to K1 (now secondary) turns that row pending,
    // with no second row; a backup then makes it current again.
    let k1_private = h.plc_secret(&k1).await.unwrap();
    crate::state::apply_env_plc_key(
        &h.state.store,
        &h.state.master_key,
        &k1,
        locus_store::keys::PlcKeySource::Private(&k1_private),
    )
    .await
    .unwrap();
    let now = held(&h).await;
    assert_eq!(now.iter().filter(|(k, _)| *k == k1).count(), 1, "{now:?}");
    assert!(now.contains(&(k1.clone(), "pending".into())), "{now:?}");
    assert!(now.contains(&(k2.clone(), "current".into())), "{now:?}");
    let (st, _) = h.backup_bytes(&owner, "backup passphrase 1").await;
    assert_eq!(st, StatusCode::OK);
    let now = held(&h).await;
    assert!(now.contains(&(k1.clone(), "current".into())), "{now:?}");
    assert!(now.contains(&(k2, "secondary".into())), "{now:?}");
}

#[tokio::test]
async fn a_pending_key_is_replaced_until_a_document_names_it() {
    let h = harness(false).await;
    let (x, _) = h.user("xia.pds.example.com").await;
    let owner = h.owner_with_password("olive", "olive-password-12").await;
    let k1 = h.state.secrets.plc_keys.current_did().unwrap();
    let set = |body: Value| {
        let (h, owner) = (&h, owner.clone());
        async move {
            h.owner_call(&owner, "owner.keys.setPlcRotationKey", body)
                .await
        }
    };
    let (st, v) = set(json!({"generate": true})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let p1 = v["didKey"].as_str().unwrap().to_owned();
    assert!(h.plc_secret(&p1).await.is_some());

    // A second key replaces the first, which leaves the key store.
    let (st, v) = set(json!({"generate": true})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let p2 = v["didKey"].as_str().unwrap().to_owned();
    assert_eq!(v["replaced"], json!(p1));
    let now = held(&h).await;
    assert!(now.contains(&(k1.clone(), "current".into())), "{now:?}");
    assert!(now.contains(&(p2.clone(), "pending".into())), "{now:?}");
    assert_eq!(now.len(), 2, "{now:?}");
    assert!(h.plc_secret(&p1).await.is_none(), "the first key is gone");

    // X's document names P2 (an op locus didn't see, after X's listing
    // row was written): replacing or discarding P2 is refused, and so is
    // a changed env value. (This node hasn't reloaded its keys since P2
    // was stored; the guard still counts P2 as held.)
    set_rotation_keys(&h, &x, &[&k1, &p2]).await;
    let (st, v) = set(json!({"generate": true})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["message"].as_str().unwrap().contains(&x), "{v}");
    let (st, v) = set(json!({"discardPending": true})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let env_key = K256Keypair::generate();
    crate::state::apply_env_plc_key(
        &h.state.store,
        &h.state.master_key,
        &env_key.did(),
        locus_store::keys::PlcKeySource::Private(&env_key.export_private_key()),
    )
    .await
    .unwrap();
    assert_eq!(held(&h).await, now, "nothing changed");
    assert!(h.plc_secret(&p2).await.is_some());
}

#[tokio::test]
async fn a_first_boot_env_plc_key_is_current_at_once_and_stays_so() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&DatabaseUrl::Sqlite(dir.path().join("t.sqlite")))
        .await
        .unwrap();
    let key = MasterKey::generate(SourceKind::Env);
    let plc = K256Keypair::generate();
    let env: HashMap<String, String> = [(
        "PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX".to_owned(),
        hex::encode(plc.export_private_key()),
    )]
    .into();
    let rows = |store: Store| async move {
        store
            .plc_key_rows()
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.did_key, r.role))
            .collect::<Vec<_>>()
    };
    let secrets = ServerSecrets::load_or_create_from(&store, &key, &env)
        .await
        .unwrap();
    assert_eq!(secrets.plc_keys.current_did(), Some(plc.did()));
    assert_eq!(
        rows(store.clone()).await,
        vec![(plc.did(), "current".to_owned())],
        "no pending step"
    );
    // The key moves on; the next boot, same value, changes nothing: the
    // first boot recorded the value as applied, so the old key isn't
    // brought back as pending.
    let next = K256Keypair::generate();
    let mut tx = store.begin().await.unwrap();
    tx.store_pending_plc_key(
        &next.did(),
        locus_store::keys::PlcKeySource::Private(&next.export_private_key()),
        &key,
    )
    .await
    .unwrap();
    tx.promote_pending_plc_key().await.unwrap();
    tx.commit().await.unwrap();
    let before = rows(store.clone()).await;
    let secrets = ServerSecrets::load_or_create_from(&store, &key, &env)
        .await
        .unwrap();
    assert_eq!(secrets.plc_keys.current_did(), Some(next.did()));
    assert_eq!(rows(store.clone()).await, before);
    assert!(before.contains(&(plc.did(), "secondary".to_owned())));
}

#[tokio::test]
async fn an_entryway_s_drop_guard_refetches_every_account() {
    let (e, s, s_url) = super::entryway_and_shard().await;
    let owner = e.owner_with_password("olive", "olive-password-12").await;
    let shard_pw = String::from_utf8(s.state.secrets.admin_password.to_vec()).unwrap();
    let (st, v) = e
        .admin_call(
            "POST",
            &ns("owner.shard.add"),
            &owner,
            Some(json!({"url": s_url, "adminPassword": shard_pw, "name": "s1"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let acct = e
        .create_account(
            "ann.entryway.example.com",
            "ann@example.com",
            "ann-password",
        )
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let k1 = e.state.secrets.plc_keys.current_did().unwrap();
    let (st, v) = e
        .owner_call(
            &owner,
            "owner.keys.setPlcRotationKey",
            json!({"generate": true}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let k2 = v["didKey"].as_str().unwrap().to_owned();
    let (st, v) = e
        .owner_call(
            &owner,
            "owner.keys.setPlcRotationKey",
            json!({"acknowledgeNoBackup": true, "acknowledgeUnconfirmed": true}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // A fresh listing that says K2, while the document (changed through
    // the shard, unseen here) lists only K1: an entryway re-fetches anyway.
    e.state
        .store
        .set_plc_key_listing(&did, std::slice::from_ref(&k2))
        .await
        .unwrap();
    let (st, v) = e
        .owner_call(&owner, "owner.keys.dropPlcKey", json!({"didKey": k1}))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["message"].as_str().unwrap().contains(&did), "{v}");
    assert!(held(&e).await.contains(&(k1, "secondary".into())));
}

#[tokio::test]
async fn dropping_a_kms_plc_key_forgets_its_id_and_says_to_disable_it() {
    let Some(key_id) = super::test_kms_k256_key().await else {
        eprintln!("LOCUS_KMS_ENDPOINT isn't set; skipping");
        return;
    };
    let h = harness(false).await;
    let owner = h.owner_with_password("olive", "olive-password-12").await;
    for body in [
        json!({"kmsKeyId": key_id}),
        json!({"acknowledgeNoBackup": true}),
        json!({"generate": true}),
        json!({"acknowledgeNoBackup": true}),
    ] {
        let (st, v) = h
            .owner_call(&owner, "owner.keys.setPlcRotationKey", body)
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
    }
    let kms_did = rows(&h)
        .await
        .into_iter()
        .find(|r| r.kms_key_id.as_deref() == Some(key_id.as_str()))
        .unwrap()
        .did_key;
    // No account lists it: dropped, its id forgotten, the operator told.
    let (st, v) = h
        .owner_call(&owner, "owner.keys.dropPlcKey", json!({"didKey": kms_did}))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(v["next"].as_str().unwrap().contains(&key_id), "{v}");
    assert!(
        rows(&h)
            .await
            .iter()
            .all(|r| r.kms_key_id.as_deref() != Some(key_id.as_str()))
    );
}
