//! A client for the PLC directory (operator-configured, so it uses the
//! operator-configured HTTP client: no SSRF rules, but timeouts and a
//! response-size cap).

use std::time::Duration;

use serde_json::Value;
use thiserror::Error;

use crate::plc::SignedOp;

/// Largest PLC response read (documents and logs are small).
const MAX_RESPONSE: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum PlcClientError {
    /// The directory refused the request (4xx); the message is its reason.
    #[error("PLC directory refused: {0}")]
    Refused(String),
    /// No clear answer (timeout, connection error, 5xx): the op may or may
    /// not have been applied.
    #[error("PLC directory unreachable: {0}")]
    Ambiguous(String),
    #[error("PLC directory sent an invalid response: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct PlcClient {
    base: String,
    http: reqwest::Client,
}

impl PlcClient {
    pub fn new(base_url: &str) -> Result<Self, PlcClientError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("aurora-locus/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| PlcClientError::Invalid(e.to_string()))?;
        Ok(Self {
            base: base_url.trim_end_matches('/').to_owned(),
            http,
        })
    }

    /// Submits a signed operation for `did`.
    pub async fn submit(&self, did: &str, op: &SignedOp) -> Result<(), PlcClientError> {
        let res = self
            .http
            .post(format!("{}/{did}", self.base))
            .json(&op.to_json())
            .send()
            .await
            .map_err(|e| PlcClientError::Ambiguous(e.to_string()))?;
        let status = res.status();
        if status.is_success() {
            return Ok(());
        }
        let body = read_capped(res).await.unwrap_or_default();
        let message = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| String::from_utf8_lossy(&body).into_owned());
        if status.is_client_error() {
            Err(PlcClientError::Refused(message))
        } else {
            Err(PlcClientError::Ambiguous(format!("{status}: {message}")))
        }
    }

    /// The DID's current document, or None if the directory doesn't know it.
    pub async fn document(&self, did: &str) -> Result<Option<Value>, PlcClientError> {
        self.get_json(&format!("{}/{did}", self.base)).await
    }

    /// The DID's latest operation, or None if unknown.
    pub async fn last_op(&self, did: &str) -> Result<Option<SignedOp>, PlcClientError> {
        match self
            .get_json(&format!("{}/{did}/log/last", self.base))
            .await?
        {
            Some(v) => SignedOp::from_json(&v)
                .map(Some)
                .map_err(|e| PlcClientError::Invalid(e.to_string())),
            None => Ok(None),
        }
    }

    async fn get_json(&self, url: &str) -> Result<Option<Value>, PlcClientError> {
        let res = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| PlcClientError::Ambiguous(e.to_string()))?;
        match res.status() {
            s if s.is_success() => {
                let body = read_capped(res).await?;
                serde_json::from_slice(&body)
                    .map(Some)
                    .map_err(|e| PlcClientError::Invalid(e.to_string()))
            }
            // Unknown, or a tombstoned DID.
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE => Ok(None),
            s => Err(PlcClientError::Ambiguous(s.to_string())),
        }
    }
}

async fn read_capped(mut res: reqwest::Response) -> Result<Vec<u8>, PlcClientError> {
    let mut out = Vec::new();
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| PlcClientError::Ambiguous(e.to_string()))?
    {
        if out.len() + chunk.len() > MAX_RESPONSE {
            return Err(PlcClientError::Invalid("response too large".into()));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}
