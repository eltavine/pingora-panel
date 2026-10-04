# 0024: Gateway benchmarks

Status: accepted.

## Context

The product specification asks for performance claims backed by a fixed
environment, repeated runs and raw data, and for a baseline against NGINX
or OpenResty under stated workloads. Proxy comparisons are easy to get
wrong: a load generator that waits for each response hides latency behind
its own back-pressure (coordinated omission), a single run hides variance,
a fixed order hides drift, and an unrecorded environment makes numbers
impossible to compare later.

## Decision

**Harness.** `panel/benchmarks/gateway/run.py` drives every run. It is a
Python script that [uv](https://docs.astral.sh/uv/) runs with its
dependencies declared inline and locked next to it; `psutil` reads the CPU
time and resident memory of each proxy's processes on Linux and macOS alike.

**Load generator.** [oha](https://github.com/hatoo/oha): HTTP/1.1 and
HTTP/2, JSON output, and a fixed-rate mode that corrects latency for
coordinated omission.

**Baseline and upstream.** NGINX serves the static upstream for both
proxies and is the proxy the gateway is compared with: the same number of
workers, upstream keep-alive, no limit on keep-alive requests, access logs
off, and the same certificate and TLS versions. Its `stub_status` counts the
connections each proxy opens to the upstream.

**The gateway as shipped.** The release `gatewayd` binary serves a snapshot
that the `bench_seed` example activates into its state directory with the
gateway's own engine, so the product has no benchmark-only switch. The
gateway keeps its production defaults, request metrics included.

**Method.** For each workload the harness probes both proxies' throughput
and fixes the rate of the latency runs at half the slower one. Each of at
least ten repetitions warms a proxy up and measures it twice: a closed loop
for throughput, CPU seconds per million successful requests, peak resident
memory, errors and upstream connections per request; then an open loop at
the fixed rate for p50, p95 and p99 latency. The proxy measured first
alternates between repetitions. The report gives medians with 95% bootstrap
confidence intervals and calls a difference only where the intervals do not
overlap. Every run is kept, with the hardware, load, versions, revision and
Pingora source commit it ran with.

## Alternatives

- wrk and wrk2 speak no HTTP/2, and wrk2 is no longer maintained.
- k6 and Gatling script load in runtimes of their own, which take CPU the
  proxies need on a shared host.
- h2load only measures HTTP/2.
- Criterion benchmarks measure functions, not a proxy process with its
  sockets and TLS.

## Consequences

- Results from one host compare the proxies on that host. The load
  generator, both proxies and the upstream share its CPUs, so the numbers
  are not capacity figures; those need a dedicated Linux host with the load
  generator on another machine.
- Shared CI runners are too noisy for these comparisons. CI only compiles
  the seed example; runs are made on purpose and their results committed
  with their environment.
