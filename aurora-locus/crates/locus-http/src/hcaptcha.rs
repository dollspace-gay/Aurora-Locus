//! hCaptcha for web sign-up, as the reference's OAuth sign-up: with a site
//! key (`accounts.hcaptcha_site_key`) and a secret (`keys set
//! hcaptcha-secret`), the sign-up forms carry the widget, and its token is
//! checked with hCaptcha's `siteverify` (the operator-configured client)
//! before the account is made. `createAccount` itself isn't gated, as the
//! reference.

use std::time::Duration;

use axum::http::{HeaderMap, header};
use base64::Engine;
use serde_json::{Value, json};

use crate::state::AppState;

/// The form field the widget fills in.
pub const FIELD: &str = "h-captcha-response";

/// The widget's script, and the origins its CSP needs (as the reference's:
/// scripts, frames, styles and requests).
pub const SCRIPT: &str = "https://js.hcaptcha.com/1/api.js";
pub const ORIGINS: &str = "https://hcaptcha.com https://*.hcaptcha.com";

/// The site key, when hCaptcha is on (a site key and a secret).
#[must_use]
pub fn site_key(s: &AppState) -> Option<String> {
    let key = s.snapshot().text("accounts.hcaptcha_site_key").trim().to_owned();
    (!key.is_empty() && s.secrets.hcaptcha_secret.is_some()).then_some(key)
}

/// The reference's client-token hash: scrypt (N = 1024, r = 8, p = 1,
/// 32 bytes) of the value under the salt, in base64.
pub(crate) fn hash_token(salt: &[u8], value: &str) -> String {
    let mut out = [0u8; 32];
    let params = scrypt::Params::new(10, 8, 1, 32).expect("valid scrypt parameters");
    scrypt::scrypt(value.as_bytes(), salt, &params, &mut out).expect("32-byte output");
    base64::engine::general_purpose::STANDARD.encode(out)
}

/// Checks a web sign-up's hCaptcha `token` for `handle`; Ok when hCaptcha
/// is off. The errors are the reference's messages.
pub async fn check(
    s: &AppState,
    headers: &HeaderMap,
    token: Option<&str>,
    handle: &str,
) -> Result<(), &'static str> {
    let Some(site_key) = site_key(s) else {
        return Ok(());
    };
    let Some(secret) = s.secrets.hcaptcha_secret.as_ref() else {
        return Ok(());
    };
    let token = token.map(str::trim).filter(|t| !t.is_empty());
    let Some(token) = token else {
        return Err("hCaptcha token is required");
    };
    let failed = "hCaptcha verification failed";
    let ip = crate::ratelimit::client_ip()
        .map(|i| i.to_string())
        .unwrap_or_default();
    let salt = s.secrets.hcaptcha_token_salt.as_slice();
    let mut tokens = json!({
        "hashedIp": hash_token(salt, &ip),
        "hashedHandle": hash_token(salt, handle),
    });
    if let Some(ua) = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
    {
        tokens["hashedUserAgent"] = json!(hash_token(salt, ua));
    }
    let url = s.snapshot().text("accounts.hcaptcha_verify_url").to_owned();
    let client = crate::pipethrough::client(s, false).map_err(|_| failed)?;
    let secret = String::from_utf8_lossy(secret).into_owned();
    let form = [
        ("secret", secret.as_str()),
        ("sitekey", site_key.as_str()),
        ("behavior_type", "signup"),
        ("response", token),
        ("remoteip", ip.as_str()),
        ("client_tokens", &tokens.to_string()),
    ];
    let res = client
        .post(&url)
        .form(&form)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    let body: Value = match res {
        Ok(r) if r.status().is_success() => r.json().await.map_err(|_| failed)?,
        Ok(r) => {
            tracing::warn!(status = %r.status(), "hCaptcha siteverify refused the request");
            return Err(failed);
        }
        Err(e) => {
            tracing::warn!("hCaptcha siteverify unreachable: {e}");
            return Err(failed);
        }
    };
    if body["success"].as_bool() == Some(true) {
        Ok(())
    } else {
        tracing::info!(errors = %body["error-codes"], "hCaptcha verification failed");
        Err(failed)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn client_tokens_hash_as_the_reference() {
        // scrypt('1.2.3.4', 'salt', 32, {cost: 1024}) in base64 (Node's
        // defaults r = 8, p = 1).
        assert_eq!(
            super::hash_token(b"salt", "1.2.3.4"),
            "oLQdqFml3e/gCOwdNn47NNVSm6nUwJgt6JphJY+SDOc="
        );
    }
}
