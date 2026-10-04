//! Setting up the first account, logging in and out, the caller's password
//! and API tokens, and administering accounts and roles.

use crate::{
    client::{Api, CliError, Result},
    credentials::{Credentials, Stored},
    output::{text, Column, Output},
};
use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{json, Map, Value};
use std::io::BufRead;

/// Where secrets come from: a prompt that does not echo, or standard input.
#[derive(Args, Clone, Debug, Default)]
pub(crate) struct SecretInput {
    /// Read passwords from standard input, one per line, instead of
    /// prompting.
    #[arg(long)]
    password_stdin: bool,
}

impl SecretInput {
    fn read(&self, prompt: &str) -> Result<String> {
        let secret = if self.password_stdin {
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .map_err(|error| CliError::Usage(format!("cannot read standard input: {error}")))?;
            line.trim_end_matches(['\n', '\r']).to_owned()
        } else {
            rpassword::prompt_password(prompt)
                .map_err(|error| CliError::Usage(format!("cannot read the password: {error}")))?
        };
        if secret.is_empty() {
            return Err(CliError::Usage("the password is empty".into()));
        }
        Ok(secret)
    }
}

#[derive(Subcommand)]
pub(crate) enum TokenCommand {
    /// Your API tokens.
    List,
    /// Creates an API token for scripts; its secret is printed once.
    Create {
        name: String,
        /// A permission it holds; all of yours by default.
        #[arg(long = "permission")]
        permissions: Vec<String>,
        /// Days until it expires, 1 to 365.
        #[arg(long, default_value_t = 90)]
        days: u32,
    },
    /// Revokes one of your API tokens.
    Revoke { id: String },
    /// Replaces one of your API tokens by one with a new secret, printed
    /// once; the old secret stops working at once.
    Rotate { id: String },
}

#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    /// Every account.
    List,
    /// One account, by username or ID.
    Show { account: String },
    /// Creates an account.
    Create {
        username: String,
        #[arg(long)]
        display_name: Option<String>,
        /// A role it holds.
        #[arg(long = "role")]
        roles: Vec<String>,
        /// Set a password now; otherwise the account cannot log in until
        /// one is set.
        #[arg(long, conflicts_with = "service")]
        with_password: bool,
        /// A service account for a program: it never signs in, and account
        /// managers issue its tokens with `ppanel account issue-token`.
        #[arg(long)]
        service: bool,
        #[command(flatten)]
        input: SecretInput,
    },
    /// An account's grants: roles given with a scope and conditions.
    Grants { account: String },
    /// Gives an account a role for one site group or site, or only under
    /// conditions.
    Grant(Box<GrantArgs>),
    /// Takes a grant back.
    RevokeGrant { account: String, grant: String },
    /// Issues an API token for a service account; it is shown once.
    IssueToken {
        account: String,
        #[arg(long)]
        name: String,
        /// A permission it grants; the service account's own without any.
        #[arg(long = "permission")]
        permissions: Vec<String>,
        #[arg(long, default_value_t = 30)]
        days: u32,
    },
    /// Changes an account's name, roles or state.
    Update {
        account: String,
        #[arg(long)]
        display_name: Option<String>,
        /// The roles it holds from now on.
        #[arg(long = "role")]
        roles: Vec<String>,
        #[arg(long, conflicts_with = "enable")]
        disable: bool,
        #[arg(long)]
        enable: bool,
        /// Keep its password sign-in when that is limited to break-glass
        /// accounts; every sign-in with it is recorded for review.
        #[arg(long, conflicts_with = "no_break_glass")]
        break_glass: bool,
        #[arg(long)]
        no_break_glass: bool,
        /// Clears failed logins and re-enables a locked password.
        #[arg(long)]
        unlock: bool,
    },
    /// Sets an account's password; its sessions end.
    Password {
        account: String,
        #[command(flatten)]
        input: SecretInput,
    },
    /// An account's sessions.
    Sessions { account: String },
    /// Ends a session of an account.
    EndSession { account: String, session: String },
    /// Ends every session of an account.
    EndSessions { account: String },
    /// An account's API tokens.
    Tokens { account: String },
    /// Revokes an API token of an account.
    RevokeToken { account: String, token: String },
}

