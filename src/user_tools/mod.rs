//! User-facing llmup tool entrypoints and shared helpers.

use std::ffi::OsStr;
use std::path::PathBuf;

pub mod agent_launcher;
pub mod config_wizard;
pub mod env_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserToolEntrypoint {
    Config,
    Codex,
    Claude,
}

pub fn entrypoint_from_argv0(program: &OsStr) -> Option<UserToolEntrypoint> {
    let name = std::path::Path::new(program)
        .file_name()
        .and_then(OsStr::to_str)?;
    match name {
        "llmup-config" => Some(UserToolEntrypoint::Config),
        "llmup-codex" => Some(UserToolEntrypoint::Codex),
        "llmup-claude" => Some(UserToolEntrypoint::Claude),
        _ => None,
    }
}

pub(crate) fn home_dir_from_env() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is required to locate llmup user directories".to_string())
}

pub(crate) fn env_path_or_default(name: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default)
}
