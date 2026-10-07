#![forbid(unsafe_code)]

//! The reference plugin (ADR 0044): it provides every port, so hosts can be
//! tested against a real process and installations checked, and shows how
//! a plugin is written with `plugin-sdk`.

pub mod package;
pub mod services;
