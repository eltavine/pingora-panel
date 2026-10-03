//! Checks an [`IdentityStore`] against the identity rules: each scenario
//! runs through [`Identity`] on an empty store with a clock it moves, and
//! reads back the events the store recorded.

use crate::{
    memory::RecordedEvent, Account, AccountChange, AccountRequest, Client, FailurePolicy, Identity,
    IdentitySettings, IdentityStore, Login, Permission, PermissionSet, Principal, RoleRequest,
    SecretHash, TokenRequest, Transport,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use panel_context::{RequestId, RequestScope};
use panel_errors::ErrorCode;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};

/// A store being checked, and the events it recorded.
#[async_trait]
pub trait StoreUnderTest: Send + Sync {
    fn store(&self) -> Arc<dyn IdentityStore>;

    async fn events(&self) -> Vec<RecordedEvent>;
}

/// Runs every scenario, each on a store `fresh` makes empty.
pub async fn check<S, F, Fut>(mut fresh: F)
where
    S: StoreUnderTest,
    F: FnMut() -> Fut,
    Fut: Future<Output = S>,
{
    the_first_account_needs_the_bootstrap_token_and_gets_every_permission(fresh().await).await;
    sessions_carry_their_transport_and_csrf_token_and_end(fresh().await).await;
    failed_logins_wait_longer_and_finally_lock_the_password(fresh().await).await;
    changing_a_password_ends_the_other_sessions(fresh().await).await;
    tokens_never_exceed_their_owner(fresh().await).await;
    the_last_account_manager_cannot_be_disabled_and_disabling_ends_access(fresh().await).await;
    custom_roles_grant_what_they_list_and_built_in_roles_stay(fresh().await).await;
    every_other_session_of_an_account_can_end_at_once(fresh().await).await;
    rotated_tokens_keep_their_grant_and_stop_the_old_secret(fresh().await).await;
}

const PASSWORD: &str = "glacier violin tapestry orbit";
const BOOTSTRAP: &str = "bootstrap-token-for-tests";

