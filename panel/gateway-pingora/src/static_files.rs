//! Static content below the gateway's static root.
//!
//! The request path is appended to the policy root (`root` semantics), and
//! files must stay inside it after symbolic links are resolved. Responses
//! follow RFC 9110 conditional requests (§13) and single byte ranges (§14).

use crate::responses;
use bytes::{Bytes, BytesMut};
use http::{header, Method};
use panel_errors::{PanelError, Result};
use panel_ir::StaticContentPolicy;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::{
    fs::Metadata,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const CHUNK_BYTES: u64 = 64 * 1024;

pub(crate) struct StaticContent {
    root: PathBuf,
    index_files: Vec<String>,
    spa_fallback: bool,
}

impl StaticContent {
    pub(crate) fn compile(policy: &StaticContentPolicy, base: Option<&Path>) -> Result<Self> {
        let invalid = |detail: &str| {
            PanelError::validation_failed(format!("static content {}: {detail}", policy.id))
        };
        let base = base.ok_or_else(|| invalid("the gateway has no static content root"))?;
        let relative = Path::new(&policy.root);
        if policy.root.is_empty()
            || !relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(invalid("root must be a relative path without '.' or '..'"));
        }
        let base = base
            .canonicalize()
            .map_err(|_| invalid("the static content root does not exist"))?;
        let joined = base.join(relative);
        let root = match joined.canonicalize() {
            Ok(root) if root.starts_with(&base) && root.is_dir() => root,
            Ok(_) => {
                return Err(invalid(
                    "root must be a directory inside the static content root",
                ))
            }
            // As in nginx, a root that is not there serves nothing until it
            // is; what is made there later is still checked as it is served.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => joined,
            Err(_) => return Err(invalid("root directory cannot be read")),
        };
        if policy.index_files.iter().any(|name| !is_file_name(name)) {
            return Err(invalid("index files must be plain file names"));
        }
        Ok(Self {
            root,
            index_files: policy.index_files.clone(),
            spa_fallback: policy.spa_fallback,
        })
    }

    async fn locate(&self, components: &[String]) -> Located {
        let mut candidate = self.root.clone();
        candidate.extend(components);
        match tokio::fs::metadata(&candidate).await {
            Ok(metadata) if metadata.is_dir() && self.contained(&candidate).await.is_some() => {
                Located::Directory(self.index_in(&candidate).await)
            }
            Ok(metadata) if metadata.is_file() => match self.contained(&candidate).await {
                Some(path) => Located::File(path, metadata),
                None => Located::Missing,
            },
            _ => Located::Missing,
        }
    }

    async fn index_in(&self, directory: &Path) -> Option<(PathBuf, Metadata)> {
        for name in &self.index_files {
            let candidate = directory.join(name);
            if let Ok(metadata) = tokio::fs::metadata(&candidate).await {
                if metadata.is_file() {
                    if let Some(path) = self.contained(&candidate).await {
                        return Some((path, metadata));
                    }
                }
            }
        }
        None
    }

    async fn contained(&self, candidate: &Path) -> Option<PathBuf> {
        tokio::fs::canonicalize(candidate)
            .await
            .ok()
            .filter(|resolved| resolved.starts_with(&self.root))
    }
}

enum Located {
    File(PathBuf, Metadata),
    /// A directory and its index file, if it has one; there are no listings.
    Directory(Option<(PathBuf, Metadata)>),
    Missing,
}

pub(crate) async fn serve(
    session: &mut Session,
    content: &StaticContent,
    path: &str,
    compression: Option<&crate::http_policy::Compression>,
) -> pingora_core::Result<()> {
    let method = session.req_header().method.clone();
    if method != Method::GET && method != Method::HEAD {
        return responses::plain(
            session,
            405,
            "method not allowed",
            &[(header::ALLOW, "GET, HEAD")],
        )
        .await;
    }
    let Some(components) = decode_components(path) else {
        return responses::plain(session, 400, "invalid path", &[]).await;
    };
    let directory_request = path.ends_with('/');
    let (file, metadata) = match content.locate(&components).await {
        Located::File(file, metadata) if !directory_request => (file, metadata),
        Located::Directory(_) if !directory_request => {
            let query = session
                .req_header()
                .uri
                .query()
                .map(|query| format!("?{query}"))
                .unwrap_or_default();
            return responses::redirect(session, 301, &format!("{path}/{query}")).await;
        }
        Located::Directory(Some(found)) => found,
        Located::Directory(None) | Located::File(..) | Located::Missing => {
            match content.spa_fallback {
                true => match content.index_in(&content.root).await {
                    Some(found) => found,
                    None => return responses::plain(session, 404, "not found", &[]).await,
                },
                false => return responses::plain(session, 404, "not found", &[]).await,
            }
        }
    };
    send_file(
        session,
        &file,
        &metadata,
        method == Method::HEAD,
        compression,
    )
    .await
}

