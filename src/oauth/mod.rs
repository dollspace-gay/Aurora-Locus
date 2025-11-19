//! OAuth 2.1 Implementation for ATProto
//!
//! This module implements the OAuth 2.1 specification with ATProto extensions:
//! - DPoP (Demonstrating Proof-of-Possession) token binding
//! - PKCE (Proof Key for Code Exchange) authorization flow
//! - Multi-device support with device management
//! - Refresh token rotation with replay detection
//!
//! References:
//! - https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1
//! - https://atproto.com/specs/oauth

pub mod authorize;
pub mod client;
pub mod consent;
pub mod device;
pub mod models;
pub mod scope;
pub mod token;
pub mod token_rotation;

pub use authorize::authorize;
pub use client::ClientManager;
pub use consent::{
    consent_screen, deny_authorization, grant_authorization,
};
pub use device::DeviceManager;
pub use scope::{
    require_scope, AtProtoScope,
    ScopeSet,
};
pub use token::token_endpoint;
