//! What applying a configuration changes: the resources it adds, changes or
//! removes, and the line differences of its files.

use crate::{edit::blocks, Sources};
use panel_config_model::ConfigModel;
use panel_dsl::format_directive;
use serde::Serialize;
use similar::TextDiff;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Change {
    Added,
    Changed,
    Removed,
}

/// One resource that differs, with the difference of its canonical block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ResourceChange {
    /// The resource path, such as `sites/<id>`.
    pub resource: String,
    pub change: Change,
    /// A unified diff of the resource's block.
    pub diff: String,
}

/// One file that differs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FileChange {
    pub path: String,
    pub change: Change,
    /// A unified diff of the file.
    pub diff: String,
}

/// Everything that differs between two configurations.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Changes {
    pub resources: Vec<ResourceChange>,
    pub files: Vec<FileChange>,
}

/// The resource and file changes from `current` to `next`.
pub fn changes(current: (&ConfigModel, &Sources), next: (&ConfigModel, &Sources)) -> Changes {
    Changes {
        resources: plan(current.0, next.0),
        files: diff_files(current.1, next.1),
    }
}

fn unified(old: &str, new: &str, old_name: &str, new_name: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(old_name, new_name)
        .to_string()
}

/// The resources `next` adds, changes and removes relative to `current`, in
/// resource order.
pub fn plan(current: &ConfigModel, next: &ConfigModel) -> Vec<ResourceChange> {
    let render = |model: &ConfigModel| -> BTreeMap<String, String> {
        blocks(model)
            .into_iter()
            .map(|(key, directive)| (key, format_directive(&directive, 0)))
            .collect()
    };
    let (before, after) = (render(current), render(next));
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter_map(|key| {
            let (change, old, new) = match (before.get(key), after.get(key)) {
                (Some(old), Some(new)) if old == new => return None,
                (Some(old), Some(new)) => (Change::Changed, old.as_str(), new.as_str()),
                (Some(old), None) => (Change::Removed, old.as_str(), ""),
                (None, Some(new)) => (Change::Added, "", new.as_str()),
                (None, None) => return None,
            };
            Some(ResourceChange {
                resource: key.clone(),
                change,
                diff: unified(old, new, key, key),
            })
        })
        .collect()
}

/// The files that differ between two configurations.
pub fn diff_files(current: &Sources, next: &Sources) -> Vec<FileChange> {
    let before: BTreeMap<&str, &str> = current.files().collect();
    let after: BTreeMap<&str, &str> = next.files().collect();
    let paths: BTreeSet<&str> = before.keys().chain(after.keys()).copied().collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let (change, old, new) = match (before.get(path), after.get(path)) {
                (Some(old), Some(new)) if old == new => return None,
                (Some(old), Some(new)) => (Change::Changed, *old, *new),
                (Some(old), None) => (Change::Removed, *old, ""),
                (None, Some(new)) => (Change::Added, "", *new),
                (None, None) => return None,
            };
            let name = |side: &str, present: bool| {
                if present {
                    format!("{side}/{path}")
                } else {
                    "/dev/null".to_owned()
                }
            };
            Some(FileChange {
                path: path.to_owned(),
                change,
                diff: unified(
                    old,
                    new,
                    &name("a", change != Change::Added),
                    &name("b", change != Change::Removed),
                ),
            })
        })
        .collect()
}