async fn send_file(
    session: &mut Session,
    file: &Path,
    metadata: &Metadata,
    head: bool,
    compression: Option<&crate::http_policy::Compression>,
) -> pingora_core::Result<()> {
    let length = metadata.len();
    let modified = metadata.modified().ok().map(truncate_to_seconds);
    let etag = entity_tag(metadata);
    let last_modified = modified.map(httpdate::fmt_http_date);
    let request = session.req_header();
    let header_text = |name| {
        request
            .headers
            .get(name)
            .and_then(|value: &http::HeaderValue| value.to_str().ok())
    };
    // RFC 9110 §13.2.2 evaluation order.
    let precondition_failed = match header_text(header::IF_MATCH) {
        Some(value) => !matches_list(value, &etag, true),
        None => header_text(header::IF_UNMODIFIED_SINCE)
            .and_then(|value| httpdate::parse_http_date(value).ok())
            .zip(modified)
            .is_some_and(|(since, modified)| modified > since),
    };
    if precondition_failed {
        return responses::plain(session, 412, "precondition failed", &[]).await;
    }
    let not_modified = match header_text(header::IF_NONE_MATCH) {
        Some(value) => matches_list(value, &etag, false),
        None => header_text(header::IF_MODIFIED_SINCE)
            .and_then(|value| httpdate::parse_http_date(value).ok())
            .zip(modified)
            .is_some_and(|(since, modified)| modified <= since),
    };
    let range = header_text(header::RANGE)
        .filter(|_| {
            header_text(header::IF_RANGE).is_none_or(|validator| {
                validator == etag && !etag.starts_with("W/")
                    || last_modified.as_deref() == Some(validator)
            })
        })
        .map(|value| parse_range(value, length));

    let mut response = ResponseHeader::build(200, Some(8))?;
    response.insert_header(header::ETAG, &etag)?;
    if let Some(last_modified) = &last_modified {
        response.insert_header(header::LAST_MODIFIED, last_modified)?;
    }
    response.insert_header(header::ACCEPT_RANGES, "bytes")?;
    if not_modified {
        response.set_status(304)?;
        return session
            .write_response_header(Box::new(response), true)
            .await;
    }
    let (start, end) = match range {
        Some(Range::Unsatisfiable) => {
            return responses::plain(
                session,
                416,
                "range not satisfiable",
                &[(header::CONTENT_RANGE, &format!("bytes */{length}"))],
            )
            .await;
        }
        Some(Range::Bytes(start, end)) => {
            response.set_status(206)?;
            response.insert_header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{length}"),
            )?;
            (start, end + 1)
        }
        Some(Range::Ignored) | None => (0, length),
    };
    response.insert_header(header::CONTENT_TYPE, content_type(file))?;
    response.insert_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")?;
    response.insert_header(header::CONTENT_LENGTH, (end - start).to_string())?;
    if head || start == end {
        return session
            .write_response_header(Box::new(response), true)
            .await;
    }
    let mut handle = match tokio::fs::File::open(file).await {
        Ok(handle) => handle,
        Err(_) => return responses::plain(session, 404, "not found", &[]).await,
    };
    if start > 0 && handle.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return responses::plain(session, 500, "read failed", &[]).await;
    }
    if let Some(compression) = compression {
        if start == 0 && end == length {
            compression.decide(session, &mut response)?;
        } else {
            crate::http_policy::disable_compression(session);
        }
    }
    session
        .write_response_header(Box::new(response), false)
        .await?;
    let mut remaining = end - start;
    while remaining > 0 {
        let size = remaining.min(CHUNK_BYTES) as usize;
        let mut buffer = BytesMut::zeroed(size);
        let read = handle.read(&mut buffer).await.map_err(|error| {
            pingora_core::Error::because(
                pingora_core::ErrorType::ReadError,
                "static file read failed",
                error,
            )
        })?;
        if read == 0 {
            return pingora_core::Error::e_explain(
                pingora_core::ErrorType::ReadError,
                "static file shrank while it was sent",
            );
        }
        buffer.truncate(read);
        remaining -= read as u64;
        session
            .write_response_body(Some(Bytes::from(buffer)), remaining == 0)
            .await?;
    }
    Ok(())
}

