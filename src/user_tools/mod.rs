//! User-facing llmup tool helpers and the `codex-setup` subcommand.
//!
//! The deprecated argv[0] aliases (`llmup-config` / `llmup-codex` /
//! `llmup-claude`) and their launcher/wizard/env-file modules have been
//! removed. The single product entrypoint is now the `codex-setup` subcommand
//! (see [`codex_setup`]). [`agent_model_profile`] remains a shared dependency
//! of the server `/models` handler.

use std::path::PathBuf;

pub mod agent_model_profile;
pub mod codex_setup;

pub(crate) fn home_dir_from_env() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is required to locate llmup user directories".to_string())
}
