#![forbid(unsafe_code)]

//! Updates against a real authoritative server. `PANEL_TEST_RFC2136_SERVER`
//! names, as `host:port`, a primary for `example.com` that lets the key
//! `panel-test-key.` (hmac-sha256 with the secret below) update TXT records,
//! such as BIND with `update-policy { grant panel-test-key. zonesub TXT; };`.
//! Without it the test is skipped. Records are read back with `dig`.

use dns_rfc2136::{Algorithm, Rfc2136, Rfc2136Settings};
use panel_acme::DnsProvider;
use std::process::Command;
use zeroize::Zeroizing;

const SECRET: &str = "c2VjcmV0IGZvciB0aGUgcGFuZWwgdGVzdCBrZXkgMjAyNg==";

fn provider(server: &str, key_name: &str) -> Rfc2136 {
    Rfc2136::new(Rfc2136Settings {
        server: server.to_owned(),
        zones: vec!["example.com".into()],
        key_name: key_name.to_owned(),
        algorithm: Algorithm::HmacSha256,
        secret: Zeroizing::new(SECRET.into()),
        ttl: Some(30),
    })
    .unwrap()
}

/// The TXT strings at `name`, sorted, as `dig` reads them over TCP.
fn txt(server: &str, name: &str) -> Vec<String> {
    let (host, port) = server.rsplit_once(':').unwrap();
    let output = Command::new("dig")
        .args([
            "+short",
            "+tcp",
            "-p",
            port,
            &format!("@{host}"),
            name,
            "TXT",
        ])
        .output()
        .expect("dig runs");
    let mut values: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| line.trim_matches('"').to_owned())
        .filter(|line| !line.is_empty())
        .collect();
    values.sort();
    values
}

#[tokio::test]
async fn a_real_primary_applies_signed_updates() {
    let Ok(server) = std::env::var("PANEL_TEST_RFC2136_SERVER") else {
        eprintln!("skipping: PANEL_TEST_RFC2136_SERVER is not set");
        return;
    };
    let dns = provider(&server, "panel-test-key.");
    let mut label = [0_u8; 4];
    getrandom::fill(&mut label).unwrap();
    let name = format!("_acme-challenge.t{}.example.com.", hex::encode(label));

    dns.add_txt(&name, "first").await.unwrap();
    dns.add_txt(&name, "second").await.unwrap();
    assert_eq!(txt(&server, &name), ["first", "second"]);
    dns.remove_txt(&name, "first").await.unwrap();
    assert_eq!(txt(&server, &name), ["second"]);
    dns.remove_txt(&name, "second").await.unwrap();
    assert!(txt(&server, &name).is_empty());

    let stranger = provider(&server, "unknown-key.")
        .add_txt(&name, "x")
        .await
        .unwrap_err();
    assert!(
        stranger.message.contains("NOTAUTH") || stranger.message.contains("BADKEY"),
        "{stranger}"
    );
}
