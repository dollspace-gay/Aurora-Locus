//! Constants used throughout the ATProto SDK

/// The DID of the official Bluesky labeler service
///
/// This is the default labeling service used by Bluesky to moderate content.
/// The labeler is responsible for applying labels to content based on community
/// standards and moderation policies.
pub const BSKY_LABELER_DID: &str = "did:plc:ar7c4by46qjdydhdevvrndac";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::is_did;

    #[test]
    fn test_bsky_labeler_did_is_valid() {
        // Ensure the constant is a valid DID
        assert!(is_did(BSKY_LABELER_DID));
    }

    #[test]
    fn test_bsky_labeler_did_value() {
        // Ensure the constant has the expected value
        assert_eq!(BSKY_LABELER_DID, "did:plc:ar7c4by46qjdydhdevvrndac");
    }

    #[test]
    fn test_bsky_labeler_did_format() {
        // Verify it's a PLC DID
        assert!(BSKY_LABELER_DID.starts_with("did:plc:"));

        // Verify length is reasonable
        assert!(BSKY_LABELER_DID.len() > 8);
        assert!(BSKY_LABELER_DID.len() < 2048);
    }

    #[test]
    fn test_bsky_labeler_did_can_be_used_as_did() {
        use crate::types::Did;

        // Should be able to create a Did from this constant
        let did = Did::new(BSKY_LABELER_DID).unwrap();
        assert_eq!(did.as_str(), BSKY_LABELER_DID);
    }
}
