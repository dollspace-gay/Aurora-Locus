/// com.atproto.repo.uploadBlob and blob serving endpoints
use crate::{
    api::middleware,
    blob_store::{BlobUploadResponse},
    context::AppContext,
    error::{PdsError, PdsResult},
};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

/// Build blob routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/xrpc/com.atproto.repo.uploadBlob", post(upload_blob))
        .route("/blob/:cid", get(get_blob))
}

/// Upload a blob (Two-phase upload)
///
/// Phase 1: Stages blob in temporary storage and returns blob reference.
/// Phase 2: Blob is committed to permanent storage when used in a record.
///
/// Accepts raw binary data in the request body with Content-Type header
async fn upload_blob(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Bytes,
) -> PdsResult<impl IntoResponse> {
    // Require authentication
    let session = middleware::require_auth(State(ctx.clone()), headers.clone()).await?;

    // Get Content-Type from header
    let mime_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Convert Bytes to Vec<u8>
    let data = body.to_vec();

    // Stage blob in temporary storage (Phase 1)
    let temp_blob = ctx
        .blob_store
        .stage_blob(data, mime_type.as_deref(), &session.did)
        .await?;

    // Return blob reference
    let blob_ref = crate::blob_store::BlobRef::new(
        temp_blob.cid,
        temp_blob.mime_type,
        temp_blob.size,
    );

    Ok((
        StatusCode::OK,
        Json(BlobUploadResponse { blob: blob_ref }),
    ))
}

/// Get a blob by CID
///
/// Serves blob content with proper Content-Type, caching headers, and Range request support
async fn get_blob(
    State(ctx): State<AppContext>,
    Path(cid): Path<String>,
    headers: HeaderMap,
) -> PdsResult<Response> {
    // Get blob from store
    let blob_data = ctx
        .blob_store
        .get(&cid)
        .await?
        .ok_or_else(|| PdsError::NotFound(format!("Blob not found: {}", cid)))?;

    let (data, mime_type) = blob_data;
    let total_size = data.len();

    // Calculate ETag from CID (CID is already content-addressed)
    let etag = format!("\"{}\"", cid);

    // Check If-None-Match header for 304 Not Modified
    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH) {
        if let Ok(if_none_match_str) = if_none_match.to_str() {
            if if_none_match_str == etag {
                return Ok(Response::builder()
                    .status(StatusCode::NOT_MODIFIED)
                    .header(header::ETAG, etag)
                    .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                    .body(axum::body::Body::empty())
                    .unwrap());
            }
        }
    }

    // Check for Range header
    if let Some(range_header) = headers.get(header::RANGE) {
        if let Ok(range_str) = range_header.to_str() {
            // Parse Range header (format: "bytes=start-end")
            if let Some(range) = parse_range(range_str, total_size) {
                let (start, end) = range;
                let length = end - start + 1;
                let partial_data = data[start..=end].to_vec();

                return Ok(Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::CONTENT_TYPE, mime_type)
                    .header(header::CONTENT_LENGTH, length.to_string())
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {}-{}/{}", start, end, total_size),
                    )
                    .header(header::ETAG, etag)
                    .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                    .header(header::ACCEPT_RANGES, "bytes")
                    .body(axum::body::Body::from(partial_data))
                    .unwrap());
            }
        }
    }

    // Return full content
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CONTENT_LENGTH, total_size.to_string())
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ACCEPT_RANGES, "bytes")
        .body(axum::body::Body::from(data))
        .unwrap())
}

/// Parse HTTP Range header
///
/// Returns (start, end) inclusive byte positions, or None if invalid
fn parse_range(range_header: &str, total_size: usize) -> Option<(usize, usize)> {
    // Expected format: "bytes=start-end" or "bytes=start-" or "bytes=-suffix"
    let range_header = range_header.trim();

    if !range_header.starts_with("bytes=") {
        return None;
    }

    let range_spec = &range_header[6..]; // Remove "bytes=" prefix

    if let Some(dash_pos) = range_spec.find('-') {
        let start_str = &range_spec[..dash_pos];
        let end_str = &range_spec[dash_pos + 1..];

        if start_str.is_empty() {
            // Suffix range: "bytes=-500" (last 500 bytes)
            if let Ok(suffix) = end_str.parse::<usize>() {
                let start = total_size.saturating_sub(suffix);
                return Some((start, total_size - 1));
            }
        } else if end_str.is_empty() {
            // Open-ended range: "bytes=500-" (from 500 to end)
            if let Ok(start) = start_str.parse::<usize>() {
                if start < total_size {
                    return Some((start, total_size - 1));
                }
            }
        } else {
            // Complete range: "bytes=500-999"
            if let (Ok(start), Ok(mut end)) = (start_str.parse::<usize>(), end_str.parse::<usize>()) {
                if start < total_size {
                    // Clamp end to total_size - 1
                    end = end.min(total_size - 1);
                    if start <= end {
                        return Some((start, end));
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routes_created() {
        let _router = routes();
        // Just verify it compiles
    }

    #[test]
    fn test_parse_range_complete() {
        // "bytes=0-499" for 1000 byte file
        assert_eq!(parse_range("bytes=0-499", 1000), Some((0, 499)));
        assert_eq!(parse_range("bytes=500-999", 1000), Some((500, 999)));
    }

    #[test]
    fn test_parse_range_open_ended() {
        // "bytes=500-" for 1000 byte file
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=0-", 1000), Some((0, 999)));
    }

    #[test]
    fn test_parse_range_suffix() {
        // "bytes=-500" for 1000 byte file (last 500 bytes)
        assert_eq!(parse_range("bytes=-500", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
    }

    #[test]
    fn test_parse_range_clamping() {
        // Request beyond file size should be clamped
        assert_eq!(parse_range("bytes=0-2000", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=900-2000", 1000), Some((900, 999)));
    }

    #[test]
    fn test_parse_range_invalid() {
        assert_eq!(parse_range("bytes=invalid", 1000), None);
        assert_eq!(parse_range("bytes=1000-", 1000), None); // Start beyond file
        assert_eq!(parse_range("bytes=500-400", 1000), None); // Start > end
        assert_eq!(parse_range("invalid", 1000), None); // Wrong prefix
    }
}
