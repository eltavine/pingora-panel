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
| `server_rewrite_by_lua_block`, `_file` | before the route is chosen; may change the URI and host it is chosen by | `http`, `server` |
| `rewrite_by_lua_block`, `_file` | after the route is chosen, before security policies | `http`, `server`, `route` |
| `access_by_lua_block`, `_file` | after security policies | `http`, `server`, `route` |
| `content_by_lua_block`, `_file` | as the action of a server or route, instead of proxying or serving files | `server`, `route` |
| `balancer_by_lua_block`, `_file` | each time an upstream endpoint is chosen, retries included | `upstream` |
| `header_filter_by_lua_block`, `_file` | on the response header, before it is sent | `http`, `server`, `route` |
| `body_filter_by_lua_block`, `_file` | on each chunk of the response body | `http`, `server`, `route` |
| `log_by_lua_block`, `_file` | after the response is sent | `http`, `server`, `route` |
| `lua_shared_dict <name> <size>` | declares a dictionary every VM shares | `http` |

The body of a `*_by_lua_block` directive is read with Lua's lexical rules,
as ngx_lua reads it, so braces inside strings, long brackets and comments
do not end it, and it is printed back unchanged. The NGINX importer carries
these directives over.

`ngx` is lua-nginx-module's API with its documented semantics, in the phases
where lua-nginx-module allows each function: `ngx.var`, `ngx.ctx`,
`ngx.req`, `ngx.resp`, `ngx.header`, `ngx.status`, `ngx.exit`,
`ngx.redirect`, `ngx.say`, `ngx.print`, `ngx.log`, the `ngx.HTTP_*` and
log level constants, `ngx.re` on PCRE2 (the engine NGINX uses),
`ngx.shared`, the time, escaping, argument, base64, digest and quoting
functions, `ngx.sleep`, `ngx.get_phase`, `ngx.worker`, `ngx.config`,
`ngx.balancer` and the TCP cosockets of `ngx.socket.tcp` and
`ngx.socket.connect` (`connect`, `sslhandshake`, `send`, `receive`,
`receiveany`, `receiveuntil`, `settimeout`, `settimeouts`, `setkeepalive`,
`getreusedtimes` and `close`), whose idle connections each VM keeps for
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
body in memory. `ngx.exec` redirects internally: the request is handled
again from `server_rewrite` with its new URI and arguments, and as in
nginx a request that changes its URI more than ten times, by jumps and
redirects together, is answered 500; `$request_uri` keeps the client's
through both. Modules OpenResty scripts commonly load are built in:
`cjson` and `cjson.safe`, `bit` with LuaJIT BitOp semantics, `table.new`,
`table.clear`, `table.nkeys`, `resty.core`, `resty.string`, `resty.md5`,
`resty.sha1`, `resty.sha256`, `resty.random`, `ngx.re`, `ngx.balancer`
and `ngx.semaphore`, whose semaphores the threads, timers and requests of
one VM share. Errors the runtime's functions raise reach `pcall` as
strings, as a C function's do. Every `ngx` function either follows its documentation or
raises an error naming it, and checking the configuration lists each place a
script uses a function this gateway does not provide.

What runs differently: LuaJIT's `ffi` and `jit` modules, `goto` (Luau has
`continue`), `string.dump` and bytecode, `setfenv`, `getfenv` and `module`
are not available; `lua_package_path`, `lua_package_cpath`,
`lua_code_cache off` and `lua_socket_send_lowat`, which Linux does not
honour for TCP, are refused.
`lua_transform_underscores_in_response_headers` and `lua_use_default_type`
apply as in lua-nginx-module. Directives with nothing to tune here —
`lua_load_resty_core`, `lua_malloc_trim`, `lua_sa_restart`,
`lua_thread_cache_max_entries`, `lua_worker_thread_vm_pool_size`,
`lua_capture_error_log`, `lua_check_client_abort`, `lua_http10_buffering`,
`rewrite_by_lua_no_postpone`, `precontent_by_lua_no_postpone`,
`lua_upstream_skip_openssl_default_verify` and `balancer_keepalive` — are
read with a warning saying why, so configurations written for OpenResty
still read. `sslhandshake` verifies the server's
certificate with the system's trusted roots unless the script passes
`ssl_verify` false, where lua-nginx-module verifies nothing by default.
Subrequests (`ngx.location.capture`), named locations, the request's own
socket, `ngx.on_abort`, worker threads and body files are not available.

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
handler that fails leaves the request as it was before it ran.

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
