//! The OAuth authorization server and resource-server checks (proto-blue's
//! OAuth is client-only), following the reference's `@atproto/oauth-provider`.

pub mod dpop;
pub mod error;
pub mod jose;
pub mod metadata;

pub use error::OAuthError;
