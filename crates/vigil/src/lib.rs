//! vigil library — exposes internals for integration tests.
//!
//! The binary crate re-uses these modules; integration tests access them here.

pub mod activity;
pub mod battery;
pub mod check;
pub mod config;
pub mod daemon;
pub mod debug;
pub mod helper;
pub mod ipc;
pub mod log;
pub mod power;
pub mod power_guard;
pub mod procscan;
pub mod refcount;
pub mod service;
pub mod thermal;

// Shared output substrate (anstream prints, comfy-table, --json). Lives in the
// library so `debug::render` (a library module) and the binary both use it.
pub mod output;

// Test-only helpers shared by the crate's in-crate unit tests (e.g. the
// leak-proof `BoundedCpuHog`). Compiled only under `cfg(test)`.
#[cfg(test)]
mod testutil;
