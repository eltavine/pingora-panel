use panel_context::Actor;
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// The kind of principal behind an event, as defined by the CloudEvents
/// Auth Context extension (`authtype`).
///
/// The extension lets implementations add values, so unknown kinds are
/// preserved rather than rejected.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PrincipalKind(String);

impl PrincipalKind {
    pub const APP_USER: &'static str = "app_user";
    pub const USER: &'static str = "user";
    pub const SERVICE_ACCOUNT: &'static str = "service_account";
    pub const API_KEY: &'static str = "api_key";
    pub const SYSTEM: &'static str = "system";
    pub const UNAUTHENTICATED: &'static str = "unauthenticated";
    pub const UNKNOWN: &'static str = "unknown";

    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let valid = (1..=32).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        if !valid {
            return Err(PanelError::invalid_argument(
                "principal kinds must contain 1..=32 lowercase ASCII letters, digits or '_'",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn known(value: &'static str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for PrincipalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PrincipalKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// The principal accountable for an occurrence (`authtype` and `authid`).
///
/// `id` must be an opaque, stable identifier rather than personal data such
/// as an e-mail address, because event attributes are routinely logged.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Principal {
    kind: PrincipalKind,
    id: Option<Actor>,
}

impl Principal {
    pub fn new(kind: PrincipalKind, id: Option<Actor>) -> Self {
        Self { kind, id }
    }

    pub fn user(id: Actor) -> Self {
        Self::new(PrincipalKind::known(PrincipalKind::USER), Some(id))
    }

    pub fn service_account(id: Actor) -> Self {
        Self::new(
            PrincipalKind::known(PrincipalKind::SERVICE_ACCOUNT),
            Some(id),
        )
    }

    pub fn api_key(id: Actor) -> Self {
        Self::new(PrincipalKind::known(PrincipalKind::API_KEY), Some(id))
    }

    /// A platform component acting on its own behalf.
    pub fn system(component: Actor) -> Self {
        Self::new(PrincipalKind::known(PrincipalKind::SYSTEM), Some(component))
    }

    pub fn unauthenticated() -> Self {
        Self::new(PrincipalKind::known(PrincipalKind::UNAUTHENTICATED), None)
    }

    pub fn unknown() -> Self {
        Self::new(PrincipalKind::known(PrincipalKind::UNKNOWN), None)
    }

    pub fn kind(&self) -> &PrincipalKind {
        &self.kind
    }

    pub fn id(&self) -> Option<&Actor> {
        self.id.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_kinds_and_extensions_are_accepted() {
        assert_eq!(
            Principal::system(Actor::new("automation-service").unwrap())
                .kind()
                .as_str(),
            "system"
        );
        assert!(Principal::unauthenticated().id().is_none());
        assert!(PrincipalKind::new("workload_identity").is_ok());
        assert!(PrincipalKind::new("Service-Account").is_err());
        assert!(PrincipalKind::new("").is_err());
    }
}
