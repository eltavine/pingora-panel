//! Engine-neutral checks for logging settings (ADR 0025).

use panel_errors::{Diagnostic, ErrorCode};
use panel_ir::{
    logging::{is_field_name, LOGGING_CAPABILITY},
    template::parse_template,
    AccessLog, RuntimeSnapshot,
};

/// The smallest size a log file may rotate at.
pub const MIN_LOG_FILE_BYTES: u64 = 64 * 1024;
/// The longest rotated files may be kept, in days.
pub const MAX_LOG_KEEP_DAYS: u32 = 3650;
/// The most rotated files kept of one log.
pub const MAX_LOG_FILES: u32 = 10_000;
/// The most extra fields one scope may add.
pub const MAX_LOG_FIELDS: usize = 32;
/// The longest template of an extra field.
const MAX_FIELD_TEMPLATE: usize = 1024;

fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"!#$%&'*+-.^_`|~".contains(&byte)
        })
}

fn check_access(access: &AccessLog, resource: &str, report: &mut impl FnMut(&str, String)) {
    if access.fields.len() > MAX_LOG_FIELDS {
        report(
            resource,
            format!("{resource} adds more than {MAX_LOG_FIELDS} log fields"),
        );
    }
    for (name, template) in &access.fields {
        if !is_field_name(name) {
            report(
                resource,
                format!(
                    "log field {name:?} of {resource} must be lowercase words joined by dots \
                     and not a name records use"
                ),
            );
        }
        if template.len() > MAX_FIELD_TEMPLATE {
            report(
                resource,
                format!("log field {name} of {resource} is longer than {MAX_FIELD_TEMPLATE} bytes"),
            );
        } else if let Err(error) = parse_template(template) {
            report(
                resource,
                format!("log field {name} of {resource} has an invalid template: {error}"),
            );
        }
    }
}

pub(crate) fn validate_logging(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    let configured = !snapshot.logging.is_default()
        || snapshot
            .sites
            .iter()
            .any(|site| !site.access_log.is_unset())
        || snapshot
            .routes
            .iter()
            .any(|route| !route.access_log.is_unset());
    if !configured {
        return;
    }
    let mut report = |resource: &str, message: String| {
        diagnostics
            .push(Diagnostic::error(ErrorCode::VALIDATION_FAILED, message).with_resource(resource));
    };
    if !snapshot
        .required_capabilities()
        .iter()
        .any(|capability| capability.name == LOGGING_CAPABILITY)
    {
        report(
            "logging",
            format!("logging settings require {LOGGING_CAPABILITY}"),
        );
    }
    let policy = &snapshot.logging;
    check_access(&policy.access, "logging", &mut report);
    for site in &snapshot.sites {
        check_access(&site.access_log, site.id.as_str(), &mut report);
    }
    for route in &snapshot.routes {
        check_access(&route.access_log, route.id.as_str(), &mut report);
    }
    let files = &policy.files;
    if files.max_size_bytes < MIN_LOG_FILE_BYTES {
        report(
            "logging",
            format!("log files must be allowed at least {MIN_LOG_FILE_BYTES} bytes"),
        );
    }
    if files.keep_days > MAX_LOG_KEEP_DAYS {
        report(
            "logging",
            format!("log files are kept at most {MAX_LOG_KEEP_DAYS} days"),
        );
    }
    if files.max_files > MAX_LOG_FILES {
        report(
            "logging",
            format!("at most {MAX_LOG_FILES} rotated files are kept of a log"),
        );
    }
    for name in &policy.redact_headers {
        if !is_header_name(name) {
            report(
                "logging",
                format!("{name:?} is not a lowercase header name"),
            );
        }
    }
    for key in policy.redact_query.iter().flatten() {
        if key.is_empty() || key.contains(['&', '=', '#']) || key.contains(char::is_control) {
            report("logging", format!("{key:?} is not a query parameter name"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{RevisionId, SiteId};
    use panel_ir::{CapabilityRequirement, SiteSpec};

    fn messages(snapshot: &RuntimeSnapshot) -> Vec<String> {
        let mut diagnostics = Vec::new();
        validate_logging(snapshot, &mut diagnostics);
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    fn logging_snapshot() -> RuntimeSnapshot {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot
            .required_capabilities
            .push(CapabilityRequirement::new(LOGGING_CAPABILITY, "1"));
        snapshot
    }

    #[test]
    fn snapshots_without_logging_settings_need_no_capability() {
        assert!(messages(&RuntimeSnapshot::empty(RevisionId::new(1))).is_empty());
    }

    #[test]
    fn logging_settings_require_the_capability() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        let mut site = SiteSpec::new(SiteId::new("site").unwrap(), "site", Vec::new());
        site.access_log.enabled = Some(false);
        snapshot.sites.push(site);
        assert_eq!(
            messages(&snapshot),
            vec![format!("logging settings require {LOGGING_CAPABILITY}")]
        );
        snapshot
            .required_capabilities
            .push(CapabilityRequirement::new(LOGGING_CAPABILITY, "1"));
        assert!(messages(&snapshot).is_empty());
    }

    #[test]
    fn fields_files_and_redaction_lists_are_checked() {
        let mut snapshot = logging_snapshot();
        let policy = &mut snapshot.logging;
        policy
            .access
            .fields
            .insert("tenant".into(), "$http_x_tenant".into());
        policy
            .access
            .fields
            .insert("url.path".into(), "$uri".into());
        policy.access.fields.insert("broken".into(), "${".into());
        policy.files.max_size_bytes = 1024;
        policy.files.keep_days = MAX_LOG_KEEP_DAYS + 1;
        policy.files.max_files = MAX_LOG_FILES + 1;
        policy.redact_headers.insert("X-Api-Key".into());
        policy.redact_query = Some(["a=b".to_owned()].into());
        let found = messages(&snapshot);
        for expected in [
            "log field \"url.path\"",
            "log field broken of logging has an invalid template",
            "at least 65536 bytes",
            "kept at most 3650 days",
            "at most 10000 rotated files",
            "\"X-Api-Key\" is not a lowercase header name",
            "\"a=b\" is not a query parameter name",
        ] {
            assert!(
                found.iter().any(|message| message.contains(expected)),
                "{expected} in {found:?}"
            );
        }
        assert_eq!(found.len(), 7, "{found:?}");
    }
}
