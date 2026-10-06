//! `resty.dns.resolver`: lua-resty-dns's resolver, asking its nameservers
//! over UDP and again over TCP when an answer is truncated, with messages
//! hickory-proto makes and reads, under the network permission cosockets
//! need.

use super::{failed, results, socket::allowed, Api};
use crate::vm::Slot;
use hickory_proto::{
    op::{Message, MessageType, OpCode, Query},
    rr::{Name, RData, Record, RecordType},
    serialize::binary::BinEncodable,
};
use mlua::{Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
    time::{timeout, Instant},
};

const CONSTANTS: [(&str, u16); 14] = [
    ("TYPE_A", 1),
    ("TYPE_NS", 2),
    ("TYPE_CNAME", 5),
    ("TYPE_SOA", 6),
    ("TYPE_PTR", 12),
    ("TYPE_MX", 15),
    ("TYPE_TXT", 16),
    ("TYPE_AAAA", 28),
    ("TYPE_SRV", 33),
    ("TYPE_SPF", 99),
    ("CLASS_IN", 1),
    ("SECTION_AN", 1),
    ("SECTION_NS", 2),
    ("SECTION_AR", 3),
];

const TYPE_PTR: u16 = 12;
const TYPE_SPF: u16 = 99;

/// What RFC 1035 §4.1.1 names a response code.
fn errstr(code: u16) -> &'static str {
    match code {
        1 => "format error",
        2 => "server failure",
        3 => "name error",
        4 => "not implemented",
        5 => "refused",
        _ => "unknown",
    }
}

