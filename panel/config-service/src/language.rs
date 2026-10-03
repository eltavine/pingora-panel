//! The draft written in the configuration language, kept in step with the
//! model it describes.

use chrono::{DateTime, Utc};
use panel_config_dsl::{
    lower, print, reconcile, variables::ENVIRONMENT_PREFIX, write_identifiers, LowerOptions,
    Lowered, Sources,
};
use panel_config_model::{validate, ConfigModel};
use panel_domain::ContentHash;
use panel_errors::{Diagnostic, DiagnosticSeverity, ErrorCode, PanelError, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

/// The environment `${env:NAME}` reads, captured once at startup.
fn environment() -> &'static BTreeMap<String, String> {
    static ENVIRONMENT: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    ENVIRONMENT.get_or_init(|| {
        std::env::vars()
            .filter(|(name, _)| name.starts_with(ENVIRONMENT_PREFIX))
            .collect()
    })
}

/// Reads `sources`, carrying metadata over from `previous`.
pub(crate) fn read(
    sources: &Sources,
    previous: Option<&ConfigModel>,
    now: DateTime<Utc>,
) -> Lowered {
    lower(
        sources,
        &LowerOptions {
            environment: environment(),
            previous,
            now,
        },
    )
}

/// The files of a draft stored before the language existed.
pub(crate) fn printed(model: &ConfigModel) -> Sources {
    Sources::single(print(model))
}

/// What the language expresses of a model: timestamps aside, and domains in
/// a stable order.
fn configuration(model: &ConfigModel) -> ConfigModel {
    let mut model = model.clone();
    for site in &mut model.sites {
        site.created_at = DateTime::UNIX_EPOCH;
        site.updated_at = DateTime::UNIX_EPOCH;
        site.domains.sort_by(|a, b| a.host.cmp(&b.host));
    }
    for upstream in &mut model.upstreams {
        upstream.created_at = DateTime::UNIX_EPOCH;
        upstream.updated_at = DateTime::UNIX_EPOCH;
    }
    model
}

/// The draft's files after a change through the API produced `next`: only
/// the blocks of resources that changed are rewritten. Should the result not
/// read back as `next`, the files are printed from `next` instead, so the
/// two never disagree.
pub(crate) fn follow(sources: &Sources, current: &ConfigModel, next: &ConfigModel) -> Sources {
    let now = Utc::now();
    let lowered = read(sources, Some(current), now);
    let edited = reconcile(sources, &lowered, next);
    let check = read(&edited, Some(next), now);
    if check.is_valid() && configuration(&check.model) == configuration(next) {
        edited
    } else {
        tracing::warn!(
            event = "draft_sources_reprinted",
            "the draft's files did not read back as the edited model; printing them afresh"
        );
        printed(next)
    }
}

/// A draft replaced by text: the model it describes, and the files with every
/// identifier the service assigned written in. Errors of the language refuse
/// the text; validation errors refuse it only when the current draft did not
/// already have them, as for any other change.
pub(crate) fn replace(
    sources: &Sources,
    current: &ConfigModel,
) -> Result<(ConfigModel, Sources, Vec<Diagnostic>)> {
    let lowered = read(sources, Some(current), Utc::now());
    let existing: BTreeSet<(Option<String>, String)> = validate(current)
        .into_iter()
        .map(|diagnostic| (diagnostic.resource_id, diagnostic.message))
        .collect();
    let blocking: Vec<Diagnostic> = lowered
        .errors()
        .filter(|diagnostic| {
            diagnostic.code.as_str() != ErrorCode::VALIDATION_FAILED
                || !existing.contains(&(diagnostic.resource_id.clone(), diagnostic.message.clone()))
        })
        .cloned()
        .collect();
    if !blocking.is_empty() {
        return Err(
            PanelError::validation_failed("the configuration has errors")
                .with_diagnostics(blocking),
        );
    }
    let warnings = lowered
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
        .cloned()
        .collect();
    let written = write_identifiers(sources, &lowered.insertions);
    Ok((lowered.model, written, warnings))
}

/// Identifies a set of files: paths and contents, in path order.
pub(crate) fn content_hash(sources: &Sources) -> ContentHash {
    let mut bytes = Vec::new();
    for (path, text) in sources.files() {
        for part in [path.as_bytes(), text.as_bytes()] {
            bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
            bytes.extend_from_slice(part);
        }
    }
    ContentHash::from_bytes(&bytes)
}

/// Files with their paths checked, from a JSON object of path to text.
pub(crate) fn sources(files: BTreeMap<String, String>) -> Result<Sources> {
    Sources::new(files).map_err(|path| {
        PanelError::invalid_argument(format!(
            "{path:?} is not a relative path inside the configuration"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "language_version 1;\nhttp {\n    # the pool\n    upstream app {\n        server 10.0.0.1:80;\n    }\n}\n";

    #[test]
    fn text_replaces_the_model_and_gains_identifiers() {
        let (model, written, warnings) =
            replace(&Sources::single(TEXT), &ConfigModel::default()).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(model.upstreams.len(), 1);
        let text = written.get("main.conf").unwrap();
        assert!(
            text.contains("    # the pool\n    upstream app {\n        id "),
            "{text}"
        );
        let (again, rewritten, _) = replace(&written, &model).unwrap();
        assert_eq!(rewritten, written);
        assert_eq!(again.upstreams[0].id, model.upstreams[0].id);
    }

    #[test]
    fn errors_refuse_the_text() {
        let error = replace(
            &Sources::single("language_version 1;\nhttp { upstream {} }\n"),
            &ConfigModel::default(),
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::VALIDATION_FAILED);
        assert!(!error.diagnostics.is_empty());
    }

    #[test]
    fn edits_follow_the_model_and_keep_comments() {
        let (model, written, _) = replace(&Sources::single(TEXT), &ConfigModel::default()).unwrap();
        let mut next = model.clone();
        next.upstreams[0].note = Some("primary".into());
        let followed = follow(&written, &model, &next);
        let text = followed.get("main.conf").unwrap();
        assert!(
            text.contains("    # the pool\n") && text.contains("note primary;"),
            "{text}"
        );
    }

    #[test]
    fn hashes_cover_paths_and_contents() {
        let mut a = Sources::single("x");
        let b = Sources::single("x");
        assert_eq!(content_hash(&a), content_hash(&b));
        a.insert("main.conf", "y");
        assert_ne!(content_hash(&a), content_hash(&b));
    }
}
