//! Who is calling the admin API, and with which operator role.
//!
//! - an operator API token (`Bearer lop_…`): the operator's role;
//! - the reference's `admin:<password>` Basic auth (while enabled): `admin`,
//!   on the reference admin surface only;
//! - a service JWT from the moderation service: the reference's mod-service
//!   access (the methods the reference opens to it);
//! - a console sign-in (`Console <session>`): the operator's role;
//! - the admin socket (its marker, see [`crate::socket`]): owner.
//!
//! Every method has a minimum role; a caller below it is refused.

use axum::http::{HeaderMap, StatusCode, header};
use base64::Engine;
use serde_json::Value;
use sha2::Digest;
use subtle::ConstantTimeEq;

use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

/// Operator roles, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Viewer,
    Moderator,
    Admin,
    Owner,
}

impl Role {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "viewer" => Self::Viewer,
            "moderator" => Self::Moderator,
            "admin" => Self::Admin,
            "owner" => Self::Owner,
            _ => return None,
        })
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Moderator => "moderator",
            Self::Admin => "admin",
            Self::Owner => "owner",
        }
    }
}

/// How the caller authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Token,
    Basic,
    ModService,
    /// A console sign-in (`Console <session>`), as the console's forms call
    /// this API.
    Console,
    /// The admin socket (root or the service's own user on this machine).
    Socket,
}

/// An authenticated admin-API caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// Who, for the audit log: `operator:<name>`, `admin`, or the mod
    /// service's DID.
    pub actor: String,
    pub role: Role,
    pub via: Via,
    /// For a console sign-in, when the operator last confirmed their
    /// identity (password and code).
    pub confirmed_at: Option<i64>,
}

/// Which credentials a method takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A `com.atproto.admin.*` (or other reference admin-token) method:
    /// Basic auth works; the mod service only where `mod_service` is set.
    Reference { mod_service: bool },
    /// A locus method: operator tokens only.
    Locus,
}

/// A new operator API token.
#[must_use]
pub fn new_token() -> String {
    format!("lop_{}", hex::encode(rand::random::<[u8; 24]>()))
}

/// How a token is stored.
#[must_use]
pub fn token_hash(token: &str) -> String {
    hex::encode(sha2::Sha256::digest(token.as_bytes()))
}

fn unauthorized() -> XrpcError {
    XrpcError::auth_required("Authentication Required")
}

fn basic_admin(s: &AppState, headers: &HeaderMap) -> bool {
    let Some(v) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(b64) = v.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
        return false;
    };
    let Some(pw) = raw.strip_prefix(b"admin:") else {
        return false;
    };
    s.snapshot().bool("admin.basic_auth_enabled")
        && bool::from(pw.ct_eq(s.secrets.admin_password.as_slice()))
}

/// Authenticates an admin-API call for `lxm` and checks `min`.
pub async fn require(
    s: &AppState,
    headers: &HeaderMap,
    lxm: &str,
    surface: Surface,
    min: Role,
) -> XrpcResult<Caller> {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let marker = headers
        .get(crate::socket::MARKER)
        .and_then(|v| v.to_str().ok());
    let caller = if let Some(m) = marker {
        if !crate::web::ct_eq(m, &s.socket_secret) {
            return Err(unauthorized());
        }
        Caller {
            actor: "socket".into(),
            role: Role::Owner,
            via: Via::Socket,
            confirmed_at: None,
        }
    } else if auth.starts_with("Basic ") {
        if !matches!(surface, Surface::Reference { .. }) || !basic_admin(s, headers) {
            return Err(unauthorized());
        }
        Caller {
            actor: "admin".into(),
            role: Role::Admin,
            via: Via::Basic,
            confirmed_at: None,
        }
    } else if let Some(token) = auth
        .strip_prefix("Bearer ")
        .filter(|t| t.starts_with("lop_"))
    {
        let op = s
            .store
            .operator_by_token(&token_hash(token))
            .await?
            .ok_or_else(unauthorized)?;
        Caller {
            actor: format!("operator:{}", op.username),
            role: Role::parse(&op.role).unwrap_or(Role::Viewer),
            via: Via::Token,
            confirmed_at: None,
        }
    } else if let Some(id) = auth.strip_prefix("Console ") {
        let idle = s.snapshot().int("admin.session_idle_minutes") * 60_000;
        let (session, op) = s
            .store
            .operator_session(&crate::console::session_hash(id.trim()), idle)
            .await?
            .ok_or_else(unauthorized)?;
        if session.stage != "full" {
            return Err(unauthorized());
        }
        Caller {
            actor: format!("operator:{}", op.username),
            role: Role::parse(&op.role).unwrap_or(Role::Viewer),
            via: Via::Console,
            confirmed_at: Some(session.confirmed_at),
        }
    } else if auth.starts_with("Bearer ") {
        let Surface::Reference { mod_service: true } = surface else {
            return Err(unauthorized());
        };
        let mod_did = s.snapshot().text("moderation.service_did").to_owned();
        if mod_did.is_empty() {
            return Err(XrpcError::new(
                StatusCode::UNAUTHORIZED,
                "UntrustedIss",
                "Untrusted issuer",
            ));
        }
        let did = crate::auth::user_service_auth(s, headers, lxm).await?;
        if did != mod_did {
            return Err(XrpcError::new(
                StatusCode::UNAUTHORIZED,
                "UntrustedIss",
                "Untrusted issuer",
            ));
        }
        Caller {
            actor: did,
            role: Role::Moderator,
            via: Via::ModService,
            confirmed_at: None,
        }
    } else {
        return Err(unauthorized());
    };
    if caller.role < min {
        return Err(XrpcError::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("This needs the {} role", min.as_str()),
        ));
    }
    Ok(caller)
}