/// Percent-decodes each segment; rejects segments that could leave the root.
fn decode_components(path: &str) -> Option<Vec<String>> {
    let mut components = Vec::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        let decoded = percent_encoding::percent_decode_str(segment)
            .decode_utf8()
            .ok()?;
        if decoded == "." || decoded == ".." || decoded.contains(['/', '\\', '\0']) {
            return None;
        }
        components.push(decoded.into_owned());
    }
    Some(components)
}

fn is_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

fn truncate_to_seconds(time: SystemTime) -> SystemTime {
    time.duration_since(UNIX_EPOCH).map_or(time, |elapsed| {
        UNIX_EPOCH + std::time::Duration::from_secs(elapsed.as_secs())
    })
}

fn entity_tag(metadata: &Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("\"{modified:x}-{:x}\"", metadata.len())
}

/// RFC 9110 §8.8.3.2: strong comparison for `If-Match`, weak otherwise.
fn matches_list(value: &str, etag: &str, strong: bool) -> bool {
    if value.trim() == "*" {
        return true;
    }
    value.split(',').map(str::trim).any(|candidate| {
        let weak = candidate.starts_with("W/");
        let opaque = candidate.trim_start_matches("W/");
        opaque == etag.trim_start_matches("W/") && !(strong && (weak || etag.starts_with("W/")))
    })
}

#[derive(Debug, Eq, PartialEq)]
enum Range {
    Bytes(u64, u64),
    Unsatisfiable,
    /// Multiple or malformed ranges are answered with the whole representation.
    Ignored,
}

fn parse_range(value: &str, length: u64) -> Range {
    let Some(spec) = value
        .split_once('=')
        .filter(|(unit, _)| unit.trim().eq_ignore_ascii_case("bytes"))
        .map(|(_, spec)| spec.trim())
    else {
        return Range::Ignored;
    };
    if spec.contains(',') {
        return Range::Ignored;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return Range::Ignored;
    };
    let parse = |text: &str| text.trim().parse::<u64>().ok();
    match (first.trim().is_empty(), parse(first), parse(last)) {
        (true, _, Some(suffix)) => {
            if suffix == 0 || length == 0 {
                Range::Unsatisfiable
            } else {
                Range::Bytes(length.saturating_sub(suffix), length - 1)
            }
        }
        (false, Some(start), None) if last.trim().is_empty() => {
            if start >= length {
                Range::Unsatisfiable
            } else {
                Range::Bytes(start, length - 1)
            }
        }
        (false, Some(start), Some(end)) if start <= end => {
            if start >= length {
                Range::Unsatisfiable
            } else {
                Range::Bytes(start, end.min(length - 1))
            }
        }
        _ => Range::Ignored,
    }
}