#[derive(Args)]
pub(crate) struct GrantArgs {
    account: String,
    #[arg(long)]
    role: String,
    /// Only the sites in this group; limits configuration permissions alone.
    #[arg(long, conflicts_with = "site")]
    site_group: Option<String>,
    /// Only this site, by ID.
    #[arg(long)]
    site: Option<String>,
    /// Only for requests from this network, such as 10.0.0.0/8.
    #[arg(long = "network")]
    networks: Vec<String>,
    /// Only within this window, as `[DAYS ]HH:MM-HH:MM[ ZONE]` with an IANA
    /// time zone, UTC without one.
    #[arg(long = "window", value_parser = crate::commands::windows::window)]
    windows: Vec<Value>,
    /// Only until this time, in RFC 3339.
    #[arg(long)]
    until: Option<String>,
}

fn grant_scope(grant: &Value) -> String {
    match grant["scope"]["kind"].as_str() {
        Some("site_group") => format!("group {}", text(&grant["scope"]["group"])),
        Some("site") => format!("site {}", text(&grant["scope"]["site"])),
        _ => "everything".into(),
    }
}

fn grant_conditions(grant: &Value) -> String {
    let conditions = &grant["conditions"];
    let mut parts = Vec::new();
    if !conditions["not_after"].is_null() {
        parts.push(format!("until {}", text(&conditions["not_after"])));
    }
    let networks = text(&conditions["networks"]);
    if networks != "-" {
        parts.push(format!("from {networks}"));
    }
    for window in conditions["windows"].as_array().into_iter().flatten() {
        parts.push(crate::commands::windows::describe(window));
    }
    if parts.is_empty() {
        "always".into()
    } else {
        parts.join("; ")
    }
}

const GRANTS: &[Column] = &[
    ("ID", |grant| text(&grant["id"])),
    ("ROLE", |grant| text(&grant["role"])),
    ("SCOPE", grant_scope),
    ("CONDITIONS", grant_conditions),
    ("BY", |grant| text(&grant["created_by"])),
];

/// What a custom role is.
#[derive(Args, Clone, Debug)]
pub(crate) struct RoleFields {
    #[arg(long)]
    name: String,
    #[arg(long, default_value = "")]
    description: String,
    /// A permission it grants; see `ppanel role permissions`.
    #[arg(long = "permission", required = true)]
    permissions: Vec<String>,
}

#[derive(Subcommand)]
pub(crate) enum RoleCommand {
    /// Every role and its permissions.
    List,
    /// The permissions roles grant.
    Permissions,
    /// Creates a role.
    Create {
        id: String,
        #[command(flatten)]
        fields: RoleFields,
    },
    /// Replaces a role that is not built in.
    Update {
        id: String,
        #[command(flatten)]
        fields: RoleFields,
    },
    /// Deletes a role that is not built in and that no account holds.
    Delete { id: String },
}

const ACCOUNTS: &[Column] = &[
    ("USERNAME", |account| text(&account["username"])),
    ("NAME", |account| text(&account["display_name"])),
    ("ROLES", |account| text(&account["roles"])),
    ("STATE", |account| state(account)),
    ("LAST LOGIN", |account| text(&account["last_login_at"])),
    ("ID", |account| text(&account["id"])),
];

const ACCOUNT: &[Column] = &[
    ("Username", |account| text(&account["username"])),
    ("Name", |account| text(&account["display_name"])),
    ("ID", |account| text(&account["id"])),
    ("Roles", |account| text(&account["roles"])),
    ("State", |account| state(account)),
    ("Created", |account| text(&account["created_at"])),
    ("Last login", |account| text(&account["last_login_at"])),
    ("Password changed", |account| {
        text(&account["password_changed_at"])
    }),
];

