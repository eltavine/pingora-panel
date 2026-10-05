//! The images on the container engines the host agent reaches.

use crate::{
    client::{Api, CliError, Result},
    output::{bytes, text, Column, Format, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};
use std::{collections::HashMap, io::Read, time::Duration};

/// The longest a pull may take: the host agent's limit, and a margin.
const PULL_LASTING: Duration = Duration::from_secs(35 * 60);

#[derive(Subcommand)]
pub(crate) enum ImageCommand {
    /// An engine's images, with the containers made from each.
    List {
        /// `docker` or `podman`.
        #[arg(long, default_value = "docker")]
        engine: String,
        /// Matched against tags and IDs, ignoring case.
        #[arg(long)]
        search: Option<String>,
    },
    /// An image's configuration; never its environment or command line.
    Inspect {
        #[command(flatten)]
        target: Target,
    },
    /// Removes a reference to an image, and the image once nothing names it.
    Remove {
        #[command(flatten)]
        target: Target,
        /// Removes an image stopped containers use, and every reference to
        /// an image named by its ID.
        #[arg(long)]
        force: bool,
        /// Confirms that the image is removed.
        #[arg(long)]
        yes: bool,
    },
    /// Pulls an image from its registry, saying on standard error how far
    /// each layer got and printing what was pulled. Interrupting leaves the
    /// pull going on.
    Pull {
        /// Such as `nginx:1.27` or `ghcr.io/example/app@sha256:…`; a name
        /// alone is its `latest` tag.
        #[arg(value_parser = reference)]
        image: String,
        /// `docker` or `podman`.
        #[arg(long, default_value = "docker")]
        engine: String,
        /// Such as `linux/arm64`; the engine's own by default.
        #[arg(long)]
        platform: Option<String>,
        /// Signs in to the registry as this user, with the password or
        /// access token from `--password-stdin`; used for this pull only.
        #[arg(long, requires = "password_stdin")]
        username: Option<String>,
        /// Reads the registry password or access token from standard input.
        #[arg(long, requires = "username")]
        password_stdin: bool,
    },
}

/// The image a command is about.
#[derive(clap::Args)]
pub(crate) struct Target {
    /// Its ID, a prefix of its ID, or a reference such as `nginx:1.27`.
    #[arg(value_parser = reference)]
    image: String,
    /// `docker` or `podman`.
    #[arg(long, default_value = "docker")]
    engine: String,
}

impl Target {
    /// Its path, with the reference's slashes percent-encoded.
    fn path(&self) -> String {
        format!(
            "/api/v1/container-engines/{}/images/{}",
            self.engine,
            self.image.replace('/', "%2F")
        )
    }
}

/// An image's ID or reference, by the characters references allow; each
/// part between slashes is a name, never `.` or `..`.
fn reference(value: &str) -> std::result::Result<String, String> {
    let valid = !value.is_empty()
        && value.len() <= 512
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@'))
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if valid {
        Ok(value.to_owned())
    } else {
        Err("not an image's ID or reference".to_owned())
    }
}

/// The tags, or `<none>` for an image nothing names any more.
fn tags(image: &Value) -> String {
    match image["tags"].as_array() {
        Some(tags) if !tags.is_empty() => text(&image["tags"]),
        _ => "<none>".into(),
    }
}

const IMAGES: &[Column] = &[
    ("TAGS", tags),
    ("ID", |image| {
        let id = image["id"].as_str().unwrap_or_default();
        id.trim_start_matches("sha256:").chars().take(12).collect()
    }),
    ("CREATED", |image| text(&image["created"])),
    ("SIZE", |image| bytes(&image["size_bytes"])),
    ("CONTAINERS", |image| text(&image["containers"])),
];

const DETAIL: &[Column] = &[
    ("Tags", |detail| tags(&detail["image"])),
    ("ID", |detail| text(&detail["image"]["id"])),
    ("Digests", |detail| text(&detail["image"]["digests"])),
    ("Created", |detail| text(&detail["image"]["created"])),
    ("Size", |detail| bytes(&detail["image"]["size_bytes"])),
    ("Containers", |detail| text(&detail["image"]["containers"])),
    ("Platform", |detail| {
        let platform = [&detail["os"], &detail["architecture"], &detail["variant"]]
            .into_iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("/");
        if platform.is_empty() {
            "-".into()
        } else {
            platform
        }
    }),
    ("Author", |detail| text(&detail["author"])),
    ("User", |detail| text(&detail["user"])),
    ("Working directory", |detail| {
        text(&detail["working_directory"])
    }),
    ("Exposed ports", |detail| text(&detail["exposed_ports"])),
    ("Volumes", |detail| text(&detail["volumes"])),
    ("Stop signal", |detail| text(&detail["stop_signal"])),
    ("Layers", |detail| text(&detail["layers"])),
];

pub async fn run(api: &Api, output: &Output, command: ImageCommand) -> Result<()> {
    match command {
        ImageCommand::List { engine, search } => {
            let query: Vec<(&str, String)> = search
                .map(|search| ("search", search))
                .into_iter()
                .collect();
            let path = format!("/api/v1/container-engines/{engine}/images");
            let list = api.get(&path, &query).await?.body;
            if output.format == Format::Json {
                output.json(&list);
            } else if list["images"].as_array().is_some_and(Vec::is_empty) {
                if !output.quiet {
                    eprintln!("no images match");
                }
            } else {
                output.list(&list["images"], IMAGES);
            }
        }
        ImageCommand::Inspect { target } => {
            let detail = api.get(&target.path(), &[]).await?.body;
            if output.format == Format::Json {
                output.json(&detail);
            } else {
                output.item(&detail, DETAIL);
            }
        }
        ImageCommand::Remove { target, force, yes } => {
            if !yes {
                return Err(CliError::Usage(
                    "the image is removed for good; pass --yes to confirm".into(),
                ));
            }
            let path = format!("{}?force={force}", target.path());
            let removed = api.change(Method::DELETE, &path, None, None).await?.body;
            if output.format == Format::Json {
                output.json(&removed);
            } else if !output.quiet {
                for untagged in removed["untagged"].as_array().into_iter().flatten() {
                    println!("untagged {}", text(untagged));
                }
                for deleted in removed["deleted"].as_array().into_iter().flatten() {
                    println!("deleted {}", text(deleted));
                }
            }
        }
        ImageCommand::Pull {
            image,
            engine,
            platform,
            username,
            password_stdin: _,
        } => {
            let mut body = json!({ "reference": image });
            if let Some(platform) = platform {
                body["platform"] = platform.into();
            }
            if let Some(username) = username {
                let mut password = String::new();
                std::io::stdin()
                    .read_to_string(&mut password)
                    .map_err(|error| {
                        CliError::Usage(format!("cannot read the password: {error}"))
                    })?;
                let password = password.trim_end_matches(['\r', '\n']);
                if password.is_empty() {
                    return Err(CliError::Usage("--password-stdin read no password".into()));
                }
                body["credentials"] = json!({ "username": username, "password": password });
            }
            pull(api, output, &engine, &image, &body).await?;
        }
    }
    Ok(())
}

/// What a layer's state reads as, as `docker pull` writes it.
fn layer_state(state: &str) -> &str {
    match state {
        "waiting" => "Waiting",
        "downloading" => "Downloading",
        "downloaded" => "Download complete",
        "extracting" => "Extracting",
        "complete" => "Pull complete",
        "exists" => "Already exists",
        other => other,
    }
}

/// Follows a pull's server-sent events to its end: each layer's change on
/// standard error, or each message as a JSON line.
async fn pull(api: &Api, output: &Output, engine: &str, image: &str, body: &Value) -> Result<()> {
    let path = format!("/api/v1/container-engines/{engine}/image-pulls");
    let mut response = api.change_events(&path, body, PULL_LASTING).await?;
    let mut buffer: Vec<u8> = Vec::new();
    let mut states: HashMap<String, String> = HashMap::new();
    while let Some(chunk) = response.chunk().await.map_err(CliError::transport)? {
        buffer.extend(chunk.iter().filter(|byte| **byte != b'\r'));
        while let Some(end) = buffer.windows(2).position(|pair| pair == b"\n\n") {
            let event: Vec<u8> = buffer.drain(..end + 2).collect();
            let event = String::from_utf8_lossy(&event);
            let data: Vec<&str> = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect();
            if data.is_empty() {
                continue;
            }
            let message: Value = serde_json::from_str(&data.join("\n"))
                .map_err(|error| CliError::Transport(format!("an unreadable event: {error}")))?;
            if output.format == Format::Json && !output.quiet {
                println!("{message}");
            }
            match message["kind"].as_str() {
                Some("pulled") => {
                    if output.format == Format::Table && !output.quiet {
                        let reference = message["image"]["tags"]
                            .get(0)
                            .and_then(Value::as_str)
                            .unwrap_or(image);
                        if let Some(digest) = message["digest"].as_str() {
                            eprintln!("Digest: {digest}");
                        }
                        eprintln!(
                            "Status: {} for {reference}",
                            if message["updated"] == true {
                                "Downloaded newer image"
                            } else {
                                "Image is up to date"
                            }
                        );
                        println!("{reference}");
                    }
                    return Ok(());
                }
                Some("failed") => {
                    return Err(CliError::Ended {
                        code: text(&message["error"]["code"]),
                        message: text(&message["error"]["message"]),
                    });
                }
                _ => {}
            }
            if output.format == Format::Table && !output.quiet {
                for layer in message["layers"].as_array().into_iter().flatten() {
                    let (id, state) = (text(&layer["id"]), text(&layer["state"]));
                    if states.get(&id) != Some(&state) {
                        eprintln!("{id}: {}", layer_state(&state));
                        states.insert(id, state);
                    }
                }
            }
        }
    }
    Err(CliError::Transport(
        "the API ended the pull without saying how it went".into(),
    ))
}
