#![forbid(unsafe_code)]

//! Backup archives (ADR 0035): a tar archive compressed with Zstandard whose
//! first member, `manifest.json`, lists every other member with its size and
//! SHA-256, so an archive is checked before anything is restored from it.

use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Read, Write},
    path::{Component, Path},
};

/// What an archive's manifest says it is.
pub const FORMAT: &str = "pingora-panel-backup";
/// The newest archive layout this build writes and reads.
pub const FORMAT_VERSION: u32 = 1;
/// The archive's first member.
pub const MANIFEST: &str = "manifest.json";
/// Manifests larger than this are refused unread.
const MOST_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const LEVEL: i32 = 3;

/// What an archive holds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    /// The version of the product that wrote the archive.
    pub product_version: String,
    pub created_at: DateTime<Utc>,
    /// What the archive was taken to hold, such as `configuration`.
    pub contents: Vec<String>,
    /// Every directory, so that empty ones come back too.
    pub directories: Vec<String>,
    pub members: Vec<Member>,
}

/// A file in an archive.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Member {
    /// Its path in the archive: relative, `/`-separated names.
    pub path: String,
    pub size: u64,
    /// SHA-256 of its content, in lowercase hexadecimal.
    pub sha256: String,
}

impl Manifest {
    pub fn member(&self, path: &str) -> Option<&Member> {
        self.members.iter().find(|member| member.path == path)
    }

    /// The bytes of every member together.
    pub fn size(&self) -> u64 {
        self.members.iter().map(|member| member.size).sum()
    }
}

/// What extracting part of an archive wrote.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Extraction {
    pub files: u64,
    pub bytes: u64,
}

/// Writes the files and directories below `staging` as an archive at
/// `destination`, which must not exist yet, and returns its manifest. A
/// failed write leaves no archive behind.
pub fn write(
    destination: &Path,
    staging: &Path,
    product_version: &str,
    contents: &[&str],
    created_at: DateTime<Utc>,
) -> Result<Manifest> {
    let mut directories = Vec::new();
    let mut members = Vec::new();
    for entry in walkdir::WalkDir::new(staging)
        .min_depth(1)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.map_err(|error| storage(error.into()))?;
        let path = archive_path(
            entry
                .path()
                .strip_prefix(staging)
                .expect("entries are below the staging directory"),
        )?;
        if entry.file_type().is_dir() {
            directories.push(path);
        } else if entry.file_type().is_file() {
            let (size, sha256) = digest(File::open(entry.path()).map_err(storage)?)?;
            members.push(Member { path, size, sha256 });
        } else {
            return Err(PanelError::invalid_argument(format!(
                "{path} is neither a file nor a directory"
            )));
        }
    }
    let manifest = Manifest {
        format: FORMAT.to_owned(),
        format_version: FORMAT_VERSION,
        product_version: product_version.to_owned(),
        created_at,
        contents: contents.iter().map(|&kind| kind.to_owned()).collect(),
        directories,
        members,
    };
    let file = create_new(destination)?;
    let written = pack(file, staging, &manifest);
    if written.is_err() {
        let _ = fs::remove_file(destination);
    }
    written.map(|()| manifest)
}

fn pack(file: File, staging: &Path, manifest: &Manifest) -> Result<()> {
    let encoder = zstd::Encoder::new(BufWriter::new(file), LEVEL).map_err(storage)?;
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    let mtime = u64::try_from(manifest.created_at.timestamp()).unwrap_or_default();
    let listed = serde_json::to_vec_pretty(manifest).expect("manifests serialize");
    let size = listed.len() as u64;
    append(
        &mut builder,
        MANIFEST,
        tar::EntryType::Regular,
        size,
        0o600,
        mtime,
        &listed[..],
    )?;
    for directory in &manifest.directories {
        append(
            &mut builder,
            directory,
            tar::EntryType::Directory,
            0,
            0o755,
            mtime,
            io::empty(),
        )?;
    }
    for member in &manifest.members {
        let file = File::open(staging.join(&member.path)).map_err(storage)?;
        let modified = file
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(mtime, |since| since.as_secs());
        let content = file.take(member.size);
        append(
            &mut builder,
            &member.path,
            tar::EntryType::Regular,
            member.size,
            0o644,
            modified,
            content,
        )?;
    }
    let encoder = builder.into_inner().map_err(storage)?;
    let file = encoder
        .finish()
        .map_err(storage)?
        .into_inner()
        .map_err(|error| storage(error.into_error()))?;
    file.sync_all().map_err(storage)
}

fn append<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    kind: tar::EntryType,
    size: u64,
    mode: u32,
    mtime: u64,
    content: impl Read,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(size);
    header.set_mode(mode);
    header.set_mtime(mtime);
    builder
        .append_data(&mut header, path, content)
        .map_err(storage)
}

type Entries = tar::Archive<zstd::Decoder<'static, io::BufReader<File>>>;

fn open(archive: &Path) -> Result<Entries> {
    let file = File::open(archive).map_err(storage)?;
    let decoder = zstd::Decoder::new(file).map_err(|error| damaged(error.to_string()))?;
    Ok(tar::Archive::new(decoder))
}

