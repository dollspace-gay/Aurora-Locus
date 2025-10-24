//! Blob handling utilities for ATProto
//!
//! This module provides utilities for working with blobs (binary data)
//! such as images and videos in the ATProto ecosystem.

/// Blob reference structure
///
/// This represents a blob that has been uploaded to the PDS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlobRef {
    #[serde(rename = "$type")]
    pub blob_type: String,
    #[serde(rename = "ref")]
    pub r#ref: serde_json::Value,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub size: u64,
}

impl BlobRef {
    /// Create a new BlobRef from upload response
    pub fn from_upload_response(blob_json: serde_json::Value) -> Option<Self> {
        serde_json::from_value(blob_json).ok()
    }
}

/// Detect MIME type from file extension
///
/// # Arguments
///
/// * `filename` - The filename or path to detect MIME type from
///
/// # Returns
///
/// Returns the MIME type string, or "application/octet-stream" if unknown.
///
/// # Examples
///
/// ```
/// use atproto::blob::detect_mime_type;
///
/// assert_eq!(detect_mime_type("photo.jpg"), "image/jpeg");
/// assert_eq!(detect_mime_type("video.mp4"), "video/mp4");
/// assert_eq!(detect_mime_type("image.png"), "image/png");
/// assert_eq!(detect_mime_type("unknown.xyz"), "application/octet-stream");
/// ```
pub fn detect_mime_type(filename: &str) -> &'static str {
    let lowercase = filename.to_lowercase();

    // Image types
    if lowercase.ends_with(".jpg") || lowercase.ends_with(".jpeg") {
        "image/jpeg"
    } else if lowercase.ends_with(".png") {
        "image/png"
    } else if lowercase.ends_with(".gif") {
        "image/gif"
    } else if lowercase.ends_with(".webp") {
        "image/webp"
    }
    // Video types
    else if lowercase.ends_with(".mp4") {
        "video/mp4"
    } else if lowercase.ends_with(".mov") {
        "video/quicktime"
    } else if lowercase.ends_with(".webm") {
        "video/webm"
    }
    // Default
    else {
        "application/octet-stream"
    }
}

/// Detect MIME type from file data (magic bytes)
///
/// # Arguments
///
/// * `data` - The first few bytes of the file
///
/// # Returns
///
/// Returns the detected MIME type, or None if not recognized.
///
/// # Examples
///
/// ```
/// use atproto::blob::detect_mime_type_from_data;
///
/// // JPEG magic bytes: FF D8 FF
/// let jpeg_data = vec![0xFF, 0xD8, 0xFF];
/// assert_eq!(detect_mime_type_from_data(&jpeg_data), Some("image/jpeg"));
///
/// // PNG magic bytes: 89 50 4E 47
/// let png_data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
/// assert_eq!(detect_mime_type_from_data(&png_data), Some("image/png"));
/// ```
pub fn detect_mime_type_from_data(data: &[u8]) -> Option<&'static str> {
    if data.len() < 4 {
        return None;
    }

    // JPEG: FF D8 FF
    if data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        return Some("image/jpeg");
    }

    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if data.len() >= 8
        && data[0] == 0x89
        && data[1] == 0x50
        && data[2] == 0x4E
        && data[3] == 0x47 {
        return Some("image/png");
    }

    // GIF: "GIF87a" or "GIF89a"
    if data.len() >= 6
        && data[0] == b'G'
        && data[1] == b'I'
        && data[2] == b'F' {
        return Some("image/gif");
    }

    // WebP: "RIFF" ... "WEBP"
    if data.len() >= 12
        && data[0] == b'R'
        && data[1] == b'I'
        && data[2] == b'F'
        && data[3] == b'F'
        && data[8] == b'W'
        && data[9] == b'E'
        && data[10] == b'B'
        && data[11] == b'P' {
        return Some("image/webp");
    }

    // MP4: Check for "ftyp" box
    if data.len() >= 12 && data[4] == b'f' && data[5] == b't' && data[6] == b'y' && data[7] == b'p' {
        return Some("video/mp4");
    }

    None
}

/// Validate image dimensions and size
///
/// ATProto has limits on image sizes for different contexts.
///
/// # Arguments
///
/// * `size_bytes` - Size of the image in bytes
/// * `max_size_bytes` - Maximum allowed size in bytes (default: 1MB for posts)
///
/// # Returns
///
/// Returns Ok(()) if valid, or Err with a message if invalid.
pub fn validate_blob_size(size_bytes: usize, max_size_bytes: usize) -> Result<(), String> {
    if size_bytes > max_size_bytes {
        Err(format!(
            "Blob size {} bytes exceeds maximum {} bytes",
            size_bytes, max_size_bytes
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_mime_type_jpeg() {
        assert_eq!(detect_mime_type("photo.jpg"), "image/jpeg");
        assert_eq!(detect_mime_type("photo.jpeg"), "image/jpeg");
        assert_eq!(detect_mime_type("PHOTO.JPG"), "image/jpeg");
    }

    #[test]
    fn test_detect_mime_type_png() {
        assert_eq!(detect_mime_type("image.png"), "image/png");
        assert_eq!(detect_mime_type("IMAGE.PNG"), "image/png");
    }

    #[test]
    fn test_detect_mime_type_gif() {
        assert_eq!(detect_mime_type("animated.gif"), "image/gif");
    }

    #[test]
    fn test_detect_mime_type_webp() {
        assert_eq!(detect_mime_type("modern.webp"), "image/webp");
    }

    #[test]
    fn test_detect_mime_type_mp4() {
        assert_eq!(detect_mime_type("video.mp4"), "video/mp4");
    }

    #[test]
    fn test_detect_mime_type_unknown() {
        assert_eq!(detect_mime_type("file.xyz"), "application/octet-stream");
        assert_eq!(detect_mime_type("noext"), "application/octet-stream");
    }

    #[test]
    fn test_detect_mime_type_from_data_jpeg() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(detect_mime_type_from_data(&jpeg), Some("image/jpeg"));
    }

    #[test]
    fn test_detect_mime_type_from_data_png() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_mime_type_from_data(&png), Some("image/png"));
    }

    #[test]
    fn test_detect_mime_type_from_data_gif() {
        let gif = b"GIF89a".to_vec();
        assert_eq!(detect_mime_type_from_data(&gif), Some("image/gif"));
    }

    #[test]
    fn test_detect_mime_type_from_data_webp() {
        let webp = vec![
            b'R', b'I', b'F', b'F',
            0x00, 0x00, 0x00, 0x00,
            b'W', b'E', b'B', b'P'
        ];
        assert_eq!(detect_mime_type_from_data(&webp), Some("image/webp"));
    }

    #[test]
    fn test_detect_mime_type_from_data_too_short() {
        let short = vec![0xFF];
        assert_eq!(detect_mime_type_from_data(&short), None);
    }

    #[test]
    fn test_detect_mime_type_from_data_unknown() {
        let unknown = vec![0x00, 0x00, 0x00, 0x00];
        assert_eq!(detect_mime_type_from_data(&unknown), None);
    }

    #[test]
    fn test_validate_blob_size_ok() {
        assert!(validate_blob_size(500_000, 1_000_000).is_ok());
    }

    #[test]
    fn test_validate_blob_size_too_large() {
        let result = validate_blob_size(2_000_000, 1_000_000);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum"));
    }

    #[test]
    fn test_validate_blob_size_exact_max() {
        assert!(validate_blob_size(1_000_000, 1_000_000).is_ok());
    }
}
