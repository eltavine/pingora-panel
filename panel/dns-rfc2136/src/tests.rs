use super::*;
use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

const SECRET: &str = "c2VjcmV0IGZvciB0aGUgcGFuZWwgdGVzdCBrZXkgMjAyNg==";

/// How the test server behaves.
#[derive(Clone, Copy)]
enum Behaviour {
    Apply,
    Refuse,
    SignWithAnotherKey,
    Silent,
}

type Records = Arc<Mutex<BTreeSet<(String, String)>>>;

fn key(secret: &[u8]) -> Key {
    Key {
        name: wire::name("panel-test-key").unwrap(),
        algorithm: Algorithm::HmacSha256,
        secret: Zeroizing::new(secret.to_vec()),
    }
}

/// The record name, class and text of an update this crate sends.
fn change(request: &[u8]) -> (String, u16, String) {
    let mut position = 12;
    while request[position] != 0 {
        position += 1 + usize::from(request[position]);
    }
    position += 5;
    let mut labels = Vec::new();
    while request[position] != 0 {
        let length = usize::from(request[position]);
        labels.push(
            String::from_utf8(request[position + 1..position + 1 + length].to_vec()).unwrap(),
        );
        position += 1 + length;
    }
    position += 3;
    let class = u16::from_be_bytes([request[position], request[position + 1]]);
    position += 8;
    let length = usize::from(request[position]);
    let text = String::from_utf8(request[position + 1..position + 1 + length].to_vec()).unwrap();
    (labels.join("."), class, text)
}

async fn server(behaviour: Behaviour, records: Records) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let secret = STANDARD.decode(SECRET).unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut length = [0_u8; 2];
            stream.read_exact(&mut length).await.unwrap();
            let mut request = vec![0; usize::from(u16::from_be_bytes(length))];
            stream.read_exact(&mut request).await.unwrap();
            if matches!(behaviour, Behaviour::Silent) {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
            let mac = wire::verify_request(&request, &key(&secret), now()).unwrap();
            let rcode = match behaviour {
                Behaviour::Refuse => 5,
                _ => {
                    let (name, class, text) = change(&request);
                    let mut records = records.lock().unwrap();
                    match class {
                        1 => records.insert((name, text)),
                        _ => records.remove(&(name, text)),
                    };
                    0
                }
            };
            let signing = match behaviour {
                Behaviour::SignWithAnotherKey => key(b"another secret"),
                _ => key(&secret),
            };
            let answer = wire::signed_answer(&request, rcode, &signing, &mac, now());
            let mut framed = u16::try_from(answer.len()).unwrap().to_be_bytes().to_vec();
            framed.extend(answer);
            stream.write_all(&framed).await.unwrap();
        }
    });
    address
}

fn provider(server: SocketAddr) -> Rfc2136 {
    Rfc2136::new(Rfc2136Settings {
        server: server.to_string(),
        zones: vec!["example.com".into(), "Sub.Example.com.".into()],
        key_name: "panel-test-key.".into(),
        algorithm: Algorithm::HmacSha256,
        secret: Zeroizing::new(SECRET.into()),
        ttl: None,
    })
    .unwrap()
    .with_timeout(Duration::from_secs(1))
}

#[tokio::test]
async fn records_are_added_and_removed_with_signed_updates() {
    let records = Records::default();
    let address = server(Behaviour::Apply, Arc::clone(&records)).await;
    let dns = provider(address);
    dns.add_txt("_acme-challenge.www.example.com.", "token-one")
        .await
        .unwrap();
    dns.add_txt("_acme-challenge.www.example.com.", "token-two")
        .await
        .unwrap();
    assert_eq!(records.lock().unwrap().len(), 2);
    dns.remove_txt("_acme-challenge.www.example.com.", "token-one")
        .await
        .unwrap();
    assert_eq!(
        *records.lock().unwrap(),
        BTreeSet::from([(
            "_acme-challenge.www.example.com".to_owned(),
            "token-two".to_owned()
        )])
    );
    assert_eq!(
        dns.zone_of("_acme-challenge.a.sub.example.com").unwrap(),
        "sub.example.com"
    );
    assert_eq!(dns.zone_of("example.com").unwrap(), "example.com");
    let outside = dns
        .add_txt("_acme-challenge.example.org.", "x")
        .await
        .unwrap_err();
    assert_eq!(outside.code.as_str(), "VALIDATION_FAILED");
}

#[tokio::test]
async fn refusals_and_unverifiable_answers_are_reported() {
    let refusing = provider(server(Behaviour::Refuse, Records::default()).await);
    let refused = refusing
        .add_txt("_acme-challenge.example.com.", "x")
        .await
        .unwrap_err();
    assert!(refused.message.contains("REFUSED"), "{refused}");

    let forging = provider(server(Behaviour::SignWithAnotherKey, Records::default()).await);
    let forged = forging
        .add_txt("_acme-challenge.example.com.", "x")
        .await
        .unwrap_err();
    assert!(forged.message.contains("does not verify"), "{forged}");

    let silent = provider(server(Behaviour::Silent, Records::default()).await);
    let late = silent
        .add_txt("_acme-challenge.example.com.", "x")
        .await
        .unwrap_err();
    assert_eq!(late.code.as_str(), "DEADLINE_EXCEEDED");
}

#[test]
fn settings_are_checked() {
    let settings = Rfc2136Settings {
        server: "127.0.0.1:53".into(),
        zones: vec!["example.com".into()],
        key_name: "panel-test-key".into(),
        algorithm: Algorithm::HmacSha256,
        secret: Zeroizing::new(SECRET.into()),
        ttl: Some(30),
    };
    assert!(Rfc2136::new(settings.clone()).is_ok());
    for broken in [
        Rfc2136Settings {
            secret: Zeroizing::new("not base64!".into()),
            ..settings.clone()
        },
        Rfc2136Settings {
            zones: Vec::new(),
            ..settings.clone()
        },
        Rfc2136Settings {
            server: " ".into(),
            ..settings.clone()
        },
    ] {
        assert!(Rfc2136::new(broken).is_err());
    }
    assert_eq!(
        Algorithm::parse("HMAC-SHA512.").unwrap(),
        Algorithm::HmacSha512
    );
    assert!(Algorithm::parse("hmac-md5").is_err());
}
