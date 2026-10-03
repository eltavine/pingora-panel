//! Settings that have no effect where they are written: every block that
//! would take them over sets its own, or nothing that uses them inherits
//! them.

use crate::{checks::label, codes, Origin, Written};
use panel_config_model::ConfigModel;
use panel_dsl::Span;
use panel_errors::Diagnostic;
use std::collections::BTreeMap;

type Finding = (String, Span, Diagnostic);

fn written<'a>(
    written: &'a BTreeMap<String, Vec<Written>>,
    resource: &str,
    directive: &str,
) -> Option<&'a Written> {
    written
        .get(resource)?
        .iter()
        .find(|written| written.directive == directive)
}

fn no_effect(message: String, help: &str) -> Diagnostic {
    Diagnostic::warning(codes::NO_EFFECT, message).with_help(help)
}

/// Every setting of `model` without effect, at the directive or resource
/// that writes it.
pub(crate) fn check(
    model: &ConfigModel,
    written_in: &BTreeMap<String, Vec<Written>>,
    origins: &BTreeMap<String, Origin>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut at_written = |written: &Written, diagnostic| {
        findings.push((written.file.clone(), written.span, diagnostic));
    };
    let mut at_origin = Vec::new();

    for site in model.sites.iter().filter(|site| !site.is_deleted()) {
        let resource = format!("sites/{}", site.id);
        let tls = model
            .listeners
            .iter()
            .filter(|listener| {
                site.listener_ids.is_empty() || site.listener_ids.contains(&listener.id)
            })
            .any(|listener| listener.tls_profile_id.is_some());
        if let (Some(profile), Some(at)) = (
            &site.tls_profile_id,
            written(written_in, &resource, "tls_profile"),
        ) {
            if !tls {
                at_written(
                    at,
                    no_effect(
                        format!("tls_profile {profile} has no effect: none of the server's listeners use TLS"),
                        "serve the server on a listener with a tls_profile, or remove this one",
                    ),
                );
            } else if !site.domains.is_empty()
                && site
                    .domains
                    .iter()
                    .all(|domain| domain.tls_profile_id.is_some())
            {
                at_written(
                    at,
                    no_effect(
                        format!("tls_profile {profile} has no effect: every host of the server sets its own tls_profile="),
                        "remove it, or drop tls_profile= from the hosts that should use it",
                    ),
                );
            }
        }
        if !tls {
            for domain in &site.domains {
                let Some(profile) = &domain.tls_profile_id else {
                    continue;
                };
                at_origin.push((
                    format!("{resource}/domains/{}", domain.host),
                    no_effect(
                        format!(
                            "tls_profile={profile} of {} has no effect: none of the server's listeners use TLS",
                            domain.host
                        ),
                        "serve the server on a listener with a tls_profile, or remove tls_profile=",
                    ),
                ));
            }
        }
        if !site.enabled {
            for route in site.routes.iter().filter(|route| route.enabled) {
                let route_resource = format!("{resource}/routes/{}", route.id);
                if let Some(at) = written(written_in, &route_resource, "enabled") {
                    at_written(
                        at,
                        no_effect(
                            format!(
                                "route {} is enabled, but server {:?} is not, so it serves nothing",
                                label(route),
                                site.name
                            ),
                            "enable the server, or remove `enabled on`",
                        ),
                    );
                }
            }
        }
    }

    for upstream in &model.upstreams {
        let resource = format!("upstreams/{}", upstream.id);
        let tls_nodes: Vec<_> = upstream.nodes.iter().filter(|node| node.tls).collect();
        if tls_nodes.is_empty() {
            if let Some(at) = written(written_in, &resource, "tls") {
                at_written(
                    at,
                    no_effect(
                        format!(
                            "tls has no effect: no node of upstream {:?} uses TLS",
                            upstream.name
                        ),
                        "add the tls flag to the nodes that speak TLS, or remove this",
                    ),
                );
            }
            if upstream.connection.http2 {
                if let Some(at) = written(written_in, &resource, "http2") {
                    at_written(
                        at,
                        no_effect(
                            "http2 has no effect: only nodes with the tls flag negotiate HTTP/2"
                                .into(),
                            "add the tls flag to the nodes, or remove this",
                        ),
                    );
                }
            }
        } else if let Some(sni) = &upstream.tls.sni {
            if tls_nodes.iter().all(|node| node.sni.is_some()) {
                if let Some(at) = written(written_in, &resource, "tls") {
                    at_written(
                        at,
                        no_effect(
                            format!("sni={sni} has no effect: every node with the tls flag sets its own sni="),
                            "remove it, or drop sni= from the nodes that should send it",
                        ),
                    );
                }
            }
        }
        for node in upstream
            .nodes
            .iter()
            .filter(|node| !node.tls && node.sni.is_some())
        {
            at_origin.push((
                format!("{resource}/nodes/{}", node.id),
                no_effect(
                    "sni= has no effect: the node does not use TLS".into(),
                    "add the tls flag, or remove sni=",
                ),
            ));
        }
    }

    findings.extend(at_origin.into_iter().filter_map(|(resource, diagnostic)| {
        let origin = origins.get(&resource)?;
        Some((origin.file.clone(), origin.span, diagnostic))
    }));
    findings
}
