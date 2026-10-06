#![forbid(unsafe_code)]

//! `resty.mysql` against a server speaking the MySQL client/server
//! protocol: its handshake, TLS upgrade, authentication plugins and text
//! results.

use http::HeaderMap;
use panel_lua::{
    Connection, Exchange, Handler, NoHost, Outcome, Phase, Program, Request, Runtime, Settings,
    SharedStore, Source,
};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};

const SCRAMBLE: &[u8; 20] = b"abcdefghij0123456789";
const SWITCHED: &[u8; 20] = b"ZYXWVUTSRQponmlkjihg";
const PASSWORD: &[u8] = b"secret";

const PROTOCOL_41: u32 = 0x200;
const SSL: u32 = 0x800;
const SECURE_CONNECTION: u32 = 0x8000;
const MULTI_RESULTS: u32 = 0x20000;
const PLUGIN_AUTH: u32 = 0x80000;
const CONNECT_WITH_DB: u32 = 0x8;

fn xor(left: &[u8], right: &[u8]) -> Vec<u8> {
    left.iter().zip(right).map(|(a, b)| a ^ b).collect()
}

fn native(scramble: &[u8]) -> Vec<u8> {
    let stage1 = Sha1::digest(PASSWORD);
    let mut joined = scramble.to_vec();
    joined.extend(Sha1::digest(stage1));
    xor(&stage1, &Sha1::digest(joined))
}

fn caching_sha2(scramble: &[u8]) -> Vec<u8> {
    let stage1 = Sha256::digest(PASSWORD);
    let mut joined = Sha256::digest(stage1).to_vec();
    joined.extend_from_slice(scramble);
    xor(&stage1, &Sha256::digest(joined))
}

fn lenenc(data: &[u8]) -> Vec<u8> {
    let mut out = vec![u8::try_from(data.len()).unwrap()];
    out.extend_from_slice(data);
    out
}

