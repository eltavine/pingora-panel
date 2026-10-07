#![forbid(unsafe_code)]

//! `ppanel`, the command line for Pingora Panel. It speaks only the public
//! HTTP API, so every change takes the same validation, preconditions and
//! idempotency as the console.

mod client;
mod commands;
mod credentials;
mod output;

use clap::{CommandFactory, Parser, Subcommand};
use client::{Api, CliError};
use commands::identity::SecretInput;
use credentials::Credentials;
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
    /// An API token, used instead of the session `ppanel login` keeps.
    #[arg(long, global = true, env = "PPANEL_TOKEN", hide_env_values = true)]
    token: Option<String>,
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
    /// Creates the first account with the deployment's bootstrap token.
    Setup {
        #[arg(long)]
        username: String,
        /// The file holding the bootstrap token.
        #[arg(long)]
        bootstrap_token_file: String,
        #[command(flatten)]
        input: SecretInput,
    },
    /// Logs in and keeps the session for later commands.
    Login {
        #[arg(long)]
        username: String,
        #[command(flatten)]
        input: SecretInput,
    },
    /// Ends the kept session.
    Logout {
        /// End every other session of your account too.
        #[arg(long)]
        everywhere: bool,
    },
    /// The account in use, its roles and permissions.
    Whoami,
    /// Changes your password; your other sessions end.
    Password {
        #[command(flatten)]
        input: SecretInput,
    },
    /// Your API tokens, for scripts.
    #[command(subcommand)]
    Token(commands::identity::TokenCommand),
    /// Your sessions in the console and on the command line.
    #[command(subcommand)]
    Session(commands::identity::SessionCommand),
    /// Accounts, their roles, sessions and tokens.
    #[command(subcommand)]
    Account(commands::identity::AccountCommand),
    /// Roles and the permissions they grant.
    #[command(subcommand)]
    Role(commands::identity::RoleCommand),
    /// Identity providers people sign in with.
    #[command(subcommand, name = "identity-provider")]
    IdentityProvider(commands::providers::IdentityProviderCommand),
    /// Who may sign in with a password.
    #[command(subcommand, name = "sign-in-policy")]
    SignInPolicy(commands::providers::SignInPolicyCommand),
    /// Programs that act as service accounts with their own short-lived
    /// tokens.
    #[command(subcommand, name = "workload-identity")]
    WorkloadIdentity(commands::workload::WorkloadCommand),
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
    /// Restrictions requests pass before sites and routes act on them.
    #[command(subcommand, name = "security-policy")]
    SecurityPolicy(commands::security::SecurityPolicyCommand),
    /// Field changes, the Server field, CORS and compression for sites and
    /// routes.
    #[command(subcommand, name = "http-policy")]
    HttpPolicy(commands::http_policies::HttpPolicyCommand),
    /// What sites' and routes' proxied responses are cached by, and for how
    /// long.
    #[command(subcommand, name = "cache-policy")]
    CachePolicy(commands::cache::CachePolicyCommand),
    /// The gateway's cache: what it holds, how it does, purges and its
    /// size.
    #[command(subcommand)]
    Cache(commands::cache::CacheCommand),
    /// Certificates the panel keeps and delivers to the gateway.
    #[command(subcommand)]
    Certificate(commands::certificates::CertificateCommand),
    /// ACME accounts and the certificates they obtain and keep renewed.
    #[command(subcommand)]
    Acme(commands::acme::AcmeCommand),
    /// The draft configuration: its files, checks, plans and applying it.
    #[command(subcommand)]
    Config(commands::config::ConfigCommand),
    /// Requests to apply changes that approval policies cover.
    #[command(subcommand)]
    Approval(commands::approvals::ApprovalCommand),
    /// Policies that ask other people to approve covered changes.
    #[command(subcommand, name = "approval-policy")]
    ApprovalPolicy(commands::approvals::ApprovalPolicyCommand),
    /// Configurations applied or attempted, their files and notes.
    #[command(subcommand)]
    Revision(commands::revisions::RevisionCommand),
    /// Lua scripts: checked, listed with where they run, and tested on a
    /// request.
    #[command(subcommand)]
    Lua(commands::lua::LuaCommand),
    /// Every change and every refused or failed attempt.
    #[command(subcommand)]
    Audit(commands::audit::AuditCommand),
    /// What the gateway served, from its metrics.
    #[command(subcommand)]
    Traffic(commands::traffic::TrafficCommand),
    /// The gateway's access and error logs: searched, followed, downloaded
    /// and deleted.
    #[command(subcommand)]
    Logs(commands::logs::LogsCommand),
    /// Rules that fire on the gateway's metrics, the webhooks they notify
    /// and the notifications sent.
    #[command(subcommand)]
    Alert(commands::alerts::AlertCommand),
    /// The host the gateway runs on: CPU, memory, filesystems, load, network
    /// and system, and what the host agent does there.
    Host(commands::host::HostArgs),
    /// The Docker and Podman engines the host agent reaches and their
    /// containers.
    #[command(subcommand)]
    Container(commands::containers::ContainerCommand),
    /// The images on the Docker and Podman engines the host agent reaches.
    #[command(subcommand)]
    Image(commands::images::ImageCommand),
    /// The networks of the Docker and Podman engines the host agent reaches.
    #[command(subcommand)]
    Network(commands::engine_resources::NetworkCommand),
    /// The volumes of the Docker and Podman engines the host agent reaches.
    #[command(subcommand)]
    Volume(commands::engine_resources::VolumeCommand),
    /// The Compose projects on the Docker and Podman engines the host agent
    /// reaches.
    #[command(subcommand)]
    Compose(commands::compose::ComposeCommand),
    /// The files below the static sites' directory.
    #[command(subcommand)]
    Files(commands::files::FilesCommand),
    /// Backups of the databases and the sites' directory.
    #[command(subcommand)]
    Backup(commands::backups::BackupCommand),
    /// External plugins: their versions, grants, settings, limits and
    /// health, the publisher keys trusted and the secrets kept for them.
    #[command(subcommand)]
    Plugin(commands::plugins::PluginCommand),
    /// The running gateway.
    #[command(subcommand)]
    Gateway(commands::gateway::GatewayCommand),
    /// The control-plane modules running now: their versions, protocol
    /// revisions and capabilities.
    Services,
    /// Prints a shell completion script.
    Completion {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    if let Command::Completion { shell } = cli.command {
        clap_complete::generate(shell, &mut Cli::command(), "ppanel", &mut std::io::stdout());
        return ExitCode::SUCCESS;
    }
    let credentials = Credentials::locate();
    let signing_in = matches!(cli.command, Command::Login { .. } | Command::Setup { .. });
    let credential = cli.token.or_else(|| {
        credentials
            .as_ref()
            .filter(|_| !signing_in)
            .and_then(|credentials| credentials.load(cli.api.trim_end_matches('/')))
            .map(|stored| stored.secret)
    });
    let result = async {
        let api = Api::new(
            &cli.api,
            credential,
            Duration::from_secs(cli.timeout.max(1)),
            cli.idempotency_key,
        )?;
        let output = Output {
            format: cli.output,
            quiet: cli.quiet,
        };
        match cli.command {
            Command::Setup {
                username,
                bootstrap_token_file,
                input,
            } => {
                commands::identity::setup(&api, &output, username, bootstrap_token_file, &input)
                    .await
            }
            Command::Login { username, input } => {
                commands::identity::login(&api, &output, credentials.as_ref(), username, &input)
                    .await
            }
            Command::Logout { everywhere } => {
                commands::identity::logout(&api, &output, credentials.as_ref(), everywhere).await
            }
            Command::Whoami => commands::identity::whoami(&api, &output).await,
            Command::Password { input } => {
                commands::identity::password(&api, &output, &input).await
            }
            Command::Token(command) => commands::identity::token(&api, &output, command).await,
            Command::Session(command) => commands::identity::session(&api, &output, command).await,
            Command::Account(command) => commands::identity::account(&api, &output, command).await,
            Command::Role(command) => commands::identity::role(&api, &output, command).await,
            Command::IdentityProvider(command) => {
                commands::providers::identity_provider(&api, &output, command).await
            }
            Command::SignInPolicy(command) => {
                commands::providers::sign_in_policy(&api, &output, command).await
            }
            Command::WorkloadIdentity(command) => {
                commands::workload::workload(&api, &output, command).await
            }
            Command::Site(command) => commands::sites::run(&api, &output, command).await,
            Command::Domain(command) => commands::domains::run(&api, &output, command).await,
            Command::Route(command) => commands::routes::run(&api, &output, command).await,
            Command::Upstream(command) => commands::upstreams::run(&api, &output, command).await,
            Command::Listener(command) => commands::gateway::listener(&api, &output, command).await,
            Command::Certificate(command) => {
                commands::certificates::run(&api, &output, command).await
            }
            Command::Acme(command) => commands::acme::run(&api, &output, command).await,
            Command::TlsProfile(command) => {
                commands::gateway::tls_profile(&api, &output, command).await
            }
            Command::SecurityPolicy(command) => {
                commands::security::run(&api, &output, command).await
            }
            Command::HttpPolicy(command) => {
                commands::http_policies::run(&api, &output, command).await
            }
            Command::CachePolicy(command) => {
                commands::cache::policies(&api, &output, command).await
            }
            Command::Cache(command) => commands::cache::run(&api, &output, command).await,
            Command::Config(command) => commands::config::run(&api, &output, command).await,
            Command::Approval(command) => {
                commands::approvals::approval(&api, &output, command).await
            }
            Command::ApprovalPolicy(command) => {
                commands::approvals::approval_policy(&api, &output, command).await
            }
            Command::Revision(command) => commands::revisions::run(&api, &output, command).await,
            Command::Lua(command) => commands::lua::run(&api, &output, command).await,
            Command::Audit(command) => commands::audit::run(&api, &output, command).await,
            Command::Traffic(command) => commands::traffic::run(&api, &output, command).await,
            Command::Logs(command) => commands::logs::run(&api, &output, command).await,
            Command::Alert(command) => commands::alerts::run(&api, &output, command).await,
            Command::Host(args) => commands::host::run(&api, &output, args).await,
            Command::Container(command) => commands::containers::run(&api, &output, command).await,
            Command::Image(command) => commands::images::run(&api, &output, command).await,
            Command::Network(command) => {
                commands::engine_resources::networks(&api, &output, command).await
            }
            Command::Volume(command) => {
                commands::engine_resources::volumes(&api, &output, command).await
            }
            Command::Compose(command) => commands::compose::run(&api, &output, command).await,
            Command::Files(command) => commands::files::run(&api, &output, command).await,
            Command::Backup(command) => commands::backups::run(&api, &output, command).await,
            Command::Plugin(command) => commands::plugins::run(&api, &output, command).await,
            Command::Gateway(command) => commands::gateway::gateway(&api, &output, command).await,
            Command::Services => commands::gateway::services(&api, &output).await,
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

    /// Every command `panel/surfaces.json` names for an operation exists
    /// (ADR 0045).
    #[test]
    fn every_command_the_surfaces_name_exists() {
        let surfaces: serde_json::Value =
            serde_json::from_str(include_str!("../../surfaces.json")).unwrap();
        let root = Cli::command();
        let mut missing = Vec::new();
        for (operation, entry) in surfaces["operations"].as_object().unwrap() {
            let Some(path) = entry["cli"].as_str() else {
                continue;
            };
            let mut command = &root;
            for word in path.split(' ') {
                match command.find_subcommand(word) {
                    Some(next) => command = next,
                    None => {
                        missing.push(format!("{operation}: ppanel {path}"));
                        break;
                    }
                }
            }
        }
        assert!(missing.is_empty(), "no such commands: {missing:#?}");
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
            vec!["ppanel", "lua", "check", "conf"],
            vec!["ppanel", "lua", "scripts", "--revision", "3"],
            vec!["ppanel", "lua", "modules"],
            vec![
                "ppanel",
                "lua",
                "test",
                "--host",
                "shop.example",
                "--target",
                "/api?x=1",
                "-H",
                "x-key: k",
                "--script",
                "lua/auth.lua",
                "--phase",
                "access",
                "--allow",
                "body",
            ],
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
                "route",
                "add",
                "0190b5b6-3f43-7a52-8a56-2f8b7a7d5a10",
                "--match",
                "prefix:/api",
                "--internal-redirect",
                "@fallback",
                "--rewrite",
                "strip_prefix /api",
                "--internal",
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
            vec![
                "ppanel",
                "setup",
                "--username",
                "root",
                "--bootstrap-token-file",
                "secrets/bootstrap-token",
                "--password-stdin",
            ],
            vec!["ppanel", "login", "--username", "root"],
            vec!["ppanel", "--token", "ppat_x", "whoami"],
            vec!["ppanel", "logout"],
            vec!["ppanel", "password", "--password-stdin"],
            vec![
                "ppanel",
                "token",
                "create",
                "ci",
                "--permission",
                "config.read",
            ],
            vec![
                "ppanel",
                "token",
                "revoke",
                "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
            ],
            vec![
                "ppanel",
                "account",
                "create",
                "ops",
                "--role",
                "operator",
                "--with-password",
            ],
            vec![
                "ppanel",
                "account",
                "update",
                "ops",
                "--unlock",
                "--disable",
            ],
            vec!["ppanel", "account", "end-session", "ops", "0190a1b2"],
            vec!["ppanel", "role", "permissions"],
            vec![
                "ppanel",
                "role",
                "create",
                "deployer",
                "--name",
                "Deployer",
                "--permission",
                "config.read",
                "--permission",
                "config.apply",
            ],
            vec!["ppanel", "role", "delete", "deployer"],
            vec![
                "ppanel",
                "token",
                "rotate",
                "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
            ],
            vec!["ppanel", "account", "end-sessions", "ops"],
            vec!["ppanel", "logout", "--everywhere"],
            vec![
                "ppanel",
                "config",
                "explain",
                "sites/shop.conf:12.9",
                "conf",
            ],
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
                "ppanel", "traffic", "summary", "--site", "shop", "--window", "15m",
            ],
            vec![
                "ppanel", "traffic", "series", "--window", "7d", "--step", "1h",
            ],
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
