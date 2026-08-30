use panel_errors::{PanelError, Result};
use std::net::SocketAddr;

/// Policy port for management-plane listener exposure.
///
/// A future mTLS transport can supply a different policy without weakening the
/// plaintext default or embedding certificate knowledge in configuration parsing.
pub trait ManagementBindPolicy: Send + Sync {
    fn validate(&self, address: SocketAddr) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LoopbackOnlyManagementBindPolicy;

impl ManagementBindPolicy for LoopbackOnlyManagementBindPolicy {
    fn validate(&self, address: SocketAddr) -> Result<()> {
        if address.ip().is_loopback() {
            return Ok(());
        }
        Err(PanelError::invalid_argument(format!(
            "plaintext gateway management address {address} is not loopback; remote binds require an authenticated transport"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_policy_accepts_only_ipv4_and_ipv6_loopback() {
        let policy = LoopbackOnlyManagementBindPolicy;

        assert!(policy.validate("127.0.0.1:50051".parse().unwrap()).is_ok());
        assert!(policy.validate("[::1]:50051".parse().unwrap()).is_ok());
        for address in ["0.0.0.0:50051", "[::]:50051", "192.0.2.10:50051"] {
            let error = policy.validate(address.parse().unwrap()).unwrap_err();
            assert_eq!(
                error.code.as_str(),
                panel_errors::ErrorCode::INVALID_ARGUMENT
            );
        }
    }
}
