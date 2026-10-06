#![forbid(unsafe_code)]

//! Handlers run against requests described here, through the same runtime
//! the gateway uses.

use async_trait::async_trait;
use bytes::Bytes;
use http::{HeaderMap, HeaderValue};
use panel_lua::{
    Connection, Exchange, FailureKind, Handler, HandlerId, Host, Limits, LogLevel, NoHost, Outcome,
    Phase, Program, ProgramBuilder, Request, Runtime, Scripts, Settings, SharedStore, Source,
    TlsTerms,
};
use std::time::{Duration, Instant};

struct Lua {
    runtime: Runtime,
    handlers: Vec<HandlerId>,
}

fn source(text: &str) -> Source {
    Source::new("main.conf", text, 1)
}

fn program(build: impl FnOnce(&mut ProgramBuilder) -> Vec<HandlerId>) -> (Program, Vec<HandlerId>) {
    let mut builder = Program::builder();
    let handlers = build(&mut builder);
    (builder.build().expect("scripts compile"), handlers)
}

fn start(vms: usize, build: impl FnOnce(&mut ProgramBuilder) -> Vec<HandlerId>) -> Lua {
    let (program, handlers) = program(build);
    let settings = Settings {
        vms,
        memory: 16 << 20,
    };
    let (runtime, _) =
        Runtime::start(&program, &settings, &SharedStore::default()).expect("starts");
    Lua { runtime, handlers }
}

fn handlers(texts: &[&str]) -> impl FnOnce(&mut ProgramBuilder) -> Vec<HandlerId> {
    let texts: Vec<String> = texts.iter().map(|text| (*text).to_owned()).collect();
    move |builder| {
        texts
            .iter()
            .map(|text| builder.handler(&source(text)))
            .collect()
    }
}

fn request(method: &str, uri: &str, headers: &[(&str, &str)]) -> Exchange {
    let (path, args) = match uri.split_once('?') {
        Some((path, args)) => (path, Some(args.to_owned())),
        None => (uri, None),
    };
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        map.append(
            http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    Exchange::new(
        Request {
            method: method.into(),
            uri: path.into(),
            request_uri: uri.into(),
            args,
            headers: map,
            ..Request::default()
        },
        Connection {
            client: Some("192.0.2.7:51000".parse().unwrap()),
            server: Some("198.51.100.1:443".parse().unwrap()),
            tls: true,
            server_name: "shop.example".into(),
            request_id: "0b1d0c8e9f2a4b6c".into(),
            ..Connection::default()
        },
    )
}

fn handler(id: HandlerId, phase: Phase) -> Handler {
    let mut handler = Handler::new(id, phase);
    handler.log_level = LogLevel::Debug;
    handler
}

async fn run(scripts: &mut Scripts, handler: Handler) -> Outcome {
    scripts.run(handler, &mut NoHost).await
}

fn kind(outcome: &Outcome) -> Option<FailureKind> {
    match outcome {
        Outcome::Failed(failure) => Some(failure.kind),
        _ => None,
    }
}

#[tokio::test]
async fn access_handlers_read_and_change_the_request() {
    let lua = start(
        1,
        handlers(&[r#"
        local h = ngx.req.get_headers()
        assert(h["x-user"] == "ann" and h.x_user == "ann" and h["X-User"] == "ann")
        local args = ngx.req.get_uri_args()
        assert(args.page == "2" and args.flag == true and args.tag[2] == "b")
        assert(ngx.var.arg_page == "2" and ngx.var.host == "shop.example")
        assert(ngx.var.remote_addr == "192.0.2.7" and ngx.var.scheme == "https")
        assert(ngx.req.get_method() == "GET" and ngx.var.request_uri == "/cart?page=2&flag&tag=a&tag=b")
        ngx.req.set_header("X-Checked", {"yes", "twice"})
        ngx.req.clear_header("X-User")
        ngx.req.set_uri_args({ page = 3 })
        ngx.var.cart_owner = "ann"
    "#]),
    );
    let mut scripts = lua.runtime.scripts(request(
        "GET",
        "/cart?page=2&flag&tag=a&tag=b",
        &[("x-user", "ann"), ("host", "shop.example")],
    ));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Access)).await;
    assert_eq!(outcome, Outcome::Continue, "{:?}", scripts.exchange().logs);
    let exchange = scripts.exchange();
    let changes = exchange.changes();
    assert!(changes.headers && changes.args && !changes.status);
    let checked: Vec<_> = exchange
        .request
        .headers
        .get_all("x-checked")
        .iter()
        .collect();
    assert_eq!(checked, ["yes", "twice"]);
    assert!(exchange.request.headers.get("x-user").is_none());
    assert_eq!(exchange.request.args.as_deref(), Some("page=3"));
    assert_eq!(exchange.variables["cart_owner"], "ann");
}

#[tokio::test]
async fn handlers_answer_with_exit_say_and_redirect() {
    let lua = start(
        1,
        handlers(&[
            r#"ngx.header["X-Reason"] = "closed"; ngx.exit(ngx.HTTP_FORBIDDEN); error("not reached")"#,
            r#"ngx.status = 201; ngx.header.content_type = "text/plain"; ngx.say("made ", 1, " ", true); ngx.print({"a", {"b"}})"#,
            r#"return ngx.redirect("/login", 303)"#,
            r#"ngx.exit(ngx.OK)"#,
        ]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[0], Phase::Access)).await,
        Outcome::Respond
    );
    assert_eq!(scripts.exchange().response.status, 403);
    assert_eq!(scripts.exchange().response.headers["x-reason"], "closed");

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[1], Phase::Content)).await,
        Outcome::Respond
    );
    {
        let exchange = scripts.exchange();
        assert_eq!(exchange.response.status, 201);
        assert_eq!(exchange.response.body, b"made 1 true\nab");
        assert_eq!(exchange.response.headers["content-type"], "text/plain");
    }

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[2], Phase::Rewrite)).await,
        Outcome::Respond
    );
    assert_eq!(scripts.exchange().response.status, 303);
    assert_eq!(scripts.exchange().response.headers["location"], "/login");

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[3], Phase::Access)).await,
        Outcome::Continue
    );
}

#[tokio::test]
async fn failed_handlers_leave_the_request_as_it_was() {
    let lua = start(1, handlers(&[
        "ngx.req.set_header('X-Half', 'done')\nngx.log(ngx.WARN, 'about to fail')\nlocal t = nil\nreturn t.field",
    ]));
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Access)).await;
    let Outcome::Failed(failure) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(failure.kind, FailureKind::Error);
    assert!(
        failure.message.starts_with("main.conf:4:"),
        "{}",
        failure.message
    );
    let exchange = scripts.exchange();
    assert!(exchange.request.headers.get("x-half").is_none());
    assert!(!exchange.changes().any());
    assert_eq!(exchange.logs.len(), 1);
    assert!(exchange.logs[0].message.contains("about to fail"));
    assert!(
        exchange.logs[0].message.starts_with("main.conf:2: "),
        "{:?}",
        exchange.logs
    );
}

#[tokio::test]
async fn work_time_and_memory_limits_end_runs_that_catch_errors() {
    let lua = start(
        1,
        handlers(&[
            "while true do pcall(function() while true do end end) end",
            "local t = {} for i = 1, 1e9 do t[i] = string.rep('x', 1024) .. i end",
            "ngx.say('still serving')",
        ]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let mut bounded = handler(lua.handlers[0], Phase::Access);
    bounded.limits = Limits {
        time: Duration::from_secs(10),
        work: 100_000,
    };
    assert_eq!(
        kind(&run(&mut scripts, bounded).await),
        Some(FailureKind::Work)
    );

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    bounded.limits = Limits {
        time: Duration::from_millis(50),
        work: u64::MAX / 2,
    };
    let started = Instant::now();
    assert_eq!(
        kind(&run(&mut scripts, bounded).await),
        Some(FailureKind::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let mut hungry = handler(lua.handlers[1], Phase::Access);
    hungry.limits.time = Duration::from_secs(10);
    assert_eq!(
        kind(&run(&mut scripts, hungry).await),
        Some(FailureKind::Memory)
    );

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[2], Phase::Content)).await,
        Outcome::Respond
    );
    assert_eq!(scripts.exchange().response.body, b"still serving\n");
}

#[tokio::test(flavor = "current_thread")]
async fn busy_scripts_let_other_requests_on_their_thread_go_first() {
    let lua = start(
        1,
        handlers(&[
            "local deadline = os.clock() + 0.3 while os.clock() < deadline do end",
            "ngx.say('quick')",
        ]),
    );
    let mut busy = lua.runtime.scripts(request("GET", "/", &[]));
    let mut quick = lua.runtime.scripts(request("GET", "/", &[]));
    let mut slow = handler(lua.handlers[0], Phase::Access);
    slow.limits.time = Duration::from_secs(5);
    let started = Instant::now();
    let quick_handler = handler(lua.handlers[1], Phase::Content);
    let (_, answered) = tokio::join!(run(&mut busy, slow), async {
        tokio::task::yield_now().await;
        let outcome = run(&mut quick, quick_handler).await;
        (outcome, started.elapsed())
    });
    assert_eq!(answered.0, Outcome::Respond);
    assert!(answered.1 < Duration::from_millis(200), "{:?}", answered.1);
}

#[tokio::test]
async fn requests_keep_their_globals_and_share_modules_and_ctx() {
    let (program, ids) = program(|builder| {
        builder.module(
            "counter",
            &Source::new("lua/counter.lua", "local M = { n = 0 } leaked = true function M.next() M.n = M.n + 1 return M.n end return M", 1),
        );
        vec![
            builder.handler(&source("assert(mine == nil) mine = ngx.var.arg_who ngx.ctx.who = mine ngx.ctx.n = require('counter').next()")),
            builder.handler(&source("assert(leaked == nil) ngx.say(mine, ' ', ngx.ctx.who, ' ', ngx.ctx.n)")),
        ]
    });
    let (runtime, _) = Runtime::start(
        &program,
        &Settings {
            vms: 1,
            memory: 16 << 20,
        },
        &SharedStore::default(),
    )
    .unwrap();
    let mut first = runtime.scripts(request("GET", "/?who=ann", &[]));
    let mut second = runtime.scripts(request("GET", "/?who=bob", &[]));
    assert_eq!(
        run(&mut first, handler(ids[0], Phase::Access)).await,
        Outcome::Continue
    );
    assert_eq!(
        run(&mut second, handler(ids[0], Phase::Access)).await,
        Outcome::Continue
    );
    assert_eq!(
        run(&mut second, handler(ids[1], Phase::Content)).await,
        Outcome::Respond
    );
    assert_eq!(second.exchange().response.body, b"bob bob 2\n");
    assert_eq!(
        run(&mut first, handler(ids[1], Phase::Content)).await,
        Outcome::Respond
    );
    assert_eq!(first.exchange().response.body, b"ann ann 1\n");
}

#[tokio::test]
async fn init_defines_globals_every_request_reads_but_cannot_change() {
    let (program, ids) = program(|builder| {
        let init = builder.handler(&source(
            "greeting = 'hello' settings = { level = 1 } ngx.log(ngx.NOTICE, 'ready')",
        ));
        builder.init(init);
        vec![
            builder.handler(&source("ngx.say(greeting, ' ', settings.level)")),
            builder.handler(&source("settings.level = 2")),
            builder.handler(&source("string.upper = nil")),
        ]
    });
    let (runtime, logs) = Runtime::start(
        &program,
        &Settings {
            vms: 1,
            memory: 16 << 20,
        },
        &SharedStore::default(),
    )
    .unwrap();
    assert!(
        logs.iter().any(|entry| entry.message.ends_with("ready")),
        "{logs:?}"
    );
    let mut scripts = runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        run(&mut scripts, handler(ids[0], Phase::Content)).await,
        Outcome::Respond
    );
    assert_eq!(scripts.exchange().response.body, b"hello 1\n");
    for changing in [ids[1], ids[2]] {
        let mut scripts = runtime.scripts(request("GET", "/", &[]));
        let outcome = run(&mut scripts, handler(changing, Phase::Access)).await;
        let Outcome::Failed(failure) = outcome else {
            panic!("{outcome:?}");
        };
        assert!(failure.message.contains("readonly"), "{}", failure.message);
    }
}