const SESSIONS: &[Column] = &[
    ("ID", |session| {
        let current = if session["current"] == true { " *" } else { "" };
        format!("{}{current}", text(&session["id"]))
    }),
    ("TRANSPORT", |session| text(&session["transport"])),
    ("CLIENT", |session| text(&session["client_address"])),
    ("CREATED", |session| text(&session["created_at"])),
    ("LAST SEEN", |session| text(&session["last_seen_at"])),
    ("EXPIRES", |session| text(&session["expires_at"])),
];

const TOKENS: &[Column] = &[
    ("NAME", |token| text(&token["name"])),
    ("PERMISSIONS", |token| text(&token["permissions"])),
    ("EXPIRES", |token| text(&token["expires_at"])),
    ("LAST USED", |token| text(&token["last_used_at"])),
    ("REVOKED", |token| text(&token["revoked_at"])),
    ("ID", |token| text(&token["id"])),
];

const ROLES: &[Column] = &[
    ("ROLE", |role| text(&role["id"])),
    ("NAME", |role| text(&role["name"])),
    ("PERMISSIONS", |role| text(&role["permissions"])),
    ("BUILT IN", |role| text(&role["built_in"])),
];

const PERMISSIONS: &[Column] = &[
    ("PERMISSION", |permission| text(&permission["name"])),
    ("ALLOWS", |permission| text(&permission["description"])),
];

fn state(account: &Value) -> String {
    let state = match (account["disabled"] == true, account["locked"] == true) {
        (true, _) => "disabled",
        (false, true) => "locked",
        _ => "active",
    };
    let mut state = state.to_owned();
    if account["service"] == true {
        state.push_str(", service");
    }
    if account["break_glass"] == true {
        state.push_str(", break-glass");
    }
    state
}

/// Creates the first account with the deployment's bootstrap token.
pub(crate) async fn setup(
    api: &Api,
    output: &Output,
    username: String,
    token_file: String,
    input: &SecretInput,
) -> Result<()> {
    let token = std::fs::read_to_string(&token_file)
        .map_err(|error| CliError::Usage(format!("cannot read {token_file}: {error}")))?;
    let password = input.read(&format!("Password for {username}: "))?;
    let account = api
        .post_read(
            "/api/v1/setup",
            &json!({
                "token": token.trim_end(),
                "username": username,
                "password": password,
            }),
        )
        .await?
        .body;
    output.done(
        &format!(
            "Created {}; log in with `ppanel login --username {}`",
            text(&account["username"]),
            text(&account["username"])
        ),
        &account,
    );
    Ok(())
}

pub(crate) async fn login(
    api: &Api,
    output: &Output,
    credentials: Option<&Credentials>,
    username: String,
    input: &SecretInput,
) -> Result<()> {
    let credentials = credentials.ok_or_else(|| {
        CliError::Usage("no configuration directory; set PPANEL_CONFIG_DIR".into())
    })?;
    let password = input.read(&format!("Password for {username}: "))?;
    let session = api
        .post_read(
            "/api/v1/session",
            &json!({ "username": username, "password": password, "transport": "bearer" }),
        )
        .await?
        .body;
    let secret = session["secret"]
        .as_str()
        .ok_or_else(|| CliError::Failed("the API returned no session".into()))?;
    credentials.save(
        api.base(),
        &Stored {
            secret: secret.to_owned(),
            username: text(&session["account"]["username"]),
        },
    )?;
    let mut shown = session.clone();
    if let Some(object) = shown.as_object_mut() {
        object.remove("secret");
    }
    output.done(
        &format!(
            "Logged in as {} until {}",
            text(&session["account"]["username"]),
            text(&session["session"]["expires_at"])
        ),
        &shown,
    );
    Ok(())
}

