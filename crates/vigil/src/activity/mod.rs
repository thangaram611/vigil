//! Per-agent activity model: the pure mtime scan ([`scan`]) and the vscode
//! hash-gate ([`vscode`]).
//!
//! The pure cores (mtime scan, vscode transition) take paths/records and have no
//! env reads, so they are exercised by cargo tests over temp dirs. The live IO
//! wrappers (host probe, recent-file collection, the read/write daemon path)
//! honor the test-seam env vars at the call site.

pub mod scan;
pub mod vscode;
