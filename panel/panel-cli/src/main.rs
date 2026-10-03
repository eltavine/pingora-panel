#![forbid(unsafe_code)]

//! `ppanel`, the command line for Pingora Panel. It speaks only the public
//! HTTP API, so every change takes the same validation, preconditions and
//! idempotency as the console.

mod client;
mod commands;
mod output;

use clap::{CommandFactory, Parser, Subcommand};
use client::{Api, CliError};
use output::{Format, Output};
use std::{process::ExitCode, time::Duration};

#[derive(Parser)]
#[command(
    name = "ppanel",
    version,
    about = "Manage Pingora Panel sites, upstreams and the gateway"
)]
struct Cli {
    /// Base URL of the Pingora Panel API.
    #[arg(
        long,
        global = true,
        env = "PPANEL_API",
        default_value = "http://127.0.0.1:8080"
    )]
    api: String,
    /// Who makes changes, as recorded with each change.
    #[arg(long, global = true, env = "PPANEL_ACTOR")]
    actor: Option<String>,
    /// Seconds to wait for each request.
    #[arg(long, global = true, default_value_t = 30)]
    timeout: u64,
    /// Reuse to retry a change safely; a new key is generated otherwise.
    #[arg(long, global = true)]
    idempotency_key: Option<String>,
    #[arg(long, short, global = true, value_enum, default_value_t = Format::Table)]
    output: Format,
    /// Print nothing; rely on the exit code.
    #[arg(long, short, global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Websites and their state.
    #[command(subcommand)]
    Site(commands::sites::SiteCommand),
    /// Domains bound to sites.
    #[command(subcommand)]
    Domain(commands::domains::DomainCommand),
    /// Routes of a site.
    #[command(subcommand)]
    Route(commands::routes::RouteCommand),
    /// Upstreams, their nodes and live health.
    #[command(subcommand)]
    Upstream(commands::upstreams::UpstreamCommand),
    /// Listening sockets of the gateway.
    #[command(subcommand)]
    Listener(commands::gateway::ListenerCommand),
    /// Certificates served on HTTPS listeners and domains.
    #[command(subcommand, name = "tls-profile")]
    TlsProfile(commands::gateway::TlsProfileCommand),
    /// The draft configuration: its files, checks, plans and applying it.
    #[command(subcommand)]
    Config(commands::config::ConfigCommand),
    /// Configurations applied or attempted, their files and notes.
    #[command(subcommand)]
    Revision(commands::revisions::RevisionCommand),
    /// Every change and every refused or failed attempt.
    #[command(subcommand)]
    Audit(commands::audit::AuditCommand),
    /// The running gateway.
    #[command(subcommand)]
    Gateway(commands::gateway::GatewayCommand),
    /// Prints a shell completion script.
    Completion {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::Completion { shell } = cli.command {
        clap_complete::generate(shell, &mut Cli::command(), "ppanel", &mut std::io::stdout());
        return ExitCode::SUCCESS;
    }
    let actor = cli
        .actor
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "ppanel".into());
    let result = async {
        let api = Api::new(
            &cli.api,
            actor,
            Duration::from_secs(cli.timeout.max(1)),
            cli.idempotency_key,
        )?;
        let output = Output {
            format: cli.output,
            quiet: cli.quiet,
        };
        match cli.command {
            Command::Site(command) => commands::sites::run(&api, &output, command).await,
            Command::Domain(command) => commands::domains::run(&api, &output, command).await,
            Command::Route(command) => commands::routes::run(&api, &output, command).await,
            Command::Upstream(command) => commands::upstreams::run(&api, &output, command).await,
            Command::Listener(command) => commands::gateway::listener(&api, &output, command).await,
            Command::TlsProfile(command) => {
                commands::gateway::tls_profile(&api, &output, command).await
            }
            Command::Config(command) => commands::config::run(&api, &output, command).await,
            Command::Revision(command) => commands::revisions::run(&api, &output, command).await,
            Command::Audit(command) => commands::audit::run(&api, &output, command).await,
            Command::Gateway(command) => commands::gateway::gateway(&api, &output, command).await,
            Command::Completion { .. } => Ok(()),
        }
    }
    .await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if !cli.quiet || !matches!(error, CliError::Api { .. }) {
                eprintln!("ppanel: {error}");
            }
            error.exit()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn resource_action_commands_parse() {
        for arguments in [
            vec![
                "ppanel", "site", "list", "--status", "running", "--tag", "prod",
            ],
            vec![
                "ppanel",
                "site",
                "create",
                "--name",
                "shop",
                "--domain",
                "shop.example.com",
                "--maintenance",
            ],
            vec!["ppanel", "domain", "check", "bücher.example"],
            vec![
                "ppanel",
                "route",
                "add",
                "0190b5b6-3f43-7a52-8a56-2f8b7a7d5a10",
                "--match",
                "prefix:/api",
                "--respond",
                "204",
            ],
            vec![
                "ppanel",
                "upstream",
                "create",
                "--name",
                "app",
                "--node",
                "10.0.0.1:8080,weight=3,backup",
            ],
            vec!["ppanel", "-o", "json", "gateway", "workers", "4"],
            vec!["ppanel", "config", "apply", "--expected-version", "7"],
            vec!["ppanel", "config", "apply", "--dry-run"],
            vec!["ppanel", "config", "apply", "--note", "launch"],
            vec!["ppanel", "config", "export", "--dir", "conf"],
            vec![
                "ppanel",
                "config",
                "import",
                "conf",
                "--expected-version",
                "3",
            ],
            vec!["ppanel", "config", "check", "main.conf"],
            vec!["ppanel", "config", "fmt", "conf", "--check"],
            vec!["ppanel", "config", "plan"],
            vec![
                "ppanel",
                "config",
                "ast",
                "conf",
                "--file",
                "sites/shop.conf",
            ],
            vec!["ppanel", "config", "ir"],
            vec!["ppanel", "config", "import-nginx", "/etc/nginx", "--save"],
            vec![
                "ppanel",
                "config",
                "import-nginx",
                "nginx.conf",
                "--dir",
                "conf",
            ],
            vec![
                "ppanel", "config", "rollback", "--to", "4", "--reason", "errors",
            ],
            vec!["ppanel", "revision", "list", "--before", "9"],
            vec!["ppanel", "revision", "show", "4", "--file", "main.conf"],
            vec!["ppanel", "revision", "diff", "4", "--against", "active"],
            vec!["ppanel", "revision", "note", "4", ""],
            vec![
                "ppanel",
                "audit",
                "list",
                "--type",
                "config.",
                "--correlation-id",
                "req-1",
                "--since",
                "2026-10-01T00:00:00Z",
            ],
            vec!["ppanel", "audit", "show", "12"],
            vec!["ppanel", "audit", "verify", "--from", "1"],
            vec![
                "ppanel",
                "listener",
                "set",
                "https",
                "--address",
                "[::]:443",
                "--tls-profile",
                "main",
                "--no-http1",
            ],
        ] {
            assert!(Cli::try_parse_from(&arguments).is_ok(), "{arguments:?}");
        }
    }
}
