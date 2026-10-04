//! The images on the container engines the host agent reaches.

use crate::{
    client::{Api, CliError, Result},
    output::{bytes, text, Column, Format, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::Value;

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
    }
    Ok(())
}
