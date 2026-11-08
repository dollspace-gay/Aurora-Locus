//! Cloudflare's published IP ranges, for the client-IP chain in Cloudflare
//! mode (`deploy.proxy_mode = cloudflare`): `CF-Connecting-IP` is believed
//! only when the hop before the trusted proxy is one of these. A bundled
//! list is the fallback; every node refreshes it daily from Cloudflare.

use std::net::IpAddr;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::ratelimit::cidr_contains;

/// The ranges as published at <https://www.cloudflare.com/ips/>.
pub const BUNDLED: &[&str] = &[
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
    "2400:cb00::/32",
    "2606:4700::/32",
    "2803:f800::/32",
    "2405:b500::/32",
    "2405:8100::/32",
    "2a06:98c0::/29",
    "2c0f:f248::/32",
];

static RANGES: LazyLock<RwLock<Vec<String>>> =
    LazyLock::new(|| RwLock::new(BUNDLED.iter().map(|s| (*s).to_owned()).collect()));

/// Whether `ip` is one of Cloudflare's.
#[must_use]
pub fn contains(ip: IpAddr) -> bool {
    RANGES
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|c| cidr_contains(c, ip))
}

/// The ranges in use now.
#[must_use]
pub fn ranges() -> Vec<String> {
    RANGES
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Lines of a published list that are CIDRs.
#[must_use]
pub fn parse_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| {
            l.split_once('/').is_some_and(|(a, b)| {
                a.parse::<IpAddr>().is_ok() && b.parse::<u8>().is_ok_and(|n| n <= 128)
            })
        })
        .map(str::to_owned)
        .collect()
}

async fn fetch() -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for url in [
        "https://www.cloudflare.com/ips-v4",
        "https://www.cloudflare.com/ips-v6",
    ] {
        let text = client
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let list = parse_list(&text);
        if list.is_empty() {
            return Err(format!("{url} listed no ranges"));
        }
        out.extend(list);
    }
    Ok(out)
}

/// Refreshes the ranges now and then daily, while `active()` says
/// Cloudflare mode is on. A failed fetch keeps what's there.
pub async fn refresh_loop(active: impl Fn() -> bool, cancel: CancellationToken) {
    loop {
        if active() {
            match fetch().await {
                Ok(list) => {
                    *RANGES
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = list;
                    tracing::info!("Cloudflare's IP ranges refreshed");
                }
                Err(e) => tracing::warn!(
                    "Cloudflare's IP ranges couldn't be refreshed (keeping the last list): {e}"
                ),
            }
        }
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(86_400)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_ranges_and_the_list_format() {
        assert!(contains("104.16.1.2".parse().unwrap()));
        assert!(contains("2606:4700::1".parse().unwrap()));
        assert!(!contains("8.8.8.8".parse().unwrap()));
        assert_eq!(
            parse_list("173.245.48.0/20\n\n# x\n2400:cb00::/32\nnonsense\n"),
            ["173.245.48.0/20", "2400:cb00::/32"]
        );
    }
}
