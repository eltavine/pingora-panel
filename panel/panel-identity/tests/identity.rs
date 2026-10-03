#![forbid(unsafe_code)]

use chrono::{DateTime, Duration, Utc};
use panel_context::{RequestId, RequestScope};
use panel_errors::ErrorCode;
use panel_identity::{
    memory::MemoryIdentityStore, AccountChange, AccountRequest, Client, FailurePolicy, Identity,
    IdentitySettings, Permission, PermissionSet, Principal, SecretHash, TokenRequest, Transport,
};
use std::sync::{Arc, Mutex};

const PASSWORD: &str = "glacier violin tapestry orbit";
const BOOTSTRAP: &str = "bootstrap-token-for-tests";

struct Harness {
    identity: Identity,
    store: Arc<MemoryIdentityStore>,
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl Harness {
    fn new(settings: IdentitySettings) -> Self {
        let store = Arc::new(MemoryIdentityStore::default());
        let now = Arc::new(Mutex::new(
            DateTime::parse_from_rfc3339("2026-10-03T08:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ));
        let clock = Arc::clone(&now);
        let identity = Identity::new(
            store.clone(),
            IdentitySettings {
                bootstrap: Some(SecretHash::of(BOOTSTRAP)),
                ..settings
            },
        )
        .with_clock(move || *clock.lock().unwrap());
        Self {
            identity,
            store,
            now,
        }
    }

    fn advance(&self, by: Duration) {
        *self.now.lock().unwrap() += by;
    }

    async fn admin(&self) -> panel_identity::Account {
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

    async fn login(
        &self,
        username: &str,
        transport: Transport,
    ) -> (panel_identity::Login, Principal) {
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

    fn events(&self, event_type: &str) -> Vec<serde_json::Value> {
        self.store
            .events()
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

#[tokio::test]
async fn the_first_account_needs_the_bootstrap_token_and_gets_every_permission() {
    let harness = Harness::new(IdentitySettings::default());
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
    assert_eq!(harness.events("identity.account.created").len(), 1);
}

#[tokio::test]
async fn sessions_carry_their_transport_and_csrf_token_and_end() {
    let harness = Harness::new(IdentitySettings::default());
    harness.admin().await;
    let (login, principal) = harness.login("root", Transport::Cookie).await;
    assert!(principal.csrf_matches(Some(login.csrf.expose())));
    assert!(!principal.csrf_matches(None));
    assert_eq!(principal.actor(), "root");
    assert!(harness
        .identity
        .authenticate_session(login.secret.expose(), Transport::Bearer)
        .await
        .unwrap()
        .is_none());
    let succeeded = harness.events("identity.login.succeeded");
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
        harness.events("identity.session.ended")[0]["reason"],
        "logout"
    );
}

#[tokio::test]
async fn failed_logins_wait_longer_and_finally_lock_the_password() {
    let harness = Harness::new(IdentitySettings {
        failures: FailurePolicy {
            free: 2,
            disable_after: 5,
            ..FailurePolicy::default()
        },
        ..IdentitySettings::default()
    });
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
    let failed = harness.events("identity.login.failed");
    assert_eq!(failed.last().unwrap()["reason"], "locked");
    assert!(failed.iter().any(|event| event["locked"] == true));

    let unknown = harness
        .identity
        .login("nobody", PASSWORD, Transport::Cookie, &client(), &scope())
        .await
        .unwrap_err();
    assert_eq!(unknown.message, "invalid username or password");
    assert_eq!(
        harness.events("identity.login.failed").last().unwrap()["reason"],
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

#[tokio::test]
async fn changing_a_password_ends_the_other_sessions() {
    let harness = Harness::new(IdentitySettings::default());
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
    assert_eq!(harness.events("identity.password.changed").len(), 1);
}

#[tokio::test]
async fn tokens_never_exceed_their_owner() {
    let harness = Harness::new(IdentitySettings::default());
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

#[tokio::test]
async fn the_last_account_manager_cannot_be_disabled_and_disabling_ends_access() {
    let harness = Harness::new(IdentitySettings::default());
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
