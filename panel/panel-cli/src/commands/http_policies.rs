use crate::{
    client::{Api, Result},
    commands::{gateway::existing, read_json, security::size},
    output::{text, Column, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub(crate) enum HttpPolicyCommand {
    /// Lists HTTP policies with the number of sites using each.
    List,
    /// Shows an HTTP policy.
    Show { id: String },
    /// Creates or replaces a policy from flags, or from a JSON document with
    /// --file. Sites and routes name it with --http-policy.
    Set(Box<PolicyArgs>),
    /// Removes a policy no site or route uses.
    Delete { id: String },
}

#[derive(clap::Args)]
pub(crate) struct PolicyArgs {
    id: String,
    /// A JSON document in the shape `show -o json` prints.
    #[arg(long, conflicts_with_all = ["request_remove", "request_set", "request_add", "response_remove", "response_set", "response_add", "server", "remove_server", "cors_origins", "compress"])]
    file: Option<String>,
    /// Request field removed before requests go upstream; repeatable.
    #[arg(long = "request-remove", value_name = "NAME")]
    request_remove: Vec<String>,
    /// Request field replaced, as "Name: value"; repeatable. Values are
    /// templates such as $host.
    #[arg(long = "request-set", value_name = "FIELD", value_parser = field)]
    request_set: Vec<Value>,
    /// Request field line appended, as "Name: value"; repeatable.
    #[arg(long = "request-add", value_name = "FIELD", value_parser = field)]
    request_add: Vec<Value>,
    /// Response field removed before responses go to clients; repeatable.
    #[arg(long = "response-remove", value_name = "NAME")]
    response_remove: Vec<String>,
    /// Response field replaced, as "Name: value"; repeatable.
    #[arg(long = "response-set", value_name = "FIELD", value_parser = field)]
    response_set: Vec<Value>,
    /// Response field line appended, as "Name: value"; repeatable.
    #[arg(long = "response-add", value_name = "FIELD", value_parser = field)]
    response_add: Vec<Value>,
    /// Replaces the Server field of responses with this value.
    #[arg(long, value_name = "VALUE", conflicts_with = "remove_server")]
    server: Option<String>,
    /// Removes the Server field of responses.
    #[arg(long)]
    remove_server: bool,
    /// Origin allowed cross-origin requests, such as `https://shop.example`,
    /// `https://*.shop.example` or `*`; repeatable. The gateway answers their
    /// preflights.
    #[arg(long = "cors-origin", value_name = "ORIGIN")]
    cors_origins: Vec<String>,
    /// Method a preflight allows besides GET, HEAD and POST; repeatable.
    #[arg(long = "cors-method", value_name = "METHOD", requires = "cors_origins")]
    cors_methods: Vec<String>,
    /// Request field a preflight allows; repeatable.
    #[arg(long = "cors-header", value_name = "NAME", requires = "cors_origins")]
    cors_headers: Vec<String>,
    /// Response field scripts may read; repeatable.
    #[arg(long = "cors-expose", value_name = "NAME", requires = "cors_origins")]
    cors_expose: Vec<String>,
    /// Allows requests with cookies or authorization; the origin is echoed.
    #[arg(long, requires = "cors_origins")]
    cors_credentials: bool,
    /// How long browsers may cache a preflight's answer, at most a day.
    #[arg(long, value_name = "SECONDS", requires = "cors_origins")]
    cors_max_age: Option<u32>,
    /// Content coding responses are compressed with: gzip, br or zstd;
    /// repeatable.
    #[arg(long, value_name = "CODING", value_parser = ["gzip", "br", "zstd"])]
    compress: Vec<String>,
    /// Media type compressed, such as text/html or text/*; repeatable.
    #[arg(long = "compress-type", value_name = "TYPE", requires = "compress")]
    compress_types: Vec<String>,
    /// Responses smaller than this, such as 1k, are sent as they are.
    #[arg(long, value_name = "SIZE", value_parser = size, requires = "compress")]
    compress_min_size: Option<u64>,
}

const POLICIES: &[Column] = &[
    ("ID", |policy| text(&policy["id"])),
    ("DOES", effects),
    ("USED BY", |policy| {
        policy["used_by"]
            .as_array()
            .map_or_else(String::new, |sites| sites.len().to_string())
    }),
];

/// What a policy does, in a few words.
fn effects(policy: &Value) -> String {
    let changes = |side: &str| {
        ["remove", "set", "add"].iter().any(|operation| {
            policy[side][operation]
                .as_array()
                .is_some_and(|fields| !fields.is_empty())
        })
    };
    let mut names = Vec::new();
    if changes("request") {
        names.push("request fields");
    }
    if changes("response") {
        names.push("response fields");
    }
    if !policy["server"].is_null() {
        names.push("server");
    }
    if !policy["cors"].is_null() {
        names.push("CORS");
    }
    if !policy["compression"].is_null() {
        names.push("compression");
    }
    names.join(", ")
}

/// A field line, `Name: value`.
fn field(value: &str) -> std::result::Result<Value, String> {
    let (name, field_value) = value
        .split_once(':')
        .ok_or_else(|| format!("{value:?} is not a field such as \"X-Frame-Options: DENY\""))?;
    Ok(json!({"name": name.trim(), "value": field_value.trim()}))
}

pub async fn run(api: &Api, output: &Output, command: HttpPolicyCommand) -> Result<()> {
    match command {
        HttpPolicyCommand::List => {
            output.list(&api.get("/api/v1/http-policies", &[]).await?.body, POLICIES)
        }
        HttpPolicyCommand::Show { id } => output.item(
            &api.get(&format!("/api/v1/http-policies/{id}"), &[])
                .await?
                .body,
            POLICIES,
        ),
        HttpPolicyCommand::Set(policy) => {
            let PolicyArgs {
                id,
                file,
                request_remove,
                request_set,
                request_add,
                response_remove,
                response_set,
                response_add,
                server,
                remove_server,
                cors_origins,
                cors_methods,
                cors_headers,
                cors_expose,
                cors_credentials,
                cors_max_age,
                compress,
                compress_types,
                compress_min_size,
            } = *policy;
            let path = format!("/api/v1/http-policies/{id}");
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
                None => {
                    let server = match (server, remove_server) {
                        (Some(value), _) => json!({"mode": "replace", "value": value}),
                        (None, true) => json!({"mode": "remove"}),
                        (None, false) => json!({"mode": "keep"}),
                    };
                    json!({
                        "id": id,
                        "request": {"remove": request_remove, "set": request_set, "add": request_add},
                        "response": {"remove": response_remove, "set": response_set, "add": response_add},
                        "server": server,
                        "cors": (!cors_origins.is_empty()).then(|| json!({
                            "allowed_origins": cors_origins,
                            "allowed_methods": cors_methods,
                            "allowed_headers": cors_headers,
                            "exposed_headers": cors_expose,
                            "allow_credentials": cors_credentials,
                            "max_age_seconds": cors_max_age,
                        })),
                        "compression": (!compress.is_empty()).then(|| json!({
                            "algorithms": compress
                                .iter()
                                .map(|coding| if coding == "br" { "brotli" } else { coding.as_str() })
                                .collect::<Vec<_>>(),
                            "types": compress_types,
                            "min_bytes": compress_min_size.unwrap_or_default(),
                        })),
                    })
                }
            };
            let policy = api
                .change(Method::PUT, &path, Some(&body), etag.as_deref())
                .await?
                .body;
            output.done(&format!("Saved HTTP policy {id}"), &policy);
        }
        HttpPolicyCommand::Delete { id } => {
            let path = format!("/api/v1/http-policies/{id}");
            let etag = existing(api, &path).await?;
            let reply = api
                .change(Method::DELETE, &path, None, etag.as_deref())
                .await?
                .body;
            output.done(&format!("Deleted HTTP policy {id}"), &reply);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_written_as_field_lines() {
        assert_eq!(
            field("X-Frame-Options: DENY"),
            Ok(json!({"name": "X-Frame-Options", "value": "DENY"}))
        );
        assert_eq!(
            field("Link: </app.css>; rel=preload").unwrap()["value"],
            "</app.css>; rel=preload"
        );
        assert!(field("X-Frame-Options").is_err());
        assert_eq!(
            effects(&json!({"response": {"set": [{"name": "a", "value": "b"}]}, "cors": {}})),
            "response fields, CORS"
        );
    }
}
