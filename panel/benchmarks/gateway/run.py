#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["psutil>=7,<8"]
# ///
"""Runs the gateway and NGINX under the same workloads (PRODUCT_SPEC 17.3).

Both proxies forward to one NGINX upstream. Every repetition measures each
proxy twice: a closed loop for throughput, CPU and memory, then an open loop
at a fixed rate for latency, corrected for coordinated omission. The proxy
that goes first alternates between repetitions. Every run is kept as raw
data next to the environment it ran in and a report of medians with 95%
bootstrap confidence intervals.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import random
import re
import resource
import shutil
import signal
import socket
import ssl
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path

import psutil

HERE = Path(__file__).resolve().parent
PANEL = HERE.parents[1]
REPOSITORY = PANEL.parent
PROXIES = ("gateway", "nginx")
DEADLINE_ABORT = "aborted due to deadline"


@dataclass(frozen=True)
class Workload:
    tls: bool
    http2: bool
    body_bytes: int

    @property
    def scheme(self) -> str:
        return "https" if self.tls else "http"


WORKLOADS = {
    "http1-1k": Workload(tls=False, http2=False, body_bytes=1024),
    "http1-64k": Workload(tls=False, http2=False, body_bytes=65536),
    "https1-1k": Workload(tls=True, http2=False, body_bytes=1024),
    "https2-1k": Workload(tls=True, http2=True, body_bytes=1024),
}


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def command_output(*command: str) -> str:
    try:
        completed = subprocess.run(command, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return ""
    return (completed.stdout + completed.stderr).strip()


def wait_until_served(url: str, process: subprocess.Popen[bytes], log: Path) -> None:
    context = ssl._create_unverified_context()
    started = time.monotonic()
    while time.monotonic() - started < 30 and process.poll() is None:
        try:
            with urllib.request.urlopen(url, timeout=1, context=context) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.1)
    tail = log.read_text(errors="replace")[-2000:] if log.exists() else ""
    raise RuntimeError(f"{url} was not served:\n{tail}")


class Processes:
    """Started processes, stopped in reverse order."""

    def __init__(self) -> None:
        self.started: list[subprocess.Popen[bytes]] = []

    def start(self, command: list[str], log: Path, env: dict[str, str] | None = None):
        handle = log.open("wb")
        process = subprocess.Popen(
            command, stdout=handle, stderr=subprocess.STDOUT, env=env, cwd=log.parent
        )
        self.started.append(process)
        return process

    def stop_all(self) -> None:
        for process in reversed(self.started):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
        self.started.clear()


def upstream_config(work: Path, port: int, stats_port: int, workers: int) -> str:
    return f"""worker_processes {workers};
pid {work}/upstream.pid;
error_log {work}/upstream-error.log warn;
events {{ worker_connections 8192; }}
http {{
    access_log off;
    sendfile on;
    tcp_nopush on;
    keepalive_requests 1000000;
    open_file_cache max=16 inactive=1h;
    server {{
        listen 127.0.0.1:{port} backlog=4096;
        root {work}/www;
    }}
    server {{
        listen 127.0.0.1:{stats_port};
        location / {{ stub_status; }}
    }}
}}
"""


def proxy_config(work: Path, port: int, upstream: int, workers: int, workload: Workload) -> str:
    tls = (
        f"""
        ssl_certificate {work}/secrets/bench.crt;
        ssl_certificate_key {work}/secrets/bench.key;
        ssl_protocols TLSv1.2 TLSv1.3;
        ssl_session_cache shared:bench:10m;"""
        if workload.tls
        else ""
    )
    return f"""worker_processes {workers};
