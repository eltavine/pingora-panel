//! A provider for tests: it signs in whoever the test chooses, checks PKCE
//! and client credentials as a real provider would, and signs ES256 ID
//! tokens with a key it generates.

use axum::{
    extract::{Form, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use percent_encoding::percent_decode_str;
use ring::{
    digest::{digest, SHA256},
    rand::{SecureRandom, SystemRandom},
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde_json::{json, Map, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpListener;

const KEY_ID: &str = "test-key";

struct Pending {
    redirect_uri: String,
    challenge: String,
    nonce: String,
    subject: String,
}

struct Provider {
    issuer: String,
    client_id: String,
    client_secret: Option<String>,
    key: EcdsaKeyPair,
    random: SystemRandom,
    users: Mutex<HashMap<String, Map<String, Value>>>,
    current: Mutex<Option<String>>,
    codes: Mutex<HashMap<String, Pending>>,
    refresh_tokens: Mutex<HashMap<String, String>>,
    disabled: Mutex<HashSet<String>>,
}

/// A running test provider.
pub struct TestProvider {
    pub issuer: String,
    provider: Arc<Provider>,
    task: tokio::task::JoinHandle<()>,
}

impl TestProvider {
    /// Starts a provider on loopback for one client.
    pub async fn start(client_id: &str, client_secret: Option<&str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
                .unwrap();
        let provider = Arc::new(Provider {
            issuer: issuer.clone(),
            client_id: client_id.into(),
            client_secret: client_secret.map(Into::into),
            key,
            random,
            users: Mutex::new(HashMap::new()),
            current: Mutex::new(None),
            codes: Mutex::new(HashMap::new()),
            refresh_tokens: Mutex::new(HashMap::new()),
            disabled: Mutex::new(HashSet::new()),
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(Arc::clone(&provider));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            issuer,
            provider,
            task,
        }
    }

    /// The person the next authorization signs in, with their claims.
    pub fn sign_in_as(&self, subject: &str, claims: Value) {
        let claims = claims.as_object().cloned().unwrap_or_default();
        self.provider
            .users
            .lock()
            .unwrap()
            .insert(subject.into(), claims);
        *self.provider.current.lock().unwrap() = Some(subject.into());
    }

    /// A token this provider signs for a workload, as CI systems issue to
    /// their jobs; `iss`, `iat` and a five-minute `exp` are filled in unless
    /// given.
    pub fn workload_token(&self, claims: Value) -> String {
        let mut claims = claims.as_object().cloned().unwrap_or_default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        claims.entry("iss").or_insert(json!(self.issuer));
        claims.entry("iat").or_insert(json!(now));
        claims.entry("exp").or_insert(json!(now + 300));
        self.provider.sign(&Value::Object(claims))
    }

    /// Ends the person's access: their refresh tokens stop working.
    pub fn disable(&self, subject: &str) {
        self.provider
            .disabled
            .lock()
            .unwrap()
            .insert(subject.into());
    }
}

impl Drop for TestProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Provider {
    fn random(&self) -> String {
        let mut bytes = [0; 24];
        self.random.fill(&mut bytes).unwrap();
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn sign(&self, claims: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(json!({"alg": "ES256", "kid": KEY_ID}).to_string());
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signed = format!("{header}.{payload}");
        let signature = self.key.sign(&self.random, signed.as_bytes()).unwrap();
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))
    }

    fn authenticated(&self, headers: &HeaderMap, form: &HashMap<String, String>) -> bool {
        let Some(secret) = &self.client_secret else {
            return form.get("client_id") == Some(&self.client_id);
        };
        let basic = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Basic "))
            .and_then(|value| STANDARD.decode(value).ok())
            .and_then(|value| String::from_utf8(value).ok());
        let decode = |value: &str| percent_decode_str(value).decode_utf8_lossy().into_owned();
        match basic.as_deref().and_then(|value| value.split_once(':')) {
            Some((id, password)) => decode(id) == self.client_id && decode(password) == *secret,
            None => {
                form.get("client_id") == Some(&self.client_id)
                    && form.get("client_secret") == Some(secret)
            }
        }
    }
}

async fn discovery(State(provider): State<Arc<Provider>>) -> Json<Value> {
    let issuer = &provider.issuer;
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
    }))
}

async fn jwks(State(provider): State<Arc<Provider>>) -> Json<Value> {
    let point = provider.key.public_key().as_ref();
    Json(json!({"keys": [{
        "kty": "EC", "crv": "P-256", "use": "sig", "alg": "ES256", "kid": KEY_ID,
        "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
        "y": URL_SAFE_NO_PAD.encode(&point[33..]),
    }]}))
}

async fn authorize(
    State(provider): State<Arc<Provider>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let get = |name: &str| query.get(name).cloned().unwrap_or_default();
    if get("response_type") != "code"
        || get("client_id") != provider.client_id
        || get("code_challenge_method") != "S256"
        || !get("scope").split(' ').any(|scope| scope == "openid")
    {
        return (StatusCode::BAD_REQUEST, "invalid authorization request").into_response();
    }
    let Some(subject) = provider.current.lock().unwrap().clone() else {
        return (StatusCode::FORBIDDEN, "nobody signs in").into_response();
    };
    let code = provider.random();
    provider.codes.lock().unwrap().insert(
        code.clone(),
        Pending {
            redirect_uri: get("redirect_uri"),
            challenge: get("code_challenge"),
            nonce: get("nonce"),
            subject,
        },
    );
    let separator = if get("redirect_uri").contains('?') {
        '&'
    } else {
        '?'
    };
    Redirect::to(&format!(
        "{}{separator}code={code}&state={}",
        get("redirect_uri"),
        get("state")
    ))
    .into_response()
}

fn oauth_error(error: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": error}))).into_response()
}

async fn token(
    State(provider): State<Arc<Provider>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    if !provider.authenticated(&headers, &form) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        )
            .into_response();
    }
    let get = |name: &str| form.get(name).cloned().unwrap_or_default();
    let subject = match get("grant_type").as_str() {
        "authorization_code" => {
            let Some(pending) = provider.codes.lock().unwrap().remove(&get("code")) else {
                return oauth_error("invalid_grant");
            };
            let challenge =
                URL_SAFE_NO_PAD.encode(digest(&SHA256, get("code_verifier").as_bytes()));
            if pending.redirect_uri != get("redirect_uri") || pending.challenge != challenge {
                return oauth_error("invalid_grant");
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let mut claims = provider
                .users
                .lock()
                .unwrap()
                .get(&pending.subject)
                .cloned()
                .unwrap_or_default();
            claims.insert("iss".into(), json!(provider.issuer));
            claims.insert("sub".into(), json!(pending.subject));
            claims.insert("aud".into(), json!(provider.client_id));
            claims.insert("iat".into(), json!(now));
            claims.insert("exp".into(), json!(now + 300));
            claims.insert("nonce".into(), json!(pending.nonce));
            let id_token = provider.sign(&Value::Object(claims));
            let refresh_token = provider.random();
            provider
                .refresh_tokens
                .lock()
                .unwrap()
                .insert(refresh_token.clone(), pending.subject);
            return Json(json!({
                "access_token": provider.random(),
                "token_type": "Bearer",
                "expires_in": 300,
                "id_token": id_token,
                "refresh_token": refresh_token,
            }))
            .into_response();
        }
        "refresh_token" => provider
            .refresh_tokens
            .lock()
            .unwrap()
            .remove(&get("refresh_token")),
        _ => return oauth_error("unsupported_grant_type"),
    };
    let Some(subject) = subject else {
        return oauth_error("invalid_grant");
    };
    if provider.disabled.lock().unwrap().contains(&subject) {
        return oauth_error("invalid_grant");
    }
    let refresh_token = provider.random();
    provider
        .refresh_tokens
        .lock()
        .unwrap()
        .insert(refresh_token.clone(), subject);
    Json(json!({
        "access_token": provider.random(),
        "token_type": "Bearer",
        "expires_in": 300,
        "refresh_token": refresh_token,
    }))
    .into_response()
}
