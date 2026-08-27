//! Provider-neutral core for the `open-agent-view` terminal application.

pub mod adapters;
pub mod aliases;
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

#[cfg(test)]
pub(crate) mod test_support {
    /// `tempfile` intentionally respects the caller's umask, while ownership
    /// registries require a mode-0700 parent. Keep unit tests deterministic on
    /// developer shells and CI runners whose conventional umask is 0022.
    pub(crate) mod tempfile {
        pub fn tempdir() -> std::io::Result<::tempfile::TempDir> {
            let directory = ::tempfile::tempdir()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(directory)
        }
    }
}
