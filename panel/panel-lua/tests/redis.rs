#![forbid(unsafe_code)]

//! `resty.redis` against a server that speaks enough RESP2 for it.

use http::HeaderMap;
use panel_lua::{
    Connection, Exchange, Handler, NoHost, Outcome, Phase, Program, Request, Runtime, Settings,
    SharedStore, Source,
};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::mpsc,
};

enum Stored {
    Text(Vec<u8>),
    Hash(BTreeMap<Vec<u8>, Vec<u8>>),
}

type Subscribers = Arc<Mutex<Vec<(usize, Vec<u8>, mpsc::UnboundedSender<Vec<u8>>)>>>;

fn bulk(data: &[u8]) -> Vec<u8> {
    let mut reply = format!("${}\r\n", data.len()).into_bytes();
    reply.extend_from_slice(data);
    reply.extend_from_slice(b"\r\n");
    reply
}

fn array(items: Vec<Vec<u8>>) -> Vec<u8> {
    let mut reply = format!("*{}\r\n", items.len()).into_bytes();
    for item in items {
        reply.extend(item);
    }
    reply
}

const WRONG_TYPE: &[u8] = b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n";

/// The reply to one command outside a transaction or subscription.
fn execute(
    args: &[Vec<u8>],
    store: &Mutex<HashMap<Vec<u8>, Stored>>,
    subscribers: &Subscribers,
) -> Vec<u8> {
    let name = String::from_utf8_lossy(&args[0]).to_lowercase();
    let mut store = store.lock();
    match (name.as_str(), &args[1..]) {
        ("ping", _) => b"+PONG\r\n".to_vec(),
        ("auth", [.., password]) if password == b"secret" => b"+OK\r\n".to_vec(),
        ("auth", _) => {
            b"-WRONGPASS invalid username-password pair or user is disabled.\r\n".to_vec()
        }
        ("select", [_]) => b"+OK\r\n".to_vec(),
        ("set", [key, value]) => {
            store.insert(key.clone(), Stored::Text(value.clone()));
            b"+OK\r\n".to_vec()
        }
        ("get", [key]) => match store.get(key) {
            Some(Stored::Text(value)) => bulk(value),
            Some(Stored::Hash(_)) => WRONG_TYPE.to_vec(),
            None => b"$-1\r\n".to_vec(),
        },
        ("incr", [key]) => {
            let next = match store.get(key) {
                Some(Stored::Text(value)) => {
                    String::from_utf8_lossy(value).parse::<i64>().unwrap_or(0) + 1
                }
                _ => 1,
            };
            store.insert(key.clone(), Stored::Text(next.to_string().into_bytes()));
            format!(":{next}\r\n").into_bytes()
        }
        ("lpop", [key]) => match store.get(key) {
            Some(_) => WRONG_TYPE.to_vec(),
            None => b"$-1\r\n".to_vec(),
        },
        ("hmset", [key, pairs @ ..]) => {
            let entry = store
                .entry(key.clone())
                .or_insert_with(|| Stored::Hash(BTreeMap::new()));
            let Stored::Hash(hash) = entry else {
                return WRONG_TYPE.to_vec();
            };
            for pair in pairs.chunks(2) {
                hash.insert(pair[0].clone(), pair[1].clone());
            }
            b"+OK\r\n".to_vec()
        }
        ("hmget", [key, fields @ ..]) => {
            let hash = match store.get(key) {
                Some(Stored::Hash(hash)) => Some(hash),
                _ => None,
            };
            array(
                fields
                    .iter()
                    .map(|field| match hash.and_then(|hash| hash.get(field)) {
                        Some(value) => bulk(value),
                        None => b"$-1\r\n".to_vec(),
                    })
                    .collect(),
            )
        }
        ("hgetall", [key]) => match store.get(key) {
            Some(Stored::Hash(hash)) => array(
                hash.iter()
                    .flat_map(|(field, value)| [bulk(field), bulk(value)])
                    .collect(),
            ),
            _ => b"*0\r\n".to_vec(),
        },
        ("bf.add", [_, _]) => b":1\r\n".to_vec(),
        ("nullarray", []) => b"*-1\r\n".to_vec(),
        ("publish", [channel, message]) => {
            let mut delivered = 0;
            for (_, wanted, sender) in subscribers.lock().iter() {
                if wanted == channel {
                    let pushed = array(vec![bulk(b"message"), bulk(channel), bulk(message)]);
                    if sender.send(pushed).is_ok() {
                        delivered += 1;
                    }
                }
            }
            format!(":{delivered}\r\n").into_bytes()
        }
        _ => format!("-ERR unknown command '{name}'\r\n").into_bytes(),
    }
}

