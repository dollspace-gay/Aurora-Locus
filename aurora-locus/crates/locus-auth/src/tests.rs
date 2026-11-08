//! Session tests against SQLite, and Postgres when `LOCUS_TEST_POSTGRES_URL` is set.

use locus_core::bootstrap::DatabaseUrl;
use locus_store::Store;
use locus_store::accounts::{NewAccount, SessionFilter, Status};
use zeroize::Zeroizing;

use crate::jwt;
use crate::session::{AuthError, LegacySessions, SCOPE_ACCESS, SCOPE_APP_PASS, SCOPE_TAKENDOWN};

const DID: &str = "did:plc:sessiontest0000000000000";
const SERVICE: &str = "did:web:pds.example.com";

struct Backend {
    name: &'static str,
    store: Store,
    _dir: Option<tempfile::TempDir>,
}

async fn backends() -> Vec<Backend> {
    let mut out = Vec::new();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&DatabaseUrl::Sqlite(dir.path().join("t.sqlite")))
        .await
        .unwrap();
    out.push(Backend {
        name: "sqlite",
        store,
        _dir: Some(dir),
    });
    if let Ok(admin_url) = std::env::var("LOCUS_TEST_POSTGRES_URL") {
        let admin = sqlx::PgPool::connect(&admin_url).await.unwrap();
        let db = format!("locus_test_{}", hex::encode(rand::random::<[u8; 8]>()));
        sqlx::query(&format!("CREATE DATABASE {db}"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let store = Store::open(&DatabaseUrl::Postgres(format!("{base}/{db}")))
            .await
            .unwrap();
        out.push(Backend {
            name: "postgres",
            store,
            _dir: None,
        });
    }
    out
}

async fn setup(b: &Backend) -> LegacySessions {
    let mut tx = b.store.begin().await.unwrap();
    tx.insert_account(&NewAccount {
        did: DID,
        handle: "alice.example.com",
        email: Some("alice@example.com"),
        password_hash: None,
        status: Status::Active,
        did_method: "plc",
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();
    LegacySessions::new(
        b.store.clone(),
        Zeroizing::new(b"test secret".to_vec()),
        SERVICE.into(),
    )
}

fn jti(token: &str) -> String {
    jwt::verify(b"test secret", jwt::TYP_REFRESH, token, 0)
        .unwrap()
        .jti
        .unwrap()
}

#[tokio::test]
async fn sessions_verify_and_logout_takes_effect_at_once() {
    for b in backends().await {
        let s = setup(&b).await;
        let t = s.create(DID, SCOPE_ACCESS, None, true).await.unwrap();
        let authed = s.verify_access(&t.access_jwt).await.unwrap();
        assert_eq!(
            (authed.did.as_str(), authed.scope.as_str()),
            (DID, SCOPE_ACCESS),
            "{}",
            b.name
        );

        s.delete(t.refresh_jwt.as_deref().unwrap()).await.unwrap();
        assert_eq!(
            s.verify_access(&t.access_jwt).await,
            Err(AuthError::RevokedToken),
            "{}",
            b.name
        );
        assert_eq!(
            s.refresh(t.refresh_jwt.as_deref().unwrap())
                .await
                .unwrap_err(),
            AuthError::RevokedToken
        );
    }
}

#[tokio::test]
async fn refresh_rotates_with_a_replay_grace() {
    for b in backends().await {
        let s = setup(&b).await;
        let t = s.create(DID, SCOPE_ACCESS, None, true).await.unwrap();
        let r1 = t.refresh_jwt.unwrap();
        let (t2, did) = s.refresh(&r1).await.unwrap();
        assert_eq!(did, DID);
        let r2 = t2.refresh_jwt.unwrap();
        assert_ne!(jti(&r1), jti(&r2), "{}", b.name);
        // Same session: the old access token still works.
        s.verify_access(&t.access_jwt).await.unwrap();
        s.verify_access(&t2.access_jwt).await.unwrap();

        // Replaying the used token inside the grace returns the same next token.
        let (t3, _) = s.refresh(&r1).await.unwrap();
        assert_eq!(
            jti(t3.refresh_jwt.as_deref().unwrap()),
            jti(&r2),
            "{}",
            b.name
        );
        // The next token rotates normally.
        let (t4, _) = s.refresh(&r2).await.unwrap();
        assert_ne!(jti(t4.refresh_jwt.as_deref().unwrap()), jti(&r2));
    }
}

#[tokio::test]
async fn takedown_applies_on_the_next_request() {
    for b in backends().await {
        let s = setup(&b).await;
        let t = s.create(DID, SCOPE_ACCESS, None, true).await.unwrap();
        let mut tx = b.store.begin().await.unwrap();
        tx.set_account_status(DID, Status::Takendown, Some("mod:1"))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            s.verify_access(&t.access_jwt).await,
            Err(AuthError::AccountTakedown),
            "{}",
            b.name
        );
        assert_eq!(
            s.refresh(t.refresh_jwt.as_deref().unwrap())
                .await
                .unwrap_err(),
            AuthError::AccountTakedown
        );
        // A takendown-scope session (the migration-out and appeal paths) still verifies.
        let td = s.create(DID, SCOPE_TAKENDOWN, None, false).await.unwrap();
        assert!(td.refresh_jwt.is_none());
        assert_eq!(
            s.verify_access(&td.access_jwt).await.unwrap().status,
            Status::Takendown
        );
    }
}

#[tokio::test]
async fn app_password_sessions_are_revoked_together() {
    for b in backends().await {
        let s = setup(&b).await;
        let a = s
            .create(DID, SCOPE_APP_PASS, Some("bot"), true)
            .await
            .unwrap();
        let other = s.create(DID, SCOPE_ACCESS, None, true).await.unwrap();
        let mut tx = b.store.begin().await.unwrap();
        assert_eq!(
            tx.revoke_sessions(DID, SessionFilter::AppPassword("bot"))
                .await
                .unwrap(),
            1
        );
        tx.commit().await.unwrap();
        assert_eq!(
            s.verify_access(&a.access_jwt).await,
            Err(AuthError::RevokedToken),
            "{}",
            b.name
        );
        s.verify_access(&other.access_jwt).await.unwrap();
    }
}

#[tokio::test]
async fn tokens_for_another_service_or_shape_are_refused() {
    for b in backends().await {
        let s = setup(&b).await;
        let t = s.create(DID, SCOPE_ACCESS, None, true).await.unwrap();
        let elsewhere = LegacySessions::new(
            b.store.clone(),
            Zeroizing::new(b"test secret".to_vec()),
            "did:web:other.example".into(),
        );
        assert_eq!(
            elsewhere.verify_access(&t.access_jwt).await,
            Err(AuthError::InvalidToken),
            "{}",
            b.name
        );
        // A refresh token isn't an access token.
        assert_eq!(
            s.verify_access(t.refresh_jwt.as_deref().unwrap()).await,
            Err(AuthError::InvalidToken)
        );
    }
}
