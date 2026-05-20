use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Map as JsonMap, Value as JsonValue};
use serde_yaml::Value as YamlValue;
use uuid::Uuid;

use super::agent_model_profile::{
    write_codex_model_catalog_for_profiles, AgentModelCatalog, DEFAULT_AGENT_MODEL_ALIAS,
};
use super::env_file::{read_env_file, EnvFile};
use super::{env_path_or_default, home_dir_from_env};
use crate::Config;

const INTERNAL_LAUNCH_PLAN_ENV: &str = "LLMUP_INTERNAL_LAUNCH_PLAN";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Codex,
    Claude,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherControl {
    pub help: bool,
    pub version: bool,
    pub no_proxy: bool,
    pub no_profile_projection: bool,
    pub model_alias: Option<String>,
    pub port: Option<u16>,
    pub config_path: Option<PathBuf>,
    pub env_file_path: Option<PathBuf>,
    pub internal_launch_plan_json: bool,
    pub internal_proxy_base: Option<String>,
    pub internal_proxy_key: Option<String>,
    pub internal_artifact_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLauncherArgs {
    pub control: LauncherControl,
    pub native_argv: Vec<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherHomes {
    pub llmup_home: PathBuf,
    pub codex_home: PathBuf,
    pub claude_config_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyMode {
    Managed {
        port: u16,
        proxy_key: String,
        secrets: EnvFile,
    },
    ManagedExternal {
        proxy_base_url: String,
        proxy_key: String,
        secrets: EnvFile,
    },
    NoProxy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileProjection {
    Enabled {
        model_catalog: Box<AgentModelCatalog>,
        codex_catalog_path: Option<PathBuf>,
    },
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfigFile {
    pub path: PathBuf,
    pub yaml: String,
}

pub fn parse_launcher_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<ParsedLauncherArgs, String> {
    let mut control = LauncherControl {
        help: false,
        version: false,
        no_proxy: false,
        no_profile_projection: false,
        model_alias: None,
        port: None,
        config_path: None,
        env_file_path: None,
        internal_launch_plan_json: false,
        internal_proxy_base: None,
        internal_proxy_key: None,
        internal_artifact_dir: None,
    };
    let args = args.into_iter().collect::<Vec<_>>();
    let mut native_argv = Vec::new();
    let mut after_delimiter = false;
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].clone();
        if after_delimiter {
            native_argv.push(arg);
            index += 1;
            continue;
        }
        if arg == "--" {
            after_delimiter = true;
            index += 1;
            continue;
        }
        let Some(text) = arg.to_str() else {
            native_argv.push(arg);
            index += 1;
            continue;
        };
        match text {
            "--llmup-help" => {
                control.help = true;
                index += 1;
            }
            "--llmup-version" => {
                control.version = true;
                index += 1;
            }
            "--llmup-no-proxy" => {
                control.no_proxy = true;
                index += 1;
            }
            "--llmup-no-profile-projection" => {
                control.no_profile_projection = true;
                index += 1;
            }
            "--llmup-model" => {
                let value = take_nonempty_utf8_value(&args, &mut index, "--llmup-model")?;
                control.model_alias = Some(value);
            }
            "--llmup-port" => {
                let value = take_os_value(&args, &mut index, "--llmup-port")?;
                control.port = Some(parse_port(&value, "--llmup-port")?);
            }
            "--llmup-config" => {
                control.config_path = Some(PathBuf::from(take_os_value(
                    &args,
                    &mut index,
                    "--llmup-config",
                )?));
            }
            "--llmup-env-file" => {
                control.env_file_path = Some(PathBuf::from(take_os_value(
                    &args,
                    &mut index,
                    "--llmup-env-file",
                )?));
            }
            "--llmup-internal-launch-plan-json" => {
                control.internal_launch_plan_json = true;
                index += 1;
            }
            "--llmup-internal-proxy-base" => {
                let value =
                    take_nonempty_utf8_value(&args, &mut index, "--llmup-internal-proxy-base")?;
                control.internal_proxy_base = Some(value);
            }
            "--llmup-internal-proxy-key" => {
                let value =
                    take_nonempty_utf8_value(&args, &mut index, "--llmup-internal-proxy-key")?;
                control.internal_proxy_key = Some(value);
            }
            "--llmup-internal-artifact-dir" => {
                let value = take_os_value(&args, &mut index, "--llmup-internal-artifact-dir")?;
                if value.is_empty() {
                    return Err("--llmup-internal-artifact-dir value must not be empty".to_string());
                }
                control.internal_artifact_dir = Some(PathBuf::from(value));
            }
            _ => {
                if let Some(value) = text.strip_prefix("--llmup-port=") {
                    control.port = Some(parse_port(OsStr::new(value), "--llmup-port")?);
                    index += 1;
                } else if let Some(value) = text.strip_prefix("--llmup-model=") {
                    control.model_alias = Some(parse_nonempty_utf8_value(value, "--llmup-model")?);
                    index += 1;
                } else if let Some(value) = text.strip_prefix("--llmup-config=") {
                    control.config_path = Some(PathBuf::from(value));
                    index += 1;
                } else if let Some(value) = text.strip_prefix("--llmup-env-file=") {
                    control.env_file_path = Some(PathBuf::from(value));
                    index += 1;
                } else if let Some(value) = text.strip_prefix("--llmup-internal-proxy-base=") {
                    control.internal_proxy_base = Some(parse_nonempty_utf8_value(
                        value,
                        "--llmup-internal-proxy-base",
                    )?);
                    index += 1;
                } else if let Some(value) = text.strip_prefix("--llmup-internal-proxy-key=") {
                    control.internal_proxy_key = Some(parse_nonempty_utf8_value(
                        value,
                        "--llmup-internal-proxy-key",
                    )?);
                    index += 1;
                } else if let Some(value) = text.strip_prefix("--llmup-internal-artifact-dir=") {
                    if value.is_empty() {
                        return Err(
                            "--llmup-internal-artifact-dir value must not be empty".to_string()
                        );
                    }
                    control.internal_artifact_dir = Some(PathBuf::from(value));
                    index += 1;
                } else if text.starts_with("--llmup-") {
                    return Err(format!("unknown llmup launcher option `{text}`"));
                } else {
                    native_argv.push(arg);
                    index += 1;
                }
            }
        }
    }

    if control.model_alias.is_some() && control.no_profile_projection {
        return Err(
            "--llmup-model and --llmup-no-profile-projection cannot be used together".to_string(),
        );
    }
    if control.model_alias.is_some() && control.no_proxy {
        return Err("--llmup-model cannot be used with --llmup-no-proxy".to_string());
    }

    Ok(ParsedLauncherArgs {
        control,
        native_argv,
    })
}

pub fn build_client_argv(
    kind: AgentKind,
    mode: &ProxyMode,
    projection: &ProfileProjection,
    native_argv: &[OsString],
) -> Vec<OsString> {
    let mut argv = match kind {
        AgentKind::Codex if mode.is_managed() => {
            let openai_base_url =
                openai_proxy_base_url(mode).expect("managed proxy mode should have a base URL");
            vec![
                "-c".into(),
                "model_provider=\"proxy\"".into(),
                "-c".into(),
                format!("openai_base_url=\"{openai_base_url}\"").into(),
                "-c".into(),
                "model_providers.proxy.name=\"llmup\"".into(),
                "-c".into(),
                format!("model_providers.proxy.base_url=\"{openai_base_url}\"").into(),
                "-c".into(),
                "model_providers.proxy.env_key=\"OPENAI_API_KEY\"".into(),
                "-c".into(),
                "model_providers.proxy.wire_api=\"responses\"".into(),
                "-c".into(),
                "model_providers.proxy.supports_websockets=false".into(),
            ]
        }
        AgentKind::Claude if mode.is_managed() => Vec::new(),
        _ => Vec::new(),
    };
    if mode.is_managed() {
        if let ProfileProjection::Enabled {
            model_catalog,
            codex_catalog_path,
        } = projection
        {
            let selected_profile = &model_catalog.selected;
            match kind {
                AgentKind::Codex => {
                    if let Some(catalog_path) = codex_catalog_path {
                        argv.push("-c".into());
                        argv.push(
                            format!("model_catalog_json=\"{}\"", catalog_path.display()).into(),
                        );
                    }
                    if selected_profile
                        .surface
                        .tools
                        .as_ref()
                        .and_then(|tools| tools.supports_search)
                        == Some(false)
                    {
                        argv.push("-c".into());
                        argv.push("tools.web_search=false".into());
                    }
                    argv.push("-m".into());
                    argv.push(selected_profile.alias.clone().into());
                }
                AgentKind::Claude => {}
            }
        }
    }
    argv.extend(native_argv.iter().cloned());
    argv
}

pub fn prepare_profile_projection(
    kind: AgentKind,
    model_catalog: AgentModelCatalog,
    run_dir: impl AsRef<Path>,
) -> Result<ProfileProjection, String> {
    let codex_catalog_path = match kind {
        AgentKind::Codex => Some(write_codex_model_catalog_for_profiles(
            &model_catalog.profiles,
            run_dir,
        )?),
        AgentKind::Claude => None,
    };
    Ok(ProfileProjection::Enabled {
        model_catalog: Box::new(model_catalog),
        codex_catalog_path,
    })
}

impl ProxyMode {
    fn is_managed(&self) -> bool {
        !matches!(self, ProxyMode::NoProxy)
    }
}

fn managed_proxy_material(mode: &ProxyMode) -> Option<(&str, &EnvFile)> {
    match mode {
        ProxyMode::Managed {
            proxy_key, secrets, ..
        }
        | ProxyMode::ManagedExternal {
            proxy_key, secrets, ..
        } => Some((proxy_key.as_str(), secrets)),
        ProxyMode::NoProxy => None,
    }
}

fn managed_proxy_origin(mode: &ProxyMode) -> Option<String> {
    match mode {
        ProxyMode::Managed { port, .. } => Some(format!("http://127.0.0.1:{port}")),
        ProxyMode::ManagedExternal { proxy_base_url, .. } => {
            Some(proxy_base_url.trim_end_matches('/').to_string())
        }
        ProxyMode::NoProxy => None,
    }
}

fn append_proxy_path(origin: String, path: &str) -> String {
    format!(
        "{}/{}",
        origin.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn openai_proxy_base_url(mode: &ProxyMode) -> Option<String> {
    managed_proxy_origin(mode).map(|origin| append_proxy_path(origin, "openai/v1"))
}

fn anthropic_proxy_base_url(mode: &ProxyMode) -> Option<String> {
    managed_proxy_origin(mode).map(|origin| append_proxy_path(origin, "anthropic"))
}

pub fn validate_native_model_flags(
    kind: AgentKind,
    projection: &ProfileProjection,
    native_argv: &[OsString],
) -> Result<(), String> {
    if !matches!(projection, ProfileProjection::Enabled { .. }) {
        return Ok(());
    }
    let Some(flag) = native_projection_override_flag(kind, native_argv) else {
        return Ok(());
    };
    Err(native_model_flag_error(kind, &flag))
}

pub fn build_client_environment(
    kind: AgentKind,
    parent: BTreeMap<OsString, OsString>,
    mode: &ProxyMode,
    homes: &LauncherHomes,
    projection: &ProfileProjection,
) -> Result<BTreeMap<OsString, OsString>, String> {
    let managed = managed_proxy_material(mode);
    let secret_names = managed
        .map(|(_, secrets)| {
            secrets
                .names()
                .map(|name| OsString::from(name.as_str()))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let secret_values = managed
        .map(|(_, secrets)| {
            secrets
                .secret_values()
                .map(|value| OsString::from(value.as_str()))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let mut env = BTreeMap::new();
    for (key, value) in parent {
        if secret_names.contains(&key) || secret_values.contains(&value) {
            continue;
        }
        if managed.is_some() && kind == AgentKind::Claude && should_scrub_claude_env(&key) {
            continue;
        }
        if managed.is_some()
            && kind == AgentKind::Claude
            && matches!(projection, ProfileProjection::Enabled { .. })
            && should_scrub_claude_profile_env(&key)
        {
            continue;
        }
        env.insert(key, value);
    }

    match kind {
        AgentKind::Codex => {
            if let Some((proxy_key, _)) = managed {
                env.insert(
                    "CODEX_HOME".into(),
                    homes.codex_home.clone().into_os_string(),
                );
                env.insert("OPENAI_API_KEY".into(), OsString::from(proxy_key));
                env.insert(
                    "OPENAI_BASE_URL".into(),
                    OsString::from(
                        openai_proxy_base_url(mode)
                            .expect("managed proxy mode should have a base URL"),
                    ),
                );
            }
        }
        AgentKind::Claude => {
            if let Some((proxy_key, _)) = managed {
                env.insert(
                    "CLAUDE_CONFIG_DIR".into(),
                    homes.claude_config_dir.clone().into_os_string(),
                );
                env.insert("ANTHROPIC_API_KEY".into(), OsString::from(proxy_key));
                env.insert(
                    "ANTHROPIC_BASE_URL".into(),
                    OsString::from(
                        anthropic_proxy_base_url(mode)
                            .expect("managed proxy mode should have a base URL"),
                    ),
                );
                env.insert("CLAUDE_CODE_ATTRIBUTION_HEADER".into(), "0".into());
                if let ProfileProjection::Enabled { model_catalog, .. } = projection {
                    let profile = &model_catalog.selected;
                    env.insert(
                        "ANTHROPIC_CUSTOM_MODEL_OPTION".into(),
                        OsString::from(profile.alias.as_str()),
                    );
                    env.insert(
                        "ANTHROPIC_MODEL".into(),
                        OsString::from(profile.alias.as_str()),
                    );
                    env.insert(
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME".into(),
                        OsString::from(profile.alias.as_str()),
                    );
                    env.insert(
                        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION".into(),
                        OsString::from(format!("llmup proxy model {}", profile.alias)),
                    );
                    env.insert(
                        "CLAUDE_CODE_SUBAGENT_MODEL".into(),
                        OsString::from(profile.alias.as_str()),
                    );
                    if let Some(max_output_tokens) = profile.claude_max_output_tokens() {
                        env.insert(
                            "CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
                            OsString::from(max_output_tokens.to_string()),
                        );
                    }
                    if let Some(auto_compact_window) = profile.claude_auto_compact_window() {
                        env.insert(
                            "CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(),
                            OsString::from(auto_compact_window.to_string()),
                        );
                    }
                    inject_claude_family_model_env(&mut env, model_catalog);
                }
            }
        }
    }

    Ok(env)
}

fn inject_claude_family_model_env(
    env: &mut BTreeMap<OsString, OsString>,
    model_catalog: &AgentModelCatalog,
) {
    for (alias, env_prefix) in [
        ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL"),
        ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL"),
    ] {
        if !model_catalog.has_alias(alias) {
            continue;
        }
        env.insert(OsString::from(env_prefix), OsString::from(alias));
        env.insert(
            OsString::from(format!("{env_prefix}_NAME")),
            OsString::from(alias),
        );
        env.insert(
            OsString::from(format!("{env_prefix}_DESCRIPTION")),
            OsString::from(format!("llmup proxy model {alias}")),
        );
    }
}

pub fn write_runtime_config_for_port(
    config_path: impl AsRef<Path>,
    run_dir: impl AsRef<Path>,
    port: u16,
) -> Result<RuntimeConfigFile, String> {
    if port == 0 {
        return Err("runtime proxy port must not be 0".to_string());
    }
    let config_path = config_path.as_ref();
    let raw = fs::read_to_string(config_path)
        .map_err(|error| format!("failed to read config {}: {error}", config_path.display()))?;
    let parsed = Config::from_yaml_str(&raw)?;
    parsed.validate()?;

    let mut value: YamlValue = serde_yaml::from_str(&raw)
        .map_err(|error| format!("failed to parse config YAML as runtime document: {error}"))?;
    let mapping = value
        .as_mapping_mut()
        .ok_or_else(|| "config YAML must be a mapping".to_string())?;
    mapping.insert(
        YamlValue::String("listen".to_string()),
        YamlValue::String(format!("127.0.0.1:{port}")),
    );
    let yaml = serde_yaml::to_string(&value)
        .map_err(|error| format!("failed to serialize runtime config YAML: {error}"))?;
    let runtime = Config::from_yaml_str(&yaml)?;
    runtime.validate()?;

    let run_dir = run_dir.as_ref();
    fs::create_dir_all(run_dir)
        .map_err(|error| format!("failed to create run dir {}: {error}", run_dir.display()))?;
    let path = run_dir.join("config.yaml");
    fs::write(&path, &yaml)
        .map_err(|error| format!("failed to write runtime config {}: {error}", path.display()))?;
    Ok(RuntimeConfigFile { path, yaml })
}

pub fn run_cli(
    kind: AgentKind,
    args: impl IntoIterator<Item = OsString>,
    stdout: &mut dyn Write,
) -> Result<i32, String> {
    let parsed = parse_launcher_args(args)?;
    validate_internal_launch_plan_control(&parsed.control)?;
    if parsed.control.help {
        stdout
            .write_all(launcher_help(kind).as_bytes())
            .map_err(|error| format!("failed to write launcher help: {error}"))?;
        return Ok(0);
    }
    if parsed.control.version {
        writeln!(
            stdout,
            "llmup {}",
            option_env!("CARGO_PKG_VERSION").unwrap_or("unknown")
        )
        .map_err(|error| format!("failed to write launcher version: {error}"))?;
        return Ok(0);
    }

    if parsed.control.no_proxy {
        let mode = ProxyMode::NoProxy;
        let projection = ProfileProjection::Disabled;
        let argv = build_client_argv(kind, &mode, &projection, &parsed.native_argv);
        return run_client(kind, &argv, std::env::vars_os().collect());
    }

    let homes = resolve_launcher_homes()?;

    if parsed.control.internal_launch_plan_json {
        return write_internal_launch_plan(kind, parsed, &homes, stdout);
    }

    fs::create_dir_all(&homes.codex_home).map_err(|error| {
        format!(
            "failed to create Codex home {}: {error}",
            homes.codex_home.display()
        )
    })?;
    fs::create_dir_all(&homes.claude_config_dir).map_err(|error| {
        format!(
            "failed to create Claude config dir {}: {error}",
            homes.claude_config_dir.display()
        )
    })?;

    let config_path = parsed
        .control
        .config_path
        .clone()
        .unwrap_or_else(|| homes.llmup_home.join("config.yaml"));
    if !config_path.exists() {
        return Err(format!(
            "llmup config not found at {}; run llmup-config first",
            config_path.display()
        ));
    }
    let user_config = Config::from_yaml_path(&config_path)?;
    user_config.validate()?;
    let base_projection = build_base_projection(&parsed.control, &user_config)?;
    validate_native_model_flags(kind, &base_projection, &parsed.native_argv)?;

    let env_file_path = parsed
        .control
        .env_file_path
        .unwrap_or_else(|| homes.llmup_home.join("secrets.env"));
    let secrets = read_env_file(&env_file_path)?;
    let proxy_key = secrets.required("LLM_UNIVERSAL_PROXY_KEY")?;

    let session_dir = homes
        .llmup_home
        .join("run")
        .join(Uuid::new_v4().to_string());
    fs::create_dir_all(&session_dir).map_err(|error| {
        format!(
            "failed to create run session dir {}: {error}",
            session_dir.display()
        )
    })?;
    let projection = match &base_projection {
        ProfileProjection::Enabled { model_catalog, .. } => {
            prepare_profile_projection(kind, model_catalog.as_ref().clone(), &session_dir)?
        }
        ProfileProjection::Disabled => ProfileProjection::Disabled,
    };
    let explicit_port = parsed.control.port;
    let attempts = if explicit_port.is_some() { 1 } else { 5 };
    let mut last_proxy_error = None;
    for _ in 0..attempts {
        let port = match explicit_port {
            Some(port) => port,
            None => choose_available_port()?,
        };
        let runtime_config = write_runtime_config_for_port(&config_path, &session_dir, port)?;
        let mut proxy = ProxyProcess::start(&runtime_config.path, &secrets, &session_dir)?;
        match proxy.wait_until_healthy(port, Duration::from_secs(5)) {
            Ok(()) => {
                let mode = ProxyMode::Managed {
                    port,
                    proxy_key,
                    secrets,
                };
                let argv = build_client_argv(kind, &mode, &projection, &parsed.native_argv);
                let env = build_client_environment(
                    kind,
                    std::env::vars_os().collect(),
                    &mode,
                    &homes,
                    &projection,
                )?;
                return run_client(kind, &argv, env);
            }
            Err(error) => {
                last_proxy_error = Some(error);
            }
        }
    }
    let error = last_proxy_error.unwrap_or_else(|| "failed to start llmup proxy".to_string());
    if let Some(port) = explicit_port {
        Err(format!(
            "failed to start llmup proxy on requested port {port}: {error}"
        ))
    } else {
        Err(error)
    }
}

fn validate_internal_launch_plan_control(control: &LauncherControl) -> Result<(), String> {
    let uses_internal_flags = control.internal_launch_plan_json
        || control.internal_proxy_base.is_some()
        || control.internal_proxy_key.is_some()
        || control.internal_artifact_dir.is_some();
    if !uses_internal_flags {
        return Ok(());
    }
    if !matches!(std::env::var(INTERNAL_LAUNCH_PLAN_ENV).as_deref(), Ok("1")) {
        return Err(format!(
            "internal launch-plan flags require {INTERNAL_LAUNCH_PLAN_ENV}=1"
        ));
    }
    if !control.internal_launch_plan_json {
        return Err(
            "internal proxy launch-plan flags require --llmup-internal-launch-plan-json"
                .to_string(),
        );
    }
    if control.no_proxy {
        return Err(
            "--llmup-internal-launch-plan-json cannot be used with --llmup-no-proxy".to_string(),
        );
    }
    if control.internal_proxy_base.is_none() {
        return Err("missing --llmup-internal-proxy-base for launch plan".to_string());
    }
    if control.internal_proxy_key.is_none() {
        return Err("missing --llmup-internal-proxy-key for launch plan".to_string());
    }
    if control.internal_artifact_dir.is_none() {
        return Err("missing --llmup-internal-artifact-dir for launch plan".to_string());
    }
    Ok(())
}

fn build_base_projection(
    control: &LauncherControl,
    user_config: &Config,
) -> Result<ProfileProjection, String> {
    if control.no_profile_projection {
        return Ok(ProfileProjection::Disabled);
    }
    let alias = control
        .model_alias
        .as_deref()
        .unwrap_or(DEFAULT_AGENT_MODEL_ALIAS);
    let model_catalog = AgentModelCatalog::from_config(user_config, alias)?;
    Ok(ProfileProjection::Enabled {
        model_catalog: Box::new(model_catalog),
        codex_catalog_path: None,
    })
}

fn write_internal_launch_plan(
    kind: AgentKind,
    parsed: ParsedLauncherArgs,
    homes: &LauncherHomes,
    stdout: &mut dyn Write,
) -> Result<i32, String> {
    let proxy_base_url = parsed
        .control
        .internal_proxy_base
        .clone()
        .expect("internal launch plan validation should require proxy base");
    let proxy_key = parsed
        .control
        .internal_proxy_key
        .clone()
        .expect("internal launch plan validation should require proxy key");
    let artifact_dir = parsed
        .control
        .internal_artifact_dir
        .clone()
        .expect("internal launch plan validation should require artifact dir");

    let config_path = parsed
        .control
        .config_path
        .clone()
        .unwrap_or_else(|| homes.llmup_home.join("config.yaml"));
    if !config_path.exists() {
        return Err(format!(
            "llmup config not found at {}; run llmup-config first",
            config_path.display()
        ));
    }
    let user_config = Config::from_yaml_path(&config_path)?;
    user_config.validate()?;
    let base_projection = build_base_projection(&parsed.control, &user_config)?;
    validate_native_model_flags(kind, &base_projection, &parsed.native_argv)?;

    let env_file_path = parsed
        .control
        .env_file_path
        .clone()
        .unwrap_or_else(|| homes.llmup_home.join("secrets.env"));
    let secrets = read_env_file(&env_file_path)?;
    let projection = match &base_projection {
        ProfileProjection::Enabled { model_catalog, .. } => {
            prepare_profile_projection(kind, model_catalog.as_ref().clone(), &artifact_dir)?
        }
        ProfileProjection::Disabled => ProfileProjection::Disabled,
    };
    let mode = ProxyMode::ManagedExternal {
        proxy_base_url,
        proxy_key,
        secrets,
    };
    let argv = build_client_argv(kind, &mode, &projection, &parsed.native_argv);
    let env = build_client_environment(kind, BTreeMap::new(), &mode, homes, &projection)?;
    let program = client_binary(kind);
    let plan = launch_plan_json(kind, &program, &argv, &env, &projection)?;
    serde_json::to_writer_pretty(&mut *stdout, &plan)
        .map_err(|error| format!("failed to serialize launch plan JSON: {error}"))?;
    stdout
        .write_all(b"\n")
        .map_err(|error| format!("failed to write launch plan JSON: {error}"))?;
    Ok(0)
}

fn launch_plan_json(
    kind: AgentKind,
    program: &OsStr,
    argv: &[OsString],
    env: &BTreeMap<OsString, OsString>,
    projection: &ProfileProjection,
) -> Result<JsonValue, String> {
    let program = os_to_json_string(program, "client program")?;
    let argv = argv
        .iter()
        .map(|arg| os_to_json_string(arg.as_os_str(), "client argv"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut env_json = JsonMap::new();
    for (key, value) in env {
        let key = os_to_json_string(key.as_os_str(), "client env key")?;
        let value = os_to_json_string(value.as_os_str(), "client env value")?;
        env_json.insert(key, JsonValue::String(value));
    }
    let (projection_json, codex_model_catalog) = match projection {
        ProfileProjection::Enabled {
            model_catalog,
            codex_catalog_path,
        } => {
            let selected_profile = &model_catalog.selected;
            let catalog = codex_catalog_path
                .as_ref()
                .map(|path| path.display().to_string());
            (
                json!({
                    "enabled": true,
                    "profile": {
                        "alias": selected_profile.alias.as_str(),
                    },
                    "codex_catalog_path": catalog.as_deref(),
                }),
                catalog,
            )
        }
        ProfileProjection::Disabled => (
            json!({
                "enabled": false,
                "profile": null,
                "codex_catalog_path": null,
            }),
            None,
        ),
    };
    Ok(json!({
        "schema_version": 1,
        "agent": match kind {
            AgentKind::Codex => "codex",
            AgentKind::Claude => "claude",
        },
        "program": program,
        "argv": argv,
        "env": JsonValue::Object(env_json),
        "projection": projection_json,
        "artifacts": {
            "codex_model_catalog": codex_model_catalog,
        },
    }))
}

fn os_to_json_string(value: &OsStr, context: &str) -> Result<String, String> {
    value
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{context} contains non-UTF-8 data and cannot be emitted as JSON"))
}

pub fn resolve_launcher_homes() -> Result<LauncherHomes, String> {
    let home = home_dir_from_env()?;
    let llmup_home = env_path_or_default("LLMUP_HOME", home.join(".llmup"));
    Ok(LauncherHomes {
        codex_home: env_path_or_default("LLMUP_CODEX_HOME", home.join(".llmup-codex")),
        claude_config_dir: env_path_or_default(
            "LLMUP_CLAUDE_CONFIG_DIR",
            home.join(".llmup-claude"),
        ),
        llmup_home,
    })
}

fn take_os_value(args: &[OsString], index: &mut usize, flag: &str) -> Result<OsString, String> {
    *index += 1;
    let value = args
        .get(*index)
        .ok_or_else(|| format!("missing value for {flag}"))?
        .clone();
    *index += 1;
    Ok(value)
}

fn take_nonempty_utf8_value(
    args: &[OsString],
    index: &mut usize,
    flag: &str,
) -> Result<String, String> {
    let value = take_os_value(args, index, flag)?;
    let text = value
        .to_str()
        .ok_or_else(|| format!("{flag} value must be valid UTF-8"))?;
    parse_nonempty_utf8_value(text, flag)
}

fn parse_nonempty_utf8_value(value: &str, flag: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err(format!("{flag} value must not be empty"));
    }
    if value == "--" || value.starts_with('-') {
        return Err(format!("missing value for {flag}"));
    }
    Ok(value.to_string())
}

fn parse_port(value: &OsStr, flag: &str) -> Result<u16, String> {
    let text = value
        .to_str()
        .ok_or_else(|| format!("{flag} value must be valid UTF-8"))?;
    let port = text
        .parse::<u16>()
        .map_err(|_| format!("{flag} must be a TCP port between 1 and 65535"))?;
    if port == 0 {
        return Err(format!("{flag} must not be 0"));
    }
    Ok(port)
}

fn launcher_help(kind: AgentKind) -> String {
    let command = match kind {
        AgentKind::Codex => "llmup-codex",
        AgentKind::Claude => "llmup-claude",
    };
    let native = match kind {
        AgentKind::Codex => "codex",
        AgentKind::Claude => "claude",
    };
    format!(
        "\
{command}

Usage:
  {command} [--llmup-help] [--llmup-version] [--llmup-no-proxy] [--llmup-model <alias>] [--llmup-no-profile-projection] [--] [native args...]

Runs {native} with llmup's local proxy and passes native args through unchanged.

Advanced / troubleshooting:
  --llmup-model <alias>           Select the llmup model alias for managed projection.
  --llmup-no-profile-projection   Keep only proxy plumbing and manage native model flags yourself.
  --llmup-no-proxy                Open the original {native} command without the llmup proxy.
  --                              Stop parsing llmup options; following args go to {native}.
"
    )
}

fn should_scrub_claude_env(key: &OsStr) -> bool {
    let Some(key) = key.to_str() else {
        return false;
    };
    const EXACT: &[&str] = &[
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_WORKSPACE_ID",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "GCLOUD_PROJECT",
        "GOOGLE_CLOUD_PROJECT",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_USE_MANTLE",
        "CLAUDE_CODE_USE_ANTHROPIC_AWS",
        "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
        "CLAUDE_CODE_SKIP_VERTEX_AUTH",
        "CLAUDE_CODE_SKIP_FOUNDRY_AUTH",
        "CLAUDE_CODE_SKIP_MANTLE_AUTH",
        "CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH",
        "CLAUDE_CODE_ATTRIBUTION_HEADER",
    ];
    const PREFIXES: &[&str] = &[
        "ANTHROPIC_BEDROCK_",
        "ANTHROPIC_VERTEX_",
        "ANTHROPIC_FOUNDRY_",
        "ANTHROPIC_AWS_",
        "AWS_",
    ];
    EXACT.contains(&key) || PREFIXES.iter().any(|prefix| key.starts_with(prefix))
}

fn should_scrub_claude_profile_env(key: &OsStr) -> bool {
    let Some(key) = key.to_str() else {
        return false;
    };
    const EXACT: &[&str] = &[
        "ANTHROPIC_MODEL",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES",
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
        "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
        "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ];
    const MODEL_PREFIXES: &[&str] = &[
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
    ];
    EXACT.contains(&key)
        || MODEL_PREFIXES.iter().any(|prefix| {
            key == *prefix
                || key
                    .strip_prefix(prefix)
                    .is_some_and(|suffix| suffix.starts_with('_'))
        })
}

fn native_projection_override_flag(kind: AgentKind, native_argv: &[OsString]) -> Option<String> {
    let mut args = native_argv.iter();
    while let Some(arg) = args.next() {
        let Some(text) = arg.to_str() else {
            continue;
        };
        match kind {
            AgentKind::Codex => {
                if codex_projection_override_arg(text) {
                    return Some(text.to_string());
                }
                if text == "-c" || text == "--config" {
                    let Some(value) = args.next().and_then(|value| value.to_str()) else {
                        continue;
                    };
                    if codex_projection_config_override(value) {
                        return Some(format!("{text} {value}"));
                    }
                    continue;
                }
                if let Some(value) = text.strip_prefix("-c") {
                    if !value.is_empty() && codex_projection_config_override(value) {
                        return Some(text.to_string());
                    }
                }
                if let Some(value) = text.strip_prefix("--config=") {
                    if codex_projection_config_override(value) {
                        return Some(text.to_string());
                    }
                }
            }
            AgentKind::Claude => {
                if text == "--model" || text.starts_with("--model=") {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

fn codex_projection_override_arg(text: &str) -> bool {
    matches!(
        text,
        "-m" | "--model" | "--oss" | "--local-provider" | "--profile"
    ) || text.starts_with("--model=")
        || text.starts_with("--local-provider=")
        || text.starts_with("--profile=")
}

fn codex_projection_config_override(value: &str) -> bool {
    let value = value.strip_prefix('=').unwrap_or(value);
    let key = value
        .split_once('=')
        .map(|(key, _)| key)
        .unwrap_or(value)
        .trim();
    matches!(
        key,
        "model" | "model_provider" | "model_catalog_json" | "openai_base_url"
    ) || key.starts_with("model_providers.")
}

fn native_model_flag_error(kind: AgentKind, flag: &str) -> String {
    format!(
        "native {native} model flag `{flag}` conflicts with llmup managed model projection; use --llmup-model <alias> or add --llmup-no-profile-projection to manage the native model yourself",
        native = native_name(kind),
    )
}

fn choose_available_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("failed to reserve a local proxy port: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("failed to inspect reserved local port: {error}"))?
        .port();
    drop(listener);
    Ok(port)
}

fn run_client(
    kind: AgentKind,
    argv: &[OsString],
    env: BTreeMap<OsString, OsString>,
) -> Result<i32, String> {
    let binary = client_binary(kind);
    let mut command = Command::new(&binary);
    command
        .args(argv)
        .env_clear()
        .envs(env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = command.status().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!(
                "{} CLI not found in PATH; install it first or use --llmup-no-proxy for native setup commands",
                native_name(kind)
            )
        } else {
            format!("failed to start {} CLI: {error}", native_name(kind))
        }
    })?;
    Ok(status.code().unwrap_or(1))
}

fn client_binary(kind: AgentKind) -> OsString {
    let override_name = match kind {
        AgentKind::Codex => "LLMUP_CODEX_BIN",
        AgentKind::Claude => "LLMUP_CLAUDE_BIN",
    };
    std::env::var_os(override_name)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from(native_name(kind)))
}

fn native_name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Codex => "codex",
        AgentKind::Claude => "claude",
    }
}

struct ProxyProcess {
    child: Child,
    stderr_path: PathBuf,
}

impl ProxyProcess {
    fn start(config_path: &Path, secrets: &EnvFile, run_dir: &Path) -> Result<Self, String> {
        let stdout_path = run_dir.join("proxy.stdout.log");
        let stderr_path = run_dir.join("proxy.stderr.log");
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stdout_path)
            .map_err(|error| format!("failed to open {}: {error}", stdout_path.display()))?;
        let stderr = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_path)
            .map_err(|error| format!("failed to open {}: {error}", stderr_path.display()))?;
        let mut command = Command::new(
            std::env::current_exe()
                .map_err(|error| format!("failed to locate current executable: {error}"))?,
        );
        command
            .arg("--config")
            .arg(config_path)
            .env("LLMUP_FORCE_SERVER", "1")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        for (key, value) in secrets.iter() {
            command.env(key, value);
        }
        let child = command
            .spawn()
            .map_err(|error| format!("failed to start llmup proxy: {error}"))?;
        Ok(Self { child, stderr_path })
    }

    fn wait_until_healthy(&mut self, port: u16, timeout: Duration) -> Result<(), String> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|error| format!("failed to inspect proxy process: {error}"))?
            {
                return Err(format!(
                    "llmup proxy exited before /health became ready with status {status}; see {}",
                    self.stderr_path.display()
                ));
            }
            if probe_health(port) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "timed out waiting for llmup proxy /health; see {}",
            self.stderr_path.display()
        ))
    }
}

impl Drop for ProxyProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn probe_health(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(100)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
    if stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buffer = [0_u8; 128];
    let Ok(count) = stream.read(&mut buffer) else {
        return false;
    };
    let response = String::from_utf8_lossy(&buffer[..count]);
    response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200")
}
