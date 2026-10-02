#![forbid(unsafe_code)]

use panel_bootstrap::{PkiPlan, PKI_CREDENTIALS_ENV, PKI_DIR_ENV};
use panel_service::Environment;
use std::{collections::HashMap, ffi::OsString};

fn plan(values: Vec<(&'static str, String)>) -> panel_errors::Result<PkiPlan> {
    let values: HashMap<&str, OsString> = values
        .into_iter()
        .map(|(key, value)| (key, OsString::from(value)))
        .collect();
    PkiPlan::read(&mut Environment::from_lookup(move |name| {
        values.get(name).cloned()
    }))
}

#[test]
fn the_authority_issues_listed_services_once_until_renewal_is_due() {
    let root = std::env::temp_dir().join(format!("panel-bootstrap-pki-{}", std::process::id()));
    let plan = plan(vec![
        (PKI_DIR_ENV, root.join("authority").display().to_string()),
        (
            PKI_CREDENTIALS_ENV,
            format!(
                "panel-api={},config-service={}",
                root.join("panel-api").display(),
                root.join("config-service").display()
            ),
        ),
    ])
    .unwrap();
    let issued = plan.apply().unwrap();
    assert_eq!(
        issued.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["panel-api", "config-service"]
    );
    assert!(root.join("panel-api/identity.pem").is_file());
    assert!(root.join("config-service/trust.pem").is_file());
    assert!(
        plan.apply().unwrap().is_empty(),
        "current credentials are kept"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn credential_targets_name_a_service_and_a_directory() {
    for invalid in ["panel-api", "Panel=/tmp/x"] {
        assert!(
            plan(vec![
                (PKI_DIR_ENV, "/tmp/authority".into()),
                (PKI_CREDENTIALS_ENV, invalid.into()),
            ])
            .is_err(),
            "{invalid}"
        );
    }
    assert!(plan(vec![(PKI_DIR_ENV, "/tmp/authority".into())]).is_err());
}
