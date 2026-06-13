//! vigil library — exposes internals for integration tests.
//!
//! The binary crate re-uses these modules; integration tests access them here.

pub mod activity;
pub mod config;
pub mod debug;
pub mod log;
pub mod procscan;
pub mod refcount;

// Shared output substrate (anstream prints, comfy-table, --json). Lives in the
// library so `debug::render` (a library module) and the binary both use it.
pub mod output;