#[tokio::test]
async fn the_sandbox_has_no_host_access() {
    let lua = start(
        1,
        handlers(&[r#"
        assert(io == nil and os.execute == nil and os.exit == nil and os.getenv == nil)
        assert(getfenv == nil and setfenv == nil and dofile == nil and loadfile == nil)
        assert(not pcall(require, "ffi") and not pcall(require, "jit") and not pcall(require, "os"))
        assert(package.loadlib == nil and debug.getregistry == nil)
        assert(type(os.time()) == "number" and type(os.clock()) == "number")
        ngx.say("sealed")
    "#]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{outcome:?}");
}

#[tokio::test]
async fn phases_and_permissions_refuse_what_they_do_not_allow() {
    let lua = start(
        1,
        handlers(&[
            "ngx.say('late')",
            "ngx.req.read_body()",
            "require('ngx.balancer').set_current_peer('10.0.0.5', 8080)",
            "ngx.req.set_body_file('/tmp/body')",
        ]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::HeaderFilter)).await;
    let Outcome::Failed(failure) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(failure.kind, FailureKind::Refused);
    assert!(failure
        .message
        .contains("API disabled in the context of header_filter_by_lua*"));

    let mut scripts = lua.runtime.scripts(request("POST", "/", &[]));
    assert_eq!(
        kind(&run(&mut scripts, handler(lua.handlers[1], Phase::Access)).await),
        Some(FailureKind::Refused)
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        kind(&run(&mut scripts, handler(lua.handlers[2], Phase::Balancer)).await),
        Some(FailureKind::Refused)
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[3], Phase::Content)).await;
    let Outcome::Failed(failure) = outcome else {
        panic!("{outcome:?}");
    };
    assert!(
        failure
            .message
            .contains("ngx.req.set_body_file is not available"),
        "{}",
        failure.message
    );
}

struct Body(&'static str);

#[async_trait]
impl Host for Body {
    async fn read_body(&mut self, limit: usize) -> Result<Bytes, String> {
        tokio::task::yield_now().await;
        if self.0.len() > limit {
            return Err("too large".into());
        }
        Ok(Bytes::from_static(self.0.as_bytes()))
    }
}

#[tokio::test]
async fn granted_scripts_read_bodies_and_choose_peers() {
    let lua = start(1, handlers(&[
        "ngx.req.read_body() local args = ngx.req.get_post_args() ngx.say(args.name, ' ', ngx.req.get_body_data())",
        "local b = require('ngx.balancer') assert(b.set_current_peer('10.0.0.5', 8080)) b.set_more_tries(2) b.set_timeouts(1, 2, 3)",
        "ngx.arg[1] = string.upper(ngx.arg[1]) if ngx.arg[2] then ngx.arg[1] = ngx.arg[1] .. '!' end",
    ]));
    let mut granted = handler(lua.handlers[0], Phase::Content);
    granted.permissions.body = true;
    let mut scripts = lua.runtime.scripts(request("POST", "/", &[]));
    assert_eq!(
        scripts.run(granted, &mut Body("name=ann&x=1")).await,
        Outcome::Respond
    );
    assert_eq!(scripts.exchange().response.body, b"ann name=ann&x=1\n");

    let mut balancer = handler(lua.handlers[1], Phase::Balancer);
    balancer.permissions.upstream = true;
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(run(&mut scripts, balancer).await, Outcome::Continue);
    {
        let exchange = scripts.exchange();
        let peer = exchange.balancer.peer.as_ref().unwrap();
        assert_eq!((peer.host.as_str(), peer.port), ("10.0.0.5", 8080));
        assert_eq!(exchange.balancer.more_tries, Some(2));
        assert_eq!(
            exchange.balancer.timeouts.read,
            Some(Duration::from_secs(3))
        );
    }

    let mut filter = handler(lua.handlers[2], Phase::BodyFilter);
    filter.permissions.body = true;
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    {
        let mut exchange = scripts.exchange();
        exchange.chunk.data = Bytes::from_static(b"hello");
        exchange.chunk.eof = true;
    }
    assert_eq!(run(&mut scripts, filter).await, Outcome::Continue);
    assert_eq!(&scripts.exchange().chunk.data[..], b"HELLO!");
}

#[tokio::test]
async fn regular_expressions_json_bits_and_codecs_follow_openresty() {
    let lua = start(
        1,
        handlers(&[r#"
        local m = ngx.re.match("hello, 1234", "([0-9])(?<rest>[0-9]+)", "jo")
        assert(m[0] == "1234" and m[1] == "1" and m.rest == "234")
        local miss = ngx.re.match("hello, world", "(world)|(hello)|(?<named>howdy)")
        assert(miss[1] == false and miss[2] == "hello" and miss.named == false)
        local ctx = { pos = 2 }
        local from, to = ngx.re.find("a1b22c333", "[0-9]+", "jo", ctx)
        assert(from == 2 and to == 2 and ctx.pos == 3)
        local s, n = ngx.re.gsub("hello, 1234", "([0-9])[0-9]", "[$0][${1}]$$")
        assert(s == "hello, [12][1]$[34][3]$" and n == 2, s)
        local up = ngx.re.gsub("a-b-c", "[a-z]", function(m) return string.upper(m[0]) end)
        assert(up == "A-B-C")
        local parts = require("ngx.re").split("a,b,c,d", "(,)")
        assert(#parts == 7 and parts[2] == ",")
        local limited = ngx.re.split("a,b,c,d", ",", nil, nil, 3)
        assert(#limited == 3 and limited[3] == "c,d")
        local seen = {}
        for word in ngx.re.gmatch("one two three", "\\w+") do seen[#seen + 1] = word[0] end
        assert(#seen == 3 and seen[3] == "three")
        assert(ngx.re.match("ABC", "abc", "i") and not ngx.re.match("xABC", "abc", "ai"))

        local cjson = require "cjson"
        assert(cjson.encode({}) == "{}" and cjson.encode(cjson.empty_array) == "[]")
        assert(cjson.encode({1, 2, "a/b"}) == '[1,2,"a\\/b"]')
        assert(cjson.encode({ok = true, n = 1.5}) == '{"ok":true,"n":1.5}' or cjson.encode({ok = true, n = 1.5}) == '{"n":1.5,"ok":true}')
        local decoded = cjson.decode('{"a":[1,2,{"b":null}]}')
        assert(decoded.a[2] == 2 and decoded.a[3].b == cjson.null)
        assert(not pcall(cjson.decode, "{bad"))
        local value, err = require("cjson.safe").decode("{bad")
        assert(value == nil and err)

        local bit = require "bit"
        assert(bit.band(0xff, 0x0f) == 15 and bit.bxor(1, 3) == 2 and bit.tobit(0xffffffff) == -1)
        assert(bit.lshift(1, 31) == -2147483648 and bit.rshift(-1, 28) == 15 and bit.tohex(255, 4) == "00ff")

        assert(ngx.escape_uri("a b/c") == "a%20b%2Fc" and ngx.unescape_uri("b%20r56+7") == "b r56 7")
        assert(ngx.encode_args({b = 2, a = {"x", "y"}, c = true}) == "a=x&a=y&b=2&c")
        assert(ngx.decode_base64(ngx.encode_base64("hi")) == "hi" and ngx.md5("") == "d41d8cd98f00b204e9800998ecf8427e")
        assert(require("resty.string").to_hex(require("resty.sha256"):new() and ngx.sha1_bin("abc")) == "a9993e364706816aba3e25717850c26c9cd0d89d")
        local sha = require("resty.sha256").new() sha:update("abc")
        assert(require("resty.string").to_hex(sha:final()) == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        assert(ngx.quote_sql_str("a'b") == "'a\\'b'" and ngx.crc32_long("hello") == 907060870)
        assert(ngx.http_time(1290079655) == "Thu, 18 Nov 2010 11:27:35 GMT" and ngx.parse_http_time("Thu, 18 Nov 2010 11:27:35 GMT") == 1290079655)
        local t = require("table.new")(4, 0) t[1] = 1 require("table.clear")(t) assert(next(t) == nil)
        assert(require("table.nkeys")({a = 1, b = 2, 3}) == 3)
        assert(ngx.get_phase() == "content" and ngx.worker.count() == 1)
        ngx.say("ok")
    "#]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{outcome:?}");
}

#[tokio::test]
async fn shared_dictionaries_are_one_for_every_vm() {
    let (program, ids) = program(|builder| {
        builder.shared_dict("hits", 1 << 20);
        vec![builder.handler(&source(
            "local hits = ngx.shared.hits local n = hits:incr('n', 1, 0) hits:set('last', ngx.var.arg_who, 60) ngx.say(n, ' ', hits:get('last'))",
        ))]
    });
    let store = SharedStore::default();
    let (runtime, _) = Runtime::start(
        &program,
        &Settings {
            vms: 2,
            memory: 16 << 20,
        },
        &store,
    )
    .unwrap();
    for (who, expected) in [("ann", "1 ann\n"), ("bob", "2 bob\n"), ("cy", "3 cy\n")] {
        let mut scripts = runtime.scripts(request("GET", &format!("/?who={who}"), &[]));
        assert_eq!(
            run(&mut scripts, handler(ids[0], Phase::Content)).await,
            Outcome::Respond
        );
        assert_eq!(scripts.exchange().response.body, expected.as_bytes());
    }
    let (restarted, _) = Runtime::start(
        &program,
        &Settings {
            vms: 1,
            memory: 16 << 20,
        },
        &store,
    )
    .unwrap();
    let mut scripts = restarted.scripts(request("GET", "/?who=dee", &[]));
    assert_eq!(
        run(&mut scripts, handler(ids[0], Phase::Content)).await,
        Outcome::Respond
    );
    assert_eq!(scripts.exchange().response.body, b"4 dee\n");
}

#[tokio::test]
async fn sleeping_waits_without_holding_the_thread() {
    let lua = start(1, handlers(&["ngx.sleep(0.05) ngx.say('rested')"]));
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let started = Instant::now();
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await,
        Outcome::Respond
    );
    assert!(started.elapsed() >= Duration::from_millis(50));
}

#[tokio::test]
async fn logs_below_the_level_are_dropped() {
    let lua = start(
        1,
        handlers(&[
            "ngx.log(ngx.INFO, 'chatty') ngx.log(ngx.ERR, 'bad ', nil, ' ', 3) print('noted')",
        ]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let mut quiet = handler(lua.handlers[0], Phase::Access);
    quiet.log_level = LogLevel::Notice;
    assert_eq!(run(&mut scripts, quiet).await, Outcome::Continue);
    let exchange = scripts.exchange();
    let messages: Vec<_> = exchange
        .logs
        .iter()
        .map(|entry| (entry.level, entry.message.as_str()))
        .collect();
    assert_eq!(
        messages,
        [
            (LogLevel::Err, "main.conf:1: bad nil 3"),
            (LogLevel::Notice, "main.conf:1: noted"),
        ]
    );
}

#[test]
fn syntax_errors_point_at_their_line_in_the_file() {
    let mut builder = Program::builder();
    builder.handler(&Source::new("main.conf", "local x = 1\nif x then\n", 12));
    let diagnostics = builder.build().unwrap_err();
    assert_eq!(diagnostics[0].source, "main.conf");
    assert_eq!(diagnostics[0].line, Some(14));
}

#[tokio::test]
async fn panel_v1_reads_and_answers_requests_without_ngx_quirks() {
    let lua = start(
        1,
        handlers(&[
            r#"
            local panel = require("panel.v1")
            assert(panel.version == 1)
            local query = panel.req.query()
            panel.ctx().seen = {
                method = panel.req.method(),
                path = panel.req.path(),
                tags = #query.tag,
                flag = panel.req.query_value("flag"),
                accept = panel.req.header("ACCEPT"),
                cookies = #panel.req.headers().cookie,
                client = panel.req.client(),
                id = panel.req.id(),
            }
            panel.req.set_header("X-Checked", "1")
            panel.req.set_header("X-Drop", nil)
            local ok = pcall(function() panel.req.extra = 1 end)
            assert(not ok, "module tables are frozen")
            "#,
            r#"
            local panel = require("panel.v1")
            local seen = panel.ctx().seen
            local digest = panel.crypto.sha256("abc")
            local mac = panel.crypto.hmac_sha256("key", "The quick brown fox jumps over the lazy dog")
            local body = panel.json.encode({
                seen = seen,
                digest = digest,
                mac = mac,
                same = panel.crypto.equal(mac, mac),
                differ = panel.crypto.equal(mac, digest),
                uuid = #panel.random.uuid(),
                bytes = #panel.random.bytes(16),
                round = panel.crypto.unbase64(panel.crypto.base64("hi")),
                match = panel.re.match("item-42", [[(\d+)]])[1],
                replaced = (panel.re.replace("a-b-c", "-", "+")),
                http = panel.time.http(0),
                rfc3339 = panel.time.rfc3339(0),
                decoded = panel.json.decode('{"a":[1,2]}').a[2],
            })
            return panel.resp.send(201, body, { ["Content-Type"] = "application/json" })
            "#,
        ]),
    );
    let mut scripts = lua.runtime.scripts(request(
        "GET",
        "/items?tag=a&tag=b&flag",
        &[
            ("accept", "text/html"),
            ("cookie", "a=1"),
            ("cookie", "b=2"),
            ("x-drop", "gone"),
        ],
    ));
    let access = run(&mut scripts, handler(lua.handlers[0], Phase::Access)).await;
    assert!(matches!(access, Outcome::Continue), "{access:?}");
    {
        let exchange = scripts.exchange();
        assert_eq!(exchange.request.headers["x-checked"], "1");
        assert!(!exchange.request.headers.contains_key("x-drop"));
    }
    let content = run(&mut scripts, handler(lua.handlers[1], Phase::Content)).await;
    assert!(matches!(content, Outcome::Respond), "{content:?}");
    let exchange = scripts.exchange();
    assert_eq!(exchange.response.status, 201);
    assert_eq!(
        exchange.response.headers["content-type"],
        "application/json"
    );
    let body: serde_json::Value = serde_json::from_slice(&exchange.response.body).unwrap();
    assert_eq!(
        body["seen"],
        serde_json::json!({
            "method": "GET", "path": "/items", "tags": 2, "flag": "", "accept": "text/html",
            "cookies": 2, "client": "192.0.2.7", "id": "0b1d0c8e9f2a4b6c",
        })
    );
    assert_eq!(
        body["digest"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        body["mac"],
        "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
    );
    assert_eq!(
        (body["same"].clone(), body["differ"].clone()),
        (true.into(), false.into())
    );
    assert_eq!(
        (body["uuid"].clone(), body["bytes"].clone()),
        (36.into(), 16.into())
    );
    assert_eq!(body["round"], "hi");
    assert_eq!(body["match"], "42");
    assert_eq!(body["replaced"], "a+b+c");
    assert_eq!(body["http"], "Thu, 01 Jan 1970 00:00:00 GMT");
    assert_eq!(body["rfc3339"], "1970-01-01T00:00:00Z");
    assert_eq!(body["decoded"], 2);
}

/// A service that answers `PING` with `PONG` and `BODY` with a body with a
/// boundary in it, counting the connections it accepts.
async fn line_service() -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&accepted);
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            counted.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let answer: &[u8] = match line.as_str() {
                        "PING" => b"PONG\r\n",
                        "BODY" => b"abc--sep--def",
                        _ => b"?\r\n",
                    };
                    if write.write_all(answer).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (port, accepted)
}

#[tokio::test]
async fn cosockets_talk_to_services_and_keep_connections_when_granted() {
    let (port, accepted) = line_service().await;
    let script = format!(
        r#"
        local sock = ngx.socket.tcp()
        sock:settimeouts(1000, 1000, 1000)
        assert(sock:connect("127.0.0.1", {port}))
        local reused = sock:getreusedtimes()
        assert(sock:send({{"PING", "\r\n"}}))
        local line = assert(sock:receive())
        assert(sock:send("BODY\r\n"))
        local reader = sock:receiveuntil("--sep--")
        local before = assert(reader())
        local after = assert(sock:receive(3))
        assert(sock:setkeepalive(10000, 4))
        ngx.say(line, " ", before, " ", after, " ", reused)
        "#
    );
    let lua = start(
        1,
        handlers(&[
            &script,
            "local sock = ngx.socket.tcp() sock:connect('127.0.0.1', 9)",
        ]),
    );
    let mut granted = handler(lua.handlers[0], Phase::Content);
    granted.permissions.network = true;
    for expected in ["PONG abc def 0\n", "PONG abc def 1\n"] {
        let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
        let outcome = run(&mut scripts, granted).await;
        assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
        assert_eq!(scripts.exchange().response.body, expected.as_bytes());
    }
    assert_eq!(
        accepted.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the second request reuses the kept connection"
    );

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let refused = run(&mut scripts, handler(lua.handlers[1], Phase::Content)).await;
    let Outcome::Failed(failure) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(failure.kind, FailureKind::Refused);
    assert!(
        failure.message.contains("network permission"),
        "{}",
        failure.message
    );

    let mut in_filter = handler(lua.handlers[1], Phase::HeaderFilter);
    in_filter.permissions.network = true;
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let Outcome::Failed(failure) = run(&mut scripts, in_filter).await else {
        panic!("sockets are not for header filters");
    };
    assert!(
        failure.message.contains("header_filter_by_lua"),
        "{}",
        failure.message
    );
}

#[tokio::test]
async fn cosockets_verify_certificates_unless_told_not_to() {
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let chain = vec![certified.cert.der().clone()];
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(chain, key.into())
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(stream).await {
                    let _ = tls.write_all(b"HELLO\r\n").await;
                    let _ = tls.flush().await;
                }
            });
        }
    });
    let trusting = format!(
        r#"
        local sock = ngx.socket.tcp()
        assert(sock:connect("127.0.0.1", {port}))
        assert(sock:sslhandshake(nil, "localhost", false))
        ngx.say(assert(sock:receive()))
        "#
    );
    let verifying = format!(
        r#"
        local sock = ngx.socket.tcp()
        assert(sock:connect("127.0.0.1", {port}))
        local ok, err = sock:sslhandshake(nil, "localhost")
        ngx.say(tostring(ok), " ", err)
        "#
    );
    let lua = start(1, handlers(&[&trusting, &verifying]));
    for (index, expected) in [(0, "HELLO\n"), (1, "nil handshake failed")] {
        let mut granted = handler(lua.handlers[index], Phase::Content);
        granted.permissions.network = true;
        let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
        assert_eq!(run(&mut scripts, granted).await, Outcome::Respond);
        let body = String::from_utf8(scripts.exchange().response.body.clone()).unwrap();
        assert!(body.starts_with(expected), "{body}");
    }
}

#[tokio::test]
async fn light_threads_run_together_and_the_run_waits_for_them() {
    let lua = start(
        1,
        handlers(&[
            r#"
            local order = {}
            local function work(name, seconds)
                ngx.sleep(seconds)
                return name, seconds
            end
            ngx.thread.spawn(function()
                order[#order + 1] = "child"
                ngx.sleep(0.01)
                ngx.log(ngx.NOTICE, "unwaited thread ended")
            end)
            order[#order + 1] = "parent"
            local started = ngx.now()
            local slow = ngx.thread.spawn(work, "slow", 0.2)
            local fast = ngx.thread.spawn(work, "fast", 0.1)
            local ok, name = ngx.thread.wait(slow, fast)
            assert(ok and name == "fast", name)
            local ok2, name2, seconds = ngx.thread.wait(slow)
            assert(ok2 and name2 == "slow" and seconds == 0.2)
            local again, err = ngx.thread.wait(fast)
            assert(again == nil and err == "already waited or killed", err)

            local bad = ngx.thread.spawn(function() error("boom") end)
            local failed, why = ngx.thread.wait(bad)
            assert(failed == false and why:find("boom"), why)

            local sleeper = ngx.thread.spawn(function() ngx.sleep(30) end)
            assert(ngx.thread.kill(sleeper))
            local killed, kill_err = ngx.thread.kill(sleeper)
            assert(killed == nil and kill_err == "already waited or killed")
            ngx.say(table.concat(order, ","), " ", ngx.now() - started < 0.35)
            "#,
            "ngx.thread.spawn(function() ngx.sleep(0.01) ngx.exit(204) end) ngx.sleep(30)",
            "ngx.thread.spawn(function() end)",
        ]),
    );
    let mut threads = handler(lua.handlers[0], Phase::Content);
    threads.limits.time = Duration::from_secs(2);
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, threads).await;
    {
        let exchange = scripts.exchange();
        assert_eq!(outcome, Outcome::Respond, "{:?}", exchange.logs);
        assert_eq!(exchange.response.body, b"child,parent true\n");
        let messages: Vec<&str> = exchange
            .logs
            .iter()
            .map(|log| log.message.as_str())
            .collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("lua user thread aborted")
                    && message.contains("boom")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.ends_with("unwaited thread ended")),
            "{messages:?}"
        );
    }

    let mut exiting = handler(lua.handlers[1], Phase::Content);
    exiting.limits.time = Duration::from_secs(2);
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let started = std::time::Instant::now();
    assert_eq!(run(&mut scripts, exiting).await, Outcome::Respond);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(scripts.exchange().response.status, 204);

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let refused = run(&mut scripts, handler(lua.handlers[2], Phase::HeaderFilter)).await;
    let Outcome::Failed(failure) = refused else {
        panic!("{refused:?}");
    };
    assert!(
        failure.message.contains("header_filter_by_lua"),
        "{}",
        failure.message
    );
}

fn timer_messages(runs: &std::sync::Mutex<Vec<panel_lua::TimerRun>>) -> Vec<String> {
    runs.lock()
        .unwrap()
        .iter()
        .flat_map(|run| run.logs.iter().map(|log| log.message.clone()))
        .collect()
}

#[tokio::test]
async fn timers_run_later_and_at_once_when_their_runtime_is_dropped() {
    let runs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let lua = start(1, |builder| {
        let worker = builder.handler(&source(
            r#"assert(ngx.timer.at(0, function(premature)
                ngx.log(ngx.NOTICE, "init_worker timer ", tostring(premature))
            end))"#,
        ));
        builder.init_worker(worker);
        vec![builder.handler(&source(
            r#"
            assert(ngx.timer.at(0.01, function(premature, word, n)
                ngx.log(ngx.NOTICE, "at ", word, " ", n, " ", tostring(premature), " ", ngx.get_phase())
            end, "hello", 2))
            local every = 0
            assert(ngx.timer.every(0.02, function()
                every = every + 1
                ngx.log(ngx.NOTICE, "every ", every)
            end))
            assert(ngx.timer.at(60, function(premature)
                ngx.log(ngx.NOTICE, "late ", tostring(premature))
            end))
            assert(ngx.timer.at(0, function() ngx.req.get_headers() end))
            ngx.say(ngx.timer.pending_count() >= 4)
            "#,
        ))]
    });
    let kept = std::sync::Arc::clone(&runs);
    lua.runtime
        .on_timer(move |run| kept.lock().unwrap().push(run));
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"true\n");
    drop(scripts);
    tokio::time::sleep(Duration::from_millis(150)).await;

    let seen = timer_messages(&runs);
    assert!(
        seen.iter()
            .any(|message| message.ends_with("init_worker timer false")),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|message| message.ends_with("at hello 2 false timer")),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .filter(|message| message.contains("every "))
            .count()
            >= 2,
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|message| message.contains("late")),
        "{seen:?}"
    );
    let refused: Vec<_> = runs
        .lock()
        .unwrap()
        .iter()
        .filter_map(|run| run.failure.clone())
        .collect();
    assert!(
        refused
            .iter()
            .any(|failure| failure.kind == FailureKind::Refused
                && failure.message.contains("context of ngx.timer")),
        "{refused:?}"
    );

    drop(lua);
    for _ in 0..100 {
        if timer_messages(&runs)
            .iter()
            .any(|message| message.ends_with("late true"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        timer_messages(&runs)
            .iter()
            .any(|message| message.ends_with("late true")),
        "the pending timer runs, premature, once the runtime is dropped"
    );
}

#[tokio::test]
async fn isolated_runtimes_run_no_timers_and_open_no_connections() {
    let runs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let lua = start(
        1,
        handlers(&[
            "assert(ngx.timer.at(60, function() ngx.log(ngx.ERR, 'ran') end))",
            "assert(ngx.timer.at(0, function() ngx.log(ngx.ERR, 'ran') end))",
            "local sock = ngx.socket.tcp() assert(sock:connect('127.0.0.1', 9))",
        ]),
    );
    let kept = std::sync::Arc::clone(&runs);
    lua.runtime
        .on_timer(move |run| kept.lock().unwrap().push(run));
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[0], Phase::Access)).await,
        Outcome::Continue
    );
    drop(scripts);
    lua.runtime.isolate();

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[1], Phase::Access)).await,
        Outcome::Continue
    );
    assert!(scripts.exchange().logs[0]
        .message
        .contains("a timer created in a test does not run"));
    let mut granted = handler(lua.handlers[2], Phase::Access);
    granted.permissions.network = true;
    let Outcome::Failed(failure) = run(&mut scripts, granted).await else {
        panic!("a test opens no connections");
    };
    assert_eq!(failure.kind, FailureKind::Refused);
    assert!(
        failure.message.contains("not opened in a test"),
        "{}",
        failure.message
    );
    drop(scripts);
    drop(lua);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(runs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn semaphores_hand_resources_to_waiters_in_order() {
    let lua = start(
        1,
        handlers(&[r#"
        local semaphore = require("ngx.semaphore")
        local sema = semaphore.new()
        local out = {}
        local function handler(id)
            local ok, err = sema:wait(1)
            out[#out + 1] = id .. " " .. (ok and "ok" or err)
        end
        assert(sema:count() == 0)
        local first = ngx.thread.spawn(handler, "a")
        local second = ngx.thread.spawn(handler, "b")
        assert(sema:count() == -2, sema:count())
        sema:post(1)
        assert(sema:count() == -1, sema:count())
        sema:post(2)
        assert(sema:count() == 1, sema:count())
        ngx.thread.wait(first)
        ngx.thread.wait(second)
        assert(sema:wait(0))
        local none, why = sema:wait(0)
        assert(none == nil and why == "timeout")
        local late, late_why = sema:wait(0.01)
        assert(late == nil and late_why == "timeout")
        assert(sema:count() == 0, sema:count())
        assert(ngx.timer.at(0, function() sema:post() end))
        assert(sema:wait(1))
        local negative = select(2, pcall(semaphore.new, -1))
        local zero = select(2, pcall(sema.post, sema, 0))
        ngx.say(table.concat(out, ","), "|", negative:match("no negative number") ~= nil,
            "|", zero:match("positive number required") ~= nil)
        "#]),
    );
    let mut waiting = handler(lua.handlers[0], Phase::Content);
    waiting.limits.time = Duration::from_secs(2);
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, waiting).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"a ok,b ok|true|true\n");
}

#[tokio::test]
async fn udp_cosockets_send_and_receive_datagrams_when_granted() {
    let service = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = service.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut buffer = [0u8; 1024];
        while let Ok((read, from)) = service.recv_from(&mut buffer).await {
            let answer = buffer[..read].to_ascii_uppercase();
            let _ = service.send_to(&answer, from).await;
        }
    });
    let script = format!(
        r#"
        assert(ngx.socket.stream == ngx.socket.tcp)
        local udp = ngx.socket.udp()
        udp:settimeout(1000)
        local unix, unix_err = udp:setpeername("unix:/tmp/service.sock")
        assert(unix == nil and unix_err:find("unix domain"), unix_err)
        assert(udp:setpeername("127.0.0.1", {port}))
        assert(udp:send({{"pi", "ng"}}))
        local data = assert(udp:receive())
        assert(udp:close())
        local closed, err = udp:close()
        ngx.say(data, " ", tostring(closed), " ", err)
        "#
    );
    let lua = start(1, handlers(&[&script]));
    let mut granted = handler(lua.handlers[0], Phase::Content);
    granted.permissions.network = true;
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, granted).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"PING nil closed\n");

    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let Outcome::Failed(failure) =
        run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await
    else {
        panic!("datagrams need the network permission");
    };
    assert!(
        failure
            .message
            .contains("ngx.socket.udp needs the network permission"),
        "{}",
        failure.message
    );
}