struct Wire<S> {
    stream: S,
    seq: u8,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Wire<S> {
    async fn read(&mut self) -> Option<Vec<u8>> {
        let mut head = [0; 4];
        self.stream.read_exact(&mut head).await.ok()?;
        let length = usize::from(head[0]) | usize::from(head[1]) << 8 | usize::from(head[2]) << 16;
        self.seq = head[3];
        let mut payload = vec![0; length];
        self.stream.read_exact(&mut payload).await.ok()?;
        Some(payload)
    }

    async fn write(&mut self, payload: &[u8]) {
        self.seq = self.seq.wrapping_add(1);
        let length = u32::try_from(payload.len()).unwrap().to_le_bytes();
        let mut packet = vec![length[0], length[1], length[2], self.seq];
        packet.extend_from_slice(payload);
        self.stream.write_all(&packet).await.unwrap();
    }

    async fn ok(&mut self, affected: u8, insert_id: u8, status: u16, message: &[u8]) {
        let mut payload = vec![0, affected, insert_id];
        payload.extend(status.to_le_bytes());
        payload.extend(1u16.to_le_bytes());
        payload.extend_from_slice(message);
        self.write(&payload).await;
    }

    async fn error(&mut self, code: u16, state: &str, message: &str) {
        let mut payload = vec![0xff];
        payload.extend(code.to_le_bytes());
        payload.push(b'#');
        payload.extend(state.bytes());
        payload.extend(message.bytes());
        self.write(&payload).await;
    }

    async fn eof(&mut self, status: u16) {
        let mut payload = vec![0xfe, 0, 0];
        payload.extend(status.to_le_bytes());
        self.write(&payload).await;
    }

    async fn result_set(&mut self, columns: &[(&str, u8)], rows: &[Vec<Option<&str>>], more: bool) {
        self.write(&[u8::try_from(columns.len()).unwrap()]).await;
        for (name, kind) in columns {
            let mut column = Vec::new();
            for part in ["def", "db", "cats", "cats", name, name] {
                column.extend(lenenc(part.as_bytes()));
            }
            column.extend([0x0c, 0x21, 0, 0xff, 0, 0, 0, *kind, 0, 0, 0, 0, 0]);
            self.write(&column).await;
        }
        self.eof(2).await;
        for row in rows {
            let mut payload = Vec::new();
            for value in row {
                match value {
                    Some(text) => payload.extend(lenenc(text.as_bytes())),
                    None => payload.push(0xfb),
                }
            }
            self.write(&payload).await;
        }
        self.eof(if more { 2 | 0x8 } else { 2 }).await;
    }
}

/// The user, auth data and plugin of a HandshakeResponse41.
fn response(payload: &[u8]) -> (String, Vec<u8>, Option<String>, String) {
    let flags = u32::from_le_bytes(payload[..4].try_into().unwrap());
    let rest = &payload[32..];
    let user_end = rest.iter().position(|byte| *byte == 0).unwrap();
    let user = String::from_utf8_lossy(&rest[..user_end]).into_owned();
    let rest = &rest[user_end + 1..];
    let auth = rest[1..=usize::from(rest[0])].to_vec();
    let mut rest = &rest[usize::from(rest[0]) + 1..];
    let mut database = None;
    if flags & CONNECT_WITH_DB != 0 {
        let end = rest.iter().position(|byte| *byte == 0).unwrap();
        database = Some(String::from_utf8_lossy(&rest[..end]).into_owned());
        rest = &rest[end + 1..];
    }
    let plugin_end = rest
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(rest.len());
    (
        user,
        auth,
        database,
        String::from_utf8_lossy(&rest[..plugin_end]).into_owned(),
    )
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(mut wire: Wire<S>, first: Vec<u8>, tls: bool) {
    let (user, auth, database, plugin) = response(&first);
    let accepted = match user.as_str() {
        "native" => auth == native(SCRAMBLE) && plugin == "mysql_native_password",
        "fast" => {
            let good = auth == caching_sha2(SCRAMBLE) && database.as_deref() == Some("zoo");
            if good {
                wire.write(&[0x01, 0x03]).await;
            }
            good
        }
        "full" => {
            wire.write(&[0x01, 0x04]).await;
            tls && wire.read().await.as_deref() == Some(b"secret\0")
        }
        "switch" => {
            let mut switch = b"\xfemysql_native_password\0".to_vec();
            switch.extend_from_slice(SWITCHED);
            switch.push(0);
            wire.write(&switch).await;
            wire.read().await == Some(native(SWITCHED))
        }
        _ => false,
    };
    if !accepted {
        let message = format!("Access denied for user '{user}'@'localhost' (using password: YES)");
        wire.error(1045, "28000", &message).await;
        return;
    }
    wire.ok(0, 0, 2, b"").await;
    while let Some(command) = wire.read().await {
        match command.first() {
            Some(0x03) => {
                let query = String::from_utf8_lossy(&command[1..]).into_owned();
                match query.as_str() {
                    "select 1; select 2" => {
                        wire.result_set(&[("1", 0x08)], &[vec![Some("1")]], true)
                            .await;
                        wire.result_set(&[("2", 0x08)], &[vec![Some("2")]], false)
                            .await;
                    }
                    "select * from cats" => {
                        let columns = [
                            ("id", 0x03),
                            ("name", 0xfd),
                            ("weight", 0xf6),
                            ("note", 0xfd),
                        ];
                        let rows = [
                            vec![Some("1"), Some("Bob"), Some("4.50"), None],
                            vec![Some("2"), Some("Marry"), Some("3.25"), Some("calm")],
                        ];
                        wire.result_set(&columns, &rows, false).await;
                    }
                    "insert into cats (name) values ('Kit'), ('Tom')" => {
                        wire.ok(2, 7, 2, b"Records: 2").await;
                    }
                    _ => {
                        wire.error(1064, "42000", "You have an error in your SQL syntax")
                            .await;
                    }
                }
            }
            _ => return,
        }
    }
}

/// A server whose greeting offers `plugin` first.
async fn server(plugin: &'static str) -> u16 {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![certified.cert.der().clone()], key.into())
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let mut wire = Wire { stream, seq: 0 };
                let capabilities =
                    PROTOCOL_41 | SSL | SECURE_CONNECTION | MULTI_RESULTS | PLUGIN_AUTH;
                let mut greeting = b"\x0a8.4.6-panel\0".to_vec();
                greeting.extend(7u32.to_le_bytes());
                greeting.extend_from_slice(&SCRAMBLE[..8]);
                greeting.push(0);
                greeting.extend(u16::try_from(capabilities & 0xffff).unwrap().to_le_bytes());
                greeting.push(255);
                greeting.extend(2u16.to_le_bytes());
                greeting.extend(u16::try_from(capabilities >> 16).unwrap().to_le_bytes());
                greeting.push(21);
                greeting.extend([0; 10]);
                greeting.extend_from_slice(&SCRAMBLE[8..]);
                greeting.push(0);
                greeting.extend(plugin.bytes());
                greeting.push(0);
                wire.seq = u8::MAX;
                wire.write(&greeting).await;
                let Some(first) = wire.read().await else {
                    return;
                };
                let flags = u32::from_le_bytes(first[..4].try_into().unwrap());
                if first.len() == 32 && flags & SSL != 0 {
                    let seq = wire.seq;
                    let Ok(tls) = acceptor.accept(wire.stream).await else {
                        return;
                    };
                    let mut secured = Wire { stream: tls, seq };
                    let Some(first) = secured.read().await else {
                        return;
                    };
                    serve(secured, first, true).await;
                } else {
                    serve(wire, first, false).await;
                }
            });
        }
    });
    port
}

