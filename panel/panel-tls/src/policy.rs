use crate::PeerIdentity;
use panel_context::ServiceName;
use panel_pki::{TrustDomain, WorkloadIdentity};
use rustls_pki_types::ServerName;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tonic::{codegen::http, Status};
use tower::{Layer, Service};
use webpki::EndEntityCert;

/// Services every authenticated peer may call.
const OPEN_TO_EVERY_PEER: &[&str] = &[
    "grpc.health.v1.Health",
    "pingora.panel.platform.v1.ServiceInfo",
];

/// Which peer identities may call each gRPC service.
///
/// Health and service description are open to every authenticated peer;
/// any other service refuses calls unless its callers are listed. Calls
/// without a client certificate are refused as unauthenticated.
#[derive(Clone, Debug)]
pub struct PeerPolicy {
    trust_domain: TrustDomain,
    rules: Arc<HashMap<String, Option<Vec<String>>>>,
}

impl PeerPolicy {
    pub fn new(trust_domain: TrustDomain) -> Self {
        Self {
            trust_domain,
            rules: Arc::new(
                OPEN_TO_EVERY_PEER
                    .iter()
                    .map(|service| ((*service).to_owned(), None))
                    .collect(),
            ),
        }
    }

    /// Lets `peers` call `grpc_service`, a fully qualified service name.
    pub fn allow(
        mut self,
        grpc_service: &str,
        peers: impl IntoIterator<Item = ServiceName>,
    ) -> Self {
        let identities = peers
            .into_iter()
            .map(|peer| WorkloadIdentity::new(peer, self.trust_domain.clone()).dns_name())
            .collect::<Vec<_>>();
        let rules = Arc::make_mut(&mut self.rules);
        if let Some(allowed) = rules
            .entry(grpc_service.to_owned())
            .or_insert_with(|| Some(Vec::new()))
        {
            allowed.extend(identities);
        }
        self
    }

    /// Whether the peer may call the method at `path`
    /// (`/<service>/<method>`).
    pub fn authorize(&self, path: &str, peer: Option<&PeerIdentity>) -> Result<(), Status> {
        let certificate = peer
            .and_then(PeerIdentity::certificate)
            .ok_or_else(|| Status::unauthenticated("a client certificate is required"))?;
        let service = path
            .strip_prefix('/')
            .and_then(|path| path.split_once('/'))
            .map(|(service, _)| service)
            .ok_or_else(|| Status::permission_denied("unknown method"))?;
        let allowed = match self.rules.get(service) {
            Some(None) => return Ok(()),
            Some(Some(allowed)) => allowed,
            None => {
                return Err(Status::permission_denied(format!(
                    "no peer may call {service}"
                )))
            }
        };
        let certificate = EndEntityCert::try_from(certificate)
            .map_err(|_| Status::unauthenticated("the client certificate is invalid"))?;
        let permitted = allowed.iter().any(|identity| {
            ServerName::try_from(identity.as_str())
                .is_ok_and(|name| certificate.verify_is_valid_for_subject_name(&name).is_ok())
        });
        if permitted {
            Ok(())
        } else {
            Err(Status::permission_denied(format!(
                "this peer may not call {service}"
            )))
        }
    }
}

impl<S> Layer<S> for PeerPolicy {
    type Service = PeerPolicyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PeerPolicyService {
            inner,
            policy: self.clone(),
        }
    }
}

/// A service that applies a [`PeerPolicy`] before its inner service.
#[derive(Clone, Debug)]
pub struct PeerPolicyService<S> {
    inner: S,
    policy: PeerPolicy,
}

type Pending<R, E> = Pin<Box<dyn Future<Output = Result<R, E>> + Send>>;

impl<S, Request, Response> Service<http::Request<Request>> for PeerPolicyService<S>
where
    S: Service<http::Request<Request>, Response = http::Response<Response>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    Response: Default + Send + 'static,
{
    type Response = http::Response<Response>;
    type Error = S::Error;
    type Future = Pending<Self::Response, S::Error>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<Request>) -> Self::Future {
        let decision = self.policy.authorize(
            request.uri().path(),
            request.extensions().get::<PeerIdentity>(),
        );
        match decision {
            Ok(()) => Box::pin(self.inner.call(request)),
            Err(status) => {
                tracing::warn!(
                    path = request.uri().path(),
                    code = ?status.code(),
                    "peer refused"
                );
                Box::pin(std::future::ready(Ok(status.into_http())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use panel_pki::{CertificateAuthority, DEFAULT_AUTHORITY_VALIDITY};
    use rustls_pki_types::{pem::PemObject, CertificateDer};
    use std::time::Duration;

    fn certificate(service: &str) -> CertificateDer<'static> {
        let directory = std::env::temp_dir().join(format!(
            "panel-tls-policy-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let (authority, _) = CertificateAuthority::load_or_create(
            &directory,
            TrustDomain::default(),
            DEFAULT_AUTHORITY_VALIDITY,
            Utc::now(),
        )
        .unwrap();
        let issued = authority
            .issue(
                &ServiceName::new(service).unwrap(),
                &[],
                Duration::from_secs(3600),
                Utc::now(),
            )
            .unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        CertificateDer::from_pem_slice(issued.certificate_pem.as_bytes()).unwrap()
    }

    fn peer(service: &str) -> PeerIdentity {
        PeerIdentity {
            remote: "127.0.0.1:1".parse().unwrap(),
            certificate: Some(certificate(service)),
        }
    }

    #[test]
    fn services_admit_only_their_listed_peers() {
        let policy = PeerPolicy::new(TrustDomain::default()).allow(
            "pingora.panel.config.v1.Publication",
            [ServiceName::new("panel-api").unwrap()],
        );
        let publication = "/pingora.panel.config.v1.Publication/Prepare";
        assert!(policy
            .authorize(publication, Some(&peer("panel-api")))
            .is_ok());
        let refused = policy
            .authorize(publication, Some(&peer("automation-service")))
            .unwrap_err();
        assert_eq!(refused.code(), tonic::Code::PermissionDenied);

        assert!(policy
            .authorize(
                "/grpc.health.v1.Health/Check",
                Some(&peer("automation-service"))
            )
            .is_ok());
        assert_eq!(
            policy
                .authorize(
                    "/pingora.panel.gateway.v1.GatewayEngine/Activate",
                    Some(&peer("panel-api"))
                )
                .unwrap_err()
                .code(),
            tonic::Code::PermissionDenied
        );
        let anonymous = PeerIdentity {
            remote: "127.0.0.1:1".parse().unwrap(),
            certificate: None,
        };
        assert_eq!(
            policy
                .authorize("/grpc.health.v1.Health/Check", Some(&anonymous))
                .unwrap_err()
                .code(),
            tonic::Code::Unauthenticated
        );
        assert_eq!(
            policy.authorize(publication, None).unwrap_err().code(),
            tonic::Code::Unauthenticated
        );
    }
}
