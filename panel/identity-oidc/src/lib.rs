#![forbid(unsafe_code)]

//! OpenID Connect sign-in for the panel (ADR 0018): provider discovery, the
//! authorization code flow with PKCE, and ID token validation.

mod client;
mod http;
pub mod jose;
#[cfg(feature = "test-support")]
pub mod testing;

pub use client::{Metadata, OidcClient};
