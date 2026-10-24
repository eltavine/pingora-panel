# Local changes to the vendored Pingora

The root workspace vendors Pingora 0.9.0, upstream commit `623e163d`. The
product specification (§3.2) allows changing the upstream crates only to
fix security or correctness problems, and only with a record here: the
upstream issue or pull request, the affected versions and the tests that
hold the change. A change goes once an upstream release carries an
equivalent fix. Every current difference shows with

```sh
git diff 623e163d HEAD -- 'pingora*' tinyufo
```

The weekly canary applies that difference to Pingora's main and checks
and tests the adapter there, so a change that no longer applies, as when
upstream has taken it, fails the canary.

## Patches

### Patched rustls release floor

- **Change:** `pingora-rustls` and `pingora-core`'s test client require
  rustls 0.23.46 or newer in the 0.23 release line.
- **Why:** 0.23.46 fixes session-secret disclosure in `Debug` output,
  buffered data sent after ECH rejection, excessive Brotli certificate
  decompression allocation and unauthenticated key-exchange hints
  ([release](https://github.com/rustls/rustls/releases/tag/v%2F0.23.46)).
- **Upstream:** [rustls's fixes](https://github.com/rustls/rustls/releases/tag/v%2F0.23.46);
  no Pingora source API change is needed.
- **Affected versions:** every Pingora release through 0.9.0 permits an
  affected rustls 0.23 release.
- **Tests:** `pingora-rustls` tests, `pingora-core`'s rustls listener tests,
  and the Panel TLS and gateway tests; both workspaces' dependency audits.
- **Goes when:** an upstream release requires at least rustls 0.23.46.

### Patched Tokio release floor

- **Change:** the root workspace requires Tokio 1.53.3 or later; Panel
  pins 1.53.3. The shared dependency policy also rejects the vulnerable
  Tokio ranges and ctutils releases before 0.4.4.
- **Why:** RUSTSEC-2026-0386 covers I/O safety in `AsyncFd` constructors;
  RUSTSEC-2026-0385 covers constant-time masks and integer comparisons.
- **Upstream:** [Tokio's fix](https://github.com/tokio-rs/tokio/pull/8587)
  and [ctutils 0.4.4](https://github.com/RustCrypto/utils/pull/1569).
- **Affected versions:** Pingora 0.9.0 allows affected Tokio releases;
  the previous Panel lock selected Tokio 1.53.2 and ctutils 0.4.2.
- **Tests:** both workspaces' dependency audits, the gateway adapter and
  composition-root suites, and the optional runtime telemetry suite.
- **Goes when:** an upstream release enforces the patched Tokio floor.

### Maintained YAML configuration parser

- **Change:** `pingora-core` and the `pingora-proxy` response-conversion
  example use `yaml_serde` 0.10.7 under the existing `serde_yaml` import.
- **Why:** `serde_yaml` depends on the archived `unsafe-libyaml`
  ([RUSTSEC-2024-0449](https://rustsec.org/advisories/RUSTSEC-2024-0449)).
  The YAML organization's maintained fork preserves the Serde API and
  uses `libyaml-rs` instead. Its Rust 1.82 minimum fits the upstream
  workspace's Rust 1.85 minimum; `serde-saphyr` 2.0 requires Rust 1.89.
  `yaml_serde` is MIT OR Apache-2.0; `libyaml-rs` is MIT.
- **Upstream:** [#779](https://github.com/cloudflare/pingora/issues/779)
  tracks replacing or making the YAML dependency configurable.
- **Affected versions:** every release through 0.9.0.
- **Tests:** the configuration tests in `pingora-core`, including
  `test_yaml_aliases_and_roundtrip` and
  `test_yaml_rejects_invalid_and_ambiguous_configuration`; the
  `pingora-proxy` response-conversion example build; both workspaces'
  dependency audits.
- **Goes when:** an upstream release replaces the unmaintained parser.

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

### Unbuffered L4 streams with Tokio 1.54

- **Change:** L4 streams start with a zero-capacity `BufWriter` variant
  instead of a zero-capacity `BufStream`. A zero read capacity keeps this
  write-only wrapper, with independent write buffering; positive read
  capacities use `BufStream`. Public settings retain their existing meaning.
- **Why:** Tokio 1.54 rejects a zero-capacity read buffer, so accepting or
  opening TCP, Unix and virtual streams otherwise panics. `BufWriter`
  supports zero write capacity and delegates reads directly, preserving
  disabled read buffering even for empty reads.
- **Upstream:** [#1059](https://github.com/cloudflare/pingora/issues/1059)
  and [#1060](https://github.com/cloudflare/pingora/pull/1060). The variant
  layout follows the proposed fix, with safe pin projection and a
  `BufWriter` variant to preserve independently disabled read buffering.
- **Affected versions:** Pingora 0.9.0 with Tokio 1.54 or later.
- **Tests:** L4 buffer settings, TCP, Unix and virtual stream tests in
  `pingora-core`, including transitions between buffered and raw streams.
- **Goes when:** an upstream release supports zero-buffer settings without
  constructing a zero-capacity Tokio read buffer.

### Dial9 S3 uploads without the legacy TLS client

- **Change:** `pingora-runtime` uses Dial9 0.5.5's recorder and Tokio attach
  API. The AWS client enables the current HTTPS connector explicitly instead
  of the deprecated Rustls connector in its default features.
- **Why:** Dial9 0.3's transfer manager retains Rustls 0.21 and
  rustls-webpki 0.101.7, including RUSTSEC-2026-0382. The maintained uploader
  uses the AWS SDK directly; six obsolete advisory exceptions are removed.
- **Upstream:** [dial9-rs/dial9#668](https://github.com/dial9-rs/dial9/pull/668)
  removes the transfer manager. Pingora 0.9.0 still uses Dial9 0.3.
- **Compatibility:** existing runtime and S3 settings are mapped into the
  recorder. The optional runtime guard type changes to `dial9::Recorder`;
  new trace segments live under `<trace_path>.segments/` and existing trace
  files are preserved. The runtime stops before the recorder drains. CPU
  profiling and scheduler-event options use Dial9's recorder sources, with
  their public configuration types re-exported from `dial9::cpu`.
- **Tests:** the dedicated telemetry CI job checks all optional features and
  tests recorder startup, legacy-file preservation and shutdown. Both
  workspaces retain the same advisory policy and version bans.
- **Goes when:** an upstream release uses the maintained Dial9 uploader.

## Test and documentation changes

These change no behaviour, so they have no upstream issue; they go with
the next upstream merge that makes them unnecessary.

| Where | Change | Why |
|---|---|---|
| `pingora-load-balancing/src/health_check.rs` tests | TCP and HTTP checks probe a local listener instead of `1.1.1.1` | The tests pass without internet access |
| `pingora-proxy/tests` | The client binds `127.0.0.1` instead of `127.0.0.2`; a cache range test compares origin request identifiers the origin's `nginx.conf` counts, instead of timestamps | macOS has no `127.0.0.2` alias; two requests can share a timestamp |
| `pingora-ketama/test-data/trace.sh` | Unused loop variable and missing final newline | ShellCheck |
| Rustdoc in `pingora-cache`, `pingora-core`, `pingora-foundations`, `pingora-pool` and `pingora-proxy` | Links to private or feature-gated items written as code | Documentation builds with warnings denied |
| `pingora-cache/src/key.rs` | A block comment written as line comments | Rustfmt nightlies format it alike |

`connectors::l4::tests::test_conn_timeout` expects packets to TEST-NET-1
(`192.0.2.1`) to be dropped, as on CI runners; a host whose proxy answers
for that range sees the connection succeed instead.

## Vendored SigV4a dependency

`vendor/aws-sigv4` contains the published `aws-sigv4` 1.6.3 source with
its existing copyright and Apache-2.0 license. The crates.io archive's
SHA-256 is `9d3d48355cb86b15ccbb38b8bc6b87500c824ee4ed8cbc10a961b435242b5e16`;
its source revision is `3a3f39bd932690cab6d2321b677e97a24b687753` in
`awslabs/aws-sdk-rust`, under `sdk/aws-sigv4`.

The root workspace applies a local Cargo patch that raises the signer's
optional `crypto-bigint` dependency from 0.5.4 to 0.7.6, matching
[smithy-rs#4958](https://github.com/smithy-lang/smithy-rs/pull/4958)
at `5c1c4bca2ea7af97496e077c9f1a718eaac64bf8`. The signing implementation
is unchanged. This removes both affected bigint versions from the
optional Dial9 S3 graph without disabling SigV4a or Multi-Region Access
Points. RUSTSEC-2026-0387 has no patched 0.5 release; the shared dependency
policy rejects all bigint releases before 0.7.6.

The package retains its upstream signing vectors and adds the proposal's
offline MRAP signing and presigning tests, with explicit test targets in
its manifest. Git preserves the HTTP vectors' canonical whitespace.
`runtime-telemetry` runs that standalone package's full
feature suite and the runtime's full feature compilation. Cargo's local
registry bookkeeping and the published package's unused original manifest
and lockfile are omitted. The package is excluded from root workspace
membership because its optional S3 dependency chain has a higher Rust
minimum than Pingora's default build.

Remove the local Cargo patch and vendor directory when a published AWS
signer enforces the patched bigint dependency. Confirm the root
workspace's full-feature dependency audit and MRAP signing tests before
removing the backport.

## Removed

- Custom L4 connectors bounded by `connection_timeout` (`ab927042`):
  reverted to upstream, which bounds them by `total_connection_timeout`
  alone. The gateway uses no custom connector.
