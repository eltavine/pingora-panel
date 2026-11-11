This is the published `sqlx-sqlite` 0.9.0 driver source with its SQLite
binding range updated. SQLx merged support for `libsqlite3-sys` 0.39 in
[transact-rs/sqlx#4502](https://github.com/transact-rs/sqlx/pull/4502).
The local range requires the patched line so bundled builds include SQLite
3.54.1 or later, addressing [RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html).
The driver implementation is unchanged. The original crates.io archive SHA-256
is `488e99c397a62007e4229aec669a179816339afc6d2620ca6fa420dbee2e982c`.
The SQLx 0.9.0 license files are included in full.

Remove the workspace patch and this copy when a released SQLx driver accepts
the patched SQLite bindings. Remove its root workspace exclusion, the
canary archive entry and the workflow path filters at the same time.
Verify the lockfile and run the Panel SQLite adapter and backup tests.
