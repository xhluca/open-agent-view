//! Provider-neutral core for the `coding-agents` terminal application.

pub mod adapters;
pub mod app;
mod codex_rpc;
pub mod codex_supervisor;
pub mod control;
pub mod doctor;
pub mod domain;
pub mod hidden;
pub mod maintenance;
pub mod native_session;
pub mod opencode_supervisor;
pub mod pi_supervisor;
pub mod process;
pub mod terminal;
pub mod ui;
