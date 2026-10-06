#![forbid(unsafe_code)]

//! Stable gateway ports and an in-memory contract implementation.

mod conditions;
mod events;
pub mod fake;
mod http;
mod logging;
mod lua;
pub mod ports;
mod resilience;
mod traffic;
mod validation;

pub use conditions::{
    problems as condition_problems, token, MOST_CONDITIONS, MOST_CONDITION_DEPTH,
};
pub use events::*;
pub use fake::FakeGatewayEngine;
pub use http::{problems as http_policy_problems, MOST_PREFLIGHT_SECONDS};
pub use lua::{
    problems as lua_problems, uses_lua, LEAST_LUA_DICT_BYTES, LEAST_LUA_ERROR_LOG_BYTES,
    LEAST_LUA_MEMORY_BYTES, LEAST_LUA_SOCKET_BUFFER, MOST_LUA_DICT_BYTES, MOST_LUA_ERROR_LOG_BYTES,
    MOST_LUA_MEMORY_BYTES, MOST_LUA_REGEX_CACHE, MOST_LUA_SCRIPT_BYTES, MOST_LUA_SOCKET_BUFFER,
    MOST_LUA_SOCKET_MS, MOST_LUA_SOCKET_POOL, MOST_LUA_TIMERS, MOST_LUA_TIME_MS,
    MOST_LUA_VARIABLES, MOST_LUA_VERIFY_DEPTH, MOST_LUA_WORK, MOST_LUA_WORKER_VMS,
};
pub use ports::*;
pub use resilience::{
    problems as resilience_problems, uses_resilience, MOST_BACKOFF_MS, MOST_OPEN_MS, MOST_QUEUE_MS,
    MOST_RETRIES,
};
pub use traffic::{route_regex_error, ROUTE_REGEX_SIZE_LIMIT};
pub use validation::validate_engine_ir;
