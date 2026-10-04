# Gateway benchmarks

`run.py` compares the gateway with NGINX in front of the same upstream, by
the performance method of the [product specification](../../../PRODUCT_SPEC.md)
(section 17.3) and [ADR 0024](../../../docs/adr/0024-gateway-benchmarks.md).

## Run

It needs [oha](https://github.com/hatoo/oha), NGINX with its SSL, HTTP/2 and
stub_status modules, OpenSSL and [uv](https://docs.astral.sh/uv/). From the
repository root:

```sh
cargo build --manifest-path panel/Cargo.toml --release --locked \
  -p gatewayd --bin gatewayd --example bench_seed
panel/benchmarks/gateway/run.py
```

`--workloads`, `--repetitions` (10), `--duration` (10 s), `--warmup` (3 s),
`--connections` (64), `--workers` (4) and `--latency-load` (0.5 of the slower
proxy's throughput) change the defaults. All four workloads take about 35
minutes. The load generator, both proxies and the upstream share the host,
so stop other work first.

## Workloads

| Name | Transport | Response |
|---|---|---|
| `http1-1k` | HTTP/1.1 | 1 KiB |
| `http1-64k` | HTTP/1.1 | 64 KiB |
| `https1-1k` | HTTP/1.1 over TLS | 1 KiB |
| `https2-1k` | HTTP/2 over TLS, 8 streams per connection | 1 KiB |

## Results

Each run writes `results/<UTC time>/`:

- `report.md`: medians with 95% confidence intervals and a verdict per
  metric;
- `runs.jsonl`: every measurement;
- `environment.json`: hardware, load, versions, revision, Pingora source
  commit, parameters, throughput probes and the fixed rates;
- the NGINX configurations used.

Numbers from one host compare the proxies on that host; they are not
capacity figures for other hardware.
