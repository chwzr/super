//! Library entry for the super CLI. Re-exports the public modules so
//! integration tests (and future external consumers) can reach them.

pub mod agents;
pub mod auth;
pub mod bootstrap;
pub mod commands;
pub mod config;
pub mod conversation;
pub mod executor;
pub mod lsp;
pub mod mcp;
pub mod providers;
pub mod sdk;
pub mod skills;
pub mod state;
pub mod tools;
pub mod tui;
pub mod utils;