worker_rlimit_nofile 65536;
pid {work}/nginx.pid;
error_log {work}/nginx-error.log warn;
events {{ worker_connections 8192; }}
http {{
    access_log off;
    keepalive_requests 1000000;
    upstream bench {{
        server 127.0.0.1:{upstream};
        keepalive 256;
        keepalive_requests 1000000;
    }}
    server {{
        listen 127.0.0.1:{port}{" ssl" if workload.tls else ""} backlog=4096;
        http2 {"on" if workload.http2 else "off"};{tls}
        location / {{
            proxy_pass http://bench;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
        }}
    }}
}}
"""


def upstream_accepts(stats_port: int) -> int:
    with urllib.request.urlopen(f"http://127.0.0.1:{stats_port}/", timeout=5) as response:
        text = response.read().decode()
    accepts, _handled, _requests = map(int, text.splitlines()[2].split())
    return accepts


class Sampler(threading.Thread):
    """Samples the resident memory of a process tree while a run lasts."""

    def __init__(self, processes: list[psutil.Process]) -> None:
        super().__init__(daemon=True)
        self.processes = processes
        self.samples: list[int] = []
        self.done = threading.Event()

    def run(self) -> None:
        while not self.done.is_set():
            self.samples.append(sum(process.memory_info().rss for process in self.processes))
            self.done.wait(0.2)


def tree(pid: int) -> list[psutil.Process]:
    root = psutil.Process(pid)
    return [root, *root.children(recursive=True)]


def cpu_seconds(processes: list[psutil.Process]) -> float:
    return sum(sum(process.cpu_times()[:2]) for process in processes)


def oha(url: str, workload: Workload, args, seconds: float, rate: int | None = None) -> dict:
    command = ["oha", "--no-tui", "--output-format", "json", "-z", f"{seconds}s", "-w"]
    if workload.http2:
        command += ["--http2", "-c", str(max(1, args.connections // 8)), "-p", "8"]
    else:
        command += ["-c", str(args.connections)]
    if workload.tls:
        command.append("--insecure")
    if rate is not None:
        command += ["-q", str(rate), "--latency-correction"]
    completed = subprocess.run([*command, url], capture_output=True, text=True, check=True)
    return json.loads(completed.stdout)


def outcome(report: dict) -> dict:
    statuses = {int(code): count for code, count in report["statusCodeDistribution"].items()}
    errors = {
        message: count
        for message, count in report["errorDistribution"].items()
        if message != DEADLINE_ABORT
    }
    succeeded = sum(count for code, count in statuses.items() if 200 <= code < 300)
    failed = sum(count for code, count in statuses.items() if not 200 <= code < 300)
    failed += sum(errors.values())
    percentiles = report["latencyPercentiles"]
    return {
        "seconds": report["summary"]["total"],
        "succeeded": succeeded,
        "failed": failed,
        "errors": errors,
        "requests_per_second": succeeded / report["summary"]["total"],
        "latency_ms": {
            key: (percentiles[key] or 0) * 1000 for key in ("p50", "p90", "p95", "p99", "p99.9")
        },
    }


def measure(proxy: dict, workload: Workload, args, stats_port: int, rate: int | None) -> dict:
    processes = tree(proxy["pid"])
    sampler = Sampler(processes)
    accepted = upstream_accepts(stats_port)
    cpu = cpu_seconds(processes)
    sampler.start()
    report = oha(proxy["url"], workload, args, args.duration, rate)
    sampler.done.set()
    sampler.join()
    result = outcome(report)
    result["cpu_seconds"] = cpu_seconds(processes) - cpu
    result["upstream_connections"] = upstream_accepts(stats_port) - accepted
    result["rss_peak_bytes"] = max(sampler.samples, default=0)
    result["rss_mean_bytes"] = int(statistics.fmean(sampler.samples)) if sampler.samples else 0
    return result


def bootstrap(values: list[float], seed: int = 1729, rounds: int = 10_000) -> dict:
    if not values:
        return {"median": None, "low": None, "high": None}
    generator = random.Random(seed)
    medians = sorted(
        statistics.median(generator.choices(values, k=len(values))) for _ in range(rounds)
    )
    return {
        "median": statistics.median(values),
        "low": medians[int(rounds * 0.025)],
        "high": medians[int(rounds * 0.975) - 1],
    }


# Metric, unit, whether higher is better, and how a run yields it.
METRICS = [
    ("throughput", "req/s", True, "closed", lambda run: run["requests_per_second"]),
    (
        "CPU per million successful requests",
        "CPU s",
        False,
        "closed",
        lambda run: run["cpu_seconds"] / run["succeeded"] * 1e6 if run["succeeded"] else None,
    ),
    ("peak RSS", "MiB", False, "closed", lambda run: run["rss_peak_bytes"] / 2**20),
    (
        "error rate",
        "%",
        False,
        "closed",
        lambda run: 100 * run["failed"] / max(1, run["succeeded"] + run["failed"]),
    ),
    (
        "upstream connections per 1000 requests",
        "",
        False,
        "closed",
        lambda run: 1000 * run["upstream_connections"] / max(1, run["succeeded"]),
    ),
    ("latency p50 at fixed rate", "ms", False, "open", lambda run: run["latency_ms"]["p50"]),
    ("latency p95 at fixed rate", "ms", False, "open", lambda run: run["latency_ms"]["p95"]),
    ("latency p99 at fixed rate", "ms", False, "open", lambda run: run["latency_ms"]["p99"]),
    # Shows that both proxies kept up with the fixed rate; not a comparison.
    ("achieved fixed rate", "req/s", None, "open", lambda run: run["requests_per_second"]),
]


def number(value: float) -> str:
    if abs(value) >= 1000:
        return f"{value:,.0f}"
    if abs(value) >= 10:
        return f"{value:.1f}"
    return f"{value:.3g}"


def verdict(gateway: dict, nginx: dict, higher_is_better: bool) -> str:
    if None in (gateway["low"], gateway["high"], nginx["low"], nginx["high"]):
        return "n/a"
    if gateway["low"] > nginx["high"]:
        return "gateway higher" if higher_is_better else "gateway worse"
    if gateway["high"] < nginx["low"]:
        return "gateway worse" if higher_is_better else "gateway lower"
    return "comparable"


def report(runs: list[dict], environment: dict) -> str:
    lines = [
        "# Gateway and NGINX",
        "",
        f"Run {environment['started_at']} on {environment['host']['cpu']} "
        f"({environment['host']['logical_cpus']} logical CPUs, "
        f"{environment['host']['memory_bytes'] / 2**30:.0f} GiB), {environment['host']['os']}.",
        f"Gateway {environment['versions']['gateway']} at {environment['versions']['revision']}, "
        f"Pingora {environment['versions']['pingora']}; {environment['versions']['nginx']}.",
        f"{environment['parameters']['repetitions']} repetitions of "
        f"{environment['parameters']['duration']} s per measurement after "
        f"{environment['parameters']['warmup']} s of warm-up; "
        f"{environment['parameters']['workers']} workers per proxy. "
        "Medians with 95% bootstrap confidence intervals; a difference is reported "
        "only where the intervals do not overlap.",
        "",
    ]
    for name in environment["parameters"]["workloads"]:
        workload = WORKLOADS[name]
        rate = environment["fixed_rates"][name]
        lines += [
            f"## {name}",
            "",
            f"{workload.scheme.upper()}, {'HTTP/2' if workload.http2 else 'HTTP/1.1'}, "
            f"{workload.body_bytes} byte responses; latency at {rate} req/s.",
            "",
            "| Metric | Gateway | NGINX | Gateway / NGINX | Verdict |",
            "|---|---|---|---|---|",
        ]
        for metric, unit, higher_is_better, mode, value in METRICS:
            summaries = {}
            for proxy in PROXIES:
                values = [
                    value(run["result"])
                    for run in runs
                    if run["workload"] == name and run["proxy"] == proxy and run["mode"] == mode
                ]
                summaries[proxy] = bootstrap([v for v in values if v is not None])

            def cell(summary: dict) -> str:
                if summary["median"] is None:
                    return "n/a"
                interval = f"{number(summary['low'])}, {number(summary['high'])}"
                return f"{number(summary['median'])} [{interval}]"

            ratio = (
                f"{summaries['gateway']['median'] / summaries['nginx']['median']:.2f}"
                if summaries["nginx"]["median"]
                else "n/a"
            )
            label = f"{metric} ({unit})" if unit else metric
            judged = (
                verdict(summaries["gateway"], summaries["nginx"], higher_is_better)
                if higher_is_better is not None
                else "—"
            )
            lines.append(
                f"| {label} | {cell(summaries['gateway'])} | {cell(summaries['nginx'])} "
                f"| {ratio} | {judged} |"
            )
        lines.append("")
    return "\n".join(lines)


def environment_record(args, started_at: str) -> dict:
    cargo_lock = (PANEL / "Cargo.lock").read_text()
    pingora = re.search(r'name = "pingora-core"\nversion = "([^"]+)"', cargo_lock)
    manifest = (PANEL / "gatewayd" / "Cargo.toml").read_text()
    gateway = re.search(r'^version = "([^"]+)"', manifest, re.MULTILINE)
    cpu = (
        command_output("/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string")
        if sys.platform == "darwin"
        else next(
            (
                line.split(":", 1)[1].strip()
                for line in Path("/proc/cpuinfo").read_text().splitlines()
                if line.startswith("model name")
            ),
            platform.processor(),
        )
    )
    dirty = bool(command_output("git", "-C", str(REPOSITORY), "status", "--porcelain"))
    return {
        "started_at": started_at,
        "host": {
            "cpu": cpu,
            "logical_cpus": os.cpu_count(),
            "memory_bytes": psutil.virtual_memory().total,
            "os": platform.platform(),
            "load_average": os.getloadavg(),
        },
        "versions": {
            "gateway": gateway.group(1) if gateway else "unknown",
            "revision": command_output("git", "-C", str(REPOSITORY), "rev-parse", "HEAD")
            + ("-dirty" if dirty else ""),
            "pingora": pingora.group(1) if pingora else "unknown",
            "pingora_source_commit": command_output(
                "git", "-C", str(REPOSITORY), "log", "-1", "--format=%H", "--", "pingora-core"
            ),
            "gateway_tls": "rustls",
            "nginx": command_output("nginx", "-v"),
            "nginx_tls": next(
                (
                    line
                    for line in command_output("nginx", "-V").splitlines()
                    if "built with" in line
                ),
                "",
            ),
            "oha": command_output("oha", "--version"),
            "python": platform.python_version(),
            "psutil": psutil.__version__,
        },
        "parameters": {
            "workloads": args.workloads,
            "definitions": {name: asdict(WORKLOADS[name]) for name in args.workloads},
            "repetitions": args.repetitions,
            "duration": args.duration,
            "warmup": args.warmup,
            "connections": args.connections,
            "http2_streams": f"{max(1, args.connections // 8)} connections x 8 streams",
            "workers": args.workers,
            "upstream_workers": args.upstream_workers,
            "latency_load": args.latency_load,
            "order": "gateway first in even repetitions, NGINX first in odd ones",
            "logging": "access logs off in both proxies and the upstream",
            "tls": "ECDSA P-256 certificate, TLS 1.2 and 1.3 offered, session resumption on",
            "upstream": "NGINX serving a static file, keep-alive without a request limit",
        },
        "probes": {},
        "fixed_rates": {},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--workloads", default=",".join(WORKLOADS), type=lambda v: v.split(","))
    parser.add_argument("--repetitions", type=int, default=10)
    parser.add_argument("--duration", type=float, default=10)
    parser.add_argument("--warmup", type=float, default=3)
    parser.add_argument("--connections", type=int, default=64)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--upstream-workers", type=int, default=2)
    parser.add_argument(
        "--latency-load",
        type=float,
        default=0.5,
        help="fixed rate of the latency runs, as a share of the slower proxy's throughput",
    )
    release = PANEL / "target" / "release"
    parser.add_argument("--gatewayd", type=Path, default=release / "gatewayd")
    parser.add_argument("--seed", type=Path, default=release / "examples" / "bench_seed")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    unknown = [name for name in args.workloads if name not in WORKLOADS]
    if unknown:
        parser.error(f"unknown workloads: {', '.join(unknown)}")
    for tool in ("oha", "nginx", "openssl"):
        if shutil.which(tool) is None:
            parser.error(f"{tool} is not on PATH")
    for binary in (args.gatewayd, args.seed):
        if not binary.is_file():
            parser.error(
                f"{binary} is missing; build it with: cargo build --manifest-path "
                "panel/Cargo.toml --release --locked -p gatewayd "
                "--bin gatewayd --example bench_seed"
            )

    def interrupted(signum: int, _frame) -> None:
        raise SystemExit(128 + signum)

    for signum in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(signum, interrupted)

    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    wanted = 65536 if hard == resource.RLIM_INFINITY else min(65536, hard)
    resource.setrlimit(resource.RLIMIT_NOFILE, (max(soft, wanted), hard))

    started_at = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    output = args.output or HERE / "results" / started_at.replace(":", "")
    output.mkdir(parents=True, exist_ok=True)
    environment = environment_record(args, started_at)
    work = Path(tempfile.mkdtemp(prefix="gateway-bench-"))
    processes = Processes()
    runs: list[dict] = []
    try:
        (work / "www").mkdir()
        (work / "logs").mkdir()
        for workload in WORKLOADS.values():
            body = bytes(range(256)) * (workload.body_bytes // 256 + 1)
            (work / "www" / str(workload.body_bytes)).write_bytes(body[: workload.body_bytes])
        secrets = work / "secrets"
        secrets.mkdir()
        subprocess.run(
            [
                "openssl",
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-256",
                "-nodes",
                "-days",
                "2",
                "-subj",
                "/CN=bench.test",
                "-keyout",
                str(secrets / "bench.key"),
                "-out",
                str(secrets / "bench.crt"),
            ],
            check=True,
            capture_output=True,
        )

        upstream_port, stats_port = free_port(), free_port()
        upstream_conf = work / "upstream.conf"
        upstream_conf.write_text(
            upstream_config(work, upstream_port, stats_port, args.upstream_workers)
        )
        shutil.copy(upstream_conf, output / "nginx-upstream.conf")
        upstream = processes.start(
            ["nginx", "-p", str(work), "-c", str(upstream_conf), "-g", "daemon off;"],
            work / "upstream.log",
        )
        wait_until_served(f"http://127.0.0.1:{upstream_port}/1024", upstream, work / "upstream.log")

        for name in args.workloads:
            workload = WORKLOADS[name]
            path = f"/{workload.body_bytes}"
            gateway_port, nginx_port = free_port(), free_port()
            state = work / f"state-{name}"
            subprocess.run(
                [
                    str(args.seed),
                    str(state),
                    f"127.0.0.1:{gateway_port}",
                    f"127.0.0.1:{upstream_port}",
                    *(["tls", str(secrets)] if workload.tls else []),
                ],
                check=True,
            )
            gateway = processes.start(
                [str(args.gatewayd)],
                work / f"gateway-{name}.log",
                env={
                    **os.environ,
                    "PINGORA_PANEL_STATE_DIR": str(state),
                    "PINGORA_PANEL_WORKERS": str(args.workers),
                    "PINGORA_PANEL_GATEWAY_ADDR": f"127.0.0.1:{free_port()}",
                    "PINGORA_PANEL_OPS_ADDR": f"127.0.0.1:{free_port()}",
                    "PINGORA_PANEL_SECRET_DIR": str(secrets),
                },
            )
            nginx_conf = work / f"proxy-{name}.conf"
            nginx_conf.write_text(
                proxy_config(work, nginx_port, upstream_port, args.workers, workload)
            )
            shutil.copy(nginx_conf, output / f"nginx-proxy-{name}.conf")
            nginx = processes.start(
                ["nginx", "-p", str(work), "-c", str(nginx_conf), "-g", "daemon off;"],
                work / f"nginx-{name}.log",
            )
            proxies = {
                "gateway": {
                    "pid": gateway.pid,
                    "url": f"{workload.scheme}://127.0.0.1:{gateway_port}{path}",
                    "process": gateway,
                    "log": work / f"gateway-{name}.log",
                },
                "nginx": {
                    "pid": nginx.pid,
                    "url": f"{workload.scheme}://127.0.0.1:{nginx_port}{path}",
                    "process": nginx,
                    "log": work / f"nginx-{name}.log",
                },
            }
            for proxy in proxies.values():
                wait_until_served(proxy["url"], proxy["process"], proxy["log"])

            probes = {}
            for proxy_name, proxy in proxies.items():
                oha(proxy["url"], workload, args, args.warmup)
                probes[proxy_name] = outcome(oha(proxy["url"], workload, args, 5))[
                    "requests_per_second"
                ]
            slower = min(probes.values())
            rate = int(float(f"{slower * args.latency_load:.2g}"))
            environment["probes"][name] = probes
            environment["fixed_rates"][name] = rate
            print(f"{name}: probes {probes}, latency runs at {rate} req/s", flush=True)

            for repetition in range(args.repetitions):
                order = PROXIES if repetition % 2 == 0 else tuple(reversed(PROXIES))
                for position, proxy_name in enumerate(order):
                    proxy = proxies[proxy_name]
                    oha(proxy["url"], workload, args, args.warmup)
                    for mode, fixed in (("closed", None), ("open", rate)):
                        result = measure(proxy, workload, args, stats_port, fixed)
                        runs.append(
                            {
                                "workload": name,
                                "proxy": proxy_name,
                                "repetition": repetition,
                                "position": position,
                                "mode": mode,
                                "rate": fixed,
                                "result": result,
                            }
                        )
                        print(
                            f"{name} #{repetition} {proxy_name} {mode}: "
                            f"{result['requests_per_second']:.0f} req/s, "
                            f"p99 {result['latency_ms']['p99']:.2f} ms, "
                            f"{result['failed']} failed",
                            flush=True,
                        )
            for process in (gateway, nginx):
                process.terminate()
                process.wait(timeout=10)
    finally:
        processes.stop_all()
        environment["finished_at"] = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
        environment["host"]["load_average_after"] = os.getloadavg()
        environment["complete"] = len(runs) == len(args.workloads) * args.repetitions * 4
        (output / "environment.json").write_text(json.dumps(environment, indent=2) + "\n")
        with (output / "runs.jsonl").open("w") as file:
            for run in runs:
                file.write(json.dumps(run) + "\n")
        shutil.rmtree(work, ignore_errors=True)
    (output / "report.md").write_text(report(runs, environment))
    print(f"results in {output}")


if __name__ == "__main__":
    main()