#[tokio::test]
async fn scripts_build_a_new_request_body_piece_by_piece() {
    let lua = start(
        1,
        handlers(&[
            r#"
            ngx.req.read_body()
            ngx.req.init_body(8)
            ngx.req.append_body("new ")
            ngx.req.append_body("body")
            ngx.req.finish_body()
            assert(ngx.req.get_body_file() == nil)
            assert(ngx.req.get_body_data() == "new body")
            "#,
            "ngx.req.append_body('x')",
        ]),
    );
    let mut granted = handler(lua.handlers[0], Phase::Access);
    granted.permissions.body = true;
    let mut scripts = lua.runtime.scripts(request("POST", "/", &[]));
    assert_eq!(
        scripts.run(granted, &mut Body("old")).await,
        Outcome::Continue
    );
    {
        let exchange = scripts.exchange();
        assert_eq!(exchange.request.body.as_deref(), Some(&b"new body"[..]));
        assert!(exchange.changes().body);
    }

    let mut unstarted = handler(lua.handlers[1], Phase::Access);
    unstarted.permissions.body = true;
    let mut scripts = lua.runtime.scripts(request("POST", "/", &[]));
    let Outcome::Failed(failure) = run(&mut scripts, unstarted).await else {
        panic!("append_body needs init_body first");
    };
    assert!(
        failure.message.contains("request body not initialized"),
        "{}",
        failure.message
    );
}

