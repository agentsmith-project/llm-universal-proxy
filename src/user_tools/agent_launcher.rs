use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_yaml::Value;
use uuid::Uuid;

use super::env_file::{read_env_file, EnvFile};
use super::{env_path_or_default, home_dir_from_env};
use crate::Config;

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
    pub port: Option<u16>,
    pub config_path: Option<PathBuf>,
    pub env_file_path: Option<PathBuf>,
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
    NoProxy,
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
        port: None,
        config_path: None,
        env_file_path: None,
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
            _ => {
                if let Some(value) = text.strip_prefix("--llmup-port=") {
                    control.port = Some(parse_port(OsStr::new(value), "--llmup-port")?);
                    index += 1;
                } else if let Some(value) = text.strip_prefix("--llmup-config=") {
                    control.config_path = Some(PathBuf::from(value));
                    index += 1;
                } else if let Some(value) = text.strip_prefix("--llmup-env-file=") {
                    control.env_file_path = Some(PathBuf::from(value));
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

    Ok(ParsedLauncherArgs {
        control,
        native_argv,
    })
}

pub fn build_client_argv(
    kind: AgentKind,
    mode: ProxyMode,
    native_argv: &[OsString],
) -> Vec<OsString> {
    let mut argv = match (kind, mode) {
        (AgentKind::Codex, ProxyMode::Managed { port, .. }) => vec![
            "-c".into(),
            "model_provider=\"proxy\"".into(),
            "-c".into(),
            "model_providers.proxy.name=\"llmup\"".into(),
            "-c".into(),
            format!("model_providers.proxy.base_url=\"http://127.0.0.1:{port}/openai/v1\"").into(),
            "-c".into(),
            "model_providers.proxy.env_key=\"OPENAI_API_KEY\"".into(),
            "-c".into(),
            "model_providers.proxy.wire_api=\"responses\"".into(),
            "-c".into(),
            "model_providers.proxy.supports_websockets=false".into(),
            "-m".into(),
            "default".into(),
        ],
        (AgentKind::Claude, ProxyMode::Managed { .. }) => {
            vec!["--model".into(), "default".into()]
        }
        (_, ProxyMode::NoProxy) => Vec::new(),
    };
    argv.extend(native_argv.iter().cloned());
    argv
}

pub fn build_client_environment(
    kind: AgentKind,
    parent: BTreeMap<OsString, OsString>,
    mode: ProxyMode,
    homes: &LauncherHomes,
) -> Result<BTreeMap<OsString, OsString>, String> {
    let managed = match &mode {
        ProxyMode::Managed {
            port,
            proxy_key,
            secrets,
        } => Some((*port, proxy_key.as_str(), secrets)),
        ProxyMode::NoProxy => None,
    };
    let secret_names = managed
        .map(|(_, _, secrets)| {
            secrets
                .names()
                .map(|name| OsString::from(name.as_str()))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let secret_values = managed
        .map(|(_, _, secrets)| {
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
        env.insert(key, value);
    }

    match kind {
        AgentKind::Codex => {
            env.insert(
                "CODEX_HOME".into(),
                homes.codex_home.clone().into_os_string(),
            );
            if let Some((port, proxy_key, _)) = managed {
                env.insert("OPENAI_API_KEY".into(), OsString::from(proxy_key));
                env.insert(
                    "OPENAI_BASE_URL".into(),
                    OsString::from(format!("http://127.0.0.1:{port}/openai/v1")),
                );
            }
        }
        AgentKind::Claude => {
            env.insert(
                "CLAUDE_CONFIG_DIR".into(),
                homes.claude_config_dir.clone().into_os_string(),
            );
            if let Some((port, proxy_key, _)) = managed {
                env.insert("ANTHROPIC_API_KEY".into(), OsString::from(proxy_key));
                env.insert(
                    "ANTHROPIC_BASE_URL".into(),
                    OsString::from(format!("http://127.0.0.1:{port}/anthropic")),
                );
                env.insert("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB".into(), "1".into());
            }
        }
    }

    Ok(env)
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

    let mut value: Value = serde_yaml::from_str(&raw)
        .map_err(|error| format!("failed to parse config YAML as runtime document: {error}"))?;
    let mapping = value
        .as_mapping_mut()
        .ok_or_else(|| "config YAML must be a mapping".to_string())?;
    mapping.insert(
        Value::String("listen".to_string()),
        Value::String(format!("127.0.0.1:{port}")),
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

    let homes = resolve_launcher_homes()?;
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

    if parsed.control.no_proxy {
        let mode = ProxyMode::NoProxy;
        let argv = build_client_argv(kind, mode.clone(), &parsed.native_argv);
        let env = build_client_environment(kind, std::env::vars_os().collect(), mode, &homes)?;
        return run_client(kind, &argv, env);
    }

    let config_path = parsed
        .control
        .config_path
        .unwrap_or_else(|| homes.llmup_home.join("config.yaml"));
    if !config_path.exists() {
        return Err(format!(
            "llmup config not found at {}; run llmup-config first",
            config_path.display()
        ));
    }
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
                let argv = build_client_argv(kind, mode.clone(), &parsed.native_argv);
                let env =
                    build_client_environment(kind, std::env::vars_os().collect(), mode, &homes)?;
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
  {command} [--llmup-help] [--llmup-version] [--llmup-no-proxy] [--] [native args...]

Runs {native} with llmup's local proxy and passes native args through unchanged.

Advanced / troubleshooting:
  --llmup-no-proxy   Open the original {native} command without the llmup proxy.
  --                 Stop parsing llmup options; following args go to {native}.
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
