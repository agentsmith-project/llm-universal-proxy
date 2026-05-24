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
            "openai-chat-completions" => Ok(Self::OpenAiChatCompletions),
            "anthropic-messages" => Ok(Self::AnthropicMessages),
            "openai-responses" => Ok(Self::OpenAiResponses),
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
pub enum ConfigCommand {
    Interactive,
    Doctor,
    List,
    Help,
    Version,
    AddModel(AddModelCliOptions),
    SetLimits(SetLimitsCliOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddModelService {
    New {
        service_name: String,
        interface: ProviderInterface,
        model_service_url: String,
        api_key_source: ApiKeySource,
    },
    Existing {
        service_name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddModelCliOptions {
    pub service: AddModelService,
    pub model_name: String,
    pub model_alias: String,
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
  llmup-config list
  llmup-config add-model --new-service --service-name <name> --interface <format> --url <url> --model <provider-model> --alias <alias> --api-key-stdin
  llmup-config add-model --service <name> --model <provider-model> --alias <alias>
  llmup-config set-limits (--alias <name> | --upstream <name>) --context-window <n> --max-output-tokens <n>
  llmup-config --help
  llmup-config --version

Configure llmup for local Codex CLI and Claude Code launchers.

Run without arguments to create the local config used by llmup-codex and
llmup-claude. The default model service type is openai-chat-completions
(/v1/chat/completions); the prompt also accepts openai-responses
(/v1/responses) or anthropic-messages (/v1/messages) when your model service
requires a different type.
Run list or doctor to inspect local config and secrets files without contacting providers.
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
        if args[0] == "doctor" {
            return Ok(ConfigCommand::Doctor);
        }
        if args[0] == "list" {
            return Ok(ConfigCommand::List);
        }
    }

    let Some(first) = args.first().and_then(|item| item.to_str()) else {
        return Err("llmup-config arguments must be valid UTF-8".to_string());
    };
    if first == "set-limits" {
        return parse_set_limits_args(&args[1..]).map(ConfigCommand::SetLimits);
    }
    if first == "add-model" {
        return parse_add_model_args(&args[1..]).map(ConfigCommand::AddModel);
    }
    Err(format!("unknown llmup-config command `{first}`"))
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
        ConfigCommand::List => run_list(stdout),
        ConfigCommand::AddModel(options) => {
            let api_key = match &options.service {
                AddModelService::New {
                    api_key_source: ApiKeySource::Stdin,
                    ..
                } => Some(read_api_key_from_stdin(stdin)?),
                AddModelService::New {
                    api_key_source: ApiKeySource::Env(name),
                    ..
                } => Some(std::env::var(name).map_err(|error| {
                    format!("failed to read API key from environment variable `{name}`: {error}")
                })?),
                AddModelService::Existing { .. } => None,
            };
            let home = home_dir_from_env()?;
            let llmup_home = env_path_or_default("LLMUP_HOME", home.join(".llmup"));
            let config_path = llmup_home.join("config.yaml");
            let secrets_path = llmup_home.join("secrets.env");
            let summary = add_model_to_local_config(
                &config_path,
                &secrets_path,
                &options,
                api_key.as_deref(),
            )?;
            stdout
                .write_all(summary.as_bytes())
                .map_err(|error| format!("failed to write summary: {error}"))?;
            Ok(0)
        }
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
        "llmup setup wizard\n\nThis connects one model service to this machine for llmup-codex and llmup-claude.\nAPI keys are saved locally."
    )
    .map_err(|error| format!("failed to write prompt: {error}"))?;

    if config_path.exists() || secrets_path.exists() {
        stdout
            .write_all(existing_config_summary(&config_path, &secrets_path).as_bytes())
            .map_err(|error| format!("failed to write existing config summary: {error}"))?;
        let rename_available = load_valid_config(&config_path)
            .map(|config| legacy_default_rename_available(&config))
            .unwrap_or(false);
        let prompt = if rename_available {
            "This config uses legacy local model `default`. Press Enter to rename it to `main`, or type reconfigure or doctor: "
        } else {
            "Press Enter to finish, type add-model, reconfigure, or doctor: "
        };
        let answer = prompt_optional_line(stdin, stdout, prompt)?;
        match answer.trim().to_ascii_lowercase().as_str() {
            "" if rename_available => {
                let summary = rename_default_alias_to_main(&config_path)?;
                stdout
                    .write_all(summary.as_bytes())
                    .map_err(|error| format!("failed to write summary: {error}"))?;
                return Ok(());
            }
            "" => {
                writeln!(
                    stdout,
                    "Keeping existing config.\nNext: run llmup-codex or llmup-claude."
                )
                .map_err(|error| format!("failed to write summary: {error}"))?;
                return Ok(());
            }
            "reconfigure" => {
                force = true;
            }
            "add-model" if !rename_available => {
                run_interactive_add_model_menu(stdin, stdout, &config_path, &secrets_path)?;
                return Ok(());
            }
            "doctor" => {
                let status = write_doctor_report(&config_path, &secrets_path, stdout)?;
                if status == 0 {
                    return Ok(());
                }
                return Err("doctor found problems in the local llmup config".to_string());
            }
            other => {
                let expected = if rename_available {
                    "press Enter to rename it to `main`, or type reconfigure or doctor"
                } else {
                    "press Enter, type add-model, reconfigure, or doctor"
                };
                return Err(format!(
                    "unknown choice `{other}`; rerun llmup-config and {expected}"
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
        "Model name, for example deepseek-v4-flash: ",
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
            model_alias: "main".to_string(),
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
        "\nModel service type [openai-chat-completions]: openai-chat-completions, openai-responses, or anthropic-messages: ",
    )?;
    let value = value.trim();
    if value.is_empty() {
        Ok(ProviderInterface::OpenAiChatCompletions)
    } else {
        ProviderInterface::parse(value)
            .map_err(|_| {
                format!(
                    "unsupported model service type `{value}`; use openai-chat-completions, openai-responses, or anthropic-messages"
                )
            })
    }
}

fn run_interactive_add_model_menu(
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    config_path: &Path,
    secrets_path: &Path,
) -> Result<(), String> {
    let answer = prompt_optional_line(
        stdin,
        stdout,
        "Add or change local model: type new-service or existing-service: ",
    )?;
    let options = match answer.trim().to_ascii_lowercase().as_str() {
        "new-service" => prompt_add_new_service(stdin, stdout)?,
        "existing-service" => prompt_add_alias_for_existing_service(stdin, stdout)?,
        other => {
            return Err(format!(
                "unknown add-model choice `{other}`; type new-service or existing-service"
            ));
        }
    };

    let api_key = match &options.service {
        AddModelService::New { .. } => Some(prompt_required_line(
            stdin,
            stdout,
            "Provider API key (saved locally; not printed again): ",
            "provider API key",
        )?),
        AddModelService::Existing { .. } => None,
    };
    let summary =
        add_model_to_local_config(config_path, secrets_path, &options, api_key.as_deref())?;
    stdout
        .write_all(summary.as_bytes())
        .map_err(|error| format!("failed to write summary: {error}"))?;
    Ok(())
}

fn prompt_add_new_service(
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<AddModelCliOptions, String> {
    let service_name = prompt_required_line(
        stdin,
        stdout,
        "Model service name, for example backup: ",
        "model service name",
    )?;
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
        "Provider model name, for example deepseek-v4-flash: ",
        "provider model name",
    )?;
    let model_alias = prompt_required_line(
        stdin,
        stdout,
        "Local model name, for example sonnet: ",
        "local model name",
    )?;

    Ok(AddModelCliOptions {
        service: AddModelService::New {
            service_name,
            interface,
            model_service_url,
            api_key_source: ApiKeySource::Stdin,
        },
        model_name,
        model_alias,
    })
}

fn prompt_add_alias_for_existing_service(
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<AddModelCliOptions, String> {
    let service_name = prompt_required_line(
        stdin,
        stdout,
        "Existing model service name: ",
        "model service name",
    )?;
    let model_name = prompt_required_line(
        stdin,
        stdout,
        "Provider model name, for example deepseek-v4-flash: ",
        "provider model name",
    )?;
    let model_alias = prompt_required_line(
        stdin,
        stdout,
        "Local model name, for example fast: ",
        "local model name",
    )?;

    Ok(AddModelCliOptions {
        service: AddModelService::Existing { service_name },
        model_name,
        model_alias,
    })
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
    validate_new_model_alias(&options.model_alias)?;
    if api_key.trim().is_empty() {
        return Err("API key must not be empty".to_string());
    }

    let config_path = options.llmup_home.join("config.yaml");
    let secrets_path = options.llmup_home.join("secrets.env");
    if !options.force {
        if config_path.exists() {
            return Err(format!(
                "{} already exists; run llmup-config and choose reconfigure to replace it",
                config_path.display()
            ));
        }
        if secrets_path.exists() {
            return Err(format!(
                "{} already exists; run llmup-config and choose reconfigure to replace it",
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
    let provider_key_env = provider_key_env_name("main");
    let secrets = format!("LLM_UNIVERSAL_PROXY_KEY={proxy_key}\n{provider_key_env}={api_key}\n");
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

fn run_list(stdout: &mut dyn Write) -> Result<i32, String> {
    let home = home_dir_from_env()?;
    let llmup_home = env_path_or_default("LLMUP_HOME", home.join(".llmup"));
    let config_path = llmup_home.join("config.yaml");
    let secrets_path = llmup_home.join("secrets.env");
    stdout
        .write_all(existing_config_summary(&config_path, &secrets_path).as_bytes())
        .map_err(|error| format!("failed to write config summary: {error}"))?;
    Ok(0)
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

    if let Some(config) = config.as_ref() {
        append_legacy_default_hint(&mut report, config);
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
        "\nCurrent llmup config\n\nPaths:\n  config: {}\n  secrets: {}\n",
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

    let secrets = match read_env_file(secrets_path) {
        Ok(secrets) => Some(secrets),
        Err(error) => {
            summary.push_str(&format!("\nSecrets status: invalid ({error})\n"));
            None
        }
    };

    append_upstream_summary(&mut summary, &config, secrets.as_ref());
    append_alias_summary(&mut summary, &config);
    append_secret_summary(&mut summary, &config, secrets.as_ref());
    append_legacy_default_hint(&mut summary, &config);

    summary
}

fn load_valid_config(path: &Path) -> Result<Config, String> {
    let config = Config::from_yaml_path(path)?;
    config.validate()?;
    Ok(config)
}

fn append_alias_summary(summary: &mut String, config: &Config) {
    summary.push_str("\nLocal models:\n");
    match config.model_aliases.len() {
        0 => summary.push_str("  none configured\n"),
        _ => {
            for (alias, target) in &config.model_aliases {
                summary.push_str(&format!(
                    "  {alias} -> {}:{}\n",
                    target.upstream_name, target.upstream_model
                ));
            }
        }
    }
}

fn append_upstream_summary(summary: &mut String, config: &Config, secrets: Option<&EnvFile>) {
    summary.push_str("\nModel services:\n");
    match config.upstreams.len() {
        0 => summary.push_str("  none configured\n"),
        _ => {
            for upstream in &config.upstreams {
                let format = upstream
                    .fixed_upstream_format
                    .map(user_facing_format_name)
                    .unwrap_or_else(|| "auto".to_string());
                summary.push_str(&format!(
                    "  {}  {}  {}\n",
                    upstream.name,
                    format,
                    redacted_service_url(&upstream.api_root)
                ));
                append_upstream_key_status(summary, upstream, secrets);
            }
        }
    }
}

fn append_upstream_key_status(
    summary: &mut String,
    upstream: &crate::config::UpstreamConfig,
    secrets: Option<&EnvFile>,
) {
    let mut env_names = BTreeSet::new();
    if let Some(name) = upstream.provider_key_env.as_deref() {
        env_names.insert(name);
    }
    if let Some(provider_key) = &upstream.provider_key {
        if provider_key
            .inline
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            summary.push_str("    provider key: inline configured\n");
        }
        if let Some(name) = provider_key.env.as_deref() {
            env_names.insert(name);
        }
    }

    if env_names.is_empty() {
        return;
    }
    for name in env_names {
        let status = match secrets {
            Some(secrets) if secrets.required(name).is_ok() => "configured",
            _ => "missing",
        };
        summary.push_str(&format!("    provider key {name}: {status}\n"));
    }
}

fn append_secret_summary(summary: &mut String, config: &Config, secrets: Option<&EnvFile>) {
    let provider_status = if provider_api_key_configured(config, secrets) {
        "all configured"
    } else {
        "missing or incomplete"
    };
    summary.push_str(&format!("\nProvider API keys: {provider_status}\n"));

    let proxy_status = match (config.data_auth.as_ref(), secrets) {
        (Some(data_auth), Some(secrets)) if matches!(data_auth.mode, DataAuthMode::ProxyKey) => {
            data_auth
                .proxy_key
                .as_ref()
                .and_then(|proxy_key| proxy_key.env.as_deref())
                .map(|name| {
                    if secrets.required(name).is_ok() {
                        "configured"
                    } else {
                        "missing"
                    }
                })
                .unwrap_or("inline or not required")
        }
        (Some(data_auth), _) if matches!(data_auth.mode, DataAuthMode::ProxyKey) => "missing",
        _ => "not required",
    };
    summary.push_str(&format!("Proxy key: {proxy_status}\n"));
}

fn append_legacy_default_hint(summary: &mut String, config: &Config) {
    if legacy_default_rename_available(config) {
        summary.push_str(
            "\nAction needed: this config uses legacy local model `default`, but llmup launchers use `main` by default. Run `llmup-config` and press Enter to rename `default` to `main`.\n",
        );
    }
}

fn legacy_default_rename_available(config: &Config) -> bool {
    config.model_aliases.contains_key("default") && !config.model_aliases.contains_key("main")
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

fn rename_default_alias_to_main(config_path: &Path) -> Result<String, String> {
    let raw = fs::read_to_string(config_path)
        .map_err(|error| format!("failed to read config {}: {error}", config_path.display()))?;
    let config = Config::from_yaml_str(&raw)
        .map_err(|error| format!("invalid config {}: {error}", config_path.display()))?;
    config
        .validate()
        .map_err(|error| format!("invalid config {}: {error}", config_path.display()))?;
    if config.model_aliases.contains_key("main") {
        return Err(
            "alias `main` already exists; cannot rename legacy alias `default`".to_string(),
        );
    }
    if !config.model_aliases.contains_key("default") {
        return Err("alias `default` was not found; nothing to rename".to_string());
    }

    let mut value: Value = serde_yaml::from_str(&raw)
        .map_err(|error| format!("failed to parse config {}: {error}", config_path.display()))?;
    let root = yaml_mapping_mut(&mut value, "config root")?;
    let aliases_value = root
        .get_mut(yaml_key("model_aliases"))
        .ok_or_else(|| "model_aliases must exist to rename alias `default`".to_string())?;
    let aliases = yaml_mapping_mut(aliases_value, "model_aliases")?;
    if aliases.contains_key(yaml_key("main")) {
        return Err(
            "alias `main` already exists; cannot rename legacy alias `default`".to_string(),
        );
    }
    let default_value = aliases
        .remove(yaml_key("default"))
        .ok_or_else(|| "alias `default` was not found; nothing to rename".to_string())?;
    aliases.insert(yaml_key("main"), default_value);

    let rendered = serde_yaml::to_string(&value)
        .map_err(|error| format!("failed to render updated config: {error}"))?;
    let updated = Config::from_yaml_str(&rendered)
        .map_err(|error| format!("updated config would be invalid: {error}"))?;
    updated
        .validate()
        .map_err(|error| format!("updated config would be invalid: {error}"))?;
    write_config_file_atomic(config_path, &rendered)?;
    Ok(format!(
        "Renamed local model default -> main in {}\n",
        config_path.display()
    ))
}

fn add_model_to_local_config(
    config_path: &Path,
    secrets_path: &Path,
    options: &AddModelCliOptions,
    api_key: Option<&str>,
) -> Result<String, String> {
    validate_plain_yaml_scalar("provider-model", &options.model_name)?;
    validate_new_model_alias(&options.model_alias)?;

    let raw = fs::read_to_string(config_path)
        .map_err(|error| format!("failed to read config {}: {error}", config_path.display()))?;
    let config = Config::from_yaml_str(&raw)
        .map_err(|error| format!("invalid config {}: {error}", config_path.display()))?;
    config
        .validate()
        .map_err(|error| format!("invalid config {}: {error}", config_path.display()))?;
    ensure_no_normalized_collision(
        "alias",
        &options.model_alias,
        config.model_aliases.keys().map(String::as_str),
    )?;

    let mut value: Value = serde_yaml::from_str(&raw)
        .map_err(|error| format!("failed to parse config {}: {error}", config_path.display()))?;
    let root = yaml_mapping_mut(&mut value, "config root")?;

    let (service_name, service_created, provider_key_env) = match &options.service {
        AddModelService::Existing { service_name } => {
            validate_existing_service_name(service_name)?;
            if config.upstream(service_name).is_none() {
                return Err(format!("unknown model service `{service_name}`"));
            }
            (service_name.clone(), false, None)
        }
        AddModelService::New {
            service_name,
            interface,
            model_service_url,
            ..
        } => {
            validate_new_service_name(service_name)?;
            validate_plain_yaml_scalar("model-service-url", model_service_url)?;
            ensure_no_normalized_collision(
                "service",
                service_name,
                config
                    .upstreams
                    .iter()
                    .map(|upstream| upstream.name.as_str()),
            )?;
            let provider_key_env = provider_key_env_name(service_name);
            add_upstream_to_yaml(
                root,
                service_name,
                *interface,
                model_service_url,
                &provider_key_env,
            )?;
            (service_name.clone(), true, Some(provider_key_env))
        }
    };

    add_alias_to_yaml(
        root,
        &options.model_alias,
        &service_name,
        &options.model_name,
    )?;

    let rendered = serde_yaml::to_string(&value)
        .map_err(|error| format!("failed to render updated config: {error}"))?;
    let updated = Config::from_yaml_str(&rendered)
        .map_err(|error| format!("updated config would be invalid: {error}"))?;
    updated
        .validate()
        .map_err(|error| format!("updated config would be invalid: {error}"))?;

    if let Some(provider_key_env) = provider_key_env.as_deref() {
        let api_key = api_key.ok_or_else(|| "provider API key is required".to_string())?;
        append_secret_env(secrets_path, provider_key_env, api_key)?;
    }
    write_config_file_atomic(config_path, &rendered)?;

    let target = format!("{}:{}", service_name, options.model_name);
    if service_created {
        Ok(format!(
            "Added model service {service_name}\nAdded local model {} -> {target}\nWrote llmup config: {}\nWrote local secrets: {}\n",
            options.model_alias,
            config_path.display(),
            secrets_path.display()
        ))
    } else {
        Ok(format!(
            "Added local model {} -> {target}\nWrote llmup config: {}\n",
            options.model_alias,
            config_path.display()
        ))
    }
}

fn add_upstream_to_yaml(
    root: &mut Mapping,
    service_name: &str,
    interface: ProviderInterface,
    model_service_url: &str,
    provider_key_env: &str,
) -> Result<(), String> {
    let upstreams = ensure_yaml_mapping_child(root, "upstreams")?;
    if upstreams.contains_key(yaml_key(service_name)) {
        return Err(format!("model service `{service_name}` already exists"));
    }

    let mut provider_key = Mapping::new();
    provider_key.insert(yaml_key("env"), Value::String(provider_key_env.to_string()));

    let mut upstream = Mapping::new();
    upstream.insert(
        yaml_key("api_root"),
        Value::String(model_service_url.to_string()),
    );
    upstream.insert(
        yaml_key("format"),
        Value::String(interface.config_format().to_string()),
    );
    upstream.insert(yaml_key("provider_key"), Value::Mapping(provider_key));
    upstream.insert(yaml_key("surface_defaults"), default_surface_yaml_value());
    upstreams.insert(yaml_key(service_name), Value::Mapping(upstream));
    Ok(())
}

fn add_alias_to_yaml(
    root: &mut Mapping,
    alias: &str,
    service_name: &str,
    model_name: &str,
) -> Result<(), String> {
    let aliases = ensure_yaml_mapping_child(root, "model_aliases")?;
    if aliases.contains_key(yaml_key(alias)) {
        return Err(format!("alias `{alias}` already exists"));
    }
    aliases.insert(
        yaml_key(alias),
        Value::String(format!("{service_name}:{model_name}")),
    );
    Ok(())
}

fn ensure_yaml_mapping_child<'a>(
    root: &'a mut Mapping,
    key: &str,
) -> Result<&'a mut Mapping, String> {
    let yaml_key = yaml_key(key);
    if !root.contains_key(&yaml_key) {
        root.insert(yaml_key.clone(), Value::Mapping(Mapping::new()));
    }
    let value = root
        .get_mut(&yaml_key)
        .ok_or_else(|| format!("missing `{key}` after insertion"))?;
    yaml_mapping_mut(value, key)
}

fn default_surface_yaml_value() -> Value {
    let mut modalities = Mapping::new();
    modalities.insert(
        yaml_key("input"),
        Value::Sequence(vec![Value::String("text".to_string())]),
    );
    modalities.insert(
        yaml_key("output"),
        Value::Sequence(vec![Value::String("text".to_string())]),
    );

    let mut tools = Mapping::new();
    tools.insert(yaml_key("supports_search"), Value::Bool(false));
    tools.insert(yaml_key("supports_view_image"), Value::Bool(false));
    tools.insert(
        yaml_key("apply_patch_transport"),
        Value::String("freeform".to_string()),
    );
    tools.insert(yaml_key("supports_parallel_calls"), Value::Bool(false));

    let mut surface = Mapping::new();
    surface.insert(yaml_key("modalities"), Value::Mapping(modalities));
    surface.insert(yaml_key("tools"), Value::Mapping(tools));
    Value::Mapping(surface)
}

fn append_secret_env(path: &Path, name: &str, value: &str) -> Result<(), String> {
    let mut secrets = read_env_file(path)?;
    secrets.insert(name.to_string(), value.to_string())?;
    let rendered = render_env_file(&secrets);
    super::env_file::parse_env_file_str(&rendered)?;
    write_secret_file(&path.to_path_buf(), &rendered, true)
}

fn render_env_file(secrets: &EnvFile) -> String {
    let mut rendered = String::new();
    for (name, value) in secrets.iter() {
        rendered.push_str(name);
        rendered.push('=');
        rendered.push_str(value);
        rendered.push('\n');
    }
    rendered
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

fn parse_add_model_args(args: &[OsString]) -> Result<AddModelCliOptions, String> {
    let mut new_service = false;
    let mut service_name = None;
    let mut existing_service = None;
    let mut interface = None;
    let mut model_service_url = None;
    let mut model_name = None;
    let mut model_alias = None;
    let mut api_key_source = None;

    let mut index = 0;
    while index < args.len() {
        let arg = args[index]
            .to_str()
            .ok_or_else(|| "llmup-config add-model arguments must be valid UTF-8".to_string())?;
        match arg {
            "--new-service" => {
                if new_service {
                    return Err("--new-service may only be provided once".to_string());
                }
                new_service = true;
                index += 1;
            }
            "--api-key-stdin" => {
                set_api_key_source(&mut api_key_source, ApiKeySource::Stdin)?;
                index += 1;
            }
            "--api-key" => {
                return Err("--api-key <value> is not supported; use --api-key-stdin".to_string());
            }
            value if value.starts_with("--api-key=") => {
                return Err("--api-key=<value> is not supported; use --api-key-stdin".to_string());
            }
            _ => {
                if let Some(value) = inline_value(arg, "--service-name") {
                    set_once_string(&mut service_name, "--service-name", value)?;
                    index += 1;
                } else if arg == "--service-name" {
                    let value = take_utf8_value(args, &mut index, "--service-name")?;
                    set_once_owned_string(&mut service_name, "--service-name", value)?;
                } else if let Some(value) = inline_value(arg, "--service") {
                    set_once_string(&mut existing_service, "--service", value)?;
                    index += 1;
                } else if arg == "--service" {
                    let value = take_utf8_value(args, &mut index, "--service")?;
                    set_once_owned_string(&mut existing_service, "--service", value)?;
                } else if let Some(value) = inline_value(arg, "--interface") {
                    set_once_interface(&mut interface, value)?;
                    index += 1;
                } else if arg == "--interface" {
                    let value = take_utf8_value(args, &mut index, "--interface")?;
                    set_once_interface(&mut interface, &value)?;
                } else if let Some(value) = inline_value(arg, "--url") {
                    set_once_string(&mut model_service_url, "--url", value)?;
                    index += 1;
                } else if arg == "--url" {
                    let value = take_utf8_value(args, &mut index, "--url")?;
                    set_once_owned_string(&mut model_service_url, "--url", value)?;
                } else if let Some(value) = inline_value(arg, "--model") {
                    set_once_string(&mut model_name, "--model", value)?;
                    index += 1;
                } else if arg == "--model" {
                    let value = take_utf8_value(args, &mut index, "--model")?;
                    set_once_owned_string(&mut model_name, "--model", value)?;
                } else if let Some(value) = inline_value(arg, "--alias") {
                    set_once_string(&mut model_alias, "--alias", value)?;
                    index += 1;
                } else if arg == "--alias" {
                    let value = take_utf8_value(args, &mut index, "--alias")?;
                    set_once_owned_string(&mut model_alias, "--alias", value)?;
                } else {
                    return Err(format!("unknown llmup-config add-model argument `{arg}`"));
                }
            }
        }
    }

    let model_name = model_name.ok_or_else(|| "--model is required".to_string())?;
    let model_alias = model_alias.ok_or_else(|| "--alias is required".to_string())?;

    if new_service {
        if existing_service.is_some() {
            return Err(
                "choose either --new-service with --service-name or --service, not both"
                    .to_string(),
            );
        }
        let service_name = service_name
            .ok_or_else(|| "--service-name is required with --new-service".to_string())?;
        let interface =
            interface.ok_or_else(|| "--interface is required with --new-service".to_string())?;
        let model_service_url =
            model_service_url.ok_or_else(|| "--url is required with --new-service".to_string())?;
        let api_key_source = api_key_source
            .ok_or_else(|| "--api-key-stdin is required with --new-service".to_string())?;
        return Ok(AddModelCliOptions {
            service: AddModelService::New {
                service_name,
                interface,
                model_service_url,
                api_key_source,
            },
            model_name,
            model_alias,
        });
    }

    if service_name.is_some() {
        return Err("--service-name requires --new-service".to_string());
    }
    if interface.is_some() {
        return Err("--interface requires --new-service".to_string());
    }
    if model_service_url.is_some() {
        return Err("--url requires --new-service".to_string());
    }
    if api_key_source.is_some() {
        return Err("--api-key-stdin requires --new-service".to_string());
    }
    let service_name = existing_service.ok_or_else(|| {
        "choose either --new-service with --service-name or --service".to_string()
    })?;
    Ok(AddModelCliOptions {
        service: AddModelService::Existing { service_name },
        model_name,
        model_alias,
    })
}

fn set_once_string(target: &mut Option<String>, flag: &str, value: &str) -> Result<(), String> {
    set_once_owned_string(target, flag, value.to_string())
}

fn set_once_owned_string(
    target: &mut Option<String>,
    flag: &str,
    value: String,
) -> Result<(), String> {
    if target.replace(value).is_some() {
        return Err(format!("{flag} may only be provided once"));
    }
    Ok(())
}

fn set_once_interface(target: &mut Option<ProviderInterface>, value: &str) -> Result<(), String> {
    if target.replace(ProviderInterface::parse(value)?).is_some() {
        return Err("--interface may only be provided once".to_string());
    }
    Ok(())
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
        .get_mut(yaml_key("model_aliases"))
        .ok_or_else(|| format!("unknown alias `{alias}`"))?;
    let aliases = yaml_mapping_mut(aliases, "model_aliases")?;
    let alias_value = aliases
        .get_mut(yaml_key(alias))
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
            validate_alias_target_value(alias, mapping.get(yaml_key("target")))?;
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
        .get_mut(yaml_key("upstreams"))
        .ok_or_else(|| format!("unknown upstream `{upstream}`"))?;
    let upstreams = yaml_mapping_mut(upstreams, "upstreams")?;
    let upstream_value = upstreams
        .get_mut(yaml_key(upstream))
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
  main:
    api_root: {api_root}
    format: {format}
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
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
  {alias}: main:{model}
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

fn validate_new_model_alias(value: &str) -> Result<(), String> {
    validate_new_name("alias", value, true)
}

fn validate_new_service_name(value: &str) -> Result<(), String> {
    validate_new_name("service-name", value, true)
}

fn validate_existing_service_name(value: &str) -> Result<(), String> {
    validate_plain_yaml_scalar("service-name", value)
}

fn validate_new_name(kind: &str, value: &str, reserve_default: bool) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{kind} must not be empty"));
    }
    if value.trim() != value {
        return Err(format!(
            "{kind} must not have leading or trailing whitespace"
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{kind} must not contain spaces"));
    }
    if value.contains(':') {
        return Err(format!("{kind} must not contain `:`"));
    }
    if value.contains('_') || value.contains('.') {
        return Err(format!(
            "{kind} must use hyphen instead of underscore or dot to avoid ambiguous names"
        ));
    }
    if reserve_default && value.eq_ignore_ascii_case("default") {
        return Err(format!(
            "{kind} `{value}` is reserved; use `main` or another explicit name"
        ));
    }
    if value.chars().any(|ch| ch.is_ascii_uppercase()) {
        return Err(format!(
            "{kind} must be lowercase to avoid case-insensitive collisions"
        ));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(format!(
            "{kind} may only contain lowercase ASCII letters, digits, and hyphen"
        ));
    }
    if value.starts_with('-') || value.ends_with('-') {
        return Err(format!("{kind} must not start or end with hyphen"));
    }
    Ok(())
}

fn ensure_no_normalized_collision<'a>(
    kind: &str,
    value: &str,
    existing: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let normalized = normalized_name_key(value);
    for existing in existing {
        if normalized_name_key(existing) == normalized {
            return Err(format!(
                "{kind} `{value}` collides with existing {kind} `{existing}` after case/separator normalization"
            ));
        }
    }
    Ok(())
}

fn normalized_name_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '_' | '.' => '-',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

fn provider_key_env_name(service_name: &str) -> String {
    let upper_snake = service_name.replace('-', "_").to_ascii_uppercase();
    format!("LLMUP_PROVIDER_{upper_snake}_API_KEY")
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
