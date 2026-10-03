//! Site scopes (ADR 0021): callers who hold a configuration permission only
//! for some sites see and change only those, and nothing shared.

use panel_application::SiteScope;
use panel_config_dsl::plan::plan;
use panel_config_model::ConfigModel;
use panel_errors::{PanelError, Result};
use std::borrow::Cow;

pub(crate) const READ: &str = "config.read";
pub(crate) const WRITE: &str = "config.write";
pub(crate) const APPLY: &str = "config.apply";

/// Whether `scope` limits `permission` to some sites.
fn limits(scope: Option<&SiteScope>, permission: &str) -> bool {
    scope.is_some_and(|scope| !scope.everywhere(permission))
}

/// Refuses callers who hold `permission` only for some sites.
pub(crate) fn require_everywhere(
    scope: Option<&SiteScope>,
    permission: &str,
    what: &str,
) -> Result<()> {
    if limits(scope, permission) {
        return Err(PanelError::permission_denied(format!(
            "{what} needs {permission} for every site"
        )));
    }
    Ok(())
}

/// The model with only the sites the caller may read.
pub(crate) fn readable<'a>(
    model: &'a ConfigModel,
    scope: Option<&SiteScope>,
) -> Cow<'a, ConfigModel> {
    match scope.filter(|scope| !scope.everywhere(READ)) {
        None => Cow::Borrowed(model),
        Some(scope) => {
            let mut visible = model.clone();
            visible
                .sites
                .retain(|site| scope.covers(READ, &site.id.to_string(), site.group.as_deref()));
            Cow::Owned(visible)
        }
    }
}

/// Refuses a change from `before` to `after` that touches anything but
/// sites the caller holds `permission` for, before and after.
pub(crate) fn check_changes(
    before: &ConfigModel,
    after: &ConfigModel,
    scope: Option<&SiteScope>,
    permission: &str,
) -> Result<()> {
    let Some(scope) = scope.filter(|_| limits(scope, permission)) else {
        return Ok(());
    };
    for change in plan(before, after) {
        let Some(id) = change.resource.strip_prefix("sites/") else {
            return Err(PanelError::permission_denied(format!(
                "changing {} needs {permission} for every site",
                change.resource
            )));
        };
        let covered = |model: &ConfigModel| {
            model
                .sites
                .iter()
                .find(|site| site.id.to_string() == id)
                .is_none_or(|site| scope.covers(permission, id, site.group.as_deref()))
        };
        if !covered(before) || !covered(after) {
            return Err(PanelError::permission_denied(format!(
                "the site {id} is outside the sites you may change"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_application::SiteAccess;
    use panel_config_model::{Listener, Site};
    use uuid::Uuid;

    fn site(name: &str, group: Option<&str>) -> Site {
        let mut site: Site = serde_json::from_value(serde_json::json!({
            "id": Uuid::now_v7(),
            "name": name,
            "action": {"type": "respond"},
            "created_at": "2026-10-03T00:00:00Z",
            "updated_at": "2026-10-03T00:00:00Z",
        }))
        .unwrap();
        site.group = group.map(str::to_owned);
        site
    }

    fn shop_writer() -> SiteScope {
        SiteScope {
            unrestricted: Vec::new(),
            limited: vec![
                SiteAccess {
                    permission: READ.into(),
                    groups: vec!["shop".into()],
                    sites: Vec::new(),
                },
                SiteAccess {
                    permission: WRITE.into(),
                    groups: vec!["shop".into()],
                    sites: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn scoped_callers_see_and_change_only_their_sites() {
        let model = ConfigModel {
            sites: vec![site("shop", Some("shop")), site("intranet", Some("corp"))],
            ..ConfigModel::default()
        };
        let scope = shop_writer();
        let visible = readable(&model, Some(&scope));
        assert_eq!(visible.sites.len(), 1);
        assert_eq!(visible.sites[0].name, "shop");
        assert_eq!(readable(&model, None).sites.len(), 2);

        let mut renamed = model.clone();
        renamed.sites[0].name = "store".into();
        assert!(check_changes(&model, &renamed, Some(&scope), WRITE).is_ok());
        let mut moved = model.clone();
        moved.sites[0].group = Some("corp".into());
        assert!(
            check_changes(&model, &moved, Some(&scope), WRITE).is_err(),
            "moved away"
        );
        let mut theirs = model.clone();
        theirs.sites[1].name = "staff".into();
        assert!(check_changes(&model, &theirs, Some(&scope), WRITE).is_err());
        let mut shared = model.clone();
        shared.listeners.push(
            serde_json::from_value::<Listener>(
                serde_json::json!({"id": "http", "address": "0.0.0.0:80"}),
            )
            .unwrap(),
        );
        assert!(check_changes(&model, &shared, Some(&scope), WRITE).is_err());
        assert!(check_changes(&model, &shared, None, WRITE).is_ok());
        assert!(require_everywhere(Some(&scope), WRITE, "replacing the files").is_err());
        assert!(require_everywhere(None, WRITE, "replacing the files").is_ok());
    }
}