struct Harness<S> {
    identity: Identity,
    subject: S,
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl<S: StoreUnderTest> Harness<S> {
    fn new(subject: S, settings: IdentitySettings) -> Self {
        let now = Arc::new(Mutex::new(
            DateTime::parse_from_rfc3339("2026-10-03T08:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ));
        let clock = Arc::clone(&now);
        let identity = Identity::new(
            subject.store(),
            IdentitySettings {
                bootstrap: Some(SecretHash::of(BOOTSTRAP)),
                ..settings
            },
        )
        .with_clock(move || *clock.lock().unwrap());
        Self {
            identity,
            subject,
            now,
        }
    }

    fn advance(&self, by: Duration) {
        *self.now.lock().unwrap() += by;
    }

    async fn admin(&self) -> Account {
        self.identity
            .setup(
                BOOTSTRAP,
                AccountRequest {
                    username: "Root".into(),
                    password: Some(PASSWORD.into()),
                    ..AccountRequest::default()
                },
                &scope(),
            )
            .await
            .unwrap()
    }

    async fn login(&self, username: &str, transport: Transport) -> (Login, Principal) {
        let login = self
            .identity
            .login(username, PASSWORD, transport, &client(), &scope())
            .await
            .unwrap();
        let principal = self
            .identity
            .authenticate_session(login.secret.expose(), transport)
            .await
            .unwrap()
            .unwrap();
        (login, principal)
    }

    async fn events(&self, event_type: &str) -> Vec<serde_json::Value> {
        self.subject
            .events()
            .await
            .into_iter()
            .filter(|event| event.event_type == event_type)
            .map(|event| event.data)
            .collect()
    }
}

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("req-1").unwrap())
}

fn client() -> Client {
    Client {
        address: Some("192.0.2.7".into()),
        user_agent: Some("tests".into()),
    }
}

async fn the_first_account_needs_the_bootstrap_token_and_gets_every_permission(
    subject: impl StoreUnderTest,
) {
    let harness = Harness::new(subject, IdentitySettings::default());
    assert!(harness.identity.setup_required().await.unwrap());
    let request = |password: &str| AccountRequest {
        username: "root".into(),
        password: Some(password.into()),
        ..AccountRequest::default()
    };
    let refused = harness
        .identity
        .setup("wrong", request(PASSWORD), &scope())
        .await
        .unwrap_err();
    assert_eq!(refused.code.as_str(), ErrorCode::PERMISSION_DENIED);
    let weak = harness
        .identity
        .setup(BOOTSTRAP, request("rootrootrootroot"), &scope())
        .await
        .unwrap_err();
    assert_eq!(weak.code.as_str(), ErrorCode::VALIDATION_FAILED);
    assert_eq!(weak.diagnostics[0].resource_id.as_deref(), Some("password"));

    let admin = harness.admin().await;
    assert_eq!(admin.username.as_str(), "root");
    assert_eq!(admin.roles, ["administrator"]);
    assert!(!harness.identity.setup_required().await.unwrap());
    let again = harness
        .identity
        .setup(BOOTSTRAP, request(PASSWORD), &scope())
        .await
        .unwrap_err();
    assert_eq!(again.code.as_str(), ErrorCode::CONFLICT);

    let (_, principal) = harness.login("ROOT", Transport::Cookie).await;
    assert_eq!(principal.permissions, PermissionSet::all());
    assert_eq!(harness.events("identity.account.created").await.len(), 1);
}

async fn sessions_carry_their_transport_and_csrf_token_and_end(subject: impl StoreUnderTest) {
    let harness = Harness::new(subject, IdentitySettings::default());
    harness.admin().await;
    let (login, principal) = harness.login("root", Transport::Cookie).await;
    assert!(principal.csrf_matches(Some(&login.csrf)));
    assert_eq!(principal.csrf_token(), Some(login.csrf.as_str()));
    assert!(!principal.csrf_matches(None));
    assert_eq!(principal.actor(), "root");
    assert!(harness
        .identity
        .authenticate_session(login.secret.expose(), Transport::Bearer)
        .await
        .unwrap()
        .is_none());
    let succeeded = harness.events("identity.login.succeeded").await;
    assert_eq!(succeeded[0]["attempt"]["client_address"], "192.0.2.7");
    assert_eq!(succeeded[0]["transport"], "cookie");

    // Activity keeps the session alive for a day, but no longer.
    for _ in 0..24 {
        harness.advance(Duration::minutes(59));
        assert!(harness
            .identity
            .authenticate_session(login.secret.expose(), Transport::Cookie)
            .await
            .unwrap()
            .is_some());
    }
    harness.advance(Duration::minutes(30));
    assert!(harness
        .identity
        .authenticate_session(login.secret.expose(), Transport::Cookie)
        .await
        .unwrap()
        .is_none());

    let (bearer, principal) = harness.login("root", Transport::Bearer).await;
    assert!(principal.csrf_matches(None));
    harness.advance(Duration::minutes(61));
    assert!(harness
        .identity
        .authenticate_session(bearer.secret.expose(), Transport::Bearer)
        .await
        .unwrap()
        .is_none());

    let (cookie, principal) = harness.login("root", Transport::Cookie).await;
    harness.identity.logout(&principal, &scope()).await.unwrap();
    assert!(harness
        .identity
        .authenticate_session(cookie.secret.expose(), Transport::Cookie)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        harness.events("identity.session.ended").await[0]["reason"],
        "logout"
    );
}

async fn failed_logins_wait_longer_and_finally_lock_the_password(subject: impl StoreUnderTest) {
    let harness = Harness::new(
        subject,
        IdentitySettings {
            failures: FailurePolicy {
                free: 2,
                disable_after: 5,
                ..FailurePolicy::default()
            },
            ..IdentitySettings::default()
        },
    );
    let root = harness.admin().await.id;
    let attempt = |password: &'static str| {
        let identity = harness.identity.clone();
        async move {
            identity
                .login("root", password, Transport::Cookie, &client(), &scope())
                .await
        }
    };
    for _ in 0..2 {
        let error = attempt("wrong password").await.unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::UNAUTHENTICATED);
    }
    attempt("wrong password").await.unwrap_err();
    // The third failure makes the account wait 30 seconds, whatever is tried.
    let waiting = attempt(PASSWORD).await.unwrap_err();
    assert_eq!(waiting.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
    assert!(
        waiting.message.contains("30 seconds"),
        "{}",
        waiting.message
    );
    harness.advance(Duration::seconds(31));
    attempt("wrong password").await.unwrap_err();
    harness.advance(Duration::seconds(31));
    assert_eq!(
        attempt(PASSWORD).await.unwrap_err().code.as_str(),
        ErrorCode::RESOURCE_EXHAUSTED
    );
    harness.advance(Duration::seconds(30));
    attempt("wrong password").await.unwrap_err();
    harness.advance(Duration::hours(2));
    let locked = attempt(PASSWORD).await.unwrap_err();
    assert_eq!(locked.code.as_str(), ErrorCode::PERMISSION_DENIED);
    let failed = harness.events("identity.login.failed").await;
    assert_eq!(failed.last().unwrap()["reason"], "locked");
    assert!(failed.iter().any(|event| event["locked"] == true));

    let unknown = harness
        .identity
        .login("nobody", PASSWORD, Transport::Cookie, &client(), &scope())
        .await
        .unwrap_err();
    assert_eq!(unknown.message, "invalid username or password");
    assert_eq!(
        harness
            .events("identity.login.failed")
            .await
            .last()
            .unwrap()["reason"],
        "unknown_account"
    );

    // Another account that can manage accounts unlocks it.
    let operator = harness
        .identity
        .create_account(
            AccountRequest {
                username: "second".into(),
                password: Some(PASSWORD.into()),
                roles: vec!["administrator".into()],
                ..AccountRequest::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap();
    harness
        .identity
        .update_account(
            root,
            AccountChange {
                unlock: true,
                ..AccountChange::default()
            },
            &scope(),
            operator.username.as_str(),
        )
        .await
        .unwrap();
    attempt(PASSWORD).await.unwrap();
}

async fn changing_a_password_ends_the_other_sessions(subject: impl StoreUnderTest) {
    let harness = Harness::new(subject, IdentitySettings::default());
    harness.admin().await;
    let (here, principal) = harness.login("root", Transport::Cookie).await;
    let (elsewhere, _) = harness.login("root", Transport::Bearer).await;
    let wrong = harness
        .identity
        .change_password(
            &principal,
            "not it",
            "lantern quarry meadow sonnet",
            &scope(),
        )
        .await
        .unwrap_err();
    assert_eq!(wrong.code.as_str(), ErrorCode::PERMISSION_DENIED);
    harness
        .identity
        .change_password(
            &principal,
            PASSWORD,
            "lantern quarry meadow sonnet",
            &scope(),
        )
        .await
        .unwrap();
    assert!(harness
        .identity
        .authenticate_session(here.secret.expose(), Transport::Cookie)
        .await
        .unwrap()
        .is_some());
    assert!(harness
        .identity
        .authenticate_session(elsewhere.secret.expose(), Transport::Bearer)
        .await
        .unwrap()
        .is_none());
    harness
        .identity
        .login(
            "root",
            "lantern quarry meadow sonnet",
            Transport::Cookie,
            &client(),
            &scope(),
        )
        .await
        .unwrap();
    assert_eq!(harness.events("identity.password.changed").await.len(), 1);
}

async fn tokens_never_exceed_their_owner(subject: impl StoreUnderTest) {
    let harness = Harness::new(subject, IdentitySettings::default());
    harness.admin().await;
    harness
        .identity
        .create_account(
            AccountRequest {
                username: "ops".into(),
                password: Some(PASSWORD.into()),
                roles: vec!["operator".into()],
                ..AccountRequest::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap();
    let (_, ops) = harness.login("ops", Transport::Bearer).await;
    let request = |permissions: Option<&[&str]>| TokenRequest {
        name: "ci".into(),
        permissions: permissions.map(|names| PermissionSet::from_names(names).unwrap()),
        lifetime: std::time::Duration::from_secs(30 * 86_400),
    };
    let denied = harness
        .identity
        .create_token(
            &ops,
            request(Some(&["config.read", "identity.manage"])),
            &scope(),
        )
        .await
        .unwrap_err();
    assert!(
        denied.message.ends_with("identity.manage"),
        "{}",
        denied.message
    );
    let (token, secret) = harness
        .identity
        .create_token(
            &ops,
            request(Some(&["config.read", "config.apply"])),
            &scope(),
        )
        .await
        .unwrap();
    assert!(secret.expose().starts_with("ppat_"));
    let principal = harness
        .identity
        .authenticate_token(secret.expose())
        .await
        .unwrap()
        .unwrap();
    assert!(principal.can(Permission::ConfigApply));
    assert!(!principal.can(Permission::ConfigWrite));
    let from_token = harness
        .identity
        .create_token(&principal, request(None), &scope())
        .await
        .unwrap_err();
    assert_eq!(from_token.code.as_str(), ErrorCode::PERMISSION_DENIED);

    // The owner losing a permission takes it from the token too.
    let ops_id = ops.account;
    harness
        .identity
        .update_account(
            ops_id,
            AccountChange {
                roles: Some(vec!["viewer".into()]),
                ..AccountChange::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap();
    let principal = harness
        .identity
        .authenticate_token(secret.expose())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(principal.permissions.names(), ["config.read"]);

    harness
        .identity
        .revoke_token(ops_id, token.id, &scope(), "ops")
        .await
        .unwrap();
    assert!(harness
        .identity
        .authenticate_token(secret.expose())
        .await
        .unwrap()
        .is_none());
    assert!(harness
        .identity
        .authenticate_token("not-a-token")
        .await
        .unwrap()
        .is_none());
    let short = harness
        .identity
        .create_token(
            &ops,
            TokenRequest {
                lifetime: std::time::Duration::from_secs(60),
                ..request(None)
            },
            &scope(),
        )
        .await
        .unwrap_err();
    assert_eq!(short.code.as_str(), ErrorCode::INVALID_ARGUMENT);
}

async fn the_last_account_manager_cannot_be_disabled_and_disabling_ends_access(
    subject: impl StoreUnderTest,
) {
    let harness = Harness::new(subject, IdentitySettings::default());
    let root = harness.admin().await;
    let last = harness
        .identity
        .update_account(
            root.id,
            AccountChange {
                disabled: Some(true),
                ..AccountChange::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap_err();
    assert_eq!(last.code.as_str(), ErrorCode::PRECONDITION_FAILED);
    let demoted = harness
        .identity
        .update_account(
            root.id,
            AccountChange {
                roles: Some(vec!["viewer".into()]),
                ..AccountChange::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap_err();
    assert_eq!(demoted.code.as_str(), ErrorCode::PRECONDITION_FAILED);

    let viewer = harness
        .identity
        .create_account(
            AccountRequest {
                username: "viewer".into(),
                password: Some(PASSWORD.into()),
                roles: vec!["viewer".into()],
                ..AccountRequest::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap();
    let (login, _) = harness.login("viewer", Transport::Cookie).await;
    assert_eq!(harness.identity.sessions(viewer.id).await.unwrap().len(), 1);
    harness
        .identity
        .update_account(
            viewer.id,
            AccountChange {
                disabled: Some(true),
                ..AccountChange::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap();
    assert!(harness
        .identity
        .authenticate_session(login.secret.expose(), Transport::Cookie)
        .await
        .unwrap()
        .is_none());
    assert!(harness
        .identity
        .sessions(viewer.id)
        .await
        .unwrap()
        .is_empty());
    let refused = harness
        .identity
        .login("viewer", PASSWORD, Transport::Cookie, &client(), &scope())
        .await
        .unwrap_err();
    assert_eq!(refused.message, "invalid username or password");

    for (request, code) in [
        (
            AccountRequest {
                username: "viewer".into(),
                ..AccountRequest::default()
            },
            ErrorCode::CONFLICT,
        ),
        (
            AccountRequest {
                username: "x".into(),
                roles: vec!["superuser".into()],
                ..AccountRequest::default()
            },
            ErrorCode::INVALID_ARGUMENT,
        ),
    ] {
        let error = harness
            .identity
            .create_account(request, &scope(), "root")
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), code);
    }
}

fn role(id: &str, permissions: &[&str]) -> RoleRequest {
    RoleRequest {
        id: id.into(),
        name: format!("Role {id}"),
        description: String::new(),
        permissions: PermissionSet::from_names(permissions).unwrap(),
    }
}

async fn custom_roles_grant_what_they_list_and_built_in_roles_stay(subject: impl StoreUnderTest) {
    let harness = Harness::new(subject, IdentitySettings::default());
    let root = harness.admin().await;
    let identity = &harness.identity;
    let created = identity
        .create_role(
            role("Deployer", &["config.read", "config.apply"]),
            &scope(),
            "root",
        )
        .await
        .unwrap();
    assert_eq!(created.id, "deployer");
    assert!(!created.built_in);
    for (request, code) in [
        (role("deployer", &["config.read"]), ErrorCode::CONFLICT),
        (
            role("no way", &["config.read"]),
            ErrorCode::INVALID_ARGUMENT,
        ),
        (role("empty", &[]), ErrorCode::INVALID_ARGUMENT),
    ] {
        let error = identity
            .create_role(request, &scope(), "root")
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), code);
    }
    let built_in = identity
        .update_role(role("administrator", &["config.read"]), &scope(), "root")
        .await
        .unwrap_err();
    assert_eq!(built_in.code.as_str(), ErrorCode::PERMISSION_DENIED);
    let missing = identity
        .update_role(role("nobody", &["config.read"]), &scope(), "root")
        .await
        .unwrap_err();
    assert_eq!(missing.code.as_str(), ErrorCode::NOT_FOUND);

    let ci = identity
        .create_account(
            AccountRequest {
                username: "ci".into(),
                password: Some(PASSWORD.into()),
                roles: vec!["deployer".into()],
                ..AccountRequest::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap();
    let (login, principal) = harness.login("ci", Transport::Bearer).await;
    assert_eq!(
        principal.permissions.names(),
        ["config.read", "config.apply"]
    );
    identity
        .update_role(role("deployer", &["config.read"]), &scope(), "root")
        .await
        .unwrap();
    let principal = identity
        .authenticate_session(login.secret.expose(), Transport::Bearer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(principal.permissions.names(), ["config.read"]);
    assert_eq!(
        identity
            .roles()
            .await
            .unwrap()
            .iter()
            .find(|role| role.id == "deployer")
            .unwrap()
            .permissions
            .names(),
        ["config.read"]
    );

    let held = identity
        .delete_role("deployer", &scope(), "root")
        .await
        .unwrap_err();
    assert_eq!(held.code.as_str(), ErrorCode::CONFLICT);
    identity
        .update_account(
            ci.id,
            AccountChange {
                roles: Some(vec!["viewer".into()]),
                ..AccountChange::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap();
    identity
        .delete_role("deployer", &scope(), "root")
        .await
        .unwrap();
    assert!(!identity
        .roles()
        .await
        .unwrap()
        .iter()
        .any(|role| role.id == "deployer"));
    let built_in = identity
        .delete_role("viewer", &scope(), "root")
        .await
        .unwrap_err();
    assert_eq!(built_in.code.as_str(), ErrorCode::PERMISSION_DENIED);

    // A custom role may be what lets the last enabled account manage accounts.
    identity
        .create_role(
            role("keeper", &["identity.read", "identity.manage"]),
            &scope(),
            "root",
        )
        .await
        .unwrap();
    identity
        .update_account(
            ci.id,
            AccountChange {
                roles: Some(vec!["keeper".into()]),
                ..AccountChange::default()
            },
            &scope(),
            "root",
        )
        .await
        .unwrap();
    identity
        .update_account(
            root.id,
            AccountChange {
                disabled: Some(true),
                ..AccountChange::default()
            },
            &scope(),
            "ci",
        )
        .await
        .unwrap();
    let stripped = identity
        .update_role(role("keeper", &["identity.read"]), &scope(), "ci")
        .await
        .unwrap_err();
    assert_eq!(stripped.code.as_str(), ErrorCode::PRECONDITION_FAILED);
    let events = harness.subject.events().await;
    for event_type in [
        "identity.role.created",
        "identity.role.updated",
        "identity.role.deleted",
    ] {
        assert!(
            events.iter().any(|event| event.event_type == event_type),
            "{event_type}"
        );
    }
}

async fn every_other_session_of_an_account_can_end_at_once(subject: impl StoreUnderTest) {
    let harness = Harness::new(subject, IdentitySettings::default());
    let root = harness.admin().await;
    let (here, principal) = harness.login("root", Transport::Cookie).await;
    let (elsewhere, _) = harness.login("root", Transport::Bearer).await;
    let (third, _) = harness.login("root", Transport::Bearer).await;
    let ended = harness
        .identity
        .end_sessions(root.id, principal.session(), &scope(), "root")
        .await
        .unwrap();
    assert_eq!(ended, 2);
    assert!(harness
        .identity
        .authenticate_session(here.secret.expose(), Transport::Cookie)
        .await
        .unwrap()
        .is_some());
    for gone in [elsewhere, third] {
        assert!(harness
            .identity
            .authenticate_session(gone.secret.expose(), Transport::Bearer)
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(harness.identity.sessions(root.id).await.unwrap().len(), 1);
    let ended = harness.events("identity.session.ended").await;
    assert_eq!(ended.last().unwrap()["sessions"], 2);
    assert_eq!(
        harness
            .identity
            .end_sessions(root.id, principal.session(), &scope(), "root")
            .await
            .unwrap(),
        0
    );
}

async fn rotated_tokens_keep_their_grant_and_stop_the_old_secret(subject: impl StoreUnderTest) {
    let harness = Harness::new(subject, IdentitySettings::default());
    harness.admin().await;
    let (_, principal) = harness.login("root", Transport::Bearer).await;
    let (token, old_secret) = harness
        .identity
        .create_token(
            &principal,
            TokenRequest {
                name: "ci".into(),
                permissions: Some(PermissionSet::from_names(&["config.read"]).unwrap()),
                lifetime: std::time::Duration::from_secs(30 * 86_400),
            },
            &scope(),
        )
        .await
        .unwrap();
    harness.advance(Duration::days(1));
    let (rotated, new_secret) = harness
        .identity
        .rotate_token(&principal, token.id, &scope())
        .await
        .unwrap();
    assert_ne!(rotated.id, token.id);
    assert_eq!(rotated.name, "ci");
    assert_eq!(rotated.permissions, token.permissions);
    assert_eq!(rotated.expires_at - rotated.created_at, Duration::days(30));
    assert!(harness
        .identity
        .authenticate_token(old_secret.expose())
        .await
        .unwrap()
        .is_none());
    let renewed = harness
        .identity
        .authenticate_token(new_secret.expose())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renewed.permissions.names(), ["config.read"]);
    let again = harness
        .identity
        .rotate_token(&principal, token.id, &scope())
        .await
        .unwrap_err();
    assert_eq!(again.code.as_str(), ErrorCode::NOT_FOUND);
    let from_token = harness
        .identity
        .rotate_token(&renewed, rotated.id, &scope())
        .await
        .unwrap_err();
    assert_eq!(from_token.code.as_str(), ErrorCode::PERMISSION_DENIED);
    let event = harness.events("identity.token.rotated").await;
    assert_eq!(event.len(), 1);
    assert_eq!(event[0]["replaces"], token.id.to_string());
}
