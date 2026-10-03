#![forbid(unsafe_code)]

//! OpenID Connect sign-in for the panel (ADR 0018): provider discovery, the
//! authorization code flow with PKCE, and ID token validation.

pub mod jose;
