//! The files of one configuration, addressed by relative paths, with
//! `main.conf` as the entry point.

use globset::GlobBuilder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ENTRY: &str = "main.conf";
const MAX_PATH: usize = 255;

/// A configuration's files by path. Paths are relative, use `/`, and contain
/// no empty, `.` or `..` segment.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(transparent)]
pub struct Sources {
    files: BTreeMap<String, String>,
}

impl Sources {
    /// A configuration made of `main.conf` alone.
    pub fn single(text: impl Into<String>) -> Self {
        let mut files = BTreeMap::new();
        files.insert(ENTRY.to_owned(), text.into());
        Self { files }
    }

    /// Files with the paths checked; returns the first invalid path.
    pub fn new(files: BTreeMap<String, String>) -> Result<Self, String> {
        if let Some(path) = files.keys().find(|path| !valid_path(path)) {
            return Err(path.clone());
        }
        Ok(Self { files })
    }

    pub fn get(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    pub fn files(&self) -> impl Iterator<Item = (&str, &str)> {
        self.files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
    }

    pub fn into_files(self) -> BTreeMap<String, String> {
        self.files
    }

    pub fn insert(&mut self, path: impl Into<String>, text: impl Into<String>) {
        self.files.insert(path.into(), text.into());
    }

    pub fn remove(&mut self, path: &str) -> Option<String> {
        self.files.remove(path)
    }

    pub fn has_entry(&self) -> bool {
        self.files.contains_key(ENTRY)
    }

    /// Files an `include` in `from` names, in sorted order. A pattern is
    /// relative to the including file's directory; a plain path that does not
    /// exist is reported by the caller, while a glob may match nothing.
    pub fn resolve(&self, from: &str, pattern: &str) -> Result<Vec<String>, String> {
        let directory = from.rsplit_once('/').map_or("", |(directory, _)| directory);
        let joined = if directory.is_empty() {
            pattern.to_owned()
        } else {
            format!("{directory}/{pattern}")
        };
        if !valid_path(&joined) {
            return Err(format!(
                "{pattern:?} is not a relative path inside the configuration"
            ));
        }
        let glob = GlobBuilder::new(&joined)
            .literal_separator(true)
            .build()
            .map_err(|error| format!("{pattern:?} is not a valid pattern: {error}"))?
            .compile_matcher();
        Ok(self
            .files
            .keys()
            .filter(|path| glob.is_match(path.as_str()))
            .cloned()
            .collect())
    }
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && !path.contains('\\')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// Whether `path` is Lua code, which the configuration runs rather than
/// reads.
pub fn is_lua(path: &str) -> bool {
    path.ends_with(".lua")
}

pub(crate) fn is_glob(pattern: &str) -> bool {
    pattern.contains(['*', '?', '[', '{'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Sources {
        let files = [
            ("main.conf", ""),
            ("sites/a.conf", ""),
            ("sites/b.conf", ""),
            ("sites/old/c.conf", ""),
            ("upstreams.conf", ""),
        ]
        .into_iter()
        .map(|(path, text)| (path.to_owned(), text.to_owned()))
        .collect();
        Sources::new(files).unwrap()
    }

    #[test]
    fn includes_resolve_relative_to_the_including_file() {
        let sources = sources();
        assert_eq!(
            sources.resolve("main.conf", "sites/*.conf").unwrap(),
            ["sites/a.conf", "sites/b.conf"]
        );
        assert_eq!(
            sources.resolve("sites/a.conf", "b.conf").unwrap(),
            ["sites/b.conf"]
        );
        assert_eq!(
            sources
                .resolve("main.conf", "sites/**/*.conf")
                .unwrap()
                .len(),
            3
        );
        assert!(sources
            .resolve("main.conf", "missing.conf")
            .unwrap()
            .is_empty());
        assert!(sources.resolve("sites/a.conf", "../main.conf").is_err());
        assert!(sources.resolve("main.conf", "/etc/passwd").is_err());
    }

    #[test]
    fn paths_must_stay_inside_the_configuration() {
        for invalid in [
            "",
            "/abs.conf",
            "a/../b.conf",
            "a//b.conf",
            "./a.conf",
            "a\\b.conf",
        ] {
            let files = [(invalid.to_owned(), String::new())].into_iter().collect();
            assert!(Sources::new(files).is_err(), "{invalid:?}");
        }
    }
}