pub(crate) async fn logout(
    api: &Api,
    output: &Output,
    credentials: Option<&Credentials>,
    everywhere: bool,
) -> Result<()> {
    if everywhere {
        api.change(Method::DELETE, "/api/v1/account/sessions", None, None)
            .await?;
    }
    let ended = api
        .change(Method::DELETE, "/api/v1/session", None, None)
        .await;
    if let Some(credentials) = credentials {
        credentials.remove(api.base())?;
    }
    match ended {
        Ok(_) | Err(CliError::Api { .. }) => {
            output.done("Logged out", &Value::Null);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub(crate) async fn whoami(api: &Api, output: &Output) -> Result<()> {
    let current = api.get("/api/v1/session", &[]).await?.body;
    let mut fields: Map<String, Value> = Map::new();
    fields.insert("username".into(), current["account"]["username"].clone());
    fields.insert("roles".into(), current["account"]["roles"].clone());
    fields.insert("permissions".into(), current["permissions"].clone());
    fields.insert("credential".into(), current["credential"].clone());
    fields.insert(
        "expires_at".into(),
        current["session"]["expires_at"].clone(),
    );
    output.item(
        &Value::Object(fields),
        &[
            ("Username", |item| text(&item["username"])),
            ("Roles", |item| text(&item["roles"])),
            ("Permissions", |item| text(&item["permissions"])),
            ("Credential", |item| text(&item["credential"])),
            ("Session expires", |item| text(&item["expires_at"])),
        ],
    );
    Ok(())
}

pub(crate) async fn password(api: &Api, output: &Output, input: &SecretInput) -> Result<()> {
    let current = input.read("Current password: ")?;
    let new = input.read("New password: ")?;
    if !input.password_stdin && input.read("Repeat the new password: ")? != new {
        return Err(CliError::Usage("the new passwords differ".into()));
    }
    api.change(
        Method::PUT,
        "/api/v1/account/password",
        Some(&json!({ "current": current, "new": new })),
        None,
    )
    .await?;
    output.done("Password changed; your other sessions ended", &Value::Null);
    Ok(())
}

pub(crate) async fn token(api: &Api, output: &Output, command: TokenCommand) -> Result<()> {
    match command {
        TokenCommand::List => {
            let tokens = api.get("/api/v1/account/tokens", &[]).await?.body;
            output.list(&tokens, TOKENS);
        }
        TokenCommand::Create {
            name,
            permissions,
            days,
        } => {
            let created = api
                .change(
                    Method::POST,
                    "/api/v1/account/tokens",
                    Some(&json!({
                        "name": name,
                        "permissions": (!permissions.is_empty()).then_some(permissions),
                        "expires_in_days": days,
                    })),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "{}\nThis is the only time the token is shown; it expires {}.",
                    text(&created["secret"]),
                    text(&created["token"]["expires_at"])
                ),
                &created,
            );
        }
        TokenCommand::Revoke { id } => {
            api.change(
                Method::DELETE,
                &format!("/api/v1/account/tokens/{id}"),
                None,
                None,
            )
            .await?;
            output.done("Token revoked", &Value::Null);
        }
        TokenCommand::Rotate { id } => {
            let rotated = api
                .change(
                    Method::POST,
                    &format!("/api/v1/account/tokens/{id}/rotate"),
                    None,
                    None,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "{}\nThis is the only time the token is shown; it expires {}. The old one no longer works.",
                    text(&rotated["secret"]),
                    text(&rotated["token"]["expires_at"])
                ),
                &rotated,
            );
        }
    }
    Ok(())
}

/// The ID of an account named by username or ID.
pub(crate) async fn account_id(api: &Api, account: &str) -> Result<String> {
    if uuid::Uuid::parse_str(account).is_ok() {
        return Ok(account.to_owned());
    }
    let accounts = api.get("/api/v1/accounts", &[]).await?.body;
    accounts
        .as_array()
        .into_iter()
        .flatten()
        .find(|candidate| candidate["username"].as_str() == Some(&account.to_ascii_lowercase()))
        .map(|candidate| text(&candidate["id"]))
        .ok_or_else(|| CliError::Failed(format!("there is no account {account:?}")))
}

pub(crate) async fn account(api: &Api, output: &Output, command: AccountCommand) -> Result<()> {
    match command {
        AccountCommand::List => {
            let accounts = api.get("/api/v1/accounts", &[]).await?.body;
            output.list(&accounts, ACCOUNTS);
        }
        AccountCommand::Show { account } => {
            let id = account_id(api, &account).await?;
            let shown = api.get(&format!("/api/v1/accounts/{id}"), &[]).await?.body;
            output.item(&shown, ACCOUNT);
        }
        AccountCommand::Create {
            username,
            display_name,
            roles,
            with_password,
            service,
            input,
        } => {
            let password = if with_password {
                Some(input.read(&format!("Password for {username}: "))?)
            } else {
                None
            };
            let created = api
                .change(
                    Method::POST,
                    "/api/v1/accounts",
                    Some(&json!({
                        "username": username,
                        "display_name": display_name,
                        "password": password,
                        "roles": roles,
                        "service": service,
                    })),
                    None,
                )
                .await?
                .body;
            output.done(&format!("Created {}", text(&created["username"])), &created);
        }
        AccountCommand::Grants { account } => {
            let id = account_id(api, &account).await?;
            let grants = api
                .get(&format!("/api/v1/accounts/{id}/grants"), &[])
                .await?
                .body;
            output.list(&grants, GRANTS);
        }
        AccountCommand::Grant(grant) => {
            let id = account_id(api, &grant.account).await?;
            let scope = match (&grant.site_group, &grant.site) {
                (Some(group), _) => json!({"kind": "site_group", "group": group}),
                (None, Some(site)) => json!({"kind": "site", "site": site}),
                (None, None) => json!({"kind": "everything"}),
            };
            let created = api
                .change(
                    Method::POST,
                    &format!("/api/v1/accounts/{id}/grants"),
                    Some(&json!({
                        "role": grant.role,
                        "scope": scope,
                        "conditions": {
                            "not_after": grant.until,
                            "networks": grant.networks,
                            "windows": grant.windows,
                        },
                    })),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "Granted {} to {} for {} ({}); grant {}",
                    grant.role,
                    grant.account,
                    grant_scope(&created),
                    grant_conditions(&created),
                    text(&created["id"])
                ),
                &created,
            );
        }
        AccountCommand::RevokeGrant { account, grant } => {
            let id = account_id(api, &account).await?;
            api.change(
                Method::DELETE,
                &format!("/api/v1/accounts/{id}/grants/{grant}"),
                None,
                None,
            )
            .await?;
            output.done(&format!("Revoked the grant {grant}"), &Value::Null);
        }
        AccountCommand::IssueToken {
            account,
            name,
            permissions,
            days,
        } => {
            let id = account_id(api, &account).await?;
            let issued = api
                .change(
                    Method::POST,
                    &format!("/api/v1/accounts/{id}/tokens"),
                    Some(&json!({
                        "name": name,
                        "permissions": (!permissions.is_empty()).then_some(permissions),
                        "expires_in_days": days,
                    })),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "{}\nThis is the only time the token is shown; it expires {}.",
                    text(&issued["secret"]),
                    text(&issued["token"]["expires_at"])
                ),
                &issued,
            );
        }
        AccountCommand::Update {
            account,
            display_name,
            roles,
            disable,
            enable,
            break_glass,
            no_break_glass,
            unlock,
        } => {
            let id = account_id(api, &account).await?;
            let mut patch = Map::new();
            if let Some(name) = display_name {
                patch.insert("display_name".into(), json!(name));
            }
            if !roles.is_empty() {
                patch.insert("roles".into(), json!(roles));
            }
            if disable || enable {
                patch.insert("disabled".into(), json!(disable));
            }
            if break_glass || no_break_glass {
                patch.insert("break_glass".into(), json!(break_glass));
            }
            if unlock {
                patch.insert("unlock".into(), json!(true));
            }
            let updated = api
                .change(
                    Method::PATCH,
                    &format!("/api/v1/accounts/{id}"),
                    Some(&Value::Object(patch)),
                    None,
                )
                .await?
                .body;
            output.done(&format!("Updated {}", text(&updated["username"])), &updated);
        }
        AccountCommand::Password { account, input } => {
            let id = account_id(api, &account).await?;
            let password = input.read(&format!("New password for {account}: "))?;
            api.change(
                Method::PUT,
                &format!("/api/v1/accounts/{id}/password"),
                Some(&json!({ "password": password })),
                None,
            )
            .await?;
            output.done("Password set; the account's sessions ended", &Value::Null);
        }
        AccountCommand::Sessions { account } => {
            let id = account_id(api, &account).await?;
            let sessions = api
                .get(&format!("/api/v1/accounts/{id}/sessions"), &[])
                .await?
                .body;
            output.list(&sessions, SESSIONS);
        }
        AccountCommand::EndSession { account, session } => {
            let id = account_id(api, &account).await?;
            api.change(
                Method::DELETE,
                &format!("/api/v1/accounts/{id}/sessions/{session}"),
                None,
                None,
            )
            .await?;
            output.done("Session ended", &Value::Null);
        }
        AccountCommand::EndSessions { account } => {
            let id = account_id(api, &account).await?;
            let ended = api
                .change(
                    Method::DELETE,
                    &format!("/api/v1/accounts/{id}/sessions"),
                    None,
                    None,
                )
                .await?
                .body;
            output.done(&format!("Ended {} sessions", text(&ended["ended"])), &ended);
        }
        AccountCommand::Tokens { account } => {
            let id = account_id(api, &account).await?;
            let tokens = api
                .get(&format!("/api/v1/accounts/{id}/tokens"), &[])
                .await?
                .body;
            output.list(&tokens, TOKENS);
        }
        AccountCommand::RevokeToken { account, token } => {
            let id = account_id(api, &account).await?;
            api.change(
                Method::DELETE,
                &format!("/api/v1/accounts/{id}/tokens/{token}"),
                None,
                None,
            )
            .await?;
            output.done("Token revoked", &Value::Null);
        }
    }
    Ok(())
}

