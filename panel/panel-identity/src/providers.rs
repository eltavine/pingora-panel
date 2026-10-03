//! Managing identity providers: validated, checked against the provider
//! before they are enabled, with the client secret sealed and never shown.

use crate::{
    service::{cause, ensure_managers, role_permissions},
    ClaimNames, GroupRole, IdentityProvider, IdentityStore, OpenIdConnect, PasswordSignIn,
    ProviderSettings, ProviderStore,
};
use chrono::{DateTime, Utc};
use panel_context::RequestScope;
use panel_errors::{PanelError, Result};
use panel_secrets::{Sealed, SecretVault};
use std::sync::Arc;

const MAX_TEXT: usize = 256;

/// What happens to the client secret.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum SecretChange {
    /// Keeps the secret a provider already has.
    #[default]
    Keep,
    Set(String),
    /// Makes the client public: it relies on PKCE alone.
    Clear,
}

/// A provider as an Administrator writes it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderRequest {
    pub display_name: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: SecretChange,
    pub scopes: Vec<String>,
    pub claims: ClaimNames,
    pub group_roles: Vec<GroupRole>,
    pub create_accounts: bool,
    pub enabled: bool,
}

/// A provider as Administrators see it: everything but the secret.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderView {
    pub id: String,
    pub display_name: String,
    pub issuer: String,
    pub client_id: String,
    pub has_client_secret: bool,
    pub scopes: Vec<String>,
    pub claims: ClaimNames,
    pub group_roles: Vec<GroupRole>,
    pub create_accounts: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&IdentityProvider> for ProviderView {
    fn from(provider: &IdentityProvider) -> Self {
        Self {
            id: provider.id.clone(),
            display_name: provider.display_name.clone(),
            issuer: provider.issuer.clone(),
            client_id: provider.client_id.clone(),
            has_client_secret: provider.client_secret.is_some(),
            scopes: provider.scopes.clone(),
            claims: provider.claims.clone(),
            group_roles: provider.group_roles.clone(),
            create_accounts: provider.create_accounts,
            enabled: provider.enabled,
            created_at: provider.created_at,
            updated_at: provider.updated_at,
        }
    }
}

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Identity providers and what signing in with them needs.
#[derive(Clone)]
pub struct ProviderDirectory {
    providers: Arc<dyn ProviderStore>,
    identity: Arc<dyn IdentityStore>,
    vault: Arc<dyn SecretVault>,
    connect: Arc<dyn OpenIdConnect>,
    clock: Clock,
}

fn secret_owner(id: &str) -> String {
    format!("identity-provider/{id}/client-secret")
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn text(value: &str, name: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        Err(PanelError::invalid_argument(format!(
            "the {name} must be 1 to {max} printable characters"
        )))
    } else {
        Ok(())
    }
}

impl ProviderDirectory {
    pub fn new(
        providers: Arc<dyn ProviderStore>,
        identity: Arc<dyn IdentityStore>,
        vault: Arc<dyn SecretVault>,
        connect: Arc<dyn OpenIdConnect>,
    ) -> Self {
        Self {
            providers,
            identity,
            vault,
            connect,
            clock: Arc::new(Utc::now),
        }
    }

    pub async fn list(&self) -> Result<Vec<ProviderView>> {
        Ok(self
            .providers
            .providers()
            .await?
            .iter()
            .map(ProviderView::from)
            .collect())
    }

    pub async fn get(&self, id: &str) -> Result<ProviderView> {
        Ok(ProviderView::from(&self.stored(id).await?))
    }

    pub(crate) async fn stored(&self, id: &str) -> Result<IdentityProvider> {
        self.providers
            .provider(id)
            .await?
            .ok_or_else(|| PanelError::not_found(format!("there is no identity provider {id}")))
    }