#[tokio::test]
async fn errors_of_host_functions_reach_pcall_as_strings() {
    let lua = start(
        1,
        handlers(&[r#"
        local cjson = require("cjson")
        local ok, err = pcall(cjson.decode, "{")
        assert(not ok and type(err) == "string", type(err))
        local _, handled = xpcall(cjson.decode, function(e) return "handled: " .. e end, "[")
        assert(type(handled) == "string" and handled:find("^handled: "), handled)
        local _, own = pcall(error, { code = 7 })
        assert(own.code == 7)
        local _, raised = pcall(error, "plain")
        ngx.say(err:sub(1, 8), "|", raised)
        "#]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    let body = String::from_utf8(scripts.exchange().response.body.clone()).unwrap();
    assert!(body.ends_with("|plain\n"), "{body}");
}

#[tokio::test]
async fn exec_redirects_internally_and_ends_the_handler() {
    let lua = start(
        1,
        handlers(&[
            r#"ngx.exec("/next?a=1", { b = "x y" }) ngx.say("not reached")"#,
            "ngx.say(tostring(ngx.req.is_internal()))",
            "ngx.say('sent') ngx.exec('/next')",
            "ngx.exec('@named')",
            "ngx.exec('/../etc')",
        ]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/first?z=9", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await,
        Outcome::Continue
    );
    {
        let exchange = scripts.exchange();
        assert!(exchange.redirected());
        assert_eq!(exchange.request.uri, "/next");
        assert_eq!(exchange.request.args.as_deref(), Some("a=1&b=x%20y"));
        assert_eq!(exchange.request.request_uri, "/first?z=9");
        assert!(exchange.response.body.is_empty());
    }
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[1], Phase::Content)).await,
        Outcome::Respond
    );
    assert_eq!(scripts.exchange().response.body, b"true\n");
    assert!(!scripts.exchange().redirected());

    let mut scripts = lua.runtime.scripts(request("GET", "/kept?z=1", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[3], Phase::Content)).await,
        Outcome::Continue
    );
    {
        let mut exchange = scripts.exchange();
        assert!(exchange.redirected());
        assert_eq!(exchange.take_named().as_deref(), Some("named"));
        assert_eq!(
            (
                exchange.request.uri.as_str(),
                exchange.request.args.as_deref()
            ),
            ("/kept", Some("z=1"))
        );
    }

    for (index, wanted) in [(2, "after sending out response headers"), (4, "unsafe uri")] {
        let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
        let Outcome::Failed(failure) =
            run(&mut scripts, handler(lua.handlers[index], Phase::Content)).await
        else {
            panic!("{wanted}");
        };
        assert!(failure.message.contains(wanted), "{}", failure.message);
    }
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let Outcome::Failed(failure) =
        run(&mut scripts, handler(lua.handlers[0], Phase::HeaderFilter)).await
    else {
        panic!("ngx.exec is not for header filters");
    };
    assert!(
        failure.message.contains("header_filter_by_lua"),
        "{}",
        failure.message
    );
}