pub(crate) async fn role(api: &Api, output: &Output, command: RoleCommand) -> Result<()> {
    match command {
        RoleCommand::List => {
            let roles = api.get("/api/v1/roles", &[]).await?.body;
            output.list(&roles, ROLES);
        }
        RoleCommand::Permissions => {
            let permissions = api.get("/api/v1/permissions", &[]).await?.body;
            output.list(&permissions, PERMISSIONS);
        }
        RoleCommand::Create { id, fields } => {
            let created = api
                .change(
                    Method::POST,
                    "/api/v1/roles",
                    Some(&json!({
                        "id": id,
                        "name": fields.name,
                        "description": fields.description,
                        "permissions": fields.permissions,
                    })),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!("Created the role {}", text(&created["id"])),
                &created,
            );
        }
        RoleCommand::Update { id, fields } => {
            let updated = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/roles/{id}"),
                    Some(&json!({
                        "name": fields.name,
                        "description": fields.description,
                        "permissions": fields.permissions,
                    })),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!("Updated the role {}", text(&updated["id"])),
                &updated,
            );
        }
        RoleCommand::Delete { id } => {
            api.change(Method::DELETE, &format!("/api/v1/roles/{id}"), None, None)
                .await?;
            output.done(&format!("Deleted the role {id}"), &Value::Null);
        }
    }
    Ok(())
}