fn content_type(file: &Path) -> String {
    let mime = mime_guess::from_path(file).first_or_octet_stream();
    if mime.type_() == mime_guess::mime::TEXT
        || matches!(mime.subtype().as_str(), "javascript" | "json" | "xml")
    {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_follow_rfc_9110() {
        assert_eq!(parse_range("bytes=0-99", 1000), Range::Bytes(0, 99));
        assert_eq!(parse_range("bytes=900-", 1000), Range::Bytes(900, 999));
        assert_eq!(parse_range("bytes=-100", 1000), Range::Bytes(900, 999));
        assert_eq!(parse_range("bytes=-5000", 1000), Range::Bytes(0, 999));
        assert_eq!(parse_range("bytes=990-2000", 1000), Range::Bytes(990, 999));
        assert_eq!(parse_range("bytes=1000-", 1000), Range::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 1000), Range::Unsatisfiable);
        assert_eq!(parse_range("bytes=5-1", 1000), Range::Ignored);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), Range::Ignored);
        assert_eq!(parse_range("items=0-1", 1000), Range::Ignored);
    }

    #[test]
    fn entity_tag_comparison_is_strong_or_weak() {
        let etag = "\"abc\"";
        assert!(matches_list("\"abc\"", etag, true));
        assert!(!matches_list("W/\"abc\"", etag, true));
        assert!(matches_list("W/\"abc\"", etag, false));
        assert!(matches_list("\"x\", \"abc\"", etag, false));
        assert!(matches_list("*", etag, true));
        assert!(!matches_list("\"other\"", etag, false));
    }

    #[test]
    fn traversal_segments_are_rejected() {
        assert_eq!(
            decode_components("/a/b%20c/").unwrap(),
            vec!["a".to_owned(), "b c".to_owned()]
        );
        assert!(decode_components("/%2e%2e/secret").is_none());
        assert!(decode_components("/a%2Fb").is_none());
        assert!(decode_components("/a%00").is_none());
        assert!(decode_components("/%ff").is_none());
    }

    #[test]
    fn roots_must_stay_inside_the_base() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir(base.path().join("site")).unwrap();
        let policy = |root: &str| StaticContentPolicy {
            id: "static".into(),
            root: root.into(),
            index_files: vec!["index.html".into()],
            spa_fallback: false,
        };
        assert!(StaticContent::compile(&policy("site"), Some(base.path())).is_ok());
        for root in ["../site", "/etc", "", "site/../site"] {
            assert!(
                StaticContent::compile(&policy(root), Some(base.path())).is_err(),
                "{root}"
            );
        }
        assert!(StaticContent::compile(&policy("site"), None).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_root_that_is_not_there_yet_serves_nothing_until_it_is() {
        use std::os::unix::fs::symlink;
        let base = tempfile::tempdir().unwrap();
        let policy = |root: &str| StaticContentPolicy {
            id: "static".into(),
            root: root.into(),
            index_files: vec!["index.html".into()],
            spa_fallback: false,
        };
        let content = StaticContent::compile(&policy("later"), Some(base.path())).unwrap();
        let page = vec!["page.html".to_owned()];
        assert!(matches!(content.locate(&page).await, Located::Missing));
        std::fs::create_dir(base.path().join("later")).unwrap();
        std::fs::write(base.path().join("later/page.html"), "page").unwrap();
        assert!(matches!(content.locate(&page).await, Located::File(..)));

        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("page.html"), "secret").unwrap();
        let linked = StaticContent::compile(&policy("linked"), Some(base.path())).unwrap();
        symlink(outside.path(), base.path().join("linked")).unwrap();
        assert!(
            matches!(linked.locate(&page).await, Located::Missing),
            "a root made a link out later still leads nowhere"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symbolic_links_are_followed_only_inside_the_root() {
        use std::os::unix::fs::symlink;
        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        let site = base.path().join("site");
        std::fs::create_dir_all(site.join("docs")).unwrap();
        std::fs::write(site.join("docs/page.html"), "page").unwrap();
        symlink(site.join("docs/page.html"), site.join("alias.html")).unwrap();
        symlink(outside.path().join("secret.txt"), site.join("escape.txt")).unwrap();
        symlink(outside.path(), site.join("elsewhere")).unwrap();
        symlink(outside.path(), base.path().join("linked-root")).unwrap();
        let policy = |root: &str| StaticContentPolicy {
            id: "static".into(),
            root: root.into(),
            index_files: vec!["index.html".into()],
            spa_fallback: false,
        };
        let content = StaticContent::compile(&policy("site"), Some(base.path())).unwrap();
        let located = |path: &'static str| {
            let components: Vec<String> = path.split('/').map(str::to_owned).collect();
            let content = &content;
            async move { content.locate(&components).await }
        };
        assert!(matches!(located("alias.html").await, Located::File(..)));
        assert!(matches!(located("escape.txt").await, Located::Missing));
        assert!(matches!(
            located("elsewhere/secret.txt").await,
            Located::Missing
        ));
        assert!(
            StaticContent::compile(&policy("linked-root"), Some(base.path())).is_err(),
            "a root may not lead out of the static content root"
        );
    }
}