    /// Creates or replaces a provider; returns it and whether it was created.
    pub async fn put(
        &self,
        id: &str,
        request: ProviderRequest,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<(ProviderView, bool)> {
        if !is_slug(id) {
            return Err(PanelError::invalid_argument(
                "the identifier must be lowercase letters, digits and inner hyphens, up to 64",
            ));
        }
        self.validate(&request).await?;
        let existing = self.providers.provider(id).await?;
        let client_secret = match request.client_secret {
            SecretChange::Keep => existing
                .as_ref()
                .and_then(|provider| provider.client_secret.clone()),
            SecretChange::Clear => None,
            SecretChange::Set(secret) => {
                text(&secret, "client secret", 4096)?;
                Some(
                    self.vault
                        .seal(&secret_owner(id), secret.as_bytes())
                        .await?
                        .as_str()
                        .to_owned(),
                )
            }
        };
        if request.enabled {
            self.connect.check(&request.issuer).await.map_err(|error| {
                PanelError::validation_failed(format!(
                    "the provider at {} cannot be used: {}",
                    request.issuer, error.message
                ))
            })?;
        }
        let now = (self.clock)();
        let mut group_roles = request.group_roles;
        group_roles.sort();
        group_roles.dedup();
        let provider = IdentityProvider {
            id: id.to_owned(),
            display_name: request.display_name.trim().to_owned(),
            issuer: request.issuer,
            client_id: request.client_id,
            client_secret,
            scopes: request.scopes,
            claims: request.claims,
            group_roles,
            create_accounts: request.create_accounts,
            enabled: request.enabled,
            created_at: existing.map_or(now, |provider| provider.created_at),
            updated_at: now,
        };
        let created = self
            .providers
            .put_provider(provider.clone(), &cause(scope, actor))
            .await?;
        Ok((ProviderView::from(&provider), created))
    }

    /// Who may sign in with a password.
    pub async fn password_sign_in(&self) -> Result<PasswordSignIn> {
        self.identity.password_sign_in().await
    }

    /// Limits password sign-in to break-glass accounts, or opens it to
    /// everyone again. Limiting it needs an enabled provider for everyone
    /// else, and an enabled break-glass account that can manage accounts.
    pub async fn set_password_sign_in(
        &self,
        policy: PasswordSignIn,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<()> {
        if policy == PasswordSignIn::BreakGlassOnly {
            let providers = self.providers.providers().await?;
            if !providers.iter().any(|provider| provider.enabled) {
                return Err(PanelError::precondition_failed(
                    "enable an identity provider before limiting password sign-in",
                ));
            }
            let permissions = role_permissions(self.identity.as_ref()).await?;
            let break_glass: Vec<(bool, Vec<String>)> = self
                .identity
                .accounts()
                .await?
                .into_iter()
                .filter(|account| account.break_glass)
                .map(|account| (account.disabled, account.roles))
                .collect();
            ensure_managers(&break_glass, &permissions).map_err(|_| {
                PanelError::precondition_failed(
                    "mark an enabled account that can manage accounts as break-glass first",
                )
            })?;
        }
        self.identity
            .set_password_sign_in(policy, (self.clock)(), &cause(scope, actor))
            .await
    }

    pub async fn delete(&self, id: &str, scope: &RequestScope, actor: &str) -> Result<()> {
        self.providers
            .delete_provider(id, &cause(scope, actor))
            .await
    }

    /// The settings to talk to `provider` with, its secret opened.
    pub(crate) async fn settings(
        &self,
        provider: &IdentityProvider,
        redirect_uri: String,
    ) -> Result<ProviderSettings> {
        let client_secret = match &provider.client_secret {
            Some(sealed) => {
                let opened = self
                    .vault
                    .open(&secret_owner(&provider.id), &Sealed::new(sealed.clone()))
                    .await?;
                Some(
                    String::from_utf8(opened.to_vec())
                        .map_err(|_| PanelError::corrupt_state("the client secret is not text"))?,
                )
            }
            None => None,
        };
        Ok(ProviderSettings {
            issuer: provider.issuer.clone(),
            client_id: provider.client_id.clone(),
            client_secret,
            scopes: provider.scopes.clone(),
            redirect_uri,
        })
    }

    async fn validate(&self, request: &ProviderRequest) -> Result<()> {
        text(&request.display_name, "display name", 64)?;
        text(&request.client_id, "client ID", MAX_TEXT)?;
        let issuer = &request.issuer;
        if issuer.len() > 512
            || !(issuer.starts_with("https://") || issuer.starts_with("http://"))
            || issuer.ends_with('/') && issuer.matches('/').count() == 3
        {
            return Err(PanelError::invalid_argument(
                "the issuer must be the provider's HTTPS URL, exactly as its tokens name it",
            ));
        }
        if request.scopes.len() > 32
            || request.scopes.iter().any(|scope| {
                scope.is_empty()
                    || scope.len() > 64
                    || !scope
                        .bytes()
                        .all(|byte| byte.is_ascii_graphic() && byte != b'"' && byte != b'\\')
            })
        {
            return Err(PanelError::invalid_argument(
                "scopes are up to 32 words of visible characters",
            ));
        }
        let claims = &request.claims;
        for (value, name) in [
            (&claims.username, "username claim"),
            (&claims.display_name, "display name claim"),
            (&claims.email, "email claim"),
            (&claims.groups, "groups claim"),
        ] {
            text(value, name, 64)?;
        }
        if request.group_roles.len() > 256 {
            return Err(PanelError::invalid_argument(
                "a provider maps at most 256 groups",
            ));
        }
        let roles = self.identity.roles().await?;
        for mapping in &request.group_roles {
            text(&mapping.group, "group", MAX_TEXT)?;
            if !roles.iter().any(|role| role.id == mapping.role) {
                return Err(PanelError::invalid_argument(format!(
                    "there is no role {:?}",
                    mapping.role
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{memory::MemoryIdentityStore, Refreshed, SignInRequest, SignedIn};
    use async_trait::async_trait;
    use panel_secrets::EnvelopeVault;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Provider {
        checked: Mutex<Vec<String>>,
        reachable: bool,
    }

    #[async_trait]
    impl OpenIdConnect for Provider {
        async fn check(&self, issuer: &str) -> Result<()> {
            self.checked.lock().unwrap().push(issuer.into());
            if self.reachable {
                Ok(())
            } else {
                Err(PanelError::unavailable("unreachable"))
            }
        }

        async fn sign_in_request(&self, _settings: &ProviderSettings) -> Result<SignInRequest> {
            unimplemented!()
        }

        async fn complete(
            &self,
            _settings: &ProviderSettings,
            _code: &str,
            _verifier: &str,
            _nonce: &str,
        ) -> Result<SignedIn> {
            unimplemented!()
        }

        async fn refresh(&self, _settings: &ProviderSettings, _token: &str) -> Result<Refreshed> {
            unimplemented!()
        }
    }

    fn directory(reachable: bool) -> (ProviderDirectory, Arc<MemoryIdentityStore>) {
        let store = Arc::new(MemoryIdentityStore::default());
        let vault = EnvelopeVault::from_keys(&EnvelopeVault::generate_key().unwrap()).unwrap();
        let directory = ProviderDirectory::new(
            store.clone(),
            store.clone(),
            Arc::new(vault),
            Arc::new(Provider {
                reachable,
                ..Provider::default()
            }),
        );
        (directory, store)
    }

    fn request() -> ProviderRequest {
        ProviderRequest {
            display_name: "Corporate sign-in".into(),
            issuer: "https://id.example".into(),
            client_id: "panel".into(),
            client_secret: SecretChange::Set("s3cret".into()),
            scopes: vec!["profile".into()],
            claims: ClaimNames::default(),
            group_roles: vec![GroupRole {
                group: "ops".into(),
                role: "operator".into(),
            }],
            create_accounts: true,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn providers_keep_their_secret_sealed() {
        let (directory, store) = directory(true);
        let scope = RequestScope::new(panel_context::RequestId::new("req-1").unwrap());
        let (view, created) = directory
            .put("corp", request(), &scope, "admin")
            .await
            .unwrap();
        assert!(created && view.has_client_secret);
        let stored = store.provider("corp").await.unwrap().unwrap();
        assert!(!stored.client_secret.as_ref().unwrap().contains("s3cret"));
        let settings = directory
            .settings(&stored, "https://panel.example/cb".into())
            .await
            .unwrap();
        assert_eq!(settings.client_secret.as_deref(), Some("s3cret"));

        let mut kept = request();
        kept.client_secret = SecretChange::Keep;
        kept.display_name = "Corp".into();
        let (view, created) = directory.put("corp", kept, &scope, "admin").await.unwrap();
        assert!(!created && view.has_client_secret);
        let mut public = request();
        public.client_secret = SecretChange::Clear;
        assert!(
            !directory
                .put("corp", public, &scope, "admin")
                .await
                .unwrap()
                .0
                .has_client_secret
        );

        let events: Vec<String> = store
            .events()
            .into_iter()
            .map(|event| event.event_type)
            .collect();
        assert_eq!(
            events,
            [
                "identity.provider.created",
                "identity.provider.updated",
                "identity.provider.updated"
            ]
        );
        directory.delete("corp", &scope, "admin").await.unwrap();
        assert!(directory.get("corp").await.is_err());
    }

    #[tokio::test]
    async fn providers_are_validated_and_checked_before_they_are_enabled() {
        let (directory, _) = directory(false);
        let scope = RequestScope::new(panel_context::RequestId::new("req-1").unwrap());
        assert!(directory
            .put("corp", request(), &scope, "admin")
            .await
            .is_err());
        let mut disabled = request();
        disabled.enabled = false;
        assert!(directory
            .put("corp", disabled, &scope, "admin")
            .await
            .is_ok());
        for (id, change) in [
            ("Corp", None),
            ("corp", Some(("issuer", "id.example"))),
            ("corp", Some(("role", "nobody"))),
            ("corp", Some(("scope", "a b"))),
        ] {
            let mut bad = request();
            bad.enabled = false;
            match change {
                Some(("issuer", value)) => bad.issuer = value.into(),
                Some(("role", value)) => bad.group_roles[0].role = value.into(),
                Some(("scope", value)) => bad.scopes = vec![value.into()],
                _ => {}
            }
            assert!(
                directory.put(id, bad, &scope, "admin").await.is_err(),
                "{id} {change:?}"
            );
        }
    }
}