/// Reads RESP arrays of bulk strings until the connection closes.
async fn commands(
    stream: tokio::net::tcp::OwnedReadHalf,
    commands: mpsc::UnboundedSender<Vec<Vec<u8>>>,
) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let Some(count) = line.trim().strip_prefix('*').and_then(|n| n.parse().ok()) else {
            return;
        };
        let mut args = Vec::with_capacity(count);
        for _ in 0..count {
            let mut header = String::new();
            reader.read_line(&mut header).await.unwrap();
            let size: usize = header.trim()[1..].parse().unwrap();
            let mut data = vec![0; size + 2];
            reader.read_exact(&mut data).await.unwrap();
            data.truncate(size);
            args.push(data);
        }
        if commands.send(args).is_err() {
            return;
        }
    }
}

/// A Redis server holding one keyspace; transactions queue commands and
/// subscriptions receive what is published.
async fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = Arc::new(Mutex::new(HashMap::new()));
    let subscribers: Subscribers = Arc::default();
    let ids = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (store, subscribers) = (Arc::clone(&store), Arc::clone(&subscribers));
            let id = ids.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let (sender, mut received) = mpsc::unbounded_channel();
                tokio::spawn(commands(reader, sender));
                let (pushes, mut pushed) = mpsc::unbounded_channel();
                let mut queued: Option<Vec<Vec<Vec<u8>>>> = None;
                loop {
                    let reply = tokio::select! {
                        args = received.recv() => {
                            let Some(args) = args else { break };
                            let name = String::from_utf8_lossy(&args[0]).to_lowercase();
                            match (name.as_str(), queued.as_mut()) {
                                ("multi", _) => {
                                    queued = Some(Vec::new());
                                    b"+OK\r\n".to_vec()
                                }
                                ("exec", Some(_)) => {
                                    let commands = queued.take().unwrap_or_default();
                                    array(commands.iter().map(|args| execute(args, &store, &subscribers)).collect())
                                }
                                ("discard", Some(_)) => {
                                    queued = None;
                                    b"+OK\r\n".to_vec()
                                }
                                (_, Some(commands)) => {
                                    commands.push(args);
                                    b"+QUEUED\r\n".to_vec()
                                }
                                ("subscribe", None) => {
                                    let mut replies = Vec::new();
                                    let mut subscribed = subscribers.lock();
                                    for channel in &args[1..] {
                                        subscribed.push((id, channel.clone(), pushes.clone()));
                                        let count = subscribed.iter().filter(|entry| entry.0 == id).count();
                                        replies.extend(array(vec![bulk(b"subscribe"), bulk(channel), format!(":{count}\r\n").into_bytes()]));
                                    }
                                    replies
                                }
                                ("unsubscribe", None) => {
                                    subscribers.lock().retain(|entry| entry.0 != id);
                                    array(vec![bulk(b"unsubscribe"), bulk(&args[1]), b":0\r\n".to_vec()])
                                }
                                _ => execute(&args, &store, &subscribers),
                            }
                        }
                        Some(message) = pushed.recv() => message,
                    };
                    if writer.write_all(&reply).await.is_err() {
                        break;
                    }
                }
                subscribers.lock().retain(|entry| entry.0 != id);
            });
        }
    });
    port
}