#[tokio::test]
async fn programs_set_the_regex_cache_match_limit_and_timer_caps() {
    let lua = start(1, |builder| {
        builder.timers(1, 0).regexes(Some(0), 100);
        vec![builder.handler(&source(
            r#"
            local m, err = ngx.re.match(string.rep("a", 24) .. "b", "^(a+)+$")
            assert(m == nil and err:find("limit"), err)
            assert(ngx.re.match("abc", "b")[0] == "b")
            assert(ngx.timer.at(60, function() end))
            local ok, too_many = ngx.timer.at(60, function() end)
            ngx.say(tostring(ok), " ", too_many)
            "#,
        ))]
    });
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(
        scripts.exchange().response.body,
        b"nil too many pending timers\n"
    );
    drop(scripts);
    lua.runtime.isolate();
}

#[tokio::test]
async fn handlers_set_the_defaults_of_their_cosockets_and_headers() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let silent = format!(
        r#"
        local sock = ngx.socket.tcp()
        assert(sock:connect("127.0.0.1", {port}))
        local reused = sock:getreusedtimes()
        local data, err = sock:receive()
        sock:close()
        local again = ngx.socket.tcp()
        assert(again:connect("127.0.0.1", {port}))
        assert(again:setkeepalive())
        ngx.sleep(0.03)
        local third = ngx.socket.tcp()
        assert(third:connect("127.0.0.1", {port}))
        ngx.say(tostring(data), " ", err, " ", reused, " ", third:getreusedtimes())
        "#
    );
    let lua = start(
        1,
        handlers(&[&silent, "ngx.header.x_custom_name = '1' ngx.say('ok')"]),
    );
    let mut quick = handler(lua.handlers[0], Phase::Content);
    quick.permissions.network = true;
    quick.limits.time = Duration::from_secs(2);
    quick.sockets.read_timeout = Duration::from_millis(50);
    quick.sockets.keepalive_timeout = Duration::from_millis(10);
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let started = Instant::now();
    let outcome = run(&mut scripts, quick).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert!(started.elapsed() < Duration::from_secs(1));
    {
        let exchange = scripts.exchange();
        assert_eq!(exchange.response.body, b"nil timeout 0 0\n");
        assert!(
            exchange
                .logs
                .iter()
                .any(|log| log.message == "lua tcp socket receive failed: timeout"),
            "{:?}",
            exchange.logs
        );
    }
    quick.sockets.log_errors = false;
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(run(&mut scripts, quick).await, Outcome::Respond);
    assert!(!scripts
        .exchange()
        .logs
        .iter()
        .any(|log| log.message.contains("socket")));

    let mut kept = handler(lua.handlers[1], Phase::Content);
    kept.transform_underscores = false;
    kept.default_type = false;
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(run(&mut scripts, kept).await, Outcome::Respond);
    {
        let exchange = scripts.exchange();
        assert!(exchange.response.headers.contains_key("x_custom_name"));
        assert!(!exchange.default_type());
    }
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[1], Phase::Content)).await,
        Outcome::Respond
    );
    let exchange = scripts.exchange();
    assert!(exchange.response.headers.contains_key("x-custom-name"));
    assert!(exchange.default_type());
}

/// A request body that arrives in pieces.
struct Pieces(std::collections::VecDeque<&'static [u8]>);

#[async_trait]
impl Host for Pieces {
    async fn read_body(&mut self, _limit: usize) -> Result<Bytes, String> {
        Err("read in pieces".into())
    }

    async fn read_body_chunk(&mut self) -> Result<Option<Bytes>, String> {
        tokio::task::yield_now().await;
        Ok(self.0.pop_front().map(Bytes::from_static))
    }
}

#[tokio::test]
async fn request_sockets_stream_the_body_across_its_pieces() {
    let lua = start(
        1,
        handlers(&[
            r#"
            local sock = assert(ngx.req.socket())
            local reader = sock:receiveuntil("--edge")
            local before = assert(reader())
            local line = assert(sock:receive())
            local rest = assert(sock:receive("*a"))
            local sent, err = sock:send("x")
            ngx.say(before, "|", line, "|", rest, "|", tostring(sent), " ", err)
            "#,
            "local sock, err = ngx.req.socket(true) ngx.say(tostring(sock), ' ', err)",
        ]),
    );
    let mut granted = handler(lua.handlers[0], Phase::Content);
    granted.permissions.body = true;
    let mut scripts = lua.runtime.scripts(request("POST", "/upload", &[]));
    let mut body = Pieces(vec![&b"first pa"[..], b"rt--ed", b"ge\r\nheader line\r\ntail"].into());
    let outcome = scripts.run(granted, &mut body).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(
        scripts.exchange().response.body,
        b"first part||header line\r\ntail|nil not supported on the request socket\n"
    );

    let mut scripts = lua.runtime.scripts(request("POST", "/", &[]));
    let Outcome::Failed(failure) =
        run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await
    else {
        panic!("the request socket needs the body permission");
    };
    assert!(
        failure.message.contains("lua_allow body"),
        "{}",
        failure.message
    );

    let mut raw = handler(lua.handlers[1], Phase::Content);
    raw.permissions.body = true;
    let mut scripts = lua.runtime.scripts(request("POST", "/", &[]));
    assert_eq!(scripts.run(raw, &mut NoHost).await, Outcome::Respond);
    assert_eq!(
        scripts.exchange().response.body,
        b"nil the raw request socket is not available here\n"
    );
}

#[tokio::test]
async fn handlers_can_find_the_body_read_before_they_run() {
    let lua = start(1, handlers(&["ngx.say(ngx.req.get_body_data())"]));
    let mut eager = handler(lua.handlers[0], Phase::Access);
    eager.permissions.body = true;
    eager.read_body_first = true;
    let mut scripts = lua.runtime.scripts(request("POST", "/", &[]));
    let outcome = scripts.run(eager, &mut Body("name=ann")).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"name=ann\n");
}