/// The role a locus method's group needs (`<ns>.viewer.*`, `.moderator.*`,
/// `.admin.*`, `.owner.*`), or None outside the groups.
#[must_use]
pub fn group_min(lxm: &str) -> Option<Role> {
    let rest = lxm.strip_prefix(locus_core::LOCUS_NS)?.strip_prefix('.')?;
    Role::parse(rest.split('.').next()?)
}

/// Refuses a call to a grouped locus method below the group's role before
/// the handler runs, so before its body is parsed. Handlers check again.
pub async fn group_gate(
    axum::extract::State(s): axum::extract::State<std::sync::Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Some(lxm) = req.uri().path().strip_prefix("/xrpc/")
        && let Some(min) = group_min(lxm)
        && let Err(e) = require(&s, req.headers(), lxm, Surface::Locus, min).await
    {
        return e.into_response();
    }
    next.run(req).await
}

/// Appends an operator or moderation action to the audit log.
pub async fn audit(
    s: &AppState,
    caller: &Caller,
    action: &str,
    target: Option<&str>,
    detail: Option<Value>,
) {
    if let Err(e) = s
        .store
        .audit(&caller.actor, action, target, detail.as_ref())
        .await
    {
        tracing::error!(action, "audit log write failed: {e}");
    }
}

/// Proof of identity a key change asks for over HTTP: the operator's
/// password and, if they have a code app, a current code.
#[derive(Debug, Default, serde::Deserialize)]
pub struct Reauth {
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

/// Checks a caller made a key change knowingly. The socket CLI is the
/// owner already and confirms on its TTY; a console sign-in must have
/// confirmed its identity in the last few minutes; an API token must send
/// its operator's password (and code).
pub async fn reauth(s: &AppState, caller: &Caller, proof: &Reauth) -> XrpcResult<()> {
    let refused = || {
        XrpcError::new(
            StatusCode::UNAUTHORIZED,
            "ReauthRequired",
            "Confirm it's you: send the operator's password (and code)",
        )
    };
    match caller.via {
        Via::Socket => Ok(()),
        Via::Console => match caller.confirmed_at {
            Some(t) if locus_core::time::now_ms() - t < crate::console::CONFIRM_WINDOW_MS => Ok(()),
            _ => Err(refused()),
        },
        Via::Token => {
            let name = caller.actor.strip_prefix("operator:").ok_or_else(refused)?;
            let op = s.store.operator(name).await?.ok_or_else(refused)?;
            let pw = proof.password.as_deref().unwrap_or_default();
            if op.disabled || !locus_auth::password::verify(pw, &op.password_hash) {
                return Err(refused());
            }
            if op.totp_secret.is_some()
                && !crate::console::code_ok(
                    s,
                    name,
                    &crate::console::totp_secret_name(name),
                    proof.code.as_deref().unwrap_or_default(),
                )
                .await
            {
                return Err(refused());
            }
            Ok(())
        }
        Via::Basic | Via::ModService => Err(refused()),
    }
}
