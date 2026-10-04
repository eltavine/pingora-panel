use crate::{
    client::{Api, Result},
    commands::{gateway::existing, read_json},
    output::{text, Column, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub(crate) enum SecurityPolicyCommand {
    /// Lists security policies with the number of sites using each.
    List,
    /// Shows a security policy.
    Show { id: String },
    /// Creates or replaces a policy from flags, or from a JSON document with
    /// --file. Sites and routes name it with --security-policy.
    Set(Box<PolicyArgs>),
    /// Removes a policy no site or route uses.
    Delete { id: String },
}

#[derive(clap::Args)]
pub(crate) struct PolicyArgs {
    id: String,
    /// A JSON document in the shape `show -o json` prints.
    #[arg(long, conflicts_with_all = ["allow", "deny", "methods", "deny_paths", "deny_user_agents", "referers", "basic_auth", "max_header_size", "max_body_size", "body_timeout", "rate_limits", "max_concurrent", "limited_status"])]
    file: Option<String>,
    /// Client network allowed, such as 10.0.0.0/8; repeatable. Once any
    /// is given, other clients get 403.
    #[arg(long = "allow", value_name = "CIDR")]
    allow: Vec<String>,
    /// Client network refused with 403 even when allowed; repeatable.
    #[arg(long = "deny", value_name = "CIDR")]
    deny: Vec<String>,
    /// Method allowed; repeatable. Others get 405, and HEAD goes with GET.
    #[arg(long = "method")]
    methods: Vec<String>,
    /// Path prefix refused with 403; repeatable.
    #[arg(long = "deny-path", value_name = "PREFIX")]
    deny_paths: Vec<String>,
    /// Case-insensitive regular expression of user agents refused with
    /// 403; repeatable.
    #[arg(long = "deny-user-agent", value_name = "REGEX")]
    deny_user_agents: Vec<String>,
    /// Host allowed to link here, such as example.com or *.example.com;
    /// repeatable. Other referring pages get 403.
    #[arg(long = "referer", value_name = "HOST")]
    referers: Vec<String>,
    /// Allows requests without a Referer as well.
    #[arg(long)]
    allow_empty_referer: bool,
    /// An htpasswd file in the gateway's secret directory, with bcrypt or
    /// Argon2 hashes.
    #[arg(long, value_name = "FILE")]
    basic_auth: Option<String>,
    /// The realm Basic authentication asks for.
    #[arg(long, requires = "basic_auth", default_value = "Restricted")]
    realm: String,
    /// The most bytes of request headers, such as 16k; larger requests
    /// get 431.
    #[arg(long, value_name = "SIZE", value_parser = size)]
    max_header_size: Option<u64>,
    /// The largest request body, such as 10m; larger ones get 413.
    #[arg(long, value_name = "SIZE", value_parser = size)]
    max_body_size: Option<u64>,
    /// The longest wait for the next part of a request body.
    #[arg(long, value_name = "SECONDS")]
    body_timeout: Option<u64>,
    /// A token bucket such as "10r/s", "300r/m burst=50" or
    /// "5r/m key=header:X-Api-Key"; repeatable. Keys are client (the
    /// default), host, route and `header:<name>`. More requests get 429.
    #[arg(long = "rate-limit", value_name = "RATE", value_parser = rate_limit)]
    rate_limits: Vec<Value>,
    /// Requests one client address may have in progress.
    #[arg(long, value_name = "N")]
    max_concurrent: Option<u64>,
    /// The status answering requests over a limit instead of 429.
    #[arg(long, value_name = "STATUS")]
    limited_status: Option<u16>,
    #[arg(long, requires = "limited_status")]
    limited_body: Option<String>,
    #[arg(long, requires = "limited_status")]
    limited_type: Option<String>,
}

const POLICIES: &[Column] = &[
    ("ID", |policy| text(&policy["id"])),
    ("RESTRICTS", restrictions),
    ("USED BY", |policy| {
        policy["used_by"]
            .as_array()
            .map_or_else(String::new, |sites| sites.len().to_string())
    }),
];

/// What a policy checks, in a few words.
fn restrictions(policy: &Value) -> String {
    let present = |field: &str| match &policy[field] {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        _ => true,
    };
    let checks = [
        ("allowed_cidrs", "networks"),
        ("denied_cidrs", "networks"),
        ("allowed_methods", "methods"),
        ("denied_path_prefixes", "paths"),
        ("denied_user_agents", "user agents"),
        ("referer", "referers"),
        ("basic_auth", "password"),
        ("max_header_bytes", "sizes"),
        ("max_body_bytes", "sizes"),
        ("body_timeout_seconds", "body timeout"),
        ("rate_limits", "rates"),
        ("max_concurrent_requests", "concurrency"),
    ];
    let mut names: Vec<&str> = Vec::new();
    for (field, name) in checks {
        if present(field) && !names.contains(&name) {
            names.push(name);
        }
    }
    names.join(", ")
}

/// A size in bytes, with an optional k, m or g suffix.
fn size(value: &str) -> std::result::Result<u64, String> {
    let lower = value.to_ascii_lowercase();
    let (digits, factor) = match lower.char_indices().last() {
        Some((index, 'k')) => (&lower[..index], 1 << 10),
        Some((index, 'm')) => (&lower[..index], 1 << 20),
        Some((index, 'g')) => (&lower[..index], 1 << 30),
        _ => (lower.as_str(), 1),
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(factor))
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| format!("{value:?} is not a size such as 8k, 10m or 1g"))
}

/// `<requests>r/<s|m|h|d|Ns>` with optional `burst=N` and `key=...`.
fn rate_limit(value: &str) -> std::result::Result<Value, String> {
    let invalid = || format!("{value:?} is not a rate limit such as \"10r/s burst=20 key=host\"");
    let mut words = value.split_whitespace();
    let (requests, period) = words
        .next()
        .and_then(|rate| rate.split_once("r/"))
        .ok_or_else(invalid)?;
    let requests: u64 = requests.parse().map_err(|_| invalid())?;
    let per_seconds: u64 = match period {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        seconds => seconds
            .strip_suffix('s')
            .and_then(|seconds| seconds.parse().ok())
            .ok_or_else(invalid)?,
    };
    let mut limit = json!({
        "key": {"kind": "client_address"},
        "requests": requests,
        "per_seconds": per_seconds,
    });
    for word in words {
        match word.split_once('=') {
            Some(("burst", burst)) => {
                limit["burst"] = json!(burst.parse::<u64>().map_err(|_| invalid())?);
            }
            Some(("key", "client")) => limit["key"] = json!({"kind": "client_address"}),
            Some(("key", "host")) => limit["key"] = json!({"kind": "host"}),
            Some(("key", "route")) => limit["key"] = json!({"kind": "route"}),
            Some(("key", key)) => match key.strip_prefix("header:") {
                Some(name) if !name.is_empty() => {
                    limit["key"] = json!({"kind": "header", "name": name});
                }
                _ => return Err(invalid()),
            },
            _ => return Err(invalid()),
        }
    }
    Ok(limit)
}

pub async fn run(api: &Api, output: &Output, command: SecurityPolicyCommand) -> Result<()> {
    match command {
        SecurityPolicyCommand::List => output.list(
            &api.get("/api/v1/security-policies", &[]).await?.body,
            POLICIES,
        ),
        SecurityPolicyCommand::Show { id } => output.item(
            &api.get(&format!("/api/v1/security-policies/{id}"), &[])
                .await?
                .body,
            POLICIES,
        ),
        SecurityPolicyCommand::Set(policy) => {
            let PolicyArgs {
                id,
                file,
                allow,
                deny,
                methods,
                deny_paths,
                deny_user_agents,
                referers,
                allow_empty_referer,
                basic_auth,
                realm,
                max_header_size,
                max_body_size,
                body_timeout,
                rate_limits,
                max_concurrent,
                limited_status,
                limited_body,
                limited_type,
            } = *policy;
            let path = format!("/api/v1/security-policies/{id}");
            let etag = existing(api, &path).await?;
            let body = match file {
                Some(file) => {
                    let mut body = read_json(&file)?;
                    if let Some(object) = body.as_object_mut() {
                        object.remove("used_by");
                        object.remove("etag");
                        object.insert("id".into(), json!(id));
                    }
                    body
                }
                None => json!({
                    "id": id,
                    "allowed_cidrs": allow,
                    "denied_cidrs": deny,
                    "allowed_methods": methods,
                    "denied_path_prefixes": deny_paths,
                    "denied_user_agents": deny_user_agents,
                    "referer": (!referers.is_empty() || allow_empty_referer).then(|| json!({
                        "allowed_hosts": referers,
                        "allow_empty": allow_empty_referer,
                    })),
                    "basic_auth": basic_auth.map(|users| json!({
                        "realm": realm,
                        "users_secret_id": users,
                    })),
                    "max_header_bytes": max_header_size,
                    "max_body_bytes": max_body_size,
                    "body_timeout_seconds": body_timeout,
                    "rate_limits": rate_limits,
                    "max_concurrent_requests": max_concurrent,
                    "limited_response": limited_status.map(|status| json!({
                        "status": status,
                        "body": limited_body.unwrap_or_default(),
                        "content_type": limited_type,
                    })),
                }),
            };
            let policy = api
                .change(Method::PUT, &path, Some(&body), etag.as_deref())
                .await?
                .body;
            output.done(&format!("Saved security policy {id}"), &policy);
        }
        SecurityPolicyCommand::Delete { id } => {
            let path = format!("/api/v1/security-policies/{id}");
            let etag = existing(api, &path).await?;
            let reply = api
                .change(Method::DELETE, &path, None, etag.as_deref())
                .await?
                .body;
            output.done(&format!("Deleted security policy {id}"), &reply);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_rates_read_like_the_configuration_language() {
        assert_eq!(size("16k"), Ok(16 << 10));
        assert_eq!(size("10M"), Ok(10 << 20));
        assert_eq!(size("512"), Ok(512));
        assert!(size("0").is_err() && size("k").is_err() && size("1t").is_err());
        assert_eq!(
            rate_limit("300r/m burst=50 key=header:X-Api-Key").unwrap(),
            json!({"key": {"kind": "header", "name": "X-Api-Key"}, "requests": 300, "per_seconds": 60, "burst": 50})
        );
        assert_eq!(rate_limit("5r/10s").unwrap()["per_seconds"], 10);
        assert_eq!(
            rate_limit("1r/s key=route").unwrap()["key"]["kind"],
            "route"
        );
        for invalid in [
            "10/s",
            "10r/x",
            "10r/s key=uri",
            "10r/s burst=-1",
            "10r/s extra",
        ] {
            assert!(rate_limit(invalid).is_err(), "{invalid}");
        }
        assert_eq!(
            restrictions(
                &json!({"allowed_cidrs": ["10.0.0.0/8"], "denied_cidrs": [], "basic_auth": {"realm": "x"}, "rate_limits": [{}]})
            ),
            "networks, password, rates"
        );
    }
}