/// An IPv6 address with each of its eight groups written out, as
/// lua-resty-dns gives addresses.
fn expanded_address(address: &std::net::Ipv6Addr) -> String {
    address
        .segments()
        .iter()
        .map(|group| format!("{group:x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The groups of a textual IPv6 address, `::` filled with zero groups and
/// each group kept as written.
fn groups(address: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = address.split("::").collect();
    let split = |text: &str| -> Vec<String> {
        if text.is_empty() {
            Vec::new()
        } else {
            text.split(':').map(str::to_owned).collect()
        }
    };
    let groups = match parts.as_slice() {
        [whole] => split(whole),
        [head, tail] => {
            let (head, tail) = (split(head), split(tail));
            let missing = 8usize.checked_sub(head.len() + tail.len())?;
            let mut groups = head;
            groups.extend(std::iter::repeat_n("0".to_owned(), missing));
            groups.extend(tail);
            groups
        }
        _ => return None,
    };
    (groups.len() == 8
        && groups
            .iter()
            .all(|group| (1..=4).contains(&group.len()) && u16::from_str_radix(group, 16).is_ok()))
    .then_some(groups)
}

fn expand_ipv6_addr(address: &str) -> String {
    groups(address).map_or_else(|| address.to_owned(), |groups| groups.join(":"))
}

/// The address with its longest run of two or more zero groups written
/// `::`, as RFC 5952 §4.2 has it, other groups kept as written.
fn compress_ipv6_addr(address: &str) -> String {
    let Some(groups) = groups(address) else {
        return address.to_owned();
    };
    let zero = |group: &String| u16::from_str_radix(group, 16) == Ok(0);
    let (mut best, mut start) = ((0, 0), None);
    for (index, group) in groups.iter().enumerate() {
        match (zero(group), start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                if index - from > best.1 - best.0 {
                    best = (from, index);
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        if groups.len() - from > best.1 - best.0 {
            best = (from, groups.len());
        }
    }
    if best.1 - best.0 < 2 {
        return groups.join(":");
    }
    format!(
        "{}::{}",
        groups[..best.0].join(":"),
        groups[best.1..].join(":")
    )
}

/// The name a PTR query asks about `address`.
fn arpa_str(address: &str) -> Option<String> {
    if let Ok(IpAddr::V4(v4)) = address.parse::<IpAddr>() {
        let [a, b, c, d] = v4.octets();
        return Some(format!("{d}.{c}.{b}.{a}.in-addr.arpa"));
    }
    let nibbles: String = groups(address)?
        .iter()
        .map(|group| format!("{group:0>4}"))
        .collect();
    let mut name: Vec<String> = nibbles.chars().rev().map(String::from).collect();
    name.push("ip6.arpa".into());
    Some(name.join("."))
}

/// A name as lua-resty-dns writes it, without the root's trailing dot.
fn text(name: &Name) -> String {
    let mut text = name.to_ascii();
    if text.len() > 1 && text.ends_with('.') {
        text.pop();
    }
    text
}

/// The character strings of TXT record data (RFC 1035 §3.3).
fn character_strings(mut data: &[u8]) -> Vec<Vec<u8>> {
    let mut strings = Vec::new();
    while let Some((&length, rest)) = data.split_first() {
        let length = usize::from(length).min(rest.len());
        strings.push(rest[..length].to_vec());
        data = &rest[length..];
    }
    strings
}

fn entry(lua: &Lua, record: &Record, section: u16) -> mlua::Result<Table> {
    let entry = lua.create_table()?;
    let kind = u16::from(record.record_type());
    entry.raw_set("name", text(&record.name))?;
    entry.raw_set("type", kind)?;
    entry.raw_set("class", u16::from(record.dns_class))?;
    entry.raw_set("ttl", record.ttl)?;
    entry.raw_set("section", section)?;
    let txt = |strings: Vec<Vec<u8>>| -> mlua::Result<Value> {
        if let [single] = strings.as_slice() {
            return Ok(Value::String(lua.create_string(single)?));
        }
        let table = lua.create_table()?;
        for string in strings {
            table.raw_push(lua.create_string(string)?)?;
        }
        Ok(Value::Table(table))
    };
    match &record.data {
        RData::A(address) => entry.raw_set("address", address.0.to_string())?,
        RData::AAAA(address) => entry.raw_set("address", expanded_address(&address.0))?,
        RData::CNAME(name) => entry.raw_set("cname", text(&name.0))?,
        RData::NS(name) => entry.raw_set("nsdname", text(&name.0))?,
        RData::PTR(name) => entry.raw_set("ptrdname", text(&name.0))?,
        RData::MX(mx) => {
            entry.raw_set("preference", mx.preference)?;
            entry.raw_set("exchange", text(&mx.exchange))?;
        }
        RData::SRV(srv) => {
            entry.raw_set("priority", srv.priority)?;
            entry.raw_set("weight", srv.weight)?;
            entry.raw_set("port", srv.port)?;
            entry.raw_set("target", text(&srv.target))?;
        }
        RData::SOA(soa) => {
            entry.raw_set("mname", text(&soa.mname))?;
            entry.raw_set("rname", text(&soa.rname))?;
            entry.raw_set("serial", soa.serial)?;
            entry.raw_set("refresh", soa.refresh)?;
            entry.raw_set("retry", soa.retry)?;
            entry.raw_set("expire", soa.expire)?;
            entry.raw_set("minimum", soa.minimum)?;
        }
        RData::TXT(data) => {
            let strings = data.txt_data.iter().map(|string| string.to_vec()).collect();
            entry.raw_set("txt", txt(strings)?)?;
        }
        RData::Unknown { rdata, .. } if kind == TYPE_SPF => {
            entry.raw_set("txt", txt(character_strings(&rdata.anything))?)?;
        }
        RData::Unknown { rdata, .. } => {
            entry.raw_set("rdata", lua.create_string(&rdata.anything)?)?
        }
        other => {
            let raw = other.to_bytes().unwrap_or_default();
            entry.raw_set("rdata", lua.create_string(raw)?)?;
        }
    }
    Ok(entry)
}

/// The query of `name` for `kind`, identified by `id`.
fn request(id: u16, name: &str, kind: u16, recurse: bool) -> Option<Vec<u8>> {
    let mut message = Message::new(id, MessageType::Query, OpCode::Query);
    message.metadata.recursion_desired = recurse;
    message.add_query(Query::query(
        Name::from_ascii(name).ok()?,
        RecordType::from(kind),
    ));
    message.to_vec().ok()
}

/// The reply to `id`, read from `data`, if it is one.
fn reply(data: &[u8], id: u16) -> Option<Message> {
    let message = Message::from_vec(data).ok()?;
    (message.metadata.id == id && message.metadata.message_type == MessageType::Response)
        .then_some(message)
}

async fn address(host: &str, port: u16) -> std::io::Result<SocketAddr> {
    tokio::net::lookup_host((host, port))
        .await?
        .next()
        .ok_or_else(|| std::io::Error::other("no address found"))
}

async fn over_udp(
    server: SocketAddr,
    query: &[u8],
    id: u16,
    wait: Duration,
) -> Result<Message, String> {
    let local: SocketAddr = if server.is_ipv4() {
        ([0, 0, 0, 0], 0).into()
    } else {
        ([0u16; 8], 0).into()
    };
    let socket = UdpSocket::bind(local)
        .await
        .map_err(|error| error.to_string())?;
    socket
        .connect(server)
        .await
        .map_err(|error| error.to_string())?;
    socket
        .send(query)
        .await
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + wait;
    let mut buffer = vec![0; 65535];
    loop {
        let received = tokio::time::timeout_at(deadline, socket.recv(&mut buffer))
            .await
            .map_err(|_| "timeout".to_owned())?
            .map_err(|error| super::socket::reason(&error))?;
        if let Some(message) = reply(&buffer[..received], id) {
            return Ok(message);
        }
    }
}

async fn over_tcp(
    server: SocketAddr,
    query: &[u8],
    id: u16,
    wait: Duration,
) -> Result<Message, String> {
    let exchange = async {
        let mut stream = TcpStream::connect(server).await?;
        let length = u16::try_from(query.len()).map_err(std::io::Error::other)?;
        let mut framed = length.to_be_bytes().to_vec();
        framed.extend_from_slice(query);
        stream.write_all(&framed).await?;
        let mut length = [0; 2];
        stream.read_exact(&mut length).await?;
        let mut data = vec![0; usize::from(u16::from_be_bytes(length))];
        stream.read_exact(&mut data).await?;
        Ok::<_, std::io::Error>(data)
    };
    let data = timeout(wait, exchange)
        .await
        .map_err(|_| "timeout".to_owned())?
        .map_err(|error| super::socket::reason(&error))?;
    reply(&data, id).ok_or_else(|| "bad reply".to_owned())
}

struct Resolver {
    slot: Arc<Slot>,
    servers: Vec<(String, u16)>,
    next: usize,
    retrans: usize,
    timeout: Duration,
    recurse: bool,
}

struct Question {
    name: String,
    kind: u16,
    authority: bool,
    additional: bool,
}

impl Question {
    fn read(name: String, options: Option<&Table>) -> mlua::Result<Self> {
        let option = |field: &str| -> mlua::Result<Option<Value>> {
            options
                .map(|options| options.get::<Value>(field))
                .transpose()
        };
        let truthy = |value: Option<Value>| {
            value.is_some_and(|value| value.as_boolean() != Some(false) && !value.is_nil())
        };
        Ok(Self {
            name,
            kind: options
                .map(|options| options.get::<Option<u16>>("qtype"))
                .transpose()?
                .flatten()
                .unwrap_or(1),
            authority: truthy(option("authority_section")?),
            additional: truthy(option("additional_section")?),
        })
    }
}

impl Resolver {
    fn server(&mut self) -> (String, u16) {
        let server = self.servers[self.next % self.servers.len()].clone();
        self.next = (self.next + 1) % self.servers.len();
        server
    }

    /// The reply to `question`, and the failure of each try that failed.
    async fn ask(
        &mut self,
        question: &Question,
        tcp: bool,
    ) -> (Result<Message, String>, Vec<String>) {
        let mut id = [0; 2];
        if getrandom::fill(&mut id).is_err() {
            return (Err("no random source is available".into()), Vec::new());
        }
        let id = u16::from_be_bytes(id);
        let Some(query) = request(id, &question.name, question.kind, self.recurse) else {
            return (Err("bad name".into()), Vec::new());
        };
        let mut tries = Vec::new();
        let attempts = if tcp { 1 } else { self.retrans.max(1) };
        for _ in 0..attempts {
            let (host, port) = self.server();
            let server = match address(&host, port).await {
                Ok(server) => server,
                Err(error) => {
                    tries.push(format!("failed to resolve nameserver {host}: {error}"));
                    continue;
                }
            };
            let answered = if tcp {
                over_tcp(server, &query, id, self.timeout)
                    .await
                    .map_err(|error| {
                        format!("failed to receive reply from TCP server {host}:{port}: {error}")
                    })
            } else {
                match over_udp(server, &query, id, self.timeout).await {
                    Ok(message) if message.metadata.truncation => over_tcp(
                        server,
                        &query,
                        id,
                        self.timeout,
                    )
                    .await
                    .map_err(|error| {
                        format!("failed to receive reply from TCP server {host}:{port}: {error}")
                    }),
                    Ok(message) => Ok(message),
                    Err(error) => Err(format!(
                        "failed to receive reply from UDP server {host}:{port}: {error}"
                    )),
                }
            };
            match answered {
                Ok(message) => return (Ok(message), tries),
                Err(error) => tries.push(error),
            }
        }
        let last = tries
            .last()
            .cloned()
            .unwrap_or_else(|| "no nameserver answered".into());
        (Err(last), tries)
    }
}

fn answers(lua: &Lua, message: &Message, question: &Question) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let code = u16::from(message.metadata.response_code);
    if code != 0 {
        table.raw_set("errcode", code)?;
        table.raw_set("errstr", errstr(code))?;
    }
    let mut sections = vec![(1, &message.answers)];
    if question.authority {
        sections.push((2, &message.authorities));
    }
    if question.additional {
        sections.push((3, &message.additionals));
    }
    for (section, records) in sections {
        for record in records {
            table.raw_push(entry(lua, record, section)?)?;
        }
    }
    Ok(table)
}

/// What `query` and its kin return: the answers, or `nil` and why, with
/// `tries` filled when given.
async fn query(
    lua: &Lua,
    resolver: &mut Resolver,
    question: Question,
    tcp: bool,
    tries: Option<Table>,
) -> mlua::Result<MultiValue> {
    allowed(&resolver.slot, if tcp { Api::Socket } else { Api::Udp })?;
    let (answered, failures) = resolver.ask(&question, tcp).await;
    if let Some(tries) = &tries {
        for failure in &failures {
            tries.raw_push(failure.as_str())?;
        }
    }
    let tries = tries.map_or(Value::Nil, Value::Table);
    match answered {
        Ok(message) => Ok(results([
            Value::Table(answers(lua, &message, &question)?),
            Value::Nil,
            tries,
        ])),
        Err(error) => Ok(results([
            Value::Nil,
            Value::String(lua.create_string(error)?),
            tries,
        ])),
    }
}

impl UserData for Resolver {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        for (name, value) in CONSTANTS {
            fields.add_field(name, value);
        }
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method_mut(
            "query",
            |lua, mut this, (name, options, tries): (String, Option<Table>, Option<Table>)| async move {
                let question = Question::read(name, options.as_ref())?;
                query(&lua, &mut this, question, false, tries).await
            },
        );
        methods.add_async_method_mut(
            "tcp_query",
            |lua, mut this, (name, options): (String, Option<Table>)| async move {
                let question = Question::read(name, options.as_ref())?;
                query(&lua, &mut this, question, true, None).await
            },
        );
        methods.add_async_method_mut(
            "reverse_query",
            |lua, mut this, address: String| async move {
                let Some(name) = arpa_str(&address) else {
                    return failed(&lua, 1, "bad address");
                };
                let question = Question {
                    name,
                    kind: TYPE_PTR,
                    authority: false,
                    additional: false,
                };
                query(&lua, &mut this, question, false, None).await
            },
        );
        methods.add_method_mut("set_timeout", |_, this, milliseconds: f64| {
            if milliseconds.is_finite() && milliseconds > 0.0 {
                this.timeout = Duration::from_secs_f64(milliseconds / 1000.0);
            }
            Ok(())
        });
        methods.add_method_mut("destroy", |_, this, ()| {
            this.servers.clear();
            Ok(())
        });
    }
}

fn nameservers(list: &Table) -> mlua::Result<Vec<(String, u16)>> {
    let mut servers = Vec::new();
    for server in list.sequence_values::<Value>() {
        servers.push(match server? {
            Value::String(host) => (host.to_str()?.to_owned(), 53),
            Value::Table(pair) => (
                pair.get::<String>(1)?,
                pair.get::<Option<u16>>(2)?.unwrap_or(53),
            ),
            _ => return Err(mlua::Error::runtime("bad nameserver")),
        });
    }
    Ok(servers)
}

/// The `resty.dns.resolver` module.
pub(super) fn module(lua: &Lua, slot: &Arc<Slot>) -> mlua::Result<Table> {
    let module = lua.create_table()?;
    module.raw_set("_VERSION", "0.23")?;
    for (name, value) in CONSTANTS {
        module.raw_set(name, value)?;
    }
    let slot = Arc::clone(slot);
    module.raw_set(
        "new",
        lua.create_function(move |lua, (_, options): (Value, Table)| {
            let Some(list) = options.get::<Option<Table>>("nameservers")? else {
                return failed(lua, 1, "no nameservers specified");
            };
            let servers = nameservers(&list)?;
            if servers.is_empty() {
                return failed(lua, 1, "no nameservers specified");
            }
            let first = if options.get::<Option<bool>>("no_random")?.unwrap_or(false) {
                0
            } else {
                let mut pick = [0; 4];
                getrandom::fill(&mut pick)
                    .map_err(|_| mlua::Error::runtime("no random source is available"))?;
                u32::from_be_bytes(pick) as usize % servers.len()
            };
            let resolver = Resolver {
                slot: Arc::clone(&slot),
                servers,
                next: first,
                retrans: options.get::<Option<usize>>("retrans")?.unwrap_or(5),
                timeout: Duration::from_millis(
                    options.get::<Option<u64>>("timeout")?.unwrap_or(2000),
                ),
                recurse: !options.get::<Option<bool>>("no_recurse")?.unwrap_or(false),
            };
            Ok(results([Value::UserData(lua.create_userdata(resolver)?)]))
        })?,
    )?;
    module.raw_set(
        "compress_ipv6_addr",
        lua.create_function(|_, address: String| Ok(compress_ipv6_addr(&address)))?,
    )?;
    module.raw_set(
        "expand_ipv6_addr",
        lua.create_function(|_, address: String| Ok(expand_ipv6_addr(&address)))?,
    )?;
    module.raw_set(
        "arpa_str",
        lua.create_function(|_, address: String| Ok(arpa_str(&address)))?,
    )?;
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_addresses_expand_and_compress_as_rfc_5952_has_them() {
        assert_eq!(compress_ipv6_addr("FF01:0:0:0:0:0:0:101"), "FF01::101");
        assert_eq!(expand_ipv6_addr("FF01::101"), "FF01:0:0:0:0:0:0:101");
        assert_eq!(
            compress_ipv6_addr("2001:db8:0:1:1:1:1:1"),
            "2001:db8:0:1:1:1:1:1"
        );
        assert_eq!(compress_ipv6_addr("2001:0:0:1:0:0:0:1"), "2001:0:0:1::1");
        assert_eq!(compress_ipv6_addr("0:0:0:0:0:0:0:1"), "::1");
        assert_eq!(compress_ipv6_addr("0:0:0:0:0:0:0:0"), "::");
        assert_eq!(expand_ipv6_addr("::"), "0:0:0:0:0:0:0:0");
        assert_eq!(expand_ipv6_addr("not an address"), "not an address");
        assert_eq!(
            expanded_address(&"2404:6800:4008:c00::68".parse().unwrap()),
            "2404:6800:4008:c00:0:0:0:68"
        );
    }

    #[test]
    fn reverse_names_are_those_ptr_queries_ask_about() {
        assert_eq!(arpa_str("1.2.3.4").unwrap(), "4.3.2.1.in-addr.arpa");
        assert_eq!(
            arpa_str("FF01::101").unwrap(),
            "1.0.1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1.0.F.F.ip6.arpa"
        );
        assert!(arpa_str("example.com").is_none());
    }
}
