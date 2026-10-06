#![forbid(unsafe_code)]

//! `resty.dns.resolver` against a nameserver answering over UDP and TCP.

use hickory_proto::{
    op::{Message, OpCode, ResponseCode},
    rr::{
        rdata::{A, AAAA, CNAME, MX, PTR, SOA, SRV, TXT},
        Name, RData, Record, RecordType,
    },
};
use http::HeaderMap;
use panel_lua::{
    Connection, Exchange, Handler, NoHost, Outcome, Phase, Program, Request, Runtime, Settings,
    SharedStore, Source,
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UdpSocket},
};

fn name(text: &str) -> Name {
    Name::from_ascii(text).unwrap()
}

/// The answer to `query`; `big.test` does not fit in UDP.
fn answer(query: &Message, over_tcp: bool) -> Message {
    let question = query.queries[0].clone();
    let asked = question.name().clone();
    let mut response = Message::response(query.metadata.id, OpCode::Query);
    response.metadata.recursion_desired = query.metadata.recursion_desired;
    response.add_query(question.clone());
    let record = |owner: &Name, data: RData| Record::from_rdata(owner.clone(), 300, data);
    let shop = name("shop.test.");
    let address = RData::A(A::new(192, 0, 2, 10));
    let answers = match (asked.to_ascii().as_str(), question.query_type()) {
        ("shop.test.", RecordType::A) => vec![record(&asked, address)],
        ("shop.test.", RecordType::AAAA) => vec![record(
            &asked,
            RData::AAAA(AAAA("2001:db8::10".parse().unwrap())),
        )],
        ("www.shop.test.", RecordType::A) => vec![
            record(&asked, RData::CNAME(CNAME(shop.clone()))),
            record(&shop, address),
        ],
        ("shop.test.", RecordType::MX) => vec![record(
            &asked,
            RData::MX(MX::new(10, name("mail.shop.test."))),
        )],
        ("shop.test.", RecordType::TXT) => vec![
            record(
                &asked,
                RData::TXT(TXT::new(vec!["v=spf1".into(), "-all".into()])),
            ),
            record(&asked, RData::TXT(TXT::new(vec!["hello".into()]))),
        ],
        ("_http._tcp.shop.test.", RecordType::SRV) => vec![record(
            &asked,
            RData::SRV(SRV::new(1, 5, 8080, name("web.shop.test."))),
        )],
        ("10.2.0.192.in-addr.arpa.", RecordType::PTR) => {
            vec![record(&asked, RData::PTR(PTR(shop.clone())))]
        }
        ("big.test.", RecordType::A) if !over_tcp => {
            response.metadata.truncation = true;
            Vec::new()
        }
        ("big.test.", RecordType::A) => (1..=3)
            .map(|last| record(&asked, RData::A(A::new(192, 0, 2, last))))
            .collect(),
        _ => {
            response.metadata.response_code = ResponseCode::NXDomain;
            response.authorities.push(record(
                &name("test."),
                RData::SOA(SOA::new(
                    name("ns.test."),
                    name("admin.test."),
                    7,
                    3600,
                    600,
                    86400,
                    60,
                )),
            ));
            Vec::new()
        }
    };
    response.answers = answers;
    response
}

/// A nameserver's port, the same for UDP and TCP.
async fn nameserver() -> u16 {
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = udp.local_addr().unwrap().port();
    let tcp = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    tokio::spawn(async move {
        let mut buffer = vec![0; 65535];
        while let Ok((size, peer)) = udp.recv_from(&mut buffer).await {
            let query = Message::from_vec(&buffer[..size]).unwrap();
            let reply = answer(&query, false).to_vec().unwrap();
            udp.send_to(&reply, peer).await.unwrap();
        }
    });
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = tcp.accept().await {
            tokio::spawn(async move {
                let mut length = [0; 2];
                stream.read_exact(&mut length).await.unwrap();
                let mut data = vec![0; usize::from(u16::from_be_bytes(length))];
                stream.read_exact(&mut data).await.unwrap();
                let reply = answer(&Message::from_vec(&data).unwrap(), true)
                    .to_vec()
                    .unwrap();
                let mut framed = u16::try_from(reply.len()).unwrap().to_be_bytes().to_vec();
                framed.extend(reply);
                stream.write_all(&framed).await.unwrap();
            });
        }
    });
    port
}