#[tokio::test]
async fn mysql_clients_speak_as_lua_resty_mysql_does() {
    let port = server("mysql_native_password").await;
    let sha2 = server("caching_sha2_password").await;
    let script = format!(
        r#"
        local mysql = require "resty.mysql"
        local function connect(user, extra)
            local db = assert(mysql:new())
            db:set_timeout(2000)
            local at = user == "native" and {port} or {sha2}
            local options = {{ host = "127.0.0.1", port = at, user = user, password = "secret" }}
            for key, value in pairs(extra or {{}}) do
                options[key] = value
            end
            return db, db:connect(options)
        end

        local db, ok, err, errcode, sqlstate = connect("native")
        assert(ok == 1, err)
        assert(db:server_ver() == "8.4.6-panel")
        local res
        res, err = db:query("select * from cats")
        assert(res and err == nil, err)
        assert(#res == 2 and res[1].id == 1 and res[1].name == "Bob" and res[1].weight == 4.5)
        assert(res[1].note == ngx.null and res[2].note == "calm")
        db:set_compact_arrays(true)
        res = assert(db:query("select * from cats"))
        assert(res[2][1] == 2 and res[2][2] == "Marry" and res[2][4] == "calm")
        db:set_compact_arrays(false)
        res = assert(db:query("insert into cats (name) values ('Kit'), ('Tom')"))
        assert(res.affected_rows == 2 and res.insert_id == 7 and res.server_status == 2)
        assert(res.warning_count == 1 and res.message == "Records: 2")
        res, err, errcode, sqlstate = db:query("bad")
        assert(res == nil and err == "You have an error in your SQL syntax", err)
        assert(errcode == 1064 and sqlstate == "42000")
        res, err = db:query("select 1; select 2")
        assert(res[1]["1"] == 1 and err == "again", err)
        assert(select(2, db:send_query("select 3")):find("^cannot send query in the current context"))
        assert(select(2, db:set_keepalive()):find("^cannot be reused"))
        res, err = db:read_result()
        assert(res[1]["2"] == 2 and err == nil, err)
        assert(select(2, db:read_result()):find("^cannot read result in the current context"))
        assert(db:set_keepalive(10000, 5) == 1)
        local again = assert(mysql:new())
        assert(again:connect({{ host = "127.0.0.1", port = {port}, user = "native", password = "secret" }}))
        assert(again:get_reused_times() == 1)
        assert(again:query("select * from cats"))
        again:close()

        db, ok, err = connect("fast", {{ database = "zoo", charset = "utf8mb4" }})
        assert(ok == 1, err)
        db:close()
        db, ok, err = connect("switch")
        assert(ok == 1, err)
        db:close()
        db, ok, err = connect("full", {{ ssl = true, ssl_verify = false }})
        assert(ok == 1, err)
        assert(db:query("select * from cats"))
        db:close()
        db, ok, err = connect("full")
        assert(ok == nil and err:find("needs ssl"), err)
        db, ok, err, errcode, sqlstate = connect("stranger")
        assert(ok == nil and err:find("^Access denied for user 'stranger'"), err)
        assert(errcode == 1045 and sqlstate == "28000")
        db, ok, err = connect("native", {{ charset = "klingon" }})
        assert(ok == nil and err == "charset 'klingon' is not supported", err)
        ngx.say("ok")
        "#
    );
    let mut builder = Program::builder();
    let id = builder.handler(&Source::new("main.conf", &script, 1));
    let program = builder.build().expect("scripts compile");
    let (runtime, _) = Runtime::start(
        &program,
        &Settings {
            vms: 1,
            memory: 16 << 20,
        },
        &SharedStore::default(),
    )
    .expect("starts");
    let exchange = Exchange::new(
        Request {
            method: "GET".into(),
            uri: "/".into(),
            request_uri: "/".into(),
            headers: HeaderMap::new(),
            ..Request::default()
        },
        Connection::default(),
    );
    let mut handler = Handler::new(id, Phase::Content);
    handler.permissions.network = true;
    handler.limits.time = Duration::from_secs(10);
    let mut scripts = runtime.scripts(exchange);
    let outcome = scripts.run(handler, &mut NoHost).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"ok\n");
}