/// Reads the manifest at the head of an archive without reading the rest.
pub fn manifest(archive: &Path) -> Result<Manifest> {
    let mut archive = open(archive)?;
    let mut entries = archive
        .entries()
        .map_err(|error| damaged(error.to_string()))?;
    head(&mut entries)
}

fn head<R: Read>(entries: &mut tar::Entries<'_, R>) -> Result<Manifest> {
    let mut entry = entries
        .next()
        .ok_or_else(|| damaged("it is empty"))?
        .map_err(|error| damaged(error.to_string()))?;
    if entry.path_bytes().as_ref() != MANIFEST.as_bytes() {
        return Err(damaged("it does not begin with its manifest"));
    }
    if entry.size() > MOST_MANIFEST_BYTES {
        return Err(damaged("its manifest is too large"));
    }
    let mut listed = Vec::new();
    entry
        .by_ref()
        .take(MOST_MANIFEST_BYTES)
        .read_to_end(&mut listed)
        .map_err(|error| damaged(error.to_string()))?;
    let manifest: Manifest = serde_json::from_slice(&listed)
        .map_err(|error| damaged(format!("its manifest is unreadable: {error}")))?;
    if manifest.format != FORMAT {
        return Err(damaged("it is not a backup"));
    }
    if manifest.format_version > FORMAT_VERSION {
        return Err(PanelError::validation_failed(format!(
            "the archive is in layout {}, newer than this version reads ({FORMAT_VERSION})",
            manifest.format_version
        )));
    }
    let mut paths = BTreeSet::new();
    for path in manifest
        .directories
        .iter()
        .chain(manifest.members.iter().map(|member| &member.path))
    {
        checked(path)?;
        if !paths.insert(path.as_str()) {
            return Err(damaged(format!("{path} is listed twice")));
        }
    }
    Ok(manifest)
}

/// Checks every member against the manifest: each listed once with its size
/// and digest, and nothing else in the archive.
pub fn verify(archive: &Path) -> Result<Manifest> {
    let mut archive = open(archive)?;
    let mut entries = archive
        .entries()
        .map_err(|error| damaged(error.to_string()))?;
    let manifest = head(&mut entries)?;
    walk(entries, &manifest, |_, _, content| {
        io::copy(content, &mut io::sink()).map(|_| ())
    })?;
    Ok(manifest)
}

/// Extracts the files and directories at and below `prefix` into
/// `destination`, which must not exist yet, checking each against the
/// manifest. Nothing is left behind when the archive is damaged.
pub fn extract(archive: &Path, prefix: &str, destination: &Path) -> Result<Extraction> {
    checked(prefix)?;
    let mut archive = open(archive)?;
    let mut entries = archive
        .entries()
        .map_err(|error| damaged(error.to_string()))?;
    let manifest = head(&mut entries)?;
    if !manifest
        .directories
        .iter()
        .any(|directory| directory == prefix)
    {
        return Err(PanelError::not_found(format!(
            "the archive holds no {prefix}"
        )));
    }
    fs::create_dir(destination).map_err(storage)?;
    let mut extraction = Extraction::default();
    let extracted = walk(entries, &manifest, |path, kind, content| {
        let Some(relative) = below(path, prefix) else {
            return io::copy(content, &mut io::sink()).map(|_| ());
        };
        let target = destination.join(relative);
        if kind.is_dir() {
            return fs::create_dir_all(target);
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = open_new(&target, 0o644)?;
        extraction.bytes += io::copy(content, &mut file)?;
        extraction.files += 1;
        file.sync_all()
    });
    if extracted.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    extracted.map(|()| extraction)
}

/// Copies the member at `path` to `destination`, which must not exist yet,
/// checking it against the manifest. Nothing is left behind when the
/// archive is damaged.
pub fn copy_member(archive: &Path, path: &str, destination: &Path) -> Result<u64> {
    let mut archive = open(archive)?;
    let mut entries = archive
        .entries()
        .map_err(|error| damaged(error.to_string()))?;
    let manifest = head(&mut entries)?;
    if manifest.member(path).is_none() {
        return Err(PanelError::not_found(format!(
            "the archive holds no {path}"
        )));
    }
    let mut file = create_new(destination)?;
    let mut copied = 0;
    let walked = walk(entries, &manifest, |name, _, content| {
        if name == path {
            copied = io::copy(content, &mut file)?;
            file.sync_all()
        } else {
            io::copy(content, &mut io::sink()).map(|_| ())
        }
    });
    if walked.is_err() {
        let _ = fs::remove_file(destination);
    }
    walked.map(|()| copied)
}

/// Reads the member at `path`, refusing one larger than `limit` bytes.
pub fn read_member(archive: &Path, path: &str, limit: u64) -> Result<Vec<u8>> {
    let mut archive = open(archive)?;
    let mut entries = archive
        .entries()
        .map_err(|error| damaged(error.to_string()))?;
    let manifest = head(&mut entries)?;
    let member = manifest
        .member(path)
        .ok_or_else(|| PanelError::not_found(format!("the archive holds no {path}")))?;
    if member.size > limit {
        return Err(PanelError::resource_exhausted(format!(
            "{path} is larger than {limit} bytes"
        )));
    }
    let mut read = Vec::new();
    walk(entries, &manifest, |name, _, content| {
        if name == path {
            content.read_to_end(&mut read).map(|_| ())
        } else {
            io::copy(content, &mut io::sink()).map(|_| ())
        }
    })?;
    Ok(read)
}

/// Visits every entry after the manifest, handing `visit` each one's
/// content, and fails when an entry is not listed or not as listed.
fn walk<R: Read>(
    entries: tar::Entries<'_, R>,
    manifest: &Manifest,
    mut visit: impl FnMut(&str, tar::EntryType, &mut dyn Read) -> io::Result<()>,
) -> Result<()> {
    let directories: BTreeSet<&str> = manifest.directories.iter().map(String::as_str).collect();
    let mut unseen: BTreeSet<&str> = manifest
        .members
        .iter()
        .map(|member| member.path.as_str())
        .collect();
    for entry in entries {
        let mut entry = entry.map_err(|error| damaged(error.to_string()))?;
        let path = String::from_utf8(entry.path_bytes().into_owned())
            .map_err(|_| damaged("a member's path is not UTF-8"))?;
        let path = path.trim_end_matches('/').to_owned();
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            if !directories.contains(path.as_str()) {
                return Err(damaged(format!("{path} is not listed")));
            }
            visit(&path, kind, &mut io::empty()).map_err(storage)?;
            continue;
        }
        if !kind.is_file() {
            return Err(damaged(format!("{path} is neither a file nor a directory")));
        }
        let member = manifest
            .member(&path)
            .filter(|_| unseen.remove(path.as_str()))
            .ok_or_else(|| damaged(format!("{path} is not listed, or is there twice")))?;
        if entry.size() != member.size {
            return Err(damaged(format!("{path} is not the size listed")));
        }
        let mut hashing = Hashing::new(entry.by_ref().take(member.size));
        visit(&path, kind, &mut hashing).map_err(storage)?;
        let (size, sha256) = hashing.finish();
        if size != member.size || sha256 != member.sha256 {
            return Err(damaged(format!("{path} is not the content listed")));
        }
    }
    match unseen.first() {
        Some(missing) => Err(damaged(format!("{missing} is missing"))),
        None => Ok(()),
    }
}

