#![forbid(unsafe_code)]

//! Who may use the management API and what they may do: accounts and
//! their passwords, login sessions, API tokens, the permission catalog and
//! roles, and the decisions that follow from them. Storage sits behind
//! [`IdentityStore`]; [`memory::MemoryIdentityStore`] keeps it in memory for
//! tests.

mod account;
#[cfg(feature = "test-support")]
pub mod conformance;
pub mod memory;
mod password;
mod permission;
mod principal;
mod provider;
mod secret;
mod service;
mod session;
pub mod store;
mod throttle;

pub use account::{Account, AccountId, Username};
pub use password::{PasswordHasher, PasswordPolicy, PasswordProblem, Verification};
pub use permission::{built_in_roles, Permission, PermissionSet, Role};
pub use principal::{Credential, Principal};
pub use provider::{OpenIdConnect, ProviderSettings, Refreshed, SignInRequest, SignedIn};
pub use secret::{csrf_token, Secret, SecretHash, TOKEN_PREFIX};
pub use service::{
    AccountRequest, Client, Identity, IdentitySettings, Login, RoleRequest, TokenRequest,
};
pub use session::{ApiToken, Session, SessionId, SessionPolicy, TokenId, Transport};
pub use store::{AccountChange, Cause, IdentityStore};
pub use throttle::FailurePolicy;
