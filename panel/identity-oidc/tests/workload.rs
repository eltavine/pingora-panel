#![forbid(unsafe_code)]

//! Exchanging a workload's token for a short session of a service account.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use identity_oidc::{testing::TestProvider, OidcClient};
use panel_context::{RequestId, RequestScope};
use panel_errors::ErrorCode;
use panel_identity::{
    memory::MemoryIdentityStore,
    store::{Cause, NewAccount},
    AccountId, Client, IdentityStore, Transport, Username, WorkloadIdentity, WorkloadRequest,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("req-1").unwrap())
}

#[tokio::test]
async fn workloads_prove_what_they_are_with_their_issuers_tokens() {
    let ci = TestProvider::start("ci", None).await;
    let impostor = TestProvider::start("ci", None).await;
    let store = Arc::new(MemoryIdentityStore::default());
    let account = AccountId::generate();
    store
        .create_account(
            NewAccount {
                id: account,
                username: Username::new("deployer").unwrap(),
                display_name: None,
                password_hash: None,
                roles: vec!["operator".into()],
                now: Utc::now(),
                first: false,
                service: true,
            },
            &Cause {
                scope: scope(),
                actor: "root".into(),
            },
        )
        .await
        .unwrap();
    let client = Arc::new(OidcClient::new(Duration::from_secs(5)).unwrap());
    let workloads = WorkloadIdentity::new(store.clone(), store.clone(), client);
    workloads
        .put(
            "shop",
            WorkloadRequest {
                account,
                issuer: ci.issuer.clone(),
                audience: "pingora-panel".into(),
                subject: "repo:shop/site:*".into(),
                claims: [("repository".to_owned(), "shop/site".to_owned())].into(),
                session_minutes: 10,
                enabled: true,
            },
            &scope(),
            "root",
        )
        .await
        .unwrap();
    let job = json!({
        "sub": "repo:shop/site:ref:refs/heads/main",
        "aud": "pingora-panel",
        "repository": "shop/site",
    });
    let browser = Client::default();
    let exchange = |token: String| {
        let workloads = workloads.clone();
        let browser = browser.clone();
        async move { workloads.exchange(&token, &browser, &scope()).await }
    };

    let login = exchange(ci.workload_token(job.clone())).await.unwrap();
    assert_eq!(login.account.id, account);
    assert_eq!(login.session.transport, Transport::Bearer);

    let code = |result: panel_errors::Result<_>| result.unwrap_err().code.as_str().to_owned();
    let mut elsewhere = job.clone();
    elsewhere["aud"] = json!("another-panel");
    assert_eq!(
        code(exchange(ci.workload_token(elsewhere)).await),
        ErrorCode::PERMISSION_DENIED
    );
    let mut stale = job.clone();
    stale["exp"] = json!(Utc::now().timestamp() - 3600);
    assert_eq!(
        code(exchange(ci.workload_token(stale)).await),
        ErrorCode::UNAUTHENTICATED
    );
    let token = ci.workload_token(job.clone());
    let mut parts: Vec<String> = token.split('.').map(str::to_owned).collect();
    let mut claims: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&parts[1]).unwrap()).unwrap();
    claims["sub"] = json!("repo:shop/site:ref:refs/heads/attacker");
    parts[1] = URL_SAFE_NO_PAD.encode(claims.to_string());
    assert_eq!(
        code(exchange(parts.join(".")).await),
        ErrorCode::UNAUTHENTICATED,
        "a changed token no longer verifies"
    );
    assert_eq!(
        code(exchange(impostor.workload_token(job.clone())).await),
        ErrorCode::UNAUTHENTICATED,
        "nobody trusts the other issuer"
    );
    let mut forged = job.clone();
    forged["iss"] = json!(ci.issuer);
    assert_eq!(
        code(exchange(impostor.workload_token(forged)).await),
        ErrorCode::UNAUTHENTICATED,
        "naming a trusted issuer is not enough without its key"
    );
}
