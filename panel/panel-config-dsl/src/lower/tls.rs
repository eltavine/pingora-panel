//! `tls_profile` blocks: the protocol versions, cipher suites and
//! session settings a listener or server offers.

use super::Lowerer;
use crate::{codes, schema::Context};
use panel_config_model::TlsProfile;
use panel_domain::CertificateId;
use panel_dsl::Directive;
use panel_errors::Diagnostic;
use panel_ir::tls::{suite_version, CIPHER_SUITES, PROTOCOLS};
use std::collections::BTreeSet;

impl<'a> Lowerer<'a> {
    pub(super) fn tls_profile(&mut self, file: &str, directive: &Directive, depth: usize) {
        let id = Self::literal(&directive.args[0]);
        if self.profiles.iter().any(|(profile, _)| profile.id == id) {
            self.error(
                file,
                directive.args[0].span,
                codes::DUPLICATE,
                format!("TLS profile {id:?} is defined twice"),
            );
            return;
        }
        let mut profile = TlsProfile {
            id,
            certificate_id: None,
            certificate_secret_id: String::new(),
            private_key_secret_id: String::new(),
            min_protocol: "TLSv1.2".into(),
            max_protocol: None,
            cipher_suites: Vec::new(),
            session_resumption: true,
            ocsp_stapling: false,
            alpn: BTreeSet::new(),
        };
        let Some(block) = directive.block() else {
            return;
        };
        self.with_scope(format!("tls-profiles/{}", profile.id), |lowerer| {
            let mut seen = BTreeSet::new();
            lowerer.each(
                file,
                &block.directives,
                Context::TlsProfile,
                depth + 1,
                &mut seen,
                &mut |lowerer, file, directive, spec, _| {
                    let arg = &directive.args[0];
                    match spec.name {
                        "certificate_id" => {
                            if let Some(value) = lowerer.value(file, arg) {
                                match CertificateId::new(&value) {
                                    Ok(id) => profile.certificate_id = Some(id),
                                    Err(error) => lowerer.error(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        error.to_string(),
                                    ),
                                }
                            }
                        }
                        "certificate" => {
                            profile.certificate_secret_id =
                                lowerer.value(file, arg).unwrap_or_default()
                        }
                        "key" => {
                            profile.private_key_secret_id =
                                lowerer.value(file, arg).unwrap_or_default()
                        }
                        "max_protocol" => {
                            if let Some(value) = lowerer.value(file, arg) {
                                if PROTOCOLS.contains(&value.as_str()) {
                                    profile.max_protocol = Some(value);
                                } else {
                                    lowerer.error(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{value:?} is not TLSv1.2 or TLSv1.3"),
                                    );
                                }
                            }
                        }
                        "ciphers" => {
                            for arg in &directive.args {
                                if suite_version(&arg.value).is_some() {
                                    profile.cipher_suites.push(arg.value.clone());
                                } else {
                                    lowerer.error_with_help(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{:?} is not a cipher suite", arg.value),
                                        format!(
                                            "expected one of {}",
                                            CIPHER_SUITES
                                                .iter()
                                                .map(|(name, _)| *name)
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        ),
                                    );
                                }
                            }
                        }
                        "session_resumption" => {
                            profile.session_resumption =
                                lowerer.bool_arg(file, arg).unwrap_or(true)
                        }
                        "ocsp_stapling" => {
                            profile.ocsp_stapling =
                                lowerer.bool_arg(file, arg).unwrap_or_default();
                            if profile.ocsp_stapling {
                                let diagnostic = Diagnostic::warning(
                                    codes::NO_EFFECT,
                                    "OCSP stapling is reserved: it is recorded, but no OCSP responses are stapled yet",
                                );
                                lowerer.report(diagnostic, file, directive.span);
                            }
                        }
                        "min_protocol" => {
                            if let Some(value) = lowerer.value(file, arg) {
                                if matches!(value.as_str(), "TLSv1.2" | "TLSv1.3") {
                                    profile.min_protocol = value;
                                } else {
                                    lowerer.error(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{value:?} is not TLSv1.2 or TLSv1.3"),
                                    );
                                }
                            }
                        }
                        "alpn" => {
                            for arg in &directive.args {
                                match arg.value.as_str() {
                                    "h2" | "http/1.1" => {
                                        profile.alpn.insert(arg.value.clone());
                                    }
                                    other => lowerer.error(
                                        file,
                                        arg.span,
                                        codes::TYPE,
                                        format!("{other:?} is not h2 or http/1.1"),
                                    ),
                                }
                            }
                        }
                        _ => unreachable!(),
                    }
                },
            );
        });
        let files = [
            ("certificate", &profile.certificate_secret_id),
            ("key", &profile.private_key_secret_id),
        ];
        let named = profile.certificate_id.is_some();
        for (field, value) in files {
            if value.is_empty() && !named {
                self.error(
                    file,
                    directive.span,
                    codes::ARGUMENTS,
                    format!(
                        "TLS profile {:?} needs 'certificate_id' or '{field}'",
                        profile.id
                    ),
                );
            }
        }
        let origin = Self::origin(file, directive, depth);
        self.origins
            .insert(format!("tls-profiles/{}", profile.id), origin.clone());
        self.profiles.push((profile, origin));
    }
}