#[tokio::test]
async fn vms_run_exit_worker_when_their_runtime_is_dropped() {
    let runs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let program = |builder: &mut ProgramBuilder| {
        let exit = builder.handler(&source(
            "ngx.log(ngx.NOTICE, 'leaving ', ngx.worker.id(), ' ', ngx.get_phase())",
        ));
        builder.exit_worker(exit);
        Vec::new()
    };
    let lua = start(2, program);
    let kept = std::sync::Arc::clone(&runs);
    lua.runtime
        .on_timer(move |run| kept.lock().unwrap().push(run));
    drop(lua);
    let runs = runs.lock().unwrap();
    assert_eq!(runs.len(), 2, "one run in each VM");
    for (index, run) in runs.iter().enumerate() {
        assert_eq!(run.phase, Phase::ExitWorker);
        assert!(run.failure.is_none(), "{:?}", run.failure);
        assert!(
            run.logs[0]
                .message
                .ends_with(&format!("leaving {index} exit_worker")),
            "{:?}",
            run.logs
        );
    }
    drop(runs);

    let quiet = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let lua = start(1, program);
    let kept = std::sync::Arc::clone(&quiet);
    lua.runtime
        .on_timer(move |run| kept.lock().unwrap().push(run));
    lua.runtime.isolate();
    drop(lua);
    assert!(
        quiet.lock().unwrap().is_empty(),
        "a test runs no exit_worker"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_threads_run_module_functions_on_threads_of_their_own() {
    let lua = start(1, |builder| {
        builder.module(
            "heavy",
            &Source::new(
                "lua/heavy.lua",
                r#"
                local M = {}
                function M.digest(text, times)
                    local out = text
                    for _ = 1, times do out = ngx.md5(out) end
                    return out, #text
                end
                function M.echo(...) return ... end
                function M.method() return ngx.req.get_method() end
                return M
                "#,
                1,
            ),
        );
        vec![builder.handler(&source(
            r#"
            local ok, digest, length = ngx.run_worker_thread("default", "heavy", "digest", "hello", 3)
            assert(ok and length == 5 and #digest == 32, tostring(digest))
            local same, t = ngx.run_worker_thread("default", "heavy", "echo", { a = { 1, 2 }, b = "x" })
            assert(same and t.a[2] == 2 and t.b == "x")
            local refused, why = ngx.run_worker_thread("default", "heavy", "method")
            assert(refused == false and why:find("ngx.run_worker_thread"), why)
            local missing, err = ngx.run_worker_thread("default", "heavy", "nothing")
            assert(missing == false and err:find("is not a function"), err)
            local passed = pcall(ngx.run_worker_thread, "default", "heavy", "echo", function() end)
            ngx.say(tostring(passed))
            "#,
        ))]
    });
    let mut worker = handler(lua.handlers[0], Phase::Content);
    worker.limits.time = Duration::from_secs(5);
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, worker).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"false\n");
}

#[tokio::test]
async fn set_handlers_give_their_variable_what_they_return() {
    let lua = start(
        1,
        handlers(&[
            "return ngx.arg[1] .. '-' .. #ngx.arg .. '-' .. ngx.var.http_x .. '-' .. ngx.get_phase()",
            "return 42",
            "return {}",
            "ngx.say('no')",
            "ngx.arg[1] = 'x'",
            "ngx.say(ngx.var.a, ' ', ngx.var.b, ' ', ngx.var.c == '')",
        ]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[("x", "h")]));
    let set = |index: usize| handler(lua.handlers[index], Phase::Set);
    let arguments = vec!["one".to_owned(), "two".to_owned()];
    assert_eq!(
        scripts.set(set(0), &mut NoHost, "a", arguments).await,
        Outcome::Continue
    );
    assert_eq!(
        scripts.set(set(1), &mut NoHost, "b", Vec::new()).await,
        Outcome::Continue
    );
    assert_eq!(
        scripts.set(set(2), &mut NoHost, "c", Vec::new()).await,
        Outcome::Continue
    );
    for (index, refusal) in [
        (3, "context of set_by_lua*"),
        (4, "read-only in set_by_lua*"),
    ] {
        let outcome = scripts.set(set(index), &mut NoHost, "d", Vec::new()).await;
        let Outcome::Failed(failure) = outcome else {
            panic!("{outcome:?}");
        };
        assert!(failure.message.contains(refusal), "{}", failure.message);
    }
    assert_eq!(scripts.exchange().variables["a"], "one-2-h-set");
    assert!(!scripts.exchange().variables.contains_key("d"));
    assert_eq!(
        run(&mut scripts, handler(lua.handlers[5], Phase::Content)).await,
        Outcome::Respond
    );
    assert_eq!(scripts.exchange().response.body, b"one-2-h-set 42 true\n");
}

fn pem(label: &str, der: &[u8]) -> Vec<u8> {
    use base64::Engine;
    let text = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in text.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out.into_bytes()
}

#[tokio::test]
async fn tls_terms_set_the_authorities_client_certificate_and_chain_length() {
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair};
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;
    let authority = |name: &str| {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name.push(DnType::CommonName, name);
        params
    };
    let root =
        CertifiedIssuer::self_signed(authority("root"), KeyPair::generate().unwrap()).unwrap();
    let intermediate = CertifiedIssuer::signed_by(
        authority("intermediate"),
        KeyPair::generate().unwrap(),
        &root,
    )
    .unwrap();
    let server_key = KeyPair::generate().unwrap();
    let server = CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .signed_by(&server_key, &intermediate)
        .unwrap();
    let client_key = KeyPair::generate().unwrap();
    let client = CertificateParams::new(vec!["client".into()])
        .unwrap()
        .signed_by(&client_key, &root)
        .unwrap();
    let mut client_roots = rustls::RootCertStore::empty();
    client_roots.add(root.der().clone()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(client_roots),
        Arc::clone(&provider),
    )
    .build()
    .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server.der().clone(), intermediate.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(stream).await {
                    let _ = tls.write_all(b"HELLO\r\n").await;
                    let _ = tls.flush().await;
                }
            });
        }
    });
    let script = format!(
        r#"
        local sock = ngx.socket.tcp()
        assert(sock:connect("127.0.0.1", {port}))
        local ok, err = sock:sslhandshake(nil, "localhost")
        if not ok then return ngx.say("handshake: ", err) end
        local line, err = sock:receive()
        ngx.say(line or ("receive: " .. err))
        "#
    );
    let roots = pem("CERTIFICATE", root.der());
    let chain = pem("CERTIFICATE", client.der());
    let key = pem("PRIVATE KEY", &client_key.serialize_der());
    let terms = |presents: bool, depth: Option<usize>| {
        let mut terms = TlsTerms::default();
        terms.roots = Some(roots.clone());
        if presents {
            terms.client = Some((chain.clone(), key.clone()));
        }
        terms.verify_depth = depth;
        terms
    };
    let mut builder = Program::builder();
    let id = builder.handler(&source(&script));
    let full = builder.tls(terms(true, None));
    let anonymous = builder.tls(terms(false, None));
    let short = builder.tls(terms(true, Some(0)));
    let deep = builder.tls(terms(true, Some(1)));
    assert_eq!(builder.tls(terms(true, None)), full);
    let program = builder.build().unwrap();
    let settings = Settings {
        vms: 1,
        memory: 16 << 20,
    };
    let (runtime, _) = Runtime::start(&program, &settings, &SharedStore::default()).unwrap();
    let system = None;
    for (tls, expected) in [
        (Some(full), "HELLO"),
        (Some(deep), "HELLO"),
        (Some(short), "handshake: handshake failed"),
        (Some(anonymous), "receive: "),
        (system, "handshake: handshake failed"),
    ] {
        let mut granted = handler(id, Phase::Content);
        granted.permissions.network = true;
        granted.sockets.tls = tls;
        let mut scripts = runtime.scripts(request("GET", "/", &[]));
        assert_eq!(run(&mut scripts, granted).await, Outcome::Respond);
        let body = String::from_utf8(scripts.exchange().response.body.clone()).unwrap();
        assert!(body.starts_with(expected), "{tls:?}: {body}");
    }

    let mut builder = Program::builder();
    let mut broken = TlsTerms::default();
    broken.roots = Some(b"not a certificate".to_vec());
    builder.tls(broken);
    let mut unknown = TlsTerms::default();
    unknown.cipher_suites = vec!["TLS_RSA_WITH_RC4_128_MD5".into()];
    builder.tls(unknown);
    let diagnostics = builder.build().unwrap_err();
    let messages: Vec<_> = diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "lua_ssl_trusted_certificate holds no certificate",
            "cipher suite TLS_RSA_WITH_RC4_128_MD5 is not offered"
        ]
    );
}

/// A client that closes the connection at `at`.
struct Leaving {
    at: tokio::time::Instant,
}

#[async_trait]
impl Host for Leaving {
    async fn read_body(&mut self, _limit: usize) -> Result<Bytes, String> {
        Ok(Bytes::new())
    }

    async fn closed(&mut self) {
        tokio::time::sleep_until(self.at).await;
    }
}

#[tokio::test]
async fn clients_that_leave_run_the_abort_callback_or_stop_the_run() {
    let lua = start(
        1,
        handlers(&[
            r#"
            assert(ngx.on_abort(function()
                ngx.log(ngx.NOTICE, "the client left")
                ngx.exit(499)
            end))
            local ok, err = ngx.on_abort(function() end)
            assert(ok == nil and err == "duplicate call", err)
            ngx.sleep(5)
            ngx.say("never")
            "#,
            "ngx.sleep(5) ngx.say('never')",
            r#"
            local ok, err = ngx.on_abort(function() end)
            ngx.sleep(0.05)
            ngx.say(tostring(ok), " ", err)
            "#,
        ]),
    );
    let leaving = || Leaving {
        at: tokio::time::Instant::now() + Duration::from_millis(20),
    };
    let watching = |index: usize| {
        let mut watching = handler(lua.handlers[index], Phase::Content);
        watching.check_client_abort = true;
        watching.limits.time = Duration::from_secs(10);
        watching
    };
    let started = Instant::now();
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = scripts.run(watching(0), &mut leaving()).await;
    assert_eq!(outcome, Outcome::Respond);
    {
        let exchange = scripts.exchange();
        assert!(exchange.client_closed());
        assert_eq!(exchange.response.status, 499);
        assert!(exchange.response.body.is_empty());
        assert!(exchange
            .logs
            .iter()
            .any(|entry| entry.message.ends_with("the client left")));
    }
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        scripts.run(watching(1), &mut leaving()).await,
        Outcome::Abort
    );
    assert!(scripts.exchange().client_closed());
    assert!(started.elapsed() < Duration::from_secs(2));

    let mut unwatched = handler(lua.handlers[2], Phase::Content);
    unwatched.limits.time = Duration::from_secs(1);
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    assert_eq!(
        scripts.run(unwatched, &mut leaving()).await,
        Outcome::Respond
    );
    assert!(!scripts.exchange().client_closed());
    assert_eq!(
        scripts.exchange().response.body,
        b"nil lua_check_client_abort is off\n"
    );
}

