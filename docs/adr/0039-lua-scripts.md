# 0039: Lua scripts

Status: accepted. Builds on [ADR 0010](0010-pingora-data-plane.md),
[ADR 0012](0012-configuration-language-and-revisions.md),
[ADR 0017](0017-request-security-policies.md) and
[ADR 0036](0036-route-conditions.md).

## Context

Operators who move from NGINX bring OpenResty scripts with them: phase
handlers written in `access_by_lua_block` or `header_filter_by_lua_file`,
modules loaded with `require`, `ngx.shared` dictionaries and `cjson`. The
gateway runs no scripts; requests pass only the fixed policies of sites and
routes. The product specification (§11.3) lets only trusted administrators
publish scripts, gives scripts versioned request, response, context,
upstream, log and crypto capabilities and nothing of the host by default,
bounds every run in work, wall-clock time and memory with an explicit
fallback, and asks for embedded Lua to be weighed against WebAssembly first.

OpenResty's model has well-known weak spots. A script stuck in a loop holds
its worker and every request on it; a worker's memory is bounded only by the
machine; `ffi`, `io` and `os.execute` reach the host; a failing script
answers 500 with whatever it changed before failing; a global written by
mistake is shared by requests until a tool finds it; and scripts can only be
tried against a running NGINX.

## Decision

