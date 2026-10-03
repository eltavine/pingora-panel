use super::*;
use hickory_proto::{
    op::OpCode,
    rr::{DNSClass, TSigResponseContext},
};
use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

const SECRET: &str = "c2VjcmV0IGZvciB0aGUgcGFuZWwgdGVzdCBrZXkgMjAyNg==";

/// `nsupdate -v -y hmac-sha256:panel-test-key.:<secret>` from BIND 9.10
/// adding `_acme-challenge.example.com. 60 TXT "LoqXcYV8…"`, signed at
/// 1791566141 (2026-10-03T13:55:41Z).
const NSUPDATE: &str = "0d9928000001000000010001076578616d706c6503636f6d00000600010f5f61636d652d6368616c6c656e6765c00c001000010000003c002c2b4c6f71586359563871354f4e624a5178626d52375343544e6f337469415844666f77796a78416a457558300e70616e656c2d746573742d6b65790000fa00ff00000000003d0b686d61632d7368613235360000006ac1093d012c00208414b93605e42a33063328d636e0a408390684bdd4751c55e51170d5300a91e10d9900000000";
const SIGNED_AT: u64 = 0x6ac1_093d;

/// How the test server behaves.
#[derive(Clone, Copy)]
enum Behaviour {
    Apply,
    Refuse,
    SignWithAnotherKey,
    Unsigned,
    Silent,
}

type Records = Arc<Mutex<BTreeSet<(String, String)>>>;

fn signer(secret: &[u8]) -> TSigner {
    TSigner::new(
        secret.to_vec(),
        TsigAlgorithm::HmacSha256,
        name("panel-test-key").unwrap(),
        FUDGE,
    )
    .unwrap()
}

fn secret() -> Vec<u8> {
    STANDARD.decode(SECRET).unwrap()
}

/// Applies the update section as a primary would.
fn apply(update: &Message, records: &Records) {
    let mut records = records.lock().unwrap();
    for record in &update.authorities {
        let RData::TXT(txt) = &record.data else {
            continue;
        };
        let entry = (
            record.name.to_string().trim_end_matches('.').to_owned(),
            String::from_utf8(txt.txt_data[0].to_vec()).unwrap(),
        );
        if record.dns_class == DNSClass::NONE {
            records.remove(&entry);
        } else {
            records.insert(entry);
        }
    }
}

async fn server(behaviour: Behaviour, records: Records) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
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
            let (request_mac, _, window) = signer(&secret())
                .verify_message_byte(&request, None, true)
                .unwrap();
            assert!(window.contains(&now()));
            let update = Message::from_vec(&request).unwrap();
            assert_eq!(update.metadata.op_code, OpCode::Update);
            let code = match behaviour {
                Behaviour::Refuse => ResponseCode::Refused,
                _ => {
                    apply(&update, &records);
                    ResponseCode::NoError
                }
            };
            let mut answer = Message::error_msg(update.metadata.id, OpCode::Update, code);
            let signing = match behaviour {
                Behaviour::SignWithAnotherKey => signer(b"another secret"),
                _ => signer(&secret()),
            };
            if !matches!(behaviour, Behaviour::Unsigned) {
                let signature =
                    TSigResponseContext::new(update.metadata.id, now(), signing, request_mac, None)
                        .sign(&answer.to_vec().unwrap())
                        .unwrap();
                answer.set_signature(signature);
            }
            let answer = answer.to_vec().unwrap();
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

    for behaviour in [Behaviour::SignWithAnotherKey, Behaviour::Unsigned] {
        let forging = provider(server(behaviour, Records::default()).await);
        let forged = forging
            .add_txt("_acme-challenge.example.com.", "x")
            .await
            .unwrap_err();
        assert!(forged.message.contains("does not verify"), "{forged}");
    }

    let silent = provider(server(Behaviour::Silent, Records::default()).await);
    let late = silent
        .add_txt("_acme-challenge.example.com.", "x")
        .await
        .unwrap_err();
    assert_eq!(late.code.as_str(), "DEADLINE_EXCEEDED");
}

#[test]
fn signatures_agree_with_bind() {
    let bind = hex::decode(NSUPDATE).unwrap();
    let (_, signed_at, window) = signer(&secret())
        .verify_message_byte(&bind, None, true)
        .expect("BIND's signature verifies");
    assert_eq!(signed_at, SIGNED_AT);
    assert!(window.contains(&(SIGNED_AT + 10)) && !window.contains(&(SIGNED_AT + 301)));
    let mut tampered = bind.clone();
    tampered[60] ^= 1;
    assert!(signer(&secret())
        .verify_message_byte(&tampered, None, true)
        .is_err());
    assert!(signer(b"another secret")
        .verify_message_byte(&bind, None, true)
        .is_err());

    let dns = provider("127.0.0.1:53".parse().unwrap());
    let mut update = dns
        .update(
            Change::Add,
            "_acme-challenge.example.com",
            "LoqXcYV8q5ONbJQxbmR7SCTNo3tiAXDfowyjxAjEuX0",
        )
        .unwrap();
    update.metadata.id = 0x0d99;
    update.finalize(&dns.signer, SIGNED_AT).unwrap();
    assert_eq!(
        hex::encode(update.to_vec().unwrap()),
        NSUPDATE,
        "signed byte for byte as BIND signs it"
    );

    let removal = dns
        .update(Change::Remove, "_acme-challenge.example.com", "value")
        .unwrap();
    let record = &removal.authorities[0];
    assert_eq!((record.dns_class, record.ttl), (DNSClass::NONE, 0));
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
            zones: vec!["bücher.example".into()],
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
