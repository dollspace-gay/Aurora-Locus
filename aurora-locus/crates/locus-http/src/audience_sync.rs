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
