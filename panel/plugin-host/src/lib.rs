#![forbid(unsafe_code)]

//! The plugins module of the control plane (ADR 0044): it finds plugin
//! versions, checks their manifests and signatures, runs enabled plugins as
//! child processes and routes the calls of their ports.

pub mod catalog;
pub mod limits;
pub mod manifest;
pub mod process;
pub mod proxy;
pub mod runtime;
pub mod settings;
pub mod signature;
