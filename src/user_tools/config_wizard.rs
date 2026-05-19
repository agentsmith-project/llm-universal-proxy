use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};
use uuid::Uuid;

use super::env_file::{read_env_file, EnvFile};
use super::{env_path_or_default, home_dir_from_env};
use crate::config::DataAuthMode;
use crate::formats::UpstreamFormat;
use crate::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderInterface {
    OpenAiChatCompletions,
    AnthropicMessages,
    OpenAiResponses,
}

impl ProviderInterface {
    fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "openai" | "openai-completion" | "openai-chat" | "openai-chat-completions"
            | "chat" | "chat-completions" => Ok(Self::OpenAiChatCompletions),
            "anthropic" | "claude" | "anthropic-messages" | "claude-messages" => {
                Ok(Self::AnthropicMessages)
            }
            "openai-responses" | "responses" => Ok(Self::OpenAiResponses),
            other => Err(format!(
                "unsupported --interface `{other}`; use openai-chat-completions, openai-responses, or anthropic-messages"
            )),
        }
    }

    fn config_format(self) -> &'static str {
        match self {
            Self::OpenAiChatCompletions => "openai-chat-completions",
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenAiResponses => "openai-responses",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeySource {
    Stdin,
    Env(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitCliOptions {
    pub interface: ProviderInterface,
    pub model_service_url: String,
    pub model_name: String,
    pub model_alias: String,
    pub force: bool,
    pub api_key_source: ApiKeySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigCommand {
    Interactive,
    Doctor,
    Help,
    Version,
    Init(InitCliOptions),
    SetLimits(SetLimitsCliOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetLimitsTarget {
    Alias(String),
    Upstream(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetLimitsCliOptions {
    pub target: SetLimitsTarget,
    pub context_window: u64,
    pub max_output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    pub llmup_home: PathBuf,
    pub codex_home: PathBuf,
    pub claude_config_dir: PathBuf,
    pub interface: ProviderInterface,
    pub model_service_url: String,
    pub model_name: String,
    pub model_alias: String,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitResult {
    pub config_path: PathBuf,
    pub secrets_path: PathBuf,
    pub codex_home: PathBuf,
    pub claude_config_dir: PathBuf,
    pub summary: String,
}

const CONFIG_HELP: &str = "\
llmup-config

Usage:
  llmup-config
  llmup-config doctor
  llmup-config set-limits (--alias <name> | --upstream <name>) --context-window <n> --max-output-tokens <n>
  llmup-config --help
  llmup-config --version

Configure llmup for local Codex CLI and Claude Code launchers.

Run without arguments to create the local config used by llmup-codex and
llmup-claude. The default setup is OpenAI Chat Completions
(/v1/chat/completions); the prompt also accepts OpenAI Responses
(/v1/responses) or Anthropic Messages (/v1/messages) when your model service
requires a different API format.
Run doctor to validate local config and secrets files without contacting providers.
";

pub fn parse_config_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<ConfigCommand, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.is_empty() {
        return Ok(ConfigCommand::Interactive);
    }
    if args.len() == 1 {
        if args[0] == "--help" || args[0] == "-h" {
            return Ok(ConfigCommand::Help);
        }
        if args[0] == "--version" {
            return Ok(ConfigCommand::Version);
        }
        if args[0] == "doctor" || args[0] == "check" {
            return Ok(ConfigCommand::Doctor);
        }
    }

    let Some(first) = args.first().and_then(|item| item.to_str()) else {
        return Err("llmup-config arguments must be valid UTF-8".to_string());
    };
    if first == "set-limits" {
        return parse_set_limits_args(&args[1..]).map(ConfigCommand::SetLimits);
    }
    if first != "init" {
        return Err(format!("unknown llmup-config command `{first}`"));
    }
    parse_hidden_init_args(&args[1..]).map(ConfigCommand::Init)
}

pub fn run_cli(
    args: impl IntoIterator<Item = OsString>,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<i32, String> {
    match parse_config_args(args)? {
        ConfigCommand::Help => {
            stdout
                .write_all(CONFIG_HELP.as_bytes())
                .map_err(|error| format!("failed to write help: {error}"))?;
            Ok(0)
        }
        ConfigCommand::Version => {
            writeln!(
                stdout,
                "llmup {}",
                option_env!("CARGO_PKG_VERSION").unwrap_or("unknown")
            )
            .map_err(|error| format!("failed to write version: {error}"))?;
            Ok(0)
        }
        ConfigCommand::Interactive => {
            run_interactive(stdin, stdout)?;
            Ok(0)
        }
        ConfigCommand::Doctor => run_doctor(stdout),
        ConfigCommand::SetLimits(options) => {
            let home = home_dir_from_env()?;
            let llmup_home = env_path_or_default("LLMUP_HOME", home.join(".llmup"));
            let config_path = llmup_home.join("config.yaml");
            let summary = set_limits_in_config(&config_path, &options)?;
            stdout
                .write_all(summary.as_bytes())
                .map_err(|error| format!("failed to write summary: {error}"))?;
            Ok(0)
        }
        ConfigCommand::Init(init) => {
            let api_key = match init.api_key_source {
                ApiKeySource::Stdin => read_api_key_from_stdin(stdin)?,
                ApiKeySource::Env(name) => std::env::var(&name).map_err(|error| {
                    format!("failed to read API key from environment variable `{name}`: {error}")
                })?,
            };
            let home = home_dir_from_env()?;
            let llmup_home = env_path_or_default("LLMUP_HOME", home.join(".llmup"));
            let options = InitOptions {
                codex_home: env_path_or_default("LLMUP_CODEX_HOME", home.join(".llmup-codex")),
                claude_config_dir: env_path_or_default(
                    "LLMUP_CLAUDE_CONFIG_DIR",
                    home.join(".llmup-claude"),
                ),
                llmup_home,
                interface: init.interface,
                model_service_url: init.model_service_url,
                model_name: init.model_name,
                model_alias: init.model_alias,
                force: init.force,
            };
            let result = init_non_interactive(options, &api_key)?;
            stdout
                .write_all(result.summary.as_bytes())
                .map_err(|error| format!("failed to write summary: {error}"))?;
            Ok(0)
        }
    }
}

fn run_interactive(stdin: &mut dyn Read, stdout: &mut dyn Write) -> Result<(), String> {
    let home = home_dir_from_env()?;
    let llmup_home = env_path_or_default("LLMUP_HOME", home.join(".llmup"));
    let config_path = llmup_home.join("config.yaml");
    let secrets_path = llmup_home.join("secrets.env");
    let mut force = false;

    writeln!(
        stdout,
        "llmup local setup\n\nThis creates a local proxy config for llmup-codex and llmup-claude.\nThe default is OpenAI Chat Completions (/v1/chat/completions)."
    )
    .map_err(|error| format!("failed to write prompt: {error}"))?;

    if config_path.exists() || secrets_path.exists() {
        stdout
            .write_all(existing_config_summary(&config_path, &secrets_path).as_bytes())
            .map_err(|error| format!("failed to write existing config summary: {error}"))?;
        let answer = prompt_optional_line(
            stdin,
            stdout,
            "Press Enter to keep it, type reconfigure, or type doctor: ",
        )?;
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "keep" | "k" | "no" | "n" => {
                writeln!(
                    stdout,
                    "Keeping existing config.\nNext: run llmup-codex or llmup-claude."
                )
                .map_err(|error| format!("failed to write summary: {error}"))?;
                return Ok(());
            }
            "reconfigure" | "replace" | "r" | "yes" | "y" => {
                force = true;
            }
            "doctor" | "check" | "d" => {
                let status = write_doctor_report(&config_path, &secrets_path, stdout)?;
                if status == 0 {
                    return Ok(());
                }
                return Err("doctor found problems in the local llmup config".to_string());
            }
            other => {
                return Err(format!(
                    "unknown choice `{other}`; rerun llmup-config and press Enter, type reconfigure, or type doctor"
                ));
            }
        }
    }

    let interface = prompt_provider_interface(stdin, stdout)?;
    let model_service_url = prompt_required_line(
        stdin,
        stdout,
        "Model service API root, for example https://api.example.com/v1: ",
        "model service API root",
    )?;
    let model_name = prompt_required_line(
        stdin,
        stdout,
        "Model name, for example MiniMax-M2.7-highspeed: ",
        "model name",
    )?;
    let api_key = prompt_required_line(
        stdin,
        stdout,
        "Provider API key (saved locally; not printed again): ",
        "provider API key",
    )?;

    let result = init_non_interactive(
        InitOptions {
            llmup_home,
            codex_home: env_path_or_default("LLMUP_CODEX_HOME", home.join(".llmup-codex")),
            claude_config_dir: env_path_or_default(
                "LLMUP_CLAUDE_CONFIG_DIR",
                home.join(".llmup-claude"),
            ),
            interface,
            model_service_url,
            model_name,
            model_alias: "default".to_string(),
            force,
        },
        &api_key,
    )?;
    stdout
        .write_all(result.summary.as_bytes())
        .map_err(|error| format!("failed to write summary: {error}"))
}

fn prompt_provider_interface(
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<ProviderInterface, String> {
    let value = prompt_optional_line(
        stdin,
        stdout,
        "\nModel service API format [openai-chat-completions]: openai-chat-completions, openai-responses, or anthropic-messages: ",
    )?;
    let value = value.trim();
    if value.is_empty() {
        Ok(ProviderInterface::OpenAiChatCompletions)
    } else {
        ProviderInterface::parse(value)
            .map_err(|_| {
                format!(
                    "unsupported model service API format `{value}`; use openai-chat-completions, openai-responses, or anthropic-messages"
                )
            })
    }
}

fn prompt_required_line(
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    prompt: &str,
    field_name: &str,
) -> Result<String, String> {
    let value = prompt_optional_line(stdin, stdout, prompt)?;
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{field_name} must not be empty"));
    }
    Ok(value.to_string())
}

fn prompt_optional_line(
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    prompt: &str,
) -> Result<String, String> {
    stdout
        .write_all(prompt.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("failed to flush prompt: {error}"))?;
    read_line(stdin)
}

fn read_line(stdin: &mut dyn Read) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut one = [0_u8; 1];
    loop {
        match stdin.read(&mut one) {
            Ok(0) => break,
            Ok(_) if one[0] == b'\n' => break,
            Ok(_) => bytes.push(one[0]),
            Err(error) => return Err(format!("failed to read input: {error}")),
        }
    }
    if bytes.ends_with(b"\r") {
        bytes.pop();
    }
    String::from_utf8(bytes).map_err(|_| "interactive input must be valid UTF-8".to_string())
}

pub fn init_non_interactive(options: InitOptions, api_key: &str) -> Result<InitResult, String> {
    validate_plain_yaml_scalar("model-service-url", &options.model_service_url)?;
    validate_plain_yaml_scalar("model-name", &options.model_name)?;
    validate_alias(&options.model_alias)?;
    if api_key.trim().is_empty() {
        return Err("API key must not be empty".to_string());
    }

    let config_path = options.llmup_home.join("config.yaml");
    let secrets_path = options.llmup_home.join("secrets.env");
    if !options.force {
        if config_path.exists() {
            return Err(format!(
                "{} already exists; rerun hidden init with --force to replace it",
                config_path.display()
            ));
        }
        if secrets_path.exists() {
            return Err(format!(
                "{} already exists; rerun hidden init with --force to replace it",
                secrets_path.display()
            ));
        }
    }

    fs::create_dir_all(&options.llmup_home).map_err(|error| {
        format!(
            "failed to create llmup home {}: {error}",
            options.llmup_home.display()
        )
    })?;
    fs::create_dir_all(&options.codex_home).map_err(|error| {
        format!(
            "failed to create Codex home {}: {error}",
            options.codex_home.display()
        )
    })?;
    fs::create_dir_all(&options.claude_config_dir).map_err(|error| {
        format!(
            "failed to create Claude config dir {}: {error}",
            options.claude_config_dir.display()
        )
    })?;

    let config_yaml = generated_config_yaml(&options);
    let parsed = Config::from_yaml_str(&config_yaml)?;
    parsed.validate()?;

    let proxy_key = format!("llmup-local-{}", Uuid::new_v4().simple());
    let secrets =
        format!("LLM_UNIVERSAL_PROXY_KEY={proxy_key}\nLLMUP_PROVIDER_DEFAULT_API_KEY={api_key}\n");
    super::env_file::parse_env_file_str(&secrets)?;

    write_text_file(&config_path, &config_yaml, options.force)?;
    write_secret_file(&secrets_path, &secrets, options.force)?;

    let summary = format!(
        "Wrote llmup config: {}\nWrote local secrets: {}\nModel alias: {}\nNext: run llmup-codex or llmup-claude.\n",
        config_path.display(),
        secrets_path.display(),
        options.model_alias
    );

    Ok(InitResult {
        config_path,
        secrets_path,
        codex_home: options.codex_home,
        claude_config_dir: options.claude_config_dir,
        summary,
    })
}

fn run_doctor(stdout: &mut dyn Write) -> Result<i32, String> {
    let home = home_dir_from_env()?;
    let llmup_home = env_path_or_default("LLMUP_HOME", home.join(".llmup"));
    let config_path = llmup_home.join("config.yaml");
    let secrets_path = llmup_home.join("secrets.env");
    write_doctor_report(&config_path, &secrets_path, stdout)
}

fn write_doctor_report(
    config_path: &Path,
    secrets_path: &Path,
    stdout: &mut dyn Write,
) -> Result<i32, String> {
    let (report, ok) = doctor_report(config_path, secrets_path);
    stdout
        .write_all(report.as_bytes())
        .map_err(|error| format!("failed to write doctor report: {error}"))?;
    Ok(if ok { 0 } else { 1 })
}

fn doctor_report(config_path: &Path, secrets_path: &Path) -> (String, bool) {
    let mut ok = true;
    let mut report = format!(
        "llmup config doctor\n\nPaths:\n  config: {}\n  secrets: {}\n\n",
        config_path.display(),
        secrets_path.display()
    );

    let config = match load_valid_config(config_path) {
        Ok(config) => {
            report.push_str("OK config YAML parses and validates\n");
            Some(config)
        }
        Err(error) => {
            ok = false;
            report.push_str(&format!("ERROR config YAML: {error}\n"));
            None
        }
    };

    let secrets = match read_env_file(secrets_path) {
        Ok(secrets) => {
            report.push_str("OK secrets.env parses\n");
            Some(secrets)
        }
        Err(error) => {
            ok = false;
            report.push_str(&format!("ERROR secrets.env: {error}\n"));
            None
        }
    };

    #[cfg(unix)]
    {
        match safe_secret_permissions(secrets_path) {
            Ok(()) => report.push_str("OK secrets permissions are owner-only\n"),
            Err(error) => {
                ok = false;
                report.push_str(&format!("ERROR secrets permissions: {error}\n"));
            }
        }
    }

    match (config.as_ref(), secrets.as_ref()) {
        (Some(config), Some(secrets)) => {
            let missing = required_secret_env_names(config)
                .into_iter()
                .filter(|name| secrets.required(name).is_err())
                .collect::<Vec<_>>();
            if missing.is_empty() {
                report.push_str("OK required secrets are configured\n");
            } else {
                ok = false;
                report.push_str(&format!(
                    "ERROR required secrets missing or empty: {}\n",
                    missing.join(", ")
                ));
            }
        }
        _ => report.push_str("SKIP required secrets check until config and secrets parse\n"),
    }

    for command in ["codex", "claude"] {
        if command_in_path(command) {
            report.push_str(&format!("OK {command} found in PATH\n"));
        } else {
            report.push_str(&format!(
                "WARNING {command} not found in PATH; install it before using the matching launcher\n"
            ));
        }
    }

    if ok {
        report.push_str("\nDoctor result: OK\n");
    } else {
        report.push_str("\nDoctor result: problems found\n");
    }
    (report, ok)
}

fn existing_config_summary(config_path: &Path, secrets_path: &Path) -> String {
    let mut summary = format!(
        "\nExisting llmup config found:\n  config: {}\n  secrets: {}\n",
        config_path.display(),
        secrets_path.display()
    );

    let config = match load_valid_config(config_path) {
        Ok(config) => config,
        Err(error) => {
            summary.push_str(&format!("  Config status: invalid ({error})\n"));
            return summary;
        }
    };

    append_alias_summary(&mut summary, &config);
    append_upstream_summary(&mut summary, &config);

    match read_env_file(secrets_path) {
        Ok(secrets) => {
            let configured = if provider_api_key_configured(&config, Some(&secrets)) {
                "yes"
            } else {
                "no"
            };
            summary.push_str(&format!("  Provider API key configured: {configured}\n"));
        }
        Err(error) => {
            summary.push_str(&format!("  Secrets status: invalid ({error})\n"));
            summary.push_str("  Provider API key configured: no\n");
        }
    }

    summary
}

fn load_valid_config(path: &Path) -> Result<Config, String> {
    let config = Config::from_yaml_path(path)?;
    config.validate()?;
    Ok(config)
}

fn append_alias_summary(summary: &mut String, config: &Config) {
    match config.model_aliases.len() {
        0 => summary.push_str("  Alias: none configured\n"),
        1 => {
            let (alias, target) = config
                .model_aliases
                .iter()
                .next()
                .expect("one alias should exist");
            summary.push_str(&format!(
                "  Alias: {alias} -> {}:{}\n",
                target.upstream_name, target.upstream_model
            ));
        }
        _ => {
            summary.push_str("  Aliases:\n");
            for (alias, target) in &config.model_aliases {
                summary.push_str(&format!(
                    "    {alias} -> {}:{}\n",
                    target.upstream_name, target.upstream_model
                ));
            }
        }
    }
}

fn append_upstream_summary(summary: &mut String, config: &Config) {
    match config.upstreams.len() {
        0 => summary.push_str("  Upstream: none configured\n"),
        1 => {
            let upstream = &config.upstreams[0];
            let format = upstream
                .fixed_upstream_format
                .map(user_facing_format_name)
                .unwrap_or_else(|| "auto".to_string());
            summary.push_str(&format!("  Format: {format}\n"));
            summary.push_str(&format!(
                "  Service URL: {}\n",
                redacted_service_url(&upstream.api_root)
            ));
        }
        _ => {
            summary.push_str("  Upstreams:\n");
            for upstream in &config.upstreams {
                let format = upstream
                    .fixed_upstream_format
                    .map(user_facing_format_name)
                    .unwrap_or_else(|| "auto".to_string());
                summary.push_str(&format!(
                    "    {}: format {}, service URL {}\n",
                    upstream.name,
                    format,
                    redacted_service_url(&upstream.api_root)
                ));
            }
        }
    }
}

fn user_facing_format_name(format: UpstreamFormat) -> String {
    match format {
        UpstreamFormat::OpenAiChatCompletions => "openai-chat-completions",
        UpstreamFormat::OpenAiResponses => "openai-responses",
        UpstreamFormat::Anthropic => "anthropic-messages",
    }
    .to_string()
}

fn provider_api_key_configured(config: &Config, secrets: Option<&EnvFile>) -> bool {
    let mut has_inline_provider_key = false;
    let mut provider_env_names = BTreeSet::new();
    for upstream in &config.upstreams {
        if let Some(name) = upstream.provider_key_env.as_deref() {
            provider_env_names.insert(name.to_string());
        }
        if let Some(provider_key) = &upstream.provider_key {
            if provider_key
                .inline
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                has_inline_provider_key = true;
            }
            if let Some(name) = provider_key.env.as_deref() {
                provider_env_names.insert(name.to_string());
            }
        }
    }

    if provider_env_names.is_empty() {
        return has_inline_provider_key;
    }

    let Some(secrets) = secrets else {
        return false;
    };
    provider_env_names
        .iter()
        .all(|name| secrets.required(name).is_ok())
}

fn required_secret_env_names(config: &Config) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if let Some(data_auth) = &config.data_auth {
        if matches!(data_auth.mode, DataAuthMode::ProxyKey) {
            if let Some(proxy_key) = &data_auth.proxy_key {
                if let Some(name) = proxy_key.env.as_deref() {
                    names.insert(name.to_string());
                }
            }
        }
    }

    for upstream in &config.upstreams {
        if let Some(name) = upstream.provider_key_env.as_deref() {
            names.insert(name.to_string());
        }
        if let Some(provider_key) = &upstream.provider_key {
            if let Some(name) = provider_key.env.as_deref() {
                names.insert(name.to_string());
            }
        }
    }
    names
}

fn redacted_service_url(value: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(value) else {
        return "<invalid URL>".to_string();
    };
    if !parsed.username().is_empty() {
        let _ = parsed.set_username("redacted");
    }
    if parsed.password().is_some() {
        let _ = parsed.set_password(Some("redacted"));
    }
    if parsed.query().is_some() {
        parsed.set_query(Some("redacted=true"));
    }
    if parsed.fragment().is_some() {
        parsed.set_fragment(Some("redacted"));
    }
    parsed.to_string()
}

#[cfg(unix)]
fn safe_secret_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 == 0 {
        Ok(())
    } else {
        Err(format!(
            "{} is mode {:03o}; run chmod 600 {}",
            path.display(),
            mode,
            path.display()
        ))
    }
}

#[cfg(unix)]
fn command_in_path(command: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(command);
        fs::metadata(candidate)
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

#[cfg(windows)]
fn command_in_path(command: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    let extensions = std::env::var_os("PATHEXT")
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
    std::env::split_paths(&paths).any(|dir| {
        if dir.join(command).is_file() {
            return true;
        }
        extensions
            .split(';')
            .any(|ext| dir.join(format!("{command}{ext}")).is_file())
    })
}

fn parse_set_limits_args(args: &[OsString]) -> Result<SetLimitsCliOptions, String> {
    let mut target = None;
    let mut context_window = None;
    let mut max_output_tokens = None;

    let mut index = 0;
    while index < args.len() {
        let arg = args[index]
            .to_str()
            .ok_or_else(|| "llmup-config set-limits arguments must be valid UTF-8".to_string())?;
        if let Some(value) = inline_value(arg, "--alias") {
            set_limits_target(
                &mut target,
                SetLimitsTarget::Alias(parse_set_limits_target_name("--alias", value)?),
            )?;
            index += 1;
        } else if arg == "--alias" {
            let value = take_utf8_value(args, &mut index, "--alias")?;
            set_limits_target(
                &mut target,
                SetLimitsTarget::Alias(parse_set_limits_target_name("--alias", &value)?),
            )?;
        } else if let Some(value) = inline_value(arg, "--upstream") {
            set_limits_target(
                &mut target,
                SetLimitsTarget::Upstream(parse_set_limits_target_name("--upstream", value)?),
            )?;
            index += 1;
        } else if arg == "--upstream" {
            let value = take_utf8_value(args, &mut index, "--upstream")?;
            set_limits_target(
                &mut target,
                SetLimitsTarget::Upstream(parse_set_limits_target_name("--upstream", &value)?),
            )?;
        } else if let Some(value) = inline_value(arg, "--context-window") {
            set_set_limits_number(&mut context_window, "--context-window", value)?;
            index += 1;
        } else if arg == "--context-window" {
            let value = take_utf8_value(args, &mut index, "--context-window")?;
            set_set_limits_number(&mut context_window, "--context-window", &value)?;
        } else if let Some(value) = inline_value(arg, "--max-output-tokens") {
            set_set_limits_number(&mut max_output_tokens, "--max-output-tokens", value)?;
            index += 1;
        } else if arg == "--max-output-tokens" {
            let value = take_utf8_value(args, &mut index, "--max-output-tokens")?;
            set_set_limits_number(&mut max_output_tokens, "--max-output-tokens", &value)?;
        } else {
            return Err(format!("unknown llmup-config set-limits argument `{arg}`"));
        }
    }

    let target = target
        .ok_or_else(|| "choose exactly one of --alias <name> or --upstream <name>".to_string())?;
    let context_window =
        context_window.ok_or_else(|| "--context-window is required".to_string())?;
    let max_output_tokens =
        max_output_tokens.ok_or_else(|| "--max-output-tokens is required".to_string())?;
    validate_set_limits_numbers(context_window, max_output_tokens)?;

    Ok(SetLimitsCliOptions {
        target,
        context_window,
        max_output_tokens,
    })
}

fn parse_set_limits_target_name(flag: &str, value: &str) -> Result<String, String> {
    validate_plain_yaml_scalar(flag, value)?;
    Ok(value.to_string())
}

fn set_limits_target(
    target: &mut Option<SetLimitsTarget>,
    next: SetLimitsTarget,
) -> Result<(), String> {
    if target.replace(next).is_some() {
        return Err("choose exactly one of --alias <name> or --upstream <name>".to_string());
    }
    Ok(())
}

fn set_set_limits_number(target: &mut Option<u64>, flag: &str, value: &str) -> Result<(), String> {
    if target.is_some() {
        return Err(format!("{flag} may only be provided once"));
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("{flag} must be a positive integer"))?;
    *target = Some(parsed);
    Ok(())
}

fn validate_set_limits_numbers(context_window: u64, max_output_tokens: u64) -> Result<(), String> {
    if context_window == 0 {
        return Err("--context-window must be greater than zero".to_string());
    }
    if max_output_tokens == 0 {
        return Err("--max-output-tokens must be greater than zero".to_string());
    }
    if max_output_tokens >= context_window {
        return Err("--max-output-tokens must be less than --context-window".to_string());
    }
    Ok(())
}

fn set_limits_in_config(path: &Path, options: &SetLimitsCliOptions) -> Result<String, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("failed to read config {}: {error}", path.display()))?;
    let mut value: Value = serde_yaml::from_str(&raw)
        .map_err(|error| format!("failed to parse config {}: {error}", path.display()))?;
    let original_value = value.clone();
    let limits = model_limits_value(options.context_window, options.max_output_tokens);

    match &options.target {
        SetLimitsTarget::Alias(alias) => update_alias_limits(&mut value, alias, limits)?,
        SetLimitsTarget::Upstream(upstream) => {
            update_upstream_limits(&mut value, upstream, limits)?;
        }
    }

    let rendered = serde_yaml::to_string(&value)
        .map_err(|error| format!("failed to render updated config: {error}"))?;
    let config = Config::from_yaml_str(&rendered)
        .map_err(|error| format!("updated config would be invalid: {error}"))?;
    config
        .validate()
        .map_err(|error| format!("updated config would be invalid: {error}"))?;

    let changed = value != original_value;
    if changed {
        write_config_file_atomic(path, &rendered)?;
    }

    Ok(set_limits_summary(path, options, changed))
}

fn update_alias_limits(root: &mut Value, alias: &str, limits: Value) -> Result<(), String> {
    let root = yaml_mapping_mut(root, "config root")?;
    let aliases = root
        .get_mut(&yaml_key("model_aliases"))
        .ok_or_else(|| format!("unknown alias `{alias}`"))?;
    let aliases = yaml_mapping_mut(aliases, "model_aliases")?;
    let alias_value = aliases
        .get_mut(&yaml_key(alias))
        .ok_or_else(|| format!("unknown alias `{alias}`"))?;

    match alias_value {
        Value::String(target) => {
            validate_alias_target_string(alias, target)?;
            let mut structured = Mapping::new();
            structured.insert(yaml_key("target"), Value::String(target.clone()));
            structured.insert(yaml_key("limits"), limits);
            *alias_value = Value::Mapping(structured);
            Ok(())
        }
        Value::Mapping(mapping) => {
            validate_alias_target_value(alias, mapping.get(&yaml_key("target")))?;
            mapping.insert(yaml_key("limits"), limits);
            Ok(())
        }
        _ => Err(format!(
            "alias `{alias}` must be a string target or an object with `target`"
        )),
    }
}

fn update_upstream_limits(root: &mut Value, upstream: &str, limits: Value) -> Result<(), String> {
    let root = yaml_mapping_mut(root, "config root")?;
    let upstreams = root
        .get_mut(&yaml_key("upstreams"))
        .ok_or_else(|| format!("unknown upstream `{upstream}`"))?;
    let upstreams = yaml_mapping_mut(upstreams, "upstreams")?;
    let upstream_value = upstreams
        .get_mut(&yaml_key(upstream))
        .ok_or_else(|| format!("unknown upstream `{upstream}`"))?;
    let upstream_mapping = yaml_mapping_mut(upstream_value, &format!("upstream `{upstream}`"))?;
    upstream_mapping.insert(yaml_key("limits"), limits);
    Ok(())
}

fn validate_alias_target_value(alias: &str, target: Option<&Value>) -> Result<(), String> {
    let Some(Value::String(target)) = target else {
        return Err(format!(
            "alias `{alias}` object must contain string `target`"
        ));
    };
    validate_alias_target_string(alias, target)
}

fn validate_alias_target_string(alias: &str, target: &str) -> Result<(), String> {
    let Some((upstream, model)) = target.split_once(':') else {
        return Err(format!(
            "alias `{alias}` target must use upstream:model syntax"
        ));
    };
    if upstream.trim().is_empty() || model.trim().is_empty() {
        return Err(format!(
            "alias `{alias}` target must use non-empty upstream:model syntax"
        ));
    }
    Ok(())
}

fn yaml_mapping_mut<'a>(value: &'a mut Value, owner: &str) -> Result<&'a mut Mapping, String> {
    value
        .as_mapping_mut()
        .ok_or_else(|| format!("{owner} must be a YAML mapping"))
}

fn yaml_key(key: &str) -> Value {
    Value::String(key.to_string())
}

fn model_limits_value(context_window: u64, max_output_tokens: u64) -> Value {
    let mut limits = Mapping::new();
    limits.insert(
        yaml_key("context_window"),
        serde_yaml::to_value(context_window).expect("u64 should serialize to YAML"),
    );
    limits.insert(
        yaml_key("max_output_tokens"),
        serde_yaml::to_value(max_output_tokens).expect("u64 should serialize to YAML"),
    );
    Value::Mapping(limits)
}

fn set_limits_summary(path: &Path, options: &SetLimitsCliOptions, changed: bool) -> String {
    let target = match &options.target {
        SetLimitsTarget::Alias(alias) => format!("alias {alias}"),
        SetLimitsTarget::Upstream(upstream) => format!("upstream {upstream}"),
    };
    let action = if changed {
        "Updated"
    } else {
        "limits unchanged for"
    };
    if changed {
        format!(
            "{action} {target} limits in {}: context_window={} max_output_tokens={}\n",
            path.display(),
            options.context_window,
            options.max_output_tokens
        )
    } else {
        format!(
            "{action} {target} in {}: context_window={} max_output_tokens={}\n",
            path.display(),
            options.context_window,
            options.max_output_tokens
        )
    }
}

fn write_config_file_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("failed to find parent directory for {}", path.display()))?;
    let original_permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("config.yaml");
    let temp_path = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

            let mode = original_permissions
                .as_ref()
                .map(|permissions| permissions.mode() & 0o777)
                .unwrap_or(0o600);
            options.mode(mode);
        }
        let mut file = options.open(&temp_path).map_err(|error| {
            format!(
                "failed to write temporary config {}: {error}",
                temp_path.display()
            )
        })?;
        if let Some(permissions) = original_permissions.clone() {
            fs::set_permissions(&temp_path, permissions).map_err(|error| {
                format!(
                    "failed to preserve permissions on temporary config {}: {error}",
                    temp_path.display()
                )
            })?;
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600)).map_err(
                    |error| {
                        format!(
                            "failed to secure temporary config {}: {error}",
                            temp_path.display()
                        )
                    },
                )?;
            }
        }
        file.write_all(contents.as_bytes()).map_err(|error| {
            format!(
                "failed to write temporary config {}: {error}",
                temp_path.display()
            )
        })?;
        file.flush().map_err(|error| {
            format!(
                "failed to flush temporary config {}: {error}",
                temp_path.display()
            )
        })?;
        file.sync_all().map_err(|error| {
            format!(
                "failed to sync temporary config {}: {error}",
                temp_path.display()
            )
        })?;
        drop(file);
        fs::rename(&temp_path, path).map_err(|error| {
            format!(
                "failed to replace config {} with {}: {error}",
                path.display(),
                temp_path.display()
            )
        })
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn parse_hidden_init_args(args: &[OsString]) -> Result<InitCliOptions, String> {
    let mut non_interactive = false;
    let mut interface = ProviderInterface::OpenAiChatCompletions;
    let mut model_service_url = None;
    let mut model_name = None;
    let mut model_alias = "default".to_string();
    let mut force = false;
    let mut api_key_source = None;

    let mut index = 0;
    while index < args.len() {
        let arg = args[index]
            .to_str()
            .ok_or_else(|| "llmup-config init arguments must be valid UTF-8".to_string())?;
        match arg {
            "--non-interactive" => {
                non_interactive = true;
                index += 1;
            }
            "--force" => {
                force = true;
                index += 1;
            }
            "--api-key-stdin" => {
                set_api_key_source(&mut api_key_source, ApiKeySource::Stdin)?;
                index += 1;
            }
            "--api-key" => {
                return Err(
                    "--api-key <value> is not supported; use --api-key-stdin or --api-key-env"
                        .to_string(),
                );
            }
            value if value.starts_with("--api-key=") => {
                return Err(
                    "--api-key=<value> is not supported; use --api-key-stdin or --api-key-env"
                        .to_string(),
                );
            }
            _ => {
                if let Some(value) = inline_value(arg, "--interface") {
                    interface = ProviderInterface::parse(value)?;
                    index += 1;
                } else if arg == "--interface" {
                    let value = take_utf8_value(args, &mut index, "--interface")?;
                    interface = ProviderInterface::parse(&value)?;
                } else if let Some(value) = inline_value(arg, "--model-service-url") {
                    model_service_url = Some(value.to_string());
                    index += 1;
                } else if arg == "--model-service-url" {
                    model_service_url =
                        Some(take_utf8_value(args, &mut index, "--model-service-url")?);
                } else if let Some(value) = inline_value(arg, "--model-name") {
                    model_name = Some(value.to_string());
                    index += 1;
                } else if arg == "--model-name" {
                    model_name = Some(take_utf8_value(args, &mut index, "--model-name")?);
                } else if let Some(value) = inline_value(arg, "--model-alias") {
                    model_alias = value.to_string();
                    index += 1;
                } else if arg == "--model-alias" {
                    model_alias = take_utf8_value(args, &mut index, "--model-alias")?;
                } else if let Some(value) = inline_value(arg, "--api-key-env") {
                    set_api_key_source(&mut api_key_source, ApiKeySource::Env(value.to_string()))?;
                    index += 1;
                } else if arg == "--api-key-env" {
                    let value = take_utf8_value(args, &mut index, "--api-key-env")?;
                    set_api_key_source(&mut api_key_source, ApiKeySource::Env(value))?;
                } else {
                    return Err(format!("unknown llmup-config init argument `{arg}`"));
                }
            }
        }
    }

    if !non_interactive {
        return Err("hidden init requires --non-interactive".to_string());
    }

    Ok(InitCliOptions {
        interface,
        model_service_url: model_service_url
            .ok_or_else(|| "--model-service-url is required".to_string())?,
        model_name: model_name.ok_or_else(|| "--model-name is required".to_string())?,
        model_alias,
        force,
        api_key_source: api_key_source
            .ok_or_else(|| "--api-key-stdin or --api-key-env is required".to_string())?,
    })
}