#[tokio::test]
async fn resolvers_query_as_lua_resty_dns_does() {
    let port = nameserver().await;
    let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dead = silent.local_addr().unwrap().port();
    let script = format!(
        r#"
        local resolver = require "resty.dns.resolver"
        assert(select(2, resolver:new({{}})) == "no nameservers specified")
        local r = assert(resolver:new({{ nameservers = {{ {{ "127.0.0.1", {port} }} }}, retrans = 2, timeout = 1000 }}))
        assert(r.TYPE_AAAA == 28 and resolver.SECTION_AR == 3)
        local answers = assert(r:query("shop.test"))
        local first = answers[1]
        assert(#answers == 1 and first.address == "192.0.2.10" and first.type == r.TYPE_A)
        assert(first.class == r.CLASS_IN and first.ttl == 300 and first.section == r.SECTION_AN)
        assert(first.name == "shop.test" and answers.errcode == nil, first.name)
        answers = assert(r:query("shop.test", {{ qtype = r.TYPE_AAAA }}))
        assert(answers[1].address == "2001:db8:0:0:0:0:0:10", answers[1].address)
        answers = assert(r:query("www.shop.test"))
        assert(answers[1].cname == "shop.test" and answers[2].address == "192.0.2.10")
        answers = assert(r:query("shop.test", {{ qtype = r.TYPE_MX }}))
        assert(answers[1].preference == 10 and answers[1].exchange == "mail.shop.test")
        answers = assert(r:query("shop.test", {{ qtype = r.TYPE_TXT }}))
        assert(answers[1].txt[1] == "v=spf1" and answers[1].txt[2] == "-all" and answers[2].txt == "hello")
        answers = assert(r:query("_http._tcp.shop.test", {{ qtype = r.TYPE_SRV }}))
        local srv = answers[1]
        assert(srv.priority == 1 and srv.weight == 5 and srv.port == 8080 and srv.target == "web.shop.test")
        answers = assert(r:query("big.test"))
        assert(#answers == 3 and answers[3].address == "192.0.2.3", #answers)
        answers = assert(r:tcp_query("shop.test"))
        assert(answers[1].address == "192.0.2.10")
        answers = assert(r:reverse_query("192.0.2.10"))
        assert(answers[1].ptrdname == "shop.test")
        answers = assert(r:query("missing.test"))
        assert(answers.errcode == 3 and answers.errstr == "name error" and #answers == 0)
        answers = assert(r:query("missing.test", {{ authority_section = true }}))
        local soa = answers[1]
        assert(#answers == 1 and soa.section == r.SECTION_NS and soa.mname == "ns.test")
        assert(soa.rname == "admin.test" and soa.serial == 7 and soa.minimum == 60)

        local failing = assert(resolver:new({{
            nameservers = {{ {{ "127.0.0.1", {dead} }}, {{ "127.0.0.1", {port} }} }},
            no_random = true,
            retrans = 3,
            timeout = 200,
        }}))
        local tries = {{}}
        answers = assert(failing:query("shop.test", nil, tries))
        assert(#tries == 1 and tries[1] == "failed to receive reply from UDP server 127.0.0.1:{dead}: timeout", tries[1])
        local unanswered = assert(resolver:new({{ nameservers = {{ {{ "127.0.0.1", {dead} }} }}, retrans = 2, timeout = 100 }}))
        local nothing, err, all = unanswered:query("shop.test", nil, {{}})
        assert(nothing == nil and err:find("timeout$") and #all == 2, err)
        assert(resolver.compress_ipv6_addr("FF01:0:0:0:0:0:0:101") == "FF01::101")
        assert(resolver.arpa_str("1.2.3.4") == "4.3.2.1.in-addr.arpa")
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
    drop(silent);
}
