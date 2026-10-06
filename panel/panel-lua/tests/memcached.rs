#![forbid(unsafe_code)]

//! `resty.memcached` against a server speaking the memcached text protocol.

use http::HeaderMap;
use panel_lua::{
    Connection, Exchange, Handler, NoHost, Outcome, Phase, Program, Request, Runtime, Settings,
    SharedStore, Source,
};
use parking_lot::Mutex;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

struct Item {
    data: Vec<u8>,
    flags: u32,
    cas: u64,
}

#[derive(Default)]
struct Store {
    items: HashMap<String, Item>,
    next_cas: u64,
}

impl Store {
    fn put(&mut self, key: &str, data: Vec<u8>, flags: u32) {
        self.next_cas += 1;
        let cas = self.next_cas;
        self.items.insert(key.to_owned(), Item { data, flags, cas });
    }
}

/// The reply to a storage command carrying `data`.
fn store(store: &mut Store, words: &[&str], data: Vec<u8>) -> String {
    let (command, key, flags) = (words[0], words[1], words[2].parse().unwrap_or(0));
    let exists = store.items.contains_key(key);
    match command {
        "set" => store.put(key, data, flags),
        "add" if exists => return "NOT_STORED\r\n".into(),
        "add" => store.put(key, data, flags),
        "replace" | "append" | "prepend" if !exists => return "NOT_STORED\r\n".into(),
        "replace" => store.put(key, data, flags),
        "append" | "prepend" => {
            let item = store.items.get_mut(key).unwrap();
            if command == "append" {
                item.data.extend(data);
            } else {
                item.data.splice(0..0, data);
            }
        }
        "cas" if !exists => return "NOT_FOUND\r\n".into(),
        "cas" if store.items[key].cas.to_string() != words[5] => return "EXISTS\r\n".into(),
        "cas" => store.put(key, data, flags),
        _ => return "ERROR\r\n".into(),
    }
    "STORED\r\n".into()
}

/// The reply to a command without data.
fn command(store: &mut Store, words: &[&str]) -> Vec<u8> {
    match words {
        ["get" | "gets", keys @ ..] => {
            let mut reply = Vec::new();
            for key in keys {
                if let Some(item) = store.items.get(*key) {
                    let cas = if words[0] == "gets" {
                        format!(" {}", item.cas)
                    } else {
                        String::new()
                    };
                    reply.extend(
                        format!("VALUE {key} {} {}{cas}\r\n", item.flags, item.data.len()).bytes(),
                    );
                    reply.extend(&item.data);
                    reply.extend(b"\r\n");
                }
            }
            reply.extend(b"END\r\n");
            reply
        }
        ["delete", key] => match store.items.remove(*key) {
            Some(_) => b"DELETED\r\n".to_vec(),
            None => b"NOT_FOUND\r\n".to_vec(),
        },
        ["incr" | "decr", key, delta] => match store.items.get_mut(*key) {
            None => b"NOT_FOUND\r\n".to_vec(),
            Some(item) => {
                let Ok(current) = String::from_utf8_lossy(&item.data).parse::<u64>() else {
                    return b"CLIENT_ERROR cannot increment or decrement non-numeric value\r\n"
                        .to_vec();
                };
                let delta: u64 = delta.parse().unwrap();
                let next = if words[0] == "incr" {
                    current + delta
                } else {
                    current.saturating_sub(delta)
                };
                item.data = next.to_string().into_bytes();
                format!("{next}\r\n").into_bytes()
            }
        },
        ["touch", key, _] => {
            if store.items.contains_key(*key) {
                b"TOUCHED\r\n".to_vec()
            } else {
                b"NOT_FOUND\r\n".to_vec()
            }
        }
        ["flush_all", ..] => {
            store.items.clear();
            b"OK\r\n".to_vec()
        }
        ["verbosity", _] => b"OK\r\n".to_vec(),
        ["version"] => b"VERSION 1.6.38\r\n".to_vec(),
        ["stats"] => b"STAT pid 7\r\nSTAT curr_items 2\r\nEND\r\n".to_vec(),
        _ => b"ERROR\r\n".to_vec(),
    }
}

async fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shared = Arc::new(Mutex::new(Store::default()));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let words: Vec<&str> = line.split_whitespace().collect();
                    if words.first() == Some(&"quit") {
                        return;
                    }
                    let reply = if matches!(
                        words.first(),
                        Some(&("set" | "add" | "replace" | "append" | "prepend" | "cas"))
                    ) {
                        let size: usize = words[4].parse().unwrap();
                        let mut data = vec![0; size + 2];
                        reader.read_exact(&mut data).await.unwrap();
                        data.truncate(size);
                        store(&mut shared.lock(), &words, data).into_bytes()
                    } else {
                        command(&mut shared.lock(), &words)
                    };
                    if writer.write_all(&reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

#[tokio::test]
async fn memcached_clients_speak_as_lua_resty_memcached_does() {
    let port = server().await;
    let script = format!(
        r#"
        local memcached = require "resty.memcached"
        assert(select(2, memcached:new({{ key_transform = {{ ngx.escape_uri }} }}))
            == "expecting key_transform = {{ escape, unescape }} table")
        local memc = assert(memcached:new())
        assert(memc:set_timeout(1000) == 1)
        assert(memc:connect("127.0.0.1", {port}))
        assert(memc:get_reused_times() == 0)

        assert(memc:set("dog", {{ "a ", {{ "kind of" }}, " animal" }}, 0, 7) == 1)
        local value, flags, err = memc:get("dog")
        assert(value == "a kind of animal" and flags == "7" and err == nil, value)
        value, flags, err = memc:get("cat")
        assert(value == nil and flags == nil and err == nil)
        assert(select(2, memc:add("dog", "again")) == "NOT_STORED")
        assert(memc:add("cat", "meow") == 1)
        assert(memc:append("cat", "!") == 1 and memc:prepend("cat", "a cat says ") == 1)
        assert(memc:get("cat") == "a cat says meow!")
        assert(select(2, memc:replace("bird", "tweet")) == "NOT_STORED")
        assert(memc:set("with space", "escaped") == 1)
        local results = assert(memc:get({{ "dog", "with space", "nothing" }}))
        assert(results.dog[1] == "a kind of animal" and results.dog[2] == "7")
        assert(results["with space"][1] == "escaped" and results.nothing == nil)
        assert(next(assert(memc:get({{}}))) == nil)

        local _, _, cas = memc:gets("dog")
        assert(cas and cas:match("^%d+$"), cas)
        assert(memc:cas("dog", "a good dog", cas) == 1)
        assert(select(2, memc:cas("dog", "stale", cas)) == "EXISTS")
        assert(select(2, memc:cas("ghost", "boo", cas)) == "NOT_FOUND")
        local listed = assert(memc:gets({{ "dog" }}))
        assert(listed.dog[1] == "a good dog" and listed.dog[3]:match("^%d+$"))

        assert(memc:set("n", 10) == 1)
        assert(memc:incr("n", 5) == "15" and memc:decr("n", 3) == "12")
        assert(select(2, memc:incr("dog", 1)):find("^CLIENT_ERROR"))
        assert(select(2, memc:incr("ghost", 1)) == "NOT_FOUND")
        assert(memc:touch("n", 60) == 1)
        assert(select(2, memc:touch("ghost", 60)) == "NOT_FOUND")
        assert(memc:delete("n") == 1 and select(2, memc:delete("n")) == "NOT_FOUND")
        assert(memc:version() == "1.6.38")
        local stats = assert(memc:stats())
        assert(stats[1] == "STAT pid 7" and #stats == 2)
        assert(memc:verbosity(1) == 1)

        memc:init_pipeline()
        assert(memc:set("p", "piped") == 1)
        assert(memc:get("p") == 1)
        assert(memc:delete("ghost") == 1)
        local replies = assert(memc:commit_pipeline())
        assert(replies[1][1] == 1 and replies[2][1] == "piped" and replies[3][2] == "NOT_FOUND")
        assert(select(2, memc:commit_pipeline()) == "no pipeline")
        memc:init_pipeline()
        assert(select(2, memc:commit_pipeline()) == "no more cmds")

        assert(memc:flush_all() == 1 and memc:get("dog") == nil)
        assert(memc:set_keepalive(10000, 10) == 1)
        local again = assert(memcached:new())
        assert(again:connect("127.0.0.1", {port}))
        assert(again:get_reused_times() == 1)
        assert(again:quit() == 1)
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
