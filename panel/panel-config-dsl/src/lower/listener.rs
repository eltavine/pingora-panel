//! `listener` blocks: addresses, protocols and the default server.

use super::{ListenerDraft, Lowerer};
use crate::{codes, schema::Context, values};
use panel_config_model::Listener;
use panel_dsl::Directive;
use panel_ir::ListenerProtocols;
use std::collections::BTreeSet;

impl<'a> Lowerer<'a> {
    pub(super) fn listener(&mut self, file: &str, directive: &Directive, depth: usize) {
        let id = Self::literal(&directive.args[0]);
        if self.listeners.iter().any(|draft| draft.listener.id == id) {
            self.error(
                file,
                directive.args[0].span,
                codes::DUPLICATE,
                format!("listener {id:?} is defined twice"),
            );
            return;
        }
        let mut listener = Listener {
            id,
            address: String::new(),
            tls_profile_id: None,
            protocols: ListenerProtocols::default(),
            reuse_port: false,
            ipv6_only: None,
            default_site_id: None,
            trusted_proxies: Vec::new(),
            real_ip_header: panel_ir::RealIpHeader::default(),
            request_head_timeout_seconds: None,
        };
        let mut default_server = None;
        let Some(block) = directive.block() else {
            return;
        };
        self.with_scope(format!("listeners/{}", listener.id), |lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::Listener,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, _| {
                    let arg = &directive.args[0];
                    match spec.name {
                        "address" => {
                            if let Some(value) = lowerer.value(file, arg) {
                                if values::parse_socket_address(&value).is_some() {
                                    listener.address = value;
                                } else {
                                    lowerer.error_with_help(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{value:?} is not an IP address and port"),
                                        "write addresses such as 0.0.0.0:80 or [::]:443",
                                    );
                                }
                            }
                        }
                        "protocols" => {
                            let mut protocols = ListenerProtocols {
                                http1: false,
                                http2: false,
                                http3: false,
                            };
                            for arg in &directive.args {
                                match arg.value.as_str() {
                                    "http1" => protocols.http1 = true,
                                    "http2" => protocols.http2 = true,
                                    "http3" => protocols.http3 = true,
                                    other => lowerer.error(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{other:?} is not http1, http2 or http3"),
                                    ),
                                }
                            }
                            listener.protocols = protocols;
                        }
                        "tls_profile" => listener.tls_profile_id = Some(Self::literal(arg)),
                        "reuse_port" => {
                            listener.reuse_port = lowerer.bool_arg(file, arg).unwrap_or_default()
                        }
                        "ipv6_only" => listener.ipv6_only = lowerer.bool_arg(file, arg),
                        "default_server" => {
                            default_server = Some((Self::literal(arg), file.to_owned(), arg.span))
                        }
                        "trusted_proxies" => {
                            let networks = lowerer.networks(file, &directive.args);
                            listener.trusted_proxies.extend(networks);
                        }
                        "real_ip_header" => {
                            if let Some(header) = lowerer.real_ip_header(file, arg) {
                                listener.real_ip_header = header;
                            }
                        }
                        "request_head_timeout" => {
                            listener.request_head_timeout_seconds =
                                lowerer.whole_seconds(file, arg);
                        }
                        _ => unreachable!(),
                    }
                },
            );
        });
        if listener.address.is_empty()
            && !directive
                .block()
                .is_some_and(|block| block.directives.iter().any(|d| d.name.value == "address"))
        {
            self.error(
                file,
                directive.span,
                codes::ARGUMENTS,
                format!("listener {:?} needs 'address'", listener.id),
            );
        }
        self.origins.insert(
            format!("listeners/{}", listener.id),
            Self::origin(file, directive, depth),
        );
        self.listeners.push(ListenerDraft {
            listener,
            default_server,
        });
    }
}