#[tokio::test]
async fn redis_clients_speak_as_lua_resty_redis_does() {
    let port = server().await;
    let script = format!(
        r#"
        local redis = require "resty.redis"
        local null = ngx.null
        local red = assert(redis:new())
        red:set_timeouts(1000, 1000, 1000)
        local ok, err = red:connect("127.0.0.1", {port}, {{ password = "wrong" }})
        assert(ok == nil and err:find("^failed to authenticate: WRONGPASS"), err)

        red = assert(redis:new())
        assert(red:connect("127.0.0.1", {port}, {{ password = "secret", db = 2 }}))
        assert(red:get_reused_times() == 0)
        assert(red:set("dog", "an animal") == "OK")
        assert(red:get("dog") == "an animal")
        assert(red:get("cat") == null)
        assert(red:incr("n") == 1 and red:incr("n") == 2)
        local nothing, why = red:lpop("dog")
        assert(nothing == false and why:find("^WRONGTYPE"), why)
        assert(red:hmset("h", {{ a = "1" }}) == "OK")
        assert(red:hmset("h", "b", "2") == "OK")
        local values = red:hmget("h", {{ "a", "b", "c" }})
        assert(values[1] == "1" and values[2] == "2" and values[3] == null)
        local hash = red:array_to_hash(red:hgetall("h"))
        assert(hash.a == "1" and hash.b == "2")
        nothing, why = red:bogus()
        assert(nothing == false and why == "ERR unknown command 'bogus'", why)
        redis.register_module_prefix("bf")
        assert(red:bf():add("dog", 1) == 1)
        assert(red:nullarray() == null)

        red:init_pipeline()
        assert(red:set("cat", "Marry") == nil)
        red:get("cat")
        red:lpop("cat")
        local results = assert(red:commit_pipeline())
        assert(results[1] == "OK" and results[2] == "Marry" and results[3][1] == false)
        assert(select(2, red:commit_pipeline()) == "no pipeline")
        red:init_pipeline()
        red:get("cat")
        red:cancel_pipeline()
        assert(red:get("cat") == "Marry")

        assert(red:multi() == "OK")
        assert(red:set("a", "abc") == "QUEUED")
        assert(red:lpop("a") == "QUEUED")
        assert(select(2, red:set_keepalive(10000, 10)) == "in transaction")
        local answers = red:exec()
        assert(answers[1] == "OK" and answers[2][1] == false)
        assert(red:set_keepalive(10000, 10) == 1)

        local again = assert(redis:new())
        assert(again:connect("127.0.0.1", {port}, {{ password = "secret", db = 2 }}))
        assert(again:get_reused_times() == 1)
        assert(again:get("dog") == "an animal")

        local sub = assert(redis:new())
        sub:set_timeouts(1000, 1000, 200)
        assert(sub:connect("127.0.0.1", {port}))
        assert(sub:get_reused_times() == 0)
        assert(select(2, sub:read_reply()) == "not subscribed")
        local res = assert(sub:subscribe("news"))
        assert(res[1] == "subscribe" and res[2] == "news" and res[3] == 1)
        assert(select(2, sub:get("dog")) == "subscribed state")
        assert(select(2, sub:set_keepalive()) == "subscribed state")
        assert(again:publish("news", "hello") == 1)
        local message = assert(sub:read_reply())
        assert(message[1] == "message" and message[2] == "news" and message[3] == "hello")
        nothing, why = sub:read_reply()
        assert(nothing == nil and why == "timeout", why)
        res = assert(sub:unsubscribe("news"))
        assert(res[1] == "unsubscribe" and res[3] == 0)
        assert(sub:ping() == "PONG")
        assert(sub:close() == 1)
        assert(again:close() == 1)

        nothing, why = assert(redis:new()):connect("unix:/tmp/redis.sock")
        assert(nothing == nil and why == "unix domain sockets are not available", why)
        assert(not pcall(red.connect, assert(redis:new()), "127.0.0.1", {port}, {{ db = "two" }}))
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
