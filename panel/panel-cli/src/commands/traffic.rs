//! What the gateway served, from its metrics.

use crate::{
    client::{Api, Result},
    output::{text, Column, Format, Output},
};
use clap::{Args, Subcommand};
use serde_json::Value;
use std::time::Duration;

#[derive(Args)]
pub(crate) struct Scope {
    /// Only the requests of this site.
    #[arg(long)]
    site: Option<String>,
    /// Only the requests of this route; needs `--site`.
    #[arg(long, requires = "site")]
    route: Option<String>,
    /// How far back to look, such as `15m`, `1h` or `7d`.
    #[arg(long, default_value = "1h", value_parser = humantime::parse_duration)]
    window: Duration,
}

impl Scope {
    fn query(self) -> Vec<(&'static str, String)> {
        let mut query = vec![("window", self.window.as_secs().to_string())];
        if let Some(site) = self.site {
            query.push(("site", site));
        }
        if let Some(route) = self.route {
            query.push(("route", route));
        }
        query
    }
}

#[derive(Subcommand)]
pub(crate) enum TrafficCommand {
    /// Requests, status classes, latency, traffic and connections, with the
    /// busiest upstreams and routes.
    Summary {
        #[command(flatten)]
        scope: Scope,
    },
    /// The request rate, server errors and latency over the window.
    Series {
        #[command(flatten)]
        scope: Scope,
        /// The time between points, such as `1m`.
        #[arg(long, value_parser = humantime::parse_duration)]
        step: Option<Duration>,
    },
}

fn count(value: &Value) -> String {
    value
        .as_f64()
        .map_or_else(|| text(value), |count| format!("{count:.0}"))
}

fn rate(value: &Value) -> String {
    value
        .as_f64()
        .map_or_else(|| text(value), |rate| format!("{rate:.2}"))
}

fn seconds(value: &Value) -> String {
    match value.as_f64() {
        Some(seconds) if seconds < 1.0 => format!("{:.0} ms", seconds * 1000.0),
        Some(seconds) => format!("{seconds:.2} s"),
        None => "-".into(),
    }
}

fn bytes(value: &Value) -> String {
    let Some(mut size) = value.as_f64() else {
        return text(value);
    };
    for unit in ["B", "KiB", "MiB", "GiB"] {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} TiB")
}

fn percent(value: &Value) -> String {
    value
        .as_f64()
        .map_or_else(|| text(value), |ratio| format!("{:.1}%", ratio * 100.0))
}

const SUMMARY: &[Column] = &[
    ("Window", |summary| {
        humantime::format_duration(Duration::from_secs(
            summary["window_seconds"].as_u64().unwrap_or_default(),
        ))
        .to_string()
    }),
    ("Requests", |summary| count(&summary["requests"])),
    ("Requests per second", |summary| {
        rate(&summary["requests_per_second"])
    }),
    ("2xx / 3xx / 4xx / 5xx", |summary| {
        let statuses = &summary["statuses"];
        format!(
            "{} / {} / {} / {}",
            count(&statuses["success"]),
            count(&statuses["redirection"]),
            count(&statuses["client_error"]),
            count(&statuses["server_error"])
        )
    }),
    ("Latency p50 / p90 / p95 / p99", |summary| {
        let latency = &summary["latency"];
        format!(
            "{} / {} / {} / {}",
            seconds(&latency["p50"]),
            seconds(&latency["p90"]),
            seconds(&latency["p95"]),
            seconds(&latency["p99"])
        )
    }),
    ("Received / sent", |summary| {
        format!(
            "{} / {}",
            bytes(&summary["bytes_received"]),
            bytes(&summary["bytes_sent"])
        )
    }),
    ("Open connections", |summary| {
        count(&summary["open_connections"])
    }),
    ("TLS handshakes", |summary| {
        count(&summary["tls_handshakes"])
    }),
    ("Revision", |summary| {
        format!(
            "{} (activated {})",
            text(&summary["revision"]),
            text(&summary["activated_at"])
        )
    }),
];

const UPSTREAMS: &[Column] = &[
    ("UPSTREAM", |upstream| text(&upstream["upstream"])),
    ("REQUESTS", |upstream| count(&upstream["requests"])),
    ("ERRORS", |upstream| percent(&upstream["error_ratio"])),
    ("P95", |upstream| seconds(&upstream["latency"]["p95"])),
];

const ROUTES: &[Column] = &[
    ("SITE", |route| text(&route["site"])),
    ("ROUTE", |route| text(&route["route"])),
    ("REQUESTS", |route| count(&route["requests"])),
];

const POINTS: &[Column] = &[
    ("TIME", |point| text(&point["at"])),
    ("REQ/S", |point| rate(&point["requests_per_second"])),
    ("5XX/S", |point| rate(&point["server_errors_per_second"])),
    ("P95", |point| seconds(&point["p95"])),
];

pub async fn run(api: &Api, output: &Output, command: TrafficCommand) -> Result<()> {
    match command {
        TrafficCommand::Summary { scope } => {
            let summary = api.get("/api/v1/traffic", &scope.query()).await?.body;
            if output.format == Format::Json {
                output.json(&summary);
            } else if !output.quiet {
                output.item(&summary, SUMMARY);
                if summary["upstreams"]
                    .as_array()
                    .is_some_and(|items| !items.is_empty())
                {
                    println!();
                    output.list(&summary["upstreams"], UPSTREAMS);
                }
                if summary["routes"]
                    .as_array()
                    .is_some_and(|items| !items.is_empty())
                {
                    println!();
                    output.list(&summary["routes"], ROUTES);
                }
            }
        }
        TrafficCommand::Series { scope, step } => {
            let mut query = scope.query();
            if let Some(step) = step {
                query.push(("step", step.as_secs().to_string()));
            }
            let series = api.get("/api/v1/traffic/series", &query).await?.body;
            if output.format == Format::Json {
                output.json(&series);
            } else {
                output.list(&series["points"], POINTS);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn figures_read_as_people_write_them() {
        assert_eq!(seconds(&json!(0.25)), "250 ms");
        assert_eq!(seconds(&json!(1.5)), "1.50 s");
        assert_eq!(seconds(&Value::Null), "-");
        assert_eq!(bytes(&json!(2048)), "2.0 KiB");
        assert_eq!(percent(&json!(0.125)), "12.5%");
        assert_eq!(count(&json!(119.6)), "120");
    }
}