fn set_api_key_source(
    target: &mut Option<ApiKeySource>,
    source: ApiKeySource,
) -> Result<(), String> {
    if target.replace(source).is_some() {
        return Err("choose only one of --api-key-stdin or --api-key-env".to_string());
    }
    Ok(())
}

fn inline_value<'a>(arg: &'a str, flag: &str) -> Option<&'a str> {
    arg.strip_prefix(flag)?.strip_prefix('=')
}

fn take_utf8_value(args: &[OsString], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    let value = args
        .get(*index)
        .ok_or_else(|| format!("missing value for {flag}"))?
        .to_str()
        .ok_or_else(|| format!("{flag} value must be valid UTF-8"))?
        .to_string();
    *index += 1;
    Ok(value)
}

fn read_api_key_from_stdin(stdin: &mut dyn Read) -> Result<String, String> {
    let mut input = String::new();
    stdin
        .read_to_string(&mut input)
        .map_err(|error| format!("failed to read API key from stdin: {error}"))?;
    while input.ends_with('\n') || input.ends_with('\r') {
        input.pop();
    }
    if input.trim().is_empty() {
        return Err("API key read from stdin must not be empty".to_string());
    }
    Ok(input)
}

fn generated_config_yaml(options: &InitOptions) -> String {
    format!(
        "\
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY

upstreams:
  DEFAULT:
    api_root: {api_root}
    format: {format}
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
    surface_defaults:
      modalities:
        input: [\"text\"]
        output: [\"text\"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false

model_aliases:
  {alias}: DEFAULT:{model}
",
        api_root = options.model_service_url,
        format = options.interface.config_format(),
        alias = options.model_alias,
        model = options.model_name
    )
}

fn validate_plain_yaml_scalar(name: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if value.trim() != value || value.contains('\n') || value.contains('\r') {
        return Err(format!("{name} must be a single non-whitespace line"));
    }
    if value.contains('#') {
        return Err(format!("{name} must not contain YAML comment syntax"));
    }
    Ok(())
}

fn validate_alias(value: &str) -> Result<(), String> {
    validate_plain_yaml_scalar("model-alias", value)?;
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(
            "model-alias may only contain ASCII letters, digits, underscore, dash, and dot"
                .to_string(),
        );
    }
    Ok(())
}

fn write_text_file(path: &PathBuf, contents: &str, force: bool) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    if force {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn write_secret_file(path: &PathBuf, contents: &str, force: bool) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    if force {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("failed to chmod {}: {error}", path.display()))?;
    }
    Ok(())
}