/// A reader that hashes what is read through it.
struct Hashing<R> {
    inner: R,
    hasher: Sha256,
    size: u64,
}

impl<R: Read> Hashing<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            size: 0,
        }
    }

    /// Reads what is left, then the size and digest of everything read.
    fn finish(mut self) -> (u64, String) {
        let _ = io::copy(&mut self, &mut io::sink());
        (self.size, hex::encode(self.hasher.finalize()))
    }
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        self.size += read as u64;
        Ok(read)
    }
}

fn digest(content: impl Read) -> Result<(u64, String)> {
    let mut hashing = Hashing::new(content);
    io::copy(&mut hashing, &mut io::sink()).map_err(storage)?;
    Ok(hashing.finish())
}

/// `path` below `prefix`, as a relative file system path.
fn below<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if path == prefix {
        Some(".")
    } else {
        path.strip_prefix(prefix)?.strip_prefix('/')
    }
}

/// `relative` as an archive path: `/`-separated names.
fn archive_path(relative: &Path) -> Result<String> {
    let mut names = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(PanelError::invalid_argument(format!(
                "{} is not a relative path",
                relative.display()
            )));
        };
        names.push(name.to_str().ok_or_else(|| {
            PanelError::invalid_argument(format!("{} is not UTF-8", relative.display()))
        })?);
    }
    let path = names.join("/");
    checked(&path).map_err(|_| {
        PanelError::invalid_argument(format!("{path:?} cannot be kept in an archive"))
    })?;
    Ok(path)
}

/// Refuses a member path that is not relative, `/`-separated names.
fn checked(path: &str) -> Result<()> {
    let fine = !path.is_empty()
        && path.len() <= 4096
        && path.split('/').all(|name| {
            !name.is_empty()
                && name != "."
                && name != ".."
                && !name.contains('\\')
                && !name.chars().any(char::is_control)
        });
    if fine {
        Ok(())
    } else {
        Err(damaged(format!("{path:?} is not a member path")))
    }
}

/// A new file only its owner reads.
fn create_new(path: &Path) -> Result<File> {
    open_new(path, 0o600).map_err(storage)
}

fn open_new(path: &Path, mode: u32) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
    #[cfg(not(unix))]
    let _ = mode;
    options.open(path)
}

fn storage(error: io::Error) -> PanelError {
    PanelError::storage_unavailable(format!("cannot read or write the archive: {error}"))
}

fn damaged(why: impl std::fmt::Display) -> PanelError {
    PanelError::validation_failed(format!("the archive is damaged: {why}"))
}

#[cfg(test)]
mod tests;
