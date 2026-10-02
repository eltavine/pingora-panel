#![forbid(unsafe_code)]

//! Service descriptions, protocol revision negotiation and capability
//! discovery, independent of transports and registries.
//!
//! Every service instance publishes a [`ServiceDescriptor`]: its build and
//! schema versions, the protocol revisions it speaks and the capabilities it
//! provides. Callers negotiate the highest common revision of a protocol
//! before relying on additions made after its first revision, and discover
//! providers of a capability through a [`ServiceDirectory`].

mod descriptor;
mod directory;
mod protocol;
mod registration;

pub use descriptor::{Capability, ServiceDescriptor};
pub use directory::{ServiceDirectory, ServiceListing, ServiceRegistrar};
pub use panel_context::ServiceName;
pub use protocol::{NegotiatedProtocol, ProtocolRange};
pub use registration::{maintain_registration, RegistrationPolicy};
