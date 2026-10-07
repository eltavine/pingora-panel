# Local changes to the vendored Pingora

The root workspace vendors Pingora 0.9.0, upstream commit `4487f7b2`. The
product specification (§3.2) allows changing the upstream crates only to
fix security or correctness problems, and only with a record here: the
upstream issue or pull request, the affected versions and the tests that
hold the change. A change goes once an upstream release carries an
equivalent fix. Every current difference shows with

```sh
git diff 4487f7b2 HEAD -- 'pingora*' tinyufo
```

The weekly canary applies that difference to Pingora's main and checks
and tests the adapter there, so a change that no longer applies, as when
upstream has taken it, fails the canary.

## Patches

### TLS settings from a rustls server configuration

- **Change:** `TlsSettings::from_server_config` and
  `Acceptor::from_server_config` in
  `pingora-core/src/listeners/tls/rustls/mod.rs` (`fdf62bc8`).
- **Why:** the gateway builds each listener's rustls `ServerConfig` at run
  time, with certificates from the panel's store, an SNI resolver and
  narrowed protocol versions and cipher suites, and needs Pingora's
  listener to take that configuration while keeping its handshake
  callbacks and offload. Upstream builds the configuration only from
  certificate and key paths.
- **Upstream:** [#832](https://github.com/cloudflare/pingora/issues/832)
  asks for the same constructor;
  [#877](https://github.com/cloudflare/pingora/pull/877) proposes a
  certificate resolver instead. Both are open.
- **Affected versions:** every release through 0.9.0.
- **Tests:** `test_from_server_config_handshake` and
  `test_settings_from_server_config_use_it_as_is` in `pingora-core`.
- **Goes when:** an upstream release lets a rustls listener take a server
  configuration or a certificate resolver, and the gateway uses it.

### PEM parsing without rustls-pemfile

- **Change:** `pingora-rustls` reads certificates and keys through
  `rustls-pki-types`' `PemObject` and moves to `rustls-native-certs` 0.8
  (`8212582b`).
- **Why:** `rustls-pemfile` is archived and unmaintained
  ([RUSTSEC-2025-0134](https://rustsec.org/advisories/RUSTSEC-2025-0134));
  `rustls-pki-types` carries the same parser.
- **Upstream:** [#772](https://github.com/cloudflare/pingora/issues/772),
  open.
- **Affected versions:** `pingora-rustls` 0.4.0 through 0.9.0.
- **Tests:** `loads_certificates_and_keys`,
  `absent_items_are_empty_rather_than_errors` and
  `ca_files_hold_only_certificates` in `pingora-rustls`.
- **Goes when:** an upstream release drops `rustls-pemfile`.

### Plain trait implementations instead of derivative

- **Change:** `Backend`'s equality, ordering and hashing in
  `pingora-load-balancing` and `PeerOptions`' `Debug` in `pingora-core`
  are written out instead of derived with `derivative` (`1b1cda83`).
- **Why:** `derivative` is unmaintained
  ([RUSTSEC-2024-0388](https://rustsec.org/advisories/RUSTSEC-2024-0388)).
- **Upstream:** [#463](https://github.com/cloudflare/pingora/issues/463)
  and [#889](https://github.com/cloudflare/pingora/issues/889), open.
- **Affected versions:** at least 0.4.0, when the advisory was raised
  upstream, through 0.9.0.
- **Tests:** `test_backend_identity_ignores_ext` in
  `pingora-load-balancing` and `debug_output_leaves_out_the_hooks` in
  `pingora-core`.
- **Goes when:** an upstream release drops `derivative`.

### s2n-tls held at 0.3.38

- **Change:** `pingora-s2n` requires `s2n-tls` and `s2n-tls-tokio`
  `=0.3.38` instead of `0.3` (`75379d4e`).
- **Why:** later 0.3 releases need Rust 1.91, above the minimum of 1.85
  the workspace shares with upstream, so an unconstrained resolution
  breaks the minimum-version build with the `s2n` feature.
- **Upstream:** none found.
- **Affected versions:** `pingora-s2n` 0.7.0 through 0.9.0.
- **Tests:** the minimum-version job of `.github/workflows/build.yml`.
- **Goes when:** the minimum Rust version reaches 1.91, or upstream
  bounds the requirement.

### The server's certificate chain in rustls client digests

- **Change:** a rustls client connection keeps the server's certificate
  chain, its own first, in its digest's `extension` as a
  `Vec<CertificateDer<'static>>` (`connect` in
  `pingora-core/src/protocols/tls/rustls/stream.rs`, `a778c6eb`).
- **Why:** `proxy_ssl_verify_by_lua` scripts judge the certificates an
  upstream presented, and the gateway sees an upstream connection only
  through its digest, which holds the certificate's organization, serial
  number and hash but not the certificate. Only Pingora can set the
  digest's extension.
- **Upstream:** [#421](https://github.com/cloudflare/pingora/issues/421)
  asks for the certificate itself in `SslDigest`; open.
- **Affected versions:** every release through 0.9.0.
- **Tests:** `proxy_verify_scripts_judge_upstream_certificates` in
  `gateway-pingora`.
- **Goes when:** an upstream release keeps the server's certificates where
  the gateway can read them.

### Upgraded HTTP/1 tunnels past the request's own body end

- **Change:** `proxy_handle_upstream` in `pingora-proxy/src/proxy_h1.rs`
  ends an upgraded tunnel on the end of the upgraded body or a closed
  pipe, not on the end of the upgrade request's own body (`1b92f9ba`).
- **Why:** the end of an upgrade request's empty body reaches the
  upstream half as a plain `Body` task. When the upstream's 101 is read
  first, as a fast upstream or a busy runtime makes likely, that task was
  taken for the end of the tunnel: the client got the 101 and then the end
  of the connection, and nothing it sent after the handshake reached the
  upstream.
- **Upstream:** [#946](https://github.com/cloudflare/pingora/issues/946);
  [#947](https://github.com/cloudflare/pingora/pull/947) and
  [#1016](https://github.com/cloudflare/pingora/pull/1016) propose fixes.
  All are open.
- **Affected versions:** 0.8.0, which switches the body mode on a 101,
  through 0.9.0.
- **Tests:** `upgraded_tunnels_outlive_a_request_body_end_read_after_the_101`
  in `pingora-proxy`, and `websocket_upgrades_go_through_beyond_body_limits`
  in `gateway-pingora`.
- **Goes when:** an upstream release keeps the tunnel open whichever of
  the two comes first.

## Test and documentation changes

These change no behaviour, so they have no upstream issue; they go with
the next upstream merge that makes them unnecessary.

| Where | Change | Why |
|---|---|---|
| `pingora-load-balancing/src/health_check.rs` tests | TCP and HTTP checks probe a local listener instead of `1.1.1.1` | The tests pass without internet access |
| `pingora-proxy/tests` | The client binds `127.0.0.1` instead of `127.0.0.2`; a cache range test compares origin request identifiers the origin's `nginx.conf` counts, instead of timestamps | macOS has no `127.0.0.2` alias; two requests can share a timestamp |
| `pingora-core/src/protocols/http/v2/server.rs` tests | The two conflicting `Content-Length` tests also accept h2 resetting the stream before Pingora sees it, which then never surfaces nor counts against the malformed-stream budget | h2 0.4.20 rejects duplicate `Content-Length` values that differ itself |
| `pingora-ketama/test-data/trace.sh` | Unused loop variable and missing final newline | ShellCheck |
| Rustdoc in `pingora-cache`, `pingora-core`, `pingora-foundations`, `pingora-pool` and `pingora-proxy` | Links to private or feature-gated items written as code | Documentation builds with warnings denied |
| `pingora-cache/src/key.rs` | A block comment written as line comments | Rustfmt nightlies format it alike |

`connectors::l4::tests::test_conn_timeout` expects packets to TEST-NET-1
(`192.0.2.1`) to be dropped, as on CI runners; a host whose proxy answers
for that range sees the connection succeed instead.

## Removed

- Custom L4 connectors bounded by `connection_timeout` (`ab927042`):
  reverted to upstream, which bounds them by `total_connection_timeout`
  alone. The gateway uses no custom connector.
