# 0029: Gateway allocator and release builds

Status: accepted.

## Context

A profile of `gatewayd` under the HTTP/1.1 workload of the
[gateway benchmarks](0024-gateway-benchmarks.md) put the platform allocator's
`malloc` and `free` among its largest costs after socket I/O. Release builds
compiled every crate as sixteen codegen units with no cross-crate
optimisation.

## Decision

**Allocator.** `gatewayd` uses jemalloc, through `tikv-jemallocator`, as its
global allocator on every target except MSVC, which keeps the platform
allocator. The control-plane services are off the request path and keep the
platform allocator.

**Release profile.** Release builds use fat link-time optimisation with one
codegen unit.

The allocators were compared on the criteria of the product specification:

| | Platform | jemalloc | mimalloc |
|---|---|---|---|
| Fit | Differs by platform: glibc, macOS, musl | Built for long-running multi-threaded servers; Pingora's own example server uses it | Same class as jemalloc |
| Maturity and upkeep | Ships with the OS | jemalloc 5.3; crate maintained by TiKV | Microsoft's mimalloc; community crate |
| Security | Platform hardening | No hardening by default | Optional secure mode, at a cost |
| Licence | — | MIT or Apache-2.0 | MIT |
| Observability | — | Statistics through `mallctl` | Statistics through `mi_stats` |
| Platforms | All | Not MSVC | All |
| Upgrade risk | — | One static in `gatewayd` | One static in `gatewayd` |

They were measured side by side on the benchmarks' `http1-1k` workload, on
one Apple M5 host: four gateway workers each, 64 connections, eight rounds
of five seconds in alternating order. Ratios are medians of each round's
pair against the first build, with the lowest and highest pair in brackets.

| Build | Throughput | CPU per request | Peak RSS |
|---|---|---|---|
| Platform allocator | 1 | 1 | 27.7 MiB |
| jemalloc | 1.063 [1.022, 1.114] | 0.916 [0.878, 0.943] | 38.9 MiB |
| mimalloc | 1.040 [0.975, 1.118] | 0.942 [0.873, 0.987] | 39.4 MiB |
| jemalloc, thin LTO | 1.018 [0.994, 1.051] against jemalloc | 0.989 [0.956, 1.001] | 39.0 MiB |
| jemalloc, fat LTO, one codegen unit | 1.026 [0.998, 1.054] against jemalloc | 0.970 [0.933, 0.999] | 37.2 MiB |

## Alternatives

- The platform allocator needs no dependency and uses the least memory, but
  was the slowest, and its behaviour changes with the C library underneath.
- mimalloc also builds for MSVC, but did less than jemalloc here for the same
  memory.
- Thin link-time optimisation builds faster, but gained less and left the
  binary larger.
- Aborting on panic was not taken: a panic in one request's task would stop
  the whole gateway, where tokio and Pingora now contain it.

## Consequences

- The gateway's peak resident memory under load rises by about 11 MiB, for
  jemalloc's arenas and thread caches.
- Release builds, the container image among them, take longer to link; debug
  and test builds are unchanged. The release `gatewayd` shrinks from 21.7 MB
  to 15.4 MB.
- jemalloc's C sources build with the C toolchain the Rust build image
  already has.
- The figures come from one macOS host. Linux, where the platform allocator
  is glibc's, still needs a baseline run by the method of ADR 0024.