**Runtime.** Scripts run on [Luau](https://luau.org), embedded through
`mlua` with Luau's sources built in. Luau keeps the language of Lua 5.1,
the dialect OpenResty's LuaJIT speaks, and is made to host code it does not
trust ([sandboxing](https://luau.org/sandbox)):

- its library has no `io`, `package`, `loadlib`, `dofile`, `loadfile`,
  `os.execute` or bytecode loading, and there is no FFI;
- its global table, libraries and string metatable are read-only, and each
  script gets an environment of its own that falls back to them;
- it calls the host's interrupt at every function call and loop iteration,
  native code included, so the host can stop or suspend a running script;
- its allocator lets the host cap a VM's memory.

Native code generation is used where Luau has it, on x86-64 and AArch64.

**OpenResty compatibility.** The configuration language takes
lua-nginx-module's directives with their names, contexts and NGINX
inheritance, where a directive in an inner block replaces the outer one:

| Directive | Runs | Contexts |
| :--- | :--- | :--- |
| `init_by_lua_block`, `_file` | once in each VM when a configuration is activated | `http` |
| `init_worker_by_lua_block`, `_file` | once in each VM after `init_by_lua` | `http` |
| `exit_worker_by_lua_block`, `_file` | once in each VM when a configuration replaces it, before its pending timers run | `http` |
| `set_by_lua_block $name [arg ...]`, `set_by_lua_file $name <file> [arg ...]` | as the request reaches the server or route, before its rewrite handler; `$name` is what the code returns | `server`, `route` |
| `server_rewrite_by_lua_block`, `_file` | before the route is chosen; may change the URI and host it is chosen by | `http`, `server` |
| `rewrite_by_lua_block`, `_file` | after the route is chosen, before security policies | `http`, `server`, `route` |
| `access_by_lua_block`, `_file` | after security policies | `http`, `server`, `route` |
| `precontent_by_lua_block`, `_file` | after access and the HTTP policies, just before the action | `http`, `server`, `route` |
| `content_by_lua_block`, `_file` | as the action of a server or route, instead of proxying or serving files | `server`, `route` |
| `balancer_by_lua_block`, `_file` | each time an upstream endpoint is chosen, retries included | `upstream` |
| `header_filter_by_lua_block`, `_file` | on the response header, before it is sent | `http`, `server`, `route` |
| `body_filter_by_lua_block`, `_file` | on each chunk of the response body | `http`, `server`, `route` |
| `log_by_lua_block`, `_file` | after the response is sent | `http`, `server`, `route` |
| `ssl_client_hello_by_lua_block`, `_file` | as a TLS handshake's hello arrives, for the server its server name selects | `http`, `server` |
| `ssl_certificate_by_lua_block`, `_file` | as a TLS handshake chooses the certificate it presents | `http`, `server` |
| `ssl_session_fetch_by_lua_block`, `_file` | as a TLS handshake offers to resume a session the listener does not hold | `http` |
| `ssl_session_store_by_lua_block`, `_file` | as a TLS handshake makes a session | `http` |
| `lua_shared_dict <name> <size>` | declares a dictionary every VM shares | `http` |

The body of a `*_by_lua_block` directive is read with Lua's lexical rules,
as ngx_lua reads it, so braces inside strings, long brackets and comments
do not end it, and it is printed back unchanged. The forms that take their
code as a string, which lua-nginx-module discourages — `init_by_lua`,
`init_worker_by_lua`, `set_by_lua`, `rewrite_by_lua`, `access_by_lua`,
`content_by_lua`, `header_filter_by_lua`, `body_filter_by_lua` and
`log_by_lua` — are read as the block they stand for, with a warning, and
printed as it. The NGINX importer carries these directives over.

**Variables.** `set` and `set_by_lua*` give the request variables in
nginx's order: those of `http` and a server as the server's
`server_rewrite` phase begins, a route's as its `rewrite` phase begins,
each before the phase's handler, so a variable is what the last block the
request reached gave it. `set_by_lua*` runs its code in the `set_by_lua*`
context lua-nginx-module documents, with the directive's arguments,
templates filled in for the request, as `ngx.arg`; the variable is the
first value the code returns, as text for strings and numbers and empty
for anything else. `set_by_lua_block` takes arguments too, which nginx's
does not, so the importer carries `set_by_lua` over with its arguments.
In a configuration with Lua handlers, a constant `set` gives is such a
variable: `ngx.var` reads it, scripts may change it, and the templates of
headers, log fields, redirects and answers read the value it has when they
are filled in, which they name as `${lua:name}`; `$name` is written so
there, and `${lua:name}` also reads what a script gave any other variable.
Where a value must be known before requests, a constant stays the text it
was set to, and a variable only scripts set cannot be used.

**TLS handshakes.** `ssl_client_hello_by_lua*` and
`ssl_certificate_by_lua*` run before rustls answers the client's hello,
through Pingora's hook for what arrives ahead of TLS: the gateway reads
the records that hold the hello, gives them back to the handshake as they
were, and runs the handlers of the server the hello's server name selects,
or of the listener's default server when it names none. nginx runs
`ssl_client_hello_by_lua*` with the default server's configuration, since
OpenSSL calls it before the server name is known; here the name is known,
so a server's handler applies to its own names. A listener none of whose
servers have such handlers reads nothing ahead of rustls. In
`ssl_client_hello_by_lua*`, `ngx.ssl.clienthello` reads the hello's server
name, versions, cipher suites and extensions, GREASE values (RFC 8701) left
out as OpenSSL leaves them out. `ngx.ssl` reads the server name, the
addresses, the version and the client's random in any phase, converts and
parses PEM and DER certificates and keys, and in `ssl_certificate_by_lua*`
presents the chain and key set with `set_der_cert` and `set_der_priv_key`,
or `set_cert` and `set_priv_key`, in place of the TLS profile's after
`clear_certs`. `ngx.exit(ngx.ERROR)` ends the handshake, as does a failed
handler unless `lua_on_error continue`, and a certificate without its key
or with a key that does not match it; the handlers have the functions
lua-nginx-module allows there (`ngx.exit`, `ngx.sleep`, cosockets, light
threads, timers and `ngx.semaphore`), and neither `ngx.ctx` nor `ngx.var`.
The functions that need OpenSSL return `nil` and why: `verify_client` and
`clienthello.set_protocols`, since rustls asks every connection to a
listener for a client certificate or none and offers them all the same
versions; `get_session_master_key`, which would give scripts what decrypts
the connection; and `get_req_ssl_pointer` and its kin, which hand out
OpenSSL handles for the FFI scripts do not have.
`ssl_session_fetch_by_lua*` and `ssl_session_store_by_lua*` share
sessions beyond one listener. rustls keeps each listener's sessions; a
hello that offers to resume one the listener does not hold — by its first
TLS 1.3 ticket, which the gateway issues as the session's ID, or by its
TLS 1.2 session ID — runs the fetch handler after
`ssl_client_hello_by_lua*`, and a session it gives with
`ngx.ssl.session.set_serialized_session` is resumed without
`ssl_certificate_by_lua*`, as in nginx. Each session a handshake makes runs
the store handler, where `get_session_id` and `get_serialized_session`
give it to keep in `ngx.shared` or, through a timer, elsewhere; as in
nginx it may not wait, and it runs apart from the handshake. The
serialized form is rustls', so gateways that share sessions run the same
version, and sessions resume only on listeners whose TLS profile resumes
them.

`ngx` is lua-nginx-module's API with its documented semantics, in the phases
where lua-nginx-module allows each function: `ngx.var`, `ngx.ctx`,
`ngx.req`, `ngx.resp`, `ngx.header`, `ngx.status`, `ngx.exit`,
`ngx.redirect`, `ngx.say`, `ngx.print`, `ngx.log`, the `ngx.HTTP_*` and
log level constants, `ngx.re` on PCRE2 (the engine NGINX uses),
`ngx.shared`, the time, escaping, argument, base64, digest and quoting
functions, `ngx.sleep`, `ngx.get_phase`, `ngx.worker`, `ngx.config`,
`ngx.balancer` and the TCP cosockets of `ngx.socket.tcp` and
`ngx.socket.connect` (`bind`, `connect`, `sslhandshake`, `send`,
`receive`, `receiveany`, `receiveuntil`, `settimeout`, `settimeouts`,
`setkeepalive`, `getreusedtimes`, `getfd` and `close`), whose idle
connections each VM keeps for
its later requests and whose defaults the `lua_socket_*` directives set
for `http`, a server or a route, and the light threads of `ngx.thread.spawn`, `wait`
and `kill`, which run on the budget of the run that spawned them; a run
ends once its entry thread and its light threads have ended, or as soon as
one of them exits. `ngx.timer.at` and `ngx.timer.every` run a callback
later on the VM that created it, detached from the request, under the
limits and permissions of the run that created it, with lua-nginx-module's
default caps of 1024 pending and 256 running timers per VM; when a
configuration replaces the one that created them, pending timers run at
once with `premature` true, as on a worker's exit. `ngx.socket.udp`
sends and receives datagrams of at most 8192 bytes under the same network
permission, `ngx.socket.stream` is the TCP cosocket, and
`ngx.req.init_body`, `append_body` and `finish_body` build a new request
body in memory, and `ngx.req.socket()` reads the request body as it
arrives with `receive`, `receiveany` and `receiveuntil`, chunked bodies
too. `ngx.exec` redirects internally: the request is handled
again from `server_rewrite` with its new URI and arguments, and as in
nginx a request that changes its URI more than ten times, by jumps and
redirects together, is answered 500; `$request_uri` keeps the client's
through both. `ngx.run_worker_thread` runs a module's function on a VM of
its own on a thread of its own, at most `lua_worker_thread_vm_pool_size`
of them at once (10 by default), and copies nil, booleans, numbers, strings
and tables of them to it and its results back; there the function has the
`ngx` functions lua-nginx-module allows in that context and no request.
Modules OpenResty scripts commonly load are built in:
`cjson` and `cjson.safe`, `bit` with LuaJIT BitOp semantics, `table.new`,
`table.clear`, `table.nkeys`, `resty.core`, `resty.string`, `resty.md5`,
`resty.sha1`, `resty.sha256`, `resty.random`, `ngx.re`, `ngx.balancer`,
`ngx.semaphore`, whose semaphores the threads, timers and requests of
one VM share, `ngx.resp` and `ngx.req` with `add_header`, `ngx.process`,
which refuses `enable_privileged_agent` because a privileged agent would
run scripts with the gateway's own privileges outside the sandbox and
`signal_graceful_exit` because scripts do not stop workers, and
`resty.lrucache` (with `resty.lrucache.pureffi`), each VM holding its own
caches, so the library scripts vendor for its FFI needs no FFI, and
`ngx.ssl` with `ngx.ssl.clienthello` and `ngx.ssl.session`. `require` refuses `ngx.pipe`,
which would start processes on the gateway's host, and LuaJIT's `ffi`,
whose native calls would leave the sandbox, saying so; the configuration
check reports them where they are required.
`lua_capture_error_log` keeps, for each VM, what its scripts log up to the
size given, oldest messages dropped first, for `ngx.errlog.get_logs`;
`ngx.errlog` also has `raw_log`, `set_filter_level` in `init_by_lua` and
`get_sys_filter_level`. Errors the runtime's functions raise reach `pcall` as
strings, as a C function's do. Every `ngx` function either follows its documentation or
raises an error naming it, and checking the configuration lists each place a
script uses a function this gateway does not provide.

What runs differently: LuaJIT's `ffi` and `jit` modules, `goto` (Luau has
`continue`), `string.dump` and bytecode, `setfenv`, `getfenv` and `module`
are not available; `lua_package_path`, `lua_package_cpath`,
`lua_code_cache off` and `lua_socket_send_lowat`, which Linux does not
honour for TCP, are refused.
`lua_transform_underscores_in_response_headers`, `lua_use_default_type` and
`lua_need_request_body` apply as in lua-nginx-module. Directives with nothing to tune here —
`lua_load_resty_core`, `lua_malloc_trim`, `lua_sa_restart`,
`lua_thread_cache_max_entries`, `lua_http10_buffering`,
`rewrite_by_lua_no_postpone`,
`precontent_by_lua_no_postpone`,
`lua_upstream_skip_openssl_default_verify` and `balancer_keepalive` — are
read with a warning saying why, so configurations written for OpenResty
still read. `sslhandshake` verifies the server's certificate unless the
script passes `ssl_verify` false, where lua-nginx-module verifies nothing
by default: against the system's trusted roots, or the authorities of
`lua_ssl_trusted_certificate` with the revocation lists of `lua_ssl_crl`.
`lua_ssl_certificate` and `lua_ssl_certificate_key` give the certificate
cosockets present when a server asks for one, `lua_ssl_verify_depth` the
most intermediate certificates a chain may have, not counting a root the
server sends along (not limited unless written, where lua-nginx-module
allows 1), `lua_ssl_protocols` the versions among TLSv1.2 and TLSv1.3, and
`lua_ssl_ciphers` the TLS 1.2 suites by their OpenSSL names, TLS 1.3's
being offered always, as OpenSSL offers them. These terms are inherited as
the `lua_socket_*` ones are, and certificates, keys and lists are secrets
of the inventory; the importer turns a system bundle such as
`/etc/ssl/certs/ca-certificates.crt` into `system`. `lua_ssl_key_log`,
which would write session keys out, and `lua_ssl_conf_command`, which
takes OpenSSL commands, are refused.
With `lua_check_client_abort on`, rewrite, access and content handlers
watch the connection once the request body is in: when the client closes
it, the function `ngx.on_abort` registered runs as a light thread of the
run and may end the request with `ngx.exit`, and without one the run stops
and the request is logged as nginx's 499; `ngx.on_abort` returns `nil,
"lua_check_client_abort is off"` otherwise, as lua-nginx-module's does.
`ngx.location.capture` and `capture_multi` make subrequests with
Pingora's own: each goes through the gateway as a request to the same site
does, with the `method`, `args`, `body`, `vars`, `copy_all_vars`,
`share_all_vars` and `always_forward_body` options lua-nginx-module takes,
and with `ctx` its scripts run on the VM of the script that made it with
that table as their `ngx.ctx`. As in nginx, subrequests skip the access
phase, are not logged, see `ngx.is_subrequest` true and may nest fifty
deep; `capture_multi` runs its subrequests at once. A named location
(`location @name`, or `match named <name>` in a route) takes no request
path: `ngx.exec("@name")` sends the request there with its URI and
arguments as they are, starting at the route's rewrite phase as nginx's
named locations do, and a name no route has is answered 500.
`ngx.req.socket(true)` gives the client's connection as a full-duplex
cosocket (`receive`, `receiveany`, `receiveuntil`, `send`, the timeouts
and `close`) once the response header went out with `ngx.send_headers()`
and `ngx.flush(true)`, as lua-resty-websocket's server sends its 101:
what it sends passes no filter, and it receives what the client sends
after the request head, the body included. Pingora writes every response
header itself, so a script cannot write its own; before the header went
out, or with output still kept, it returns `nil` and why. Request bodies
are held in memory, never in files: `ngx.req.get_body_file` returns `nil`,
as lua-nginx-module does for a body in memory, and `ngx.req.set_body_file`
is refused, since scripts do not reach the file system.

**Native API.** Next to `ngx`, `require("panel.v1")` returns the
capabilities the specification names — `req`, `resp`, `ctx`, `upstream`,
`log` and `crypto` — with `json`, `re`, `time` and `random`, as typed
functions without OpenResty's historical quirks. A later version is a new
module, so scripts written against `panel.v1` keep working.

**Scripts and modules.** Scripts are part of the configuration: inline
blocks, and `.lua` files under the configuration's `lua/` directory. They
are checked, versioned, compared and rolled back with every other file, so
a revision fixes the exact scripts it ran. `*_by_lua_file` names a file
under `lua/`; `require("a.b")` loads a built-in module or `lua/a/b.lua` and
nothing else. A module runs once per VM in an environment of its own, and
its result is cached and shared by the requests that VM serves, as
OpenResty shares modules within a worker. Changing a script or a Lua
directive takes the `config.lua` permission, which only the Administrator
role holds by default; approval policies can cover Lua changes as a
resource of their own.

**Execution.** Each gateway generation starts one VM per data-plane worker
thread, loads the configuration's modules and handlers into it and runs
`init_by_lua` and `init_worker_by_lua`. A request takes one VM at its first
handler and keeps it until its log phase. Each handler runs in a coroutine
of that VM with a global environment of the request's own: writes stay
with the request and reads fall through to the read-only globals, which is
the request isolation OpenResty documents, enforced. `ngx.ctx` is a table
of the request shared by its phases. `ngx.shared` dictionaries live outside
the VMs, so every VM sees the same data, and a dictionary keeps its
contents across activations while its name and size stay.

What a handler changes — the request line and header, the response status
and header, a response it starts — is held until the handler returns. A
handler that fails leaves the request as it was before it ran. A response
a handler makes goes to the client as nginx sends it, before the handler
ends: `ngx.flush`, `ngx.send_headers`, `ngx.eof` and output past 64 KiB
send the header, through the header filter, and then what was printed,
each piece through the body filter; both filters run on the handler's VM
with its `ngx.ctx`, and over HTTP/1.1 the body is chunked. `ngx.eof` ends
the response but not the handler, which goes on: what it prints after
returns `nil, "seen eof"`, as lua-nginx-module's output functions do. A
handler that fails once its header went out cannot take it back: the
connection closes, unless `ngx.eof` had ended the response. Where a
response is taken by a script, from a subrequest, or tried with
`ppanel lua test`, output is kept until the handler ends, up to 16 MiB.

**Limits.** Each run of a handler is bounded:

- in wall-clock time, waits included, by `lua_time_limit` (100 ms by
  default);
- in work, counted in the VM's interrupt checks — function calls and loop
  iterations — by `lua_work_limit` (ten million by default);
- in memory, by `lua_memory_limit` per VM (64 MiB by default); an
  allocation over the limit fails the run that made it.

`lua_max_pending_timers` and `lua_max_running_timers` cap each VM's
timers, `lua_regex_cache_max_entries` the compiled expressions it keeps
(none at 0) and `lua_regex_match_limit` PCRE2's match limit, with
lua-nginx-module's defaults.

A run that keeps the CPU for more than a millisecond is suspended at its
next interrupt and resumed after other tasks have run, so a busy script
slows its own request and not the others on its thread. The host functions
scripts call bound their own work: regular expressions with PCRE2's match
limit, JSON and bodies with size limits. Handlers that can only run to
completion, such as `body_filter_by_lua`, are not suspended.

Access handlers run after the security policies of their site and
route, as access modules run before `access_by_lua` in NGINX;
`access_by_lua_no_postpone on` runs them first.

**Permissions.** Reading and changing the request and response, `ngx.ctx`,
logs, time, JSON, regular expressions, digests and random values are always
available. `lua_allow` grants a site's or route's scripts more: `body` to
read and replace request and response bodies, `upstream` to choose
upstream endpoints, and `network` to open sockets, which is off unless
granted. Declaring a shared dictionary grants its use. A function the
scripts have not been granted raises an error.

**Failures.** `lua_on_error` chooses what a handler's failure does: `fail`
answers 500, or 502 in the balancer, as OpenResty does; `continue` goes on
as if the handler had not run; a status code answers with that status.
Errors, timeouts and exhausted limits are counted and logged with the
script, line, phase and request ID.

**Observability.** `ngx.log` and `print` write the gateway's error log with
the level, site, route, phase, script and request ID; levels below
`lua_log_level` are dropped. The gateway counts runs by site, route, phase
and outcome in `pingora_panel_gateway_lua_runs`, measures them in
`pingora_panel_gateway_lua_run_duration_seconds`, and reports each VM's
memory in `pingora_panel_gateway_lua_memory_bytes`. Runs longer than
`lua_slow_threshold` (10 ms by default) are logged with their duration and
counted as slow, and `lua_debug on` logs every handler's start, end,
duration and outcome.

**Checking and testing.** The control plane compiles every script with the
same Luau compiler when a draft is checked. Syntax errors, missing files,
modules `require` cannot load, functions the gateway does not provide or a
phase does not allow, and globals a module writes are diagnostics located
in the script. Testing runs one handler against a request described in the
call, in the control plane with the gateway's runtime and limits, and
reports what it did: the changes to the request and response, the response
it sent, its logs, duration and errors. A test reaches nothing outside: it
opens no connections, and the timers it creates do not run.

**Switch.** `lua off;` in `http` keeps the scripts in the configuration but
runs none of them.

**Contracts.** The model, the configuration language, the IR and the
gateway's contract carry scripts as source text with their SHA-256,
handlers by phase, limits, permissions and fallbacks; a snapshot with any of
them requires the `lua.scripts` capability. The gateway compiles scripts
itself and refuses a snapshot whose scripts do not compile. The runtime is a
crate of its own that knows neither Pingora nor the control plane; the
gateway gives it requests through a port, and the control plane checks and
tests scripts with it.

## Alternatives

- LuaJIT, OpenResty's own VM: fastest on numeric code and runs `ffi` and
  `goto` code unchanged. But compiled traces never call debug hooks, so
  neither work nor time can be bounded while the JIT compiler is on;
  `mlua` cannot cap its memory, since 64-bit LuaJIT refuses a host
  allocator; its globals cannot be made read-only; and FFI has to be
  removed by hand. The limits of §11.3 would hold only with the JIT off.
- Lua 5.4: bounded by count hooks and its allocator, but its language
  differs from Lua 5.1 in `setfenv`, `unpack`, `loadstring` and integer
  arithmetic and printing, so more OpenResty code breaks than on Luau, and
  it has no read-only tables.
- WebAssembly on Wasmtime with the proxy-wasm ABI: fuel metering or epoch
  interruption bound work and time, each instance's linear memory is
  capped, isolation is stronger, and proxy-wasm is a settled ABI that
  Envoy, Istio, Apache APISIX and NGINX's ngx_wasm_module host. But it runs
  no OpenResty script: Lua would run in an interpreter compiled to
  WebAssembly, slower than Luau and without `ngx`, and Pingora has no
  proxy-wasm host, which would be written from nothing. It stays open as a
  later extension runtime at the same phases.
- Scripts in an external plugin process (§14.2): isolated by the operating
  system, but every phase of every request would wait for a round trip.
- A script library beside the configuration: a second history next to
  revisions, and scripts could change what runs without a revision.

## Consequences

- A gateway without the `lua.scripts` capability refuses snapshots with
  scripts instead of ignoring them.
- OpenResty code that uses `ffi`, `jit` or `goto` needs changes; checking
  points to each place.
- Scripts are trusted code: the sandbox and limits defend in depth and do
  not separate tenants.
- Each VM keeps its own copy of modules and their state, as each OpenResty
  worker does, while shared dictionaries are one for the process.
- A request's handlers run on one VM, so a VM serves its requests one
  handler run at a time; one VM per worker thread keeps that from
  limiting throughput.
