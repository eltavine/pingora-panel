//! Listings of static directories without an index file (ADR 0042).

use panel_ir::MOST_LISTED;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::Serialize;
use std::{
    fmt::Write as _,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// Bytes a path segment keeps as they are (RFC 3986 §3.3), without `:` so
/// a name never reads as a scheme.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'!')
    .remove(b'$')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b';')
    .remove(b'=')
    .remove(b'@');

pub(crate) struct Entry {
    name: String,
    directory: bool,
    size: u64,
    modified: Option<SystemTime>,
}

pub(crate) struct Listing {
    entries: Vec<Entry>,
    truncated: bool,
}

/// The entries of `directory` a listing shows: not hidden, and inside
/// `root` once symbolic links are resolved; directories first, then by
/// name.
pub(crate) async fn read(directory: &Path, root: &Path) -> std::io::Result<Listing> {
    let mut reader = tokio::fs::read_dir(directory).await?;
    let mut listing = Listing {
        entries: Vec::new(),
        truncated: false,
    };
    while let Some(entry) = reader.next_entry().await? {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = tokio::fs::metadata(&path).await else {
            continue;
        };
        let linked = entry.file_type().await.is_ok_and(|kind| kind.is_symlink());
        if linked
            && !tokio::fs::canonicalize(&path)
                .await
                .is_ok_and(|resolved| resolved.starts_with(root))
        {
            continue;
        }
        if !metadata.is_dir() && !metadata.is_file() {
            continue;
        }
        if listing.entries.len() == MOST_LISTED {
            listing.truncated = true;
            break;
        }
        listing.entries.push(Entry {
            name,
            directory: metadata.is_dir(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }
    listing.entries.sort_by(|left, right| {
        right
            .directory
            .cmp(&left.directory)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(listing)
}

fn modified(entry: &Entry) -> Option<String> {
    entry
        .modified
        .filter(|time| time.duration_since(UNIX_EPOCH).is_ok())
        .map(httpdate::fmt_http_date)
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for char in text.chars() {
        match char {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            char => escaped.push(char),
        }
    }
    escaped
}

/// An HTML page listing `listing` as the directory at `path`.
pub(crate) fn html(listing: &Listing, path: &str) -> String {
    let shown = escape(&percent_encoding::percent_decode_str(path).decode_utf8_lossy());
    let mut page = format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Index of {shown}</title>\n</head>\n<body>\n<h1>Index of {shown}</h1>\n\
         <table>\n<thead><tr><th>Name</th><th>Last modified</th><th>Size</th></tr></thead>\n<tbody>\n"
    );
    if path != "/" {
        page.push_str("<tr><td><a href=\"../\">../</a></td><td></td><td></td></tr>\n");
    }
    for entry in &listing.entries {
        let slash = if entry.directory { "/" } else { "" };
        let href = escape(&utf8_percent_encode(&entry.name, SEGMENT).to_string());
        let size = if entry.directory {
            "-".to_owned()
        } else {
            entry.size.to_string()
        };
        let _ = writeln!(
            page,
            "<tr><td><a href=\"{href}{slash}\">{}{slash}</a></td><td>{}</td><td>{size}</td></tr>",
            escape(&entry.name),
            modified(entry).unwrap_or_default(),
        );
    }
    page.push_str("</tbody>\n</table>\n");
    if listing.truncated {
        let _ = writeln!(
            page,
            "<p>Only the first {MOST_LISTED} entries are listed.</p>"
        );
    }
    page.push_str("</body>\n</html>\n");
    page
}

#[derive(Serialize)]
struct JsonEntry<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    mtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
}

/// The entries as nginx's `autoindex_format json` writes them.
pub(crate) fn json(listing: &Listing) -> Vec<u8> {
    let entries: Vec<JsonEntry<'_>> = listing
        .entries
        .iter()
        .map(|entry| JsonEntry {
            name: &entry.name,
            kind: if entry.directory { "directory" } else { "file" },
            mtime: modified(entry),
            size: (!entry.directory).then_some(entry.size),
        })
        .collect();
    serde_json::to_vec(&entries).expect("listings serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn listings_hide_dotfiles_and_links_out_and_put_directories_first() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().canonicalize().unwrap().join("site");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("b.txt"), "bb").unwrap();
        std::fs::write(root.join("a <&> \"b\".txt"), "a").unwrap();
        std::fs::write(root.join(".env"), "secret").unwrap();
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret"), "x").unwrap();
            std::os::unix::fs::symlink(outside.path().join("secret"), root.join("escape")).unwrap();
            std::os::unix::fs::symlink(root.join("b.txt"), root.join("alias.txt")).unwrap();
        }
        let listing = read(&root, &root).await.unwrap();
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        #[cfg(unix)]
        assert_eq!(
            names,
            ["assets", "docs", "a <&> \"b\".txt", "alias.txt", "b.txt"]
        );
        assert!(!listing.truncated);

        let page = html(&listing, "/files/");
        assert!(page.contains("<title>Index of /files/</title>"));
        assert!(page.contains("<a href=\"../\">../</a>"));
        assert!(page.contains("<a href=\"docs/\">docs/</a>"));
        assert!(
            page.contains(
                "<a href=\"a%20%3C%26%3E%20%22b%22.txt\">a &lt;&amp;&gt; &quot;b&quot;.txt</a>"
            ),
            "{page}"
        );
        assert!(!page.contains(".env"));
        assert!(!html(&listing, "/").contains("../"));

        let entries: serde_json::Value = serde_json::from_slice(&json(&listing)).unwrap();
        assert_eq!(entries[0]["type"], "directory");
        assert!(entries[0].get("size").is_none());
        let b = entries
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "b.txt")
            .unwrap();
        assert_eq!(
            (b["type"].as_str(), b["size"].as_u64()),
            (Some("file"), Some(2))
        );
        assert!(httpdate::parse_http_date(b["mtime"].as_str().unwrap()).is_ok());
    }

    #[test]
    fn names_never_read_as_schemes_or_leave_their_segment() {
        let href = |name: &str| utf8_percent_encode(name, SEGMENT).to_string();
        assert_eq!(href("javascript:alert(1)"), "javascript%3Aalert(1)");
        assert_eq!(href("a/b"), "a%2Fb");
        assert_eq!(href("~user's file"), "~user's%20file");
        assert_eq!(href("百科"), "%E7%99%BE%E7%A7%91");
    }
}
