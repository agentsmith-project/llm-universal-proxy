use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::env_file::{read_env_file, EnvFile};
use super::{env_path_or_default, home_dir_from_env};
use crate::config::DataAuthMode;
use crate::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderInterface {
    OpenAi,
    Anthropic,
    OpenAiResponses,
}

impl ProviderInterface {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "openai" | "openai-completion" => Ok(Self::OpenAi),
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "openai-responses" | "responses" => Ok(Self::OpenAiResponses),
            other => Err(format!(
                "unsupported --interface `{other}`; use openai, anthropic, or openai-responses"
            )),
        }
    }

    fn config_format(self) -> &'static str {
        match self {
            Self::OpenAi => "openai-completion",
            Self::Anthropic => "anthropic",
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
  llmup-config --help
  llmup-config --version

Configure llmup for local Codex CLI and Claude Code launchers.

Run without arguments to create the local config used by llmup-codex and
llmup-claude. The default setup is for OpenAI-compatible providers.
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
        "llmup local setup\n\nThis creates a local proxy config for llmup-codex and llmup-claude.\nThe default is an OpenAI-compatible provider."
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

    let model_service_url = prompt_required_line(
        stdin,
        stdout,
        "\nModel service API root, for example https://api.minimaxi.com/v1: ",
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
            interface: ProviderInterface::OpenAi,
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
                .map(|format| format.to_string())
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
                    .map(|format| format.to_string())
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

fn parse_hidden_init_args(args: &[OsString]) -> Result<InitCliOptions, String> {
    let mut non_interactive = false;
    let mut interface = ProviderInterface::OpenAi;
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
