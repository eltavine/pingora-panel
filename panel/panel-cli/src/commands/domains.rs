use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub(crate) enum DomainCommand {
    /// Lists domains with the sites they belong to.
    List {
        #[arg(long)]
        site: Option<String>,
        #[arg(long)]
        q: Option<String>,
    },
    /// Checks host syntax, shows the ASCII and Unicode forms and the owner.
    Check {
        #[arg(required = true)]
        hosts: Vec<String>,
    },
    /// Binds domains to a site; one duplicate refuses all of them.
    Add {
        site: String,
        hosts: Vec<String>,
        /// A file with one host per line; `#` starts a comment.
        #[arg(long)]
        file: Option<String>,
        /// Redirect these hosts to the site's primary domain.
        #[arg(long)]
        redirect: bool,
        #[arg(long)]
        tls_profile: Option<String>,
    },
    /// Changes a domain's state, role or certificate.
    Update {
        site: String,
        host: String,
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
        /// Makes this the site's primary domain.
        #[arg(long)]
        primary: bool,
        #[arg(long, conflicts_with = "serve")]
        redirect: bool,
        /// Serves the site instead of redirecting.
        #[arg(long)]
        serve: bool,
        #[arg(long, conflicts_with = "no_tls_profile")]
        tls_profile: Option<String>,
        #[arg(long)]
        no_tls_profile: bool,
    },
    Remove {
        site: String,
        host: String,
    },
}

const COLUMNS: &[Column] = &[
    ("HOST", |domain| text(&domain["host"])),
    ("UNICODE", |domain| text(&domain["unicode_host"])),
    ("SITE", |domain| text(&domain["site_name"])),
    ("ENABLED", |domain| text(&domain["enabled"])),
    ("PRIMARY", |domain| text(&domain["primary"])),
    ("REDIRECT", |domain| text(&domain["redirect"])),
    ("TLS", |domain| text(&domain["tls_profile_id"])),
];

/// Hosts from arguments and a file, without blank lines or comments.
fn hosts(arguments: Vec<String>, file: Option<String>) -> Result<Vec<String>> {
    let mut hosts = arguments;
    if let Some(file) = file {
        let text = std::fs::read_to_string(&file)
            .map_err(|error| CliError::Usage(format!("cannot read {file}: {error}")))?;
        hosts.extend(
            text.lines()
                .map(|line| line.split('#').next().unwrap_or("").trim())
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        );
    }
    if hosts.is_empty() {
        return Err(CliError::Usage("name at least one host".into()));
    }
    Ok(hosts)
}

pub async fn run(api: &Api, output: &Output, command: DomainCommand) -> Result<()> {
    match command {
        DomainCommand::List { site, q } => {
            let mut query = Vec::new();
            if let Some(site) = site {
                query.push(("site_id", site));
            }
            if let Some(q) = q {
                query.push(("q", q));
            }
            output.list(&api.get("/api/v1/domains", &query).await?.body, COLUMNS);
        }
        DomainCommand::Check { hosts } => {
            let checks = api
                .post_read("/api/v1/domains/check", &json!({ "hosts": hosts }))
                .await?
                .body;
            output.list(
                &checks,
                &[
                    ("INPUT", |check| text(&check["input"])),
                    ("HOST", |check| text(&check["host"])),
                    ("UNICODE", |check| text(&check["unicode_host"])),
                    ("WILDCARD", |check| text(&check["wildcard"])),
                    ("OWNER", |check| text(&check["owner"]["site_name"])),
                    ("ERROR", |check| text(&check["error"])),
                ],
            );
        }
        DomainCommand::Add {
            site,
            hosts: arguments,
            file,
            redirect,
            tls_profile,
        } => {
            let domains: Vec<Value> = hosts(arguments, file)?
                .into_iter()
                .map(|host| json!({ "host": host, "redirect": redirect, "tls_profile_id": tls_profile }))
                .collect();
            let count = domains.len();
            let site = api
                .change(
                    Method::POST,
                    &format!("/api/v1/sites/{site}/domains"),
                    Some(&Value::Array(domains)),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!("Bound {count} domains to {}", text(&site["name"])),
                &site,
            );
        }
        DomainCommand::Update {
            site,
            host,
            enable,
            disable,
            primary,
            redirect,
            serve,
            tls_profile,
            no_tls_profile,
        } => {
            let current = api.get(&format!("/api/v1/sites/{site}"), &[]).await?;
            let checked = api
                .post_read("/api/v1/domains/check", &json!({ "hosts": [host] }))
                .await?
                .body;
            let ascii = checked[0]["host"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or(host);
            let mut domain = current.body["domains"]
                .as_array()
                .and_then(|domains| {
                    domains
                        .iter()
                        .find(|domain| domain["host"] == ascii.as_str())
                })
                .cloned()
                .ok_or_else(|| CliError::Usage(format!("{ascii} is not bound to this site")))?;
            if enable || disable {
                domain["enabled"] = json!(enable);
            }
            if primary {
                domain["primary"] = json!(true);
            }
            if redirect || serve {
                domain["redirect"] = json!(redirect);
            }
            if let Some(profile) = tls_profile {
                domain["tls_profile_id"] = json!(profile);
            } else if no_tls_profile {
                domain["tls_profile_id"] = Value::Null;
            }
            let site = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/sites/{site}/domains/{ascii}"),
                    Some(&domain),
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(
                &format!("Updated {ascii} on {}", text(&site["name"])),
                &site,
            );
        }
        DomainCommand::Remove { site, host } => {
            let current = api.get(&format!("/api/v1/sites/{site}"), &[]).await?;
            let site = api
                .change(
                    Method::DELETE,
                    &format!("/api/v1/sites/{site}/domains/{host}"),
                    None,
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(
                &format!("Unbound {host} from {}", text(&site["name"])),
                &site,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_files_ignore_blank_lines_and_comments() {
        let file = std::env::temp_dir().join(format!("ppanel-hosts-{}", std::process::id()));
        std::fs::write(&file, "a.example\n\n# comment\nb.example # trailing\n").unwrap();
        let hosts = hosts(vec!["c.example".into()], Some(file.display().to_string())).unwrap();
        assert_eq!(hosts, ["c.example", "a.example", "b.example"]);
        std::fs::remove_file(file).unwrap();
        assert!(super::hosts(Vec::new(), None).is_err());
    }
}
