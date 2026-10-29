//! The SUPER daemon: one long-lived process per user that hosts coding-agent
//! sessions and serves them to local clients over a Unix socket.
//!
//! `super daemon run` calls [`run`]. The protocol a client speaks is defined in
//! `super_sdk::daemon` and described in `docs/daemon.md`; the design is in
//! `.super/specs/2026-10-29-daemon-design.md`.

pub mod env;
pub mod host;
pub mod lock;
pub mod log;
pub mod paths;
