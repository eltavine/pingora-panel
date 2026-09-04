#![forbid(unsafe_code)]
#![no_std]
#![forbid(missing_docs)]

//! Pure control-plane domain rules.
//!
//! This crate deliberately owns no persistence, transport, clock, queue, or
//! gateway implementation. Adapters may persist or project these values, but
//! external representations must not leak back into the transition model.

mod revision;

pub use revision::{
    RevisionState, RevisionStatus, RevisionTransition, RevisionTransitionError,
    RevisionTransitionOutcome,
};
