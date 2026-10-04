//! What the service's events carry (ADR 0004).

use crate::certificates::Cause;
use panel_errors::{PanelError, Result};
use panel_event_contracts::tls::v1::{
    AcmeAccountRefused, AcmeCertificateRefused, CertificateRefused, DnsProviderRefused,
};
use panel_events::EventData;
use panel_sqlite::EventLog;

/// The data of a `*.refused` event: which change of what was refused, and
/// why.
pub trait Refusal: EventData {
    fn new(id: &str, operation: &str, error: &PanelError) -> Self;
}

macro_rules! refusals {
    ($($message:ty),* $(,)?) => {$(
        impl Refusal for $message {
            fn new(id: &str, operation: &str, error: &PanelError) -> Self {
                Self {
                    id: id.to_owned(),
                    operation: operation.to_owned(),
                    code: error.code.as_str().to_owned(),
                    message: error.message.clone(),
                }
            }
        }
    )*};
}

refusals!(
    AcmeAccountRefused,
    AcmeCertificateRefused,
    CertificateRefused,
    DnsProviderRefused,
);

/// Records the refusal of `operation` on `aggregate` if `result` failed;
/// what it would have changed is unchanged.
pub async fn refused<E: Refusal, T>(
    events: &EventLog,
    cause: Cause<'_>,
    aggregate: (&str, &str),
    operation: &str,
    result: Result<T>,
) -> Result<T> {
    if let Err(error) = &result {
        let data = E::new(aggregate.1, operation, error);
        events
            .record_by(aggregate, cause.scope, cause.principal, &data)
            .await;
    }
    result
}