#[tokio::test]
async fn handlers_go_on_after_eof_without_output() {
    let lua = start(
        1,
        handlers(&[r#"
            ngx.say("a")
            assert(ngx.eof() == 1)
            local said, err = ngx.say("b")
            local flushed, flush_err = ngx.flush(true)
            local again, again_err = ngx.eof()
            ngx.log(ngx.NOTICE, "after eof ", tostring(said), " ", err, " ",
                tostring(flushed), " ", flush_err, " ", tostring(again), " ", again_err)
        "#]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond);
    let exchange = scripts.exchange();
    assert_eq!(exchange.response.body, b"a\n");
    assert!(
        exchange.logs.iter().any(|entry| entry
            .message
            .ends_with("after eof nil seen eof nil seen eof nil seen eof")),
        "{:?}",
        exchange.logs
    );
}

/// Answers subrequests with their method, path and arguments, and keeps what
/// it was asked.
struct Subrequests {
    asked: Vec<panel_lua::Capture>,
}

#[async_trait]
impl Host for Subrequests {
    async fn read_body(&mut self, _limit: usize) -> Result<Bytes, String> {
        Ok(Bytes::from_static(b"parent body"))
    }

    async fn capture(
        &mut self,
        requests: Vec<panel_lua::Capture>,
    ) -> Result<Vec<panel_lua::Captured>, String> {
        let answers = requests
            .iter()
            .map(|request| {
                let mut headers = HeaderMap::new();
                headers.append("content-type", HeaderValue::from_static("text/plain"));
                headers.append("set-cookie", HeaderValue::from_static("a=1"));
                headers.append("set-cookie", HeaderValue::from_static("b=2"));
                let body = format!(
                    "{} {}?{}",
                    request.method,
                    request.path,
                    request.args.clone().unwrap_or_default()
                );
                let mut captured = panel_lua::Captured::new(201, headers, Bytes::from(body));
                if request.share_variables {
                    captured.variables = Some(std::collections::HashMap::from([(
                        "from_sub".to_owned(),
                        "yes".to_owned(),
                    )]));
                }
                captured
            })
            .collect();
        self.asked.extend(requests);
        Ok(answers)
    }
}

#[tokio::test]
async fn subrequests_carry_their_options_and_come_back_as_tables() {
    let lua = start(
        1,
        handlers(&[r#"
            ngx.req.read_body()
            ngx.var.parent = "p"
            local res = ngx.location.capture("/sub?x=1", {
                args = { y = 2 }, method = ngx.HTTP_POST, vars = { v = "1" },
                copy_all_vars = true, ctx = ngx.ctx,
            })
            assert(res.status == 201, res.status)
            assert(res.header["Content-Type"] == "text/plain")
            assert(res.header.content_type == "text/plain")
            assert(#res.header["Set-Cookie"] == 2)
            assert(res.body == "POST /sub?x=1&y=2", res.body)
            assert(res.truncated == false)
            local a, b = ngx.location.capture_multi({ { "/a" }, { "/b", { share_all_vars = true } } })
            assert(a.body == "GET /a?" and b.body == "GET /b?", a.body .. b.body)
            ngx.say(ngx.var.from_sub)
        "#]),
    );
    let mut granted = handler(lua.handlers[0], Phase::Content);
    granted.permissions.body = true;
    let mut host = Subrequests { asked: Vec::new() };
    let mut scripts = lua.runtime.scripts(request("POST", "/", &[]));
    let outcome = scripts.run(granted, &mut host).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"yes\n");
    let first = &host.asked[0];
    assert_eq!(first.method, http::Method::POST);
    assert_eq!(
        (first.path.as_str(), first.args.as_deref()),
        ("/sub", Some("x=1&y=2"))
    );
    assert_eq!(first.body.as_deref(), Some(&b"parent body"[..]));
    assert_eq!(first.variables["parent"], "p");
    assert_eq!(first.variables["v"], "1");
    assert!(first.share.is_some());
    assert!(host.asked[1].body.is_none() && host.asked[1].variables.is_empty());
    assert!(host.asked[2].share_variables);
}

#[tokio::test]
async fn resty_core_modules_and_lrucache_work_as_in_openresty() {
    let lua = start(
        1,
        handlers(&[r#"
            local lrucache = require "resty.lrucache"
            local c = assert(lrucache.new(2))
            c:set("a", 1)
            c:set("b", 2, 0, 7)
            assert(c:get("a") == 1)
            c:set("c", 3)
            assert(c:get("b") == nil, "b was used least recently")
            assert(c:get("a") == 1)
            assert(c:count() == 2 and c:capacity() == 2)
            local keys = c:get_keys()
            assert(keys[1] == "a" and keys[2] == "c" and keys[3] == nil)
            c:set("t", "x", 0.001, 5)
            ngx.sleep(0.01)
            local fresh, stale, flags = c:get("t")
            assert(fresh == nil and stale == "x" and flags == 5)
            assert(c:delete("t") and not c:delete("t"))
            c:flush_all()
            assert(c:count() == 0 and c:get("a") == nil)
            assert(require("resty.lrucache.pureffi").new(1))
            assert(lrucache.new(0) == nil)

            local resp = require "ngx.resp"
            resp.add_header("X-Multi", "1")
            resp.add_header("X-Multi", "2")
            local req = require "ngx.req"
            req.add_header("X-Added", "a")
            req.add_header("X-Added", { "b", "c" })
            local process = require "ngx.process"
            assert(process.type() == "worker" and process.get_master_pid() > 0)
            local ok, err = process.enable_privileged_agent()
            assert(ok == nil and err:find("privileged"), err)
            ngx.say(#ngx.req.get_headers()["X-Added"])
        "#]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    let exchange = scripts.exchange();
    assert_eq!(exchange.response.body, b"3\n");
    assert_eq!(
        exchange.response.headers.get_all("x-multi").iter().count(),
        2
    );
}

#[tokio::test]
async fn errlog_reads_back_what_scripts_logged() {
    let lua = start(1, |builder| {
        builder.capture_error_log(4096);
        vec![builder.handler(&source(
            r#"
            local errlog = require "ngx.errlog"
            ngx.log(ngx.ERR, "first")
            errlog.raw_log(ngx.WARN, "plain")
            local logs = assert(errlog.get_logs(10))
            assert(#logs == 6, #logs)
            assert(logs[1] == ngx.ERR and type(logs[2]) == "number" and logs[3]:find("first$"))
            assert(logs[4] == ngx.WARN and logs[6] == "plain", logs[6])
            assert(#assert(errlog.get_logs()) == 0)
            assert(errlog.get_sys_filter_level() == ngx.DEBUG)
            ngx.say("ok")
            "#,
        ))]
    });
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"ok\n");
    assert!(scripts
        .exchange()
        .logs
        .iter()
        .any(|entry| entry.message == "plain"));

    let unconfigured = start(
        1,
        handlers(&[r#"
            local logs, err = require("ngx.errlog").get_logs()
            ngx.say(tostring(logs), " ", err)
        "#]),
    );
    let mut scripts = unconfigured.runtime.scripts(request("GET", "/", &[]));
    run(
        &mut scripts,
        handler(unconfigured.handlers[0], Phase::Content),
    )
    .await;
    assert_eq!(
        scripts.exchange().response.body,
        b"nil the 'lua_capture_error_log' directive is not configured\n"
    );
}

/// A ClientHello for `shop.example` offering TLS 1.3 and 1.2, and a GREASE
/// cipher.
fn client_hello() -> Vec<u8> {
    let name = b"shop.example";
    let mut extensions = vec![0, 0];
    extensions.extend((name.len() as u16 + 5).to_be_bytes());
    extensions.extend((name.len() as u16 + 3).to_be_bytes());
    extensions.push(0);
    extensions.extend((name.len() as u16).to_be_bytes());
    extensions.extend(name);
    extensions.extend([0, 43, 0, 5, 4, 0x03, 0x04, 0x03, 0x03]);
    let mut body = vec![0x03, 0x03];
    body.extend([7u8; 32]);
    body.push(0);
    body.extend([0, 6, 0x3a, 0x3a, 0x13, 0x01, 0xc0, 0x2f, 1, 0]);
    body.extend((extensions.len() as u16).to_be_bytes());
    body.extend(extensions);
    let mut message = vec![1];
    message.extend(&(body.len() as u32).to_be_bytes()[1..]);
    message.extend(body);
    message
}

#[tokio::test]
async fn ssl_handlers_read_the_hello_and_choose_the_certificate() {
    let certified = rcgen::generate_simple_self_signed(vec!["shop.example".into()]).unwrap();
    let hex = |der: &[u8]| {
        der.iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let certificate = hex(certified.cert.der());
    let key = hex(&certified.signing_key.serialize_der());
    let hello = r#"
        local clienthello = require "ngx.ssl.clienthello"
        local ssl = require "ngx.ssl"
        assert(ngx.get_phase() == "ssl_client_hello")
        assert(clienthello.get_client_hello_server_name() == "shop.example")
        local versions = assert(clienthello.get_supported_versions())
        assert(#versions == 2 and versions[1] == "TLSv1.3" and versions[2] == "TLSv1.2")
        local ciphers = clienthello.get_client_hello_ciphers()
        assert(#ciphers == 2 and ciphers[1] == 4865 and ciphers[2] == 49199, #ciphers)
        local present = clienthello.get_client_hello_ext_present()
        assert(#present == 2 and present[1] == 0 and present[2] == 43)
        assert(#clienthello.get_client_hello_ext(0) == 17)
        assert(clienthello.get_client_hello_ext(16) == nil)
        assert(ssl.server_name() == "shop.example")
        assert(ssl.get_tls1_version() == 0x0304 and ssl.get_tls1_version_str() == "TLSv1.3")
        assert(ssl.get_client_random(0) == 32 and #ssl.get_client_random(8) == 8)
        local ok, err = pcall(ssl.clear_certs)
        assert(not ok and err:find("API disabled in the current context"), err)
        assert(not pcall(function() return ngx.ctx end))
        local ok, err = clienthello.set_protocols({ "TLSv1.2" })
        assert(ok == nil and err:find("TLS profiles"), err)
    "#;
    let choose = format!(
        r#"
        local ssl = require "ngx.ssl"
        local function bytes(hex)
            return (hex:gsub("%x%x", function(pair) return string.char(tonumber(pair, 16)) end))
        end
        local function pem(label, der)
            return "-----BEGIN " .. label .. "-----\n" .. ngx.encode_base64(der)
                .. "\n-----END " .. label .. "-----\n"
        end
        assert(ngx.get_phase() == "ssl_cert")
        assert(not pcall(require("ngx.ssl.clienthello").get_client_hello_server_name))
        assert(ssl.clear_certs())
        local certificate, key = bytes("{certificate}"), bytes("{key}")
        local der = assert(ssl.cert_pem_to_der(pem("CERTIFICATE", certificate)))
        assert(der == certificate)
        assert(ssl.set_der_cert(der))
        assert(ssl.priv_key_pem_to_der(pem("PRIVATE KEY", key)) == key)
        assert(ssl.parse_der_cert(der) and ssl.parse_der_priv_key(key))
        assert(ssl.set_priv_key(assert(ssl.parse_pem_priv_key(pem("PRIVATE KEY", key)))))
        local nothing, err = ssl.set_der_cert("not DER")
        assert(nothing == nil and err, err)
        local ok, err = ssl.verify_client()
        assert(ok == nil and err:find("trusted"), err)
        assert(ssl.verify_client(assert(ssl.parse_der_cert(certificate)), 2))
        local secret, err = ssl.get_session_master_key()
        assert(secret == nil and err:find("decrypt"), err)
        local address, kind = ssl.raw_client_addr()
        assert(kind == "inet" and #address == 4, kind)
        ngx.sleep(0)
        "#
    );
    let lua = start(1, handlers(&[hello, &choose, "ngx.exit(ngx.ERROR)"]));
    let mut exchange = request("GET", "/", &[]);
    exchange.handshake = panel_lua::Handshake::parse(&client_hello()).unwrap();
    let mut scripts = lua.runtime.scripts(exchange);
    let outcome = run(
        &mut scripts,
        handler(lua.handlers[0], Phase::SslClientHello),
    )
    .await;
    assert_eq!(outcome, Outcome::Continue, "{:?}", scripts.exchange().logs);
    let outcome = run(
        &mut scripts,
        handler(lua.handlers[1], Phase::SslCertificate),
    )
    .await;
    assert_eq!(outcome, Outcome::Continue, "{:?}", scripts.exchange().logs);
    {
        let exchange = scripts.exchange();
        let handshake = &exchange.handshake;
        assert!(handshake.cleared);
        let asked = handshake.client_auth.as_ref().unwrap();
        assert_eq!(asked.depth, 2);
        assert_eq!(asked.authorities, [certified.cert.der().to_vec()]);
        assert_eq!(
            handshake.chain.as_deref(),
            Some(&[certified.cert.der().to_vec()][..])
        );
        assert_eq!(
            handshake.key.as_deref(),
            Some(&certified.signing_key.serialize_der()[..])
        );
    }
    let outcome = run(
        &mut scripts,
        handler(lua.handlers[2], Phase::SslCertificate),
    )
    .await;
    assert_eq!(outcome, Outcome::Abort);
}

#[tokio::test]
async fn modules_refused_for_the_sandbox_say_why() {
    let lua = start(
        1,
        handlers(&[r#"
            for _, name in ipairs({ "ngx.pipe", "ffi" }) do
                local ok, err = pcall(require, name)
                ngx.say(name, " ", tostring(ok), " ", tostring(err))
            end
        "#]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    let body = String::from_utf8(scripts.exchange().response.body.clone()).unwrap();
    assert!(
        body.contains("ngx.pipe false") && body.contains("start processes"),
        "{body}"
    );
    assert!(body.contains("ffi false") && body.contains("FFI"), "{body}");
}

/// A client that takes a handler's output as it is sent.
#[derive(Default)]
struct Streamer {
    sent: Vec<(Option<u16>, Vec<u8>, bool)>,
}

#[async_trait]
impl Host for Streamer {
    async fn read_body(&mut self, _limit: usize) -> Result<Bytes, String> {
        Ok(Bytes::new())
    }

    fn streams(&self) -> bool {
        true
    }

    async fn send(&mut self, output: panel_lua::Output) -> Result<(), String> {
        let status = output.header.map(|exchange| exchange.response.status);
        self.sent.push((status, output.body.to_vec(), output.last));
        Ok(())
    }
}

#[tokio::test]
async fn output_goes_to_the_client_as_handlers_flush_it() {
    let lua = start(
        1,
        handlers(&[
            r#"
            ngx.status = 201
            ngx.say("first")
            assert(ngx.flush(true) == 1)
            ngx.print(string.rep("x", 70000))
            ngx.say("last")
            assert(ngx.eof() == 1)
            local ok, err = ngx.say("ignored")
            assert(ok == nil and err == "seen eof", err)
            "#,
            r#"
            ngx.say("partial")
            ngx.flush()
            error("broken")
            "#,
        ]),
    );
    let mut streamer = Streamer::default();
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = scripts
        .run(handler(lua.handlers[0], Phase::Content), &mut streamer)
        .await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    let sent: Vec<_> = streamer
        .sent
        .iter()
        .map(|(status, body, last)| (*status, body.len(), *last))
        .collect();
    assert_eq!(
        sent,
        [(Some(201), 6, false), (None, 70000, false), (None, 5, true)]
    );
    assert_eq!(streamer.sent[2].1, b"last\n");
    assert!(scripts.exchange().streaming() && scripts.exchange().ended());

    let mut kept = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut kept, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond);
    assert_eq!(kept.exchange().response.body.len(), 6 + 70000 + 5);
    assert!(!kept.exchange().streaming());

    let mut streamer = Streamer::default();
    let mut broken = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = broken
        .run(handler(lua.handlers[1], Phase::Content), &mut streamer)
        .await;
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    assert_eq!(streamer.sent.len(), 1);
    assert!(broken.exchange().streaming() && !broken.exchange().ended());
}

#[tokio::test]
async fn raw_request_sockets_wait_for_a_client_and_a_sent_header() {
    let lua = start(
        1,
        handlers(&[r#"
            local sock, err = ngx.req.socket(true)
            ngx.say(tostring(sock), " ", err)
        "#]),
    );
    let mut granted = handler(lua.handlers[0], Phase::Content);
    granted.permissions.body = true;
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    run(&mut scripts, granted).await;
    assert_eq!(
        scripts.exchange().response.body,
        b"nil the raw request socket is not available here\n"
    );

    let mut streamer = Streamer::default();
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    scripts.run(granted, &mut streamer).await;
    assert!(streamer.sent.is_empty());
    let printed = String::from_utf8(scripts.exchange().response.body.clone()).unwrap();
    assert!(
        printed.starts_with("nil the raw request socket follows"),
        "{printed}"
    );
}

#[tokio::test]
async fn session_handlers_read_and_give_sessions() {
    let lua = start(
        1,
        handlers(&[
            r#"
            local session = require "ngx.ssl.session"
            assert(ngx.get_phase() == "ssl_session_fetch")
            assert(session.get_session_id() == "0a0b0c")
            assert(not pcall(session.get_serialized_session))
            assert(session.set_serialized_session("kept elsewhere"))
            "#,
            r#"
            local session = require "ngx.ssl.session"
            assert(session.get_session_id() == "0a0b0c")
            assert(session.get_serialized_session() == "fresh")
            assert(not pcall(session.set_serialized_session, "x"))
            assert(not pcall(ngx.sleep, 0))
            assert(ngx.timer.at(0, function() end))
            "#,
        ]),
    );
    let mut exchange = request("GET", "/", &[]);
    exchange.handshake.session = Some(Bytes::from_static(&[10, 11, 12]));
    let mut scripts = lua.runtime.scripts(exchange);
    let outcome = run(
        &mut scripts,
        handler(lua.handlers[0], Phase::SslSessionFetch),
    )
    .await;
    assert_eq!(outcome, Outcome::Continue, "{:?}", scripts.exchange().logs);
    assert_eq!(
        scripts.exchange().handshake.serialized.as_deref(),
        Some(&b"kept elsewhere"[..])
    );
    scripts.exchange().handshake.serialized = Some(Bytes::from_static(b"fresh"));
    let outcome = run(
        &mut scripts,
        handler(lua.handlers[1], Phase::SslSessionStore),
    )
    .await;
    assert_eq!(outcome, Outcome::Continue, "{:?}", scripts.exchange().logs);
}

#[tokio::test]
async fn cosockets_bind_their_local_address_and_give_their_descriptor() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = tokio::spawn(async move { listener.accept().await.unwrap().1 });
    let lua = start(
        1,
        handlers(&[&format!(
            r#"
            local sock = ngx.socket.tcp()
            local nothing, err = sock:bind("not an address")
            assert(nothing == nil and err == "bad address", err)
            assert(sock:bind("127.0.0.1"))
            assert(sock:connect("127.0.0.1", {port}))
            local fd = assert(sock:getfd())
            assert(math.type and math.type(fd) == "integer" or type(fd) == "number")
            sock:close()
            local closed, why = sock:getfd()
            assert(closed == nil and why == "closed", why)
            ngx.say("ok")
            "#
        )]),
    );
    let mut granted = handler(lua.handlers[0], Phase::Content);
    granted.permissions.network = true;
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, granted).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"ok\n");
    assert_eq!(accepted.await.unwrap().ip().to_string(), "127.0.0.1");
}

#[tokio::test]
async fn proxy_certificate_handlers_choose_what_the_upstream_connection_presents() {
    let certified = rcgen::generate_simple_self_signed(vec!["client.example".into()]).unwrap();
    let hex = |der: &[u8]| {
        der.iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let script = format!(
        r#"
        local proxy = require "ngx.ssl.proxysslcert"
        local ssl = require "ngx.ssl"
        local function bytes(hex)
            return (hex:gsub("%x%x", function(pair) return string.char(tonumber(pair, 16)) end))
        end
        assert(ngx.get_phase() == "proxy_ssl_cert")
        assert(ngx.var.host == "shop.example", ngx.var.host)
        assert(not pcall(ssl.clear_certs))
        assert(proxy.clear_certs())
        assert(proxy.set_cert(assert(ssl.parse_der_cert(bytes("{}")))))
        assert(proxy.set_priv_key(assert(ssl.parse_der_priv_key(bytes("{}")))))
        "#,
        hex(certified.cert.der()),
        hex(&certified.signing_key.serialize_der()),
    );
    let lua = start(1, handlers(&[&script]));
    let mut scripts = lua
        .runtime
        .scripts(request("GET", "/", &[("host", "shop.example")]));
    let outcome = run(
        &mut scripts,
        handler(lua.handlers[0], Phase::ProxySslCertificate),
    )
    .await;
    assert_eq!(outcome, Outcome::Continue, "{:?}", scripts.exchange().logs);
    let exchange = scripts.exchange();
    assert_eq!(
        exchange.handshake.chain.as_deref(),
        Some(&[certified.cert.der().to_vec()][..])
    );
    assert!(exchange.handshake.cleared && exchange.handshake.key.is_some());
}

#[tokio::test]
async fn requests_read_the_client_certificate_their_connection_verified() {
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["client.example".into()]).unwrap();
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Shop, Inc");
    params
        .distinguished_name
        .push(DnType::CommonName, "shop client");
    params.serial_number = Some(vec![0x0a, 0xbc].into());
    let certificate = params.self_signed(&key).unwrap();
    let lua = start(
        1,
        handlers(&[
            r#"
            ngx.say(ngx.var.ssl_client_verify)
            ngx.say(ngx.var.ssl_client_s_dn)
            ngx.say(ngx.var.ssl_client_i_dn)
            ngx.say(ngx.var.ssl_client_serial)
            ngx.say(#ngx.var.ssl_client_fingerprint)
            ngx.say(ngx.var.ssl_client_raw_cert:sub(1, 27))
            ngx.say(select(2, ngx.var.ssl_client_cert:gsub("\n\t", "")) > 0)
        "#,
            r#"ngx.say(ngx.var.ssl_client_verify, " ", tostring(ngx.var.ssl_client_s_dn))"#,
        ]),
    );
    let mut exchange = request("GET", "/", &[]);
    exchange.handshake.client_verify = Some("SUCCESS".into());
    exchange.handshake.client_chain = vec![certificate.der().to_vec()];
    let mut scripts = lua.runtime.scripts(exchange);
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    let said = String::from_utf8(scripts.exchange().response.body.clone()).unwrap();
    assert_eq!(
        said,
        "SUCCESS\nCN=shop client,O=Shop\\, Inc\nCN=shop client,O=Shop\\, Inc\n0ABC\n40\n-----BEGIN CERTIFICATE-----\ntrue\n"
    );

    let mut plain = lua.runtime.scripts(request("GET", "/", &[]));
    run(&mut plain, handler(lua.handlers[1], Phase::Content)).await;
    assert_eq!(plain.exchange().response.body, b"NONE nil\n");
}

#[tokio::test]
async fn resty_core_modules_load_as_lua_resty_core_gives_them() {
    let lua = start(
        1,
        handlers(&[r#"
            local base = require "resty.core.base"
            assert(require("resty.core").version and require("resty.core.regex").version)
            local tab = base.new_tab(4, 0)
            tab[1] = "x"
            base.clear_tab(tab)
            assert(next(tab) == nil)
            local refs = {}
            local first = base.ref_in_table(refs, "a")
            local second = base.ref_in_table(refs, "b")
            base.unref_in_table(refs, first)
            assert(base.ref_in_table(refs, "c") == first and refs[second] == "b")
            base.allows_subsystem("http", "stream")
            assert(not pcall(base.allows_subsystem, "stream"))
            assert(base.FFI_OK == 0 and base.FFI_DECLINED == -5)
            local verify = require "ngx.ssl.proxysslverify"
            local ok, err = pcall(verify.get_verify_result)
            assert(not ok and tostring(err):find("API disabled in the current context"), err)
            ngx.say("ok")
        "#]),
    );
    let mut scripts = lua.runtime.scripts(request("GET", "/", &[]));
    let outcome = run(&mut scripts, handler(lua.handlers[0], Phase::Content)).await;
    assert_eq!(outcome, Outcome::Respond, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().response.body, b"ok\n");
}

#[tokio::test]
async fn proxy_verify_handlers_judge_the_upstream_certificate() {
    let lua = start(
        1,
        handlers(&[r#"
            local verify = require "ngx.ssl.proxysslverify"
            local proxyssl = require "ngx.proxyssl"
            assert(ngx.get_phase() == "proxy_ssl_verify")
            assert(verify.get_verify_result() == 20)
            assert(verify.get_verify_cert() == "\1\2\3\4")
            assert(proxyssl.get_tls1_version() == proxyssl.TLS1_3_VERSION)
            assert(proxyssl.get_tls1_version_str() == "TLSv1.3")
            assert(ngx.var.host == "shop.example")
            assert(verify.set_verify_result(0))
        "#]),
    );
    let mut exchange = request("GET", "/", &[("host", "shop.example")]);
    exchange.upstream_tls.chain = vec![vec![1, 2], vec![3, 4]];
    exchange.upstream_tls.version = Some(0x0304);
    exchange.upstream_tls.verify_result = 20;
    let mut scripts = lua.runtime.scripts(exchange);
    let outcome = run(
        &mut scripts,
        handler(lua.handlers[0], Phase::ProxySslVerify),
    )
    .await;
    assert_eq!(outcome, Outcome::Continue, "{:?}", scripts.exchange().logs);
    assert_eq!(scripts.exchange().upstream_tls.verdict, Some(0));
}
