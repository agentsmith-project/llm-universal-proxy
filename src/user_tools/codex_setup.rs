//! `codex-setup` subcommand: configure Codex **V1** hybrid sub-agents.
//!
//! Generates the files Codex needs to route a child sub-agent through the llmup
//! proxy while the official main agent keeps using its own credentials:
//!
//! - `~/.codex/llmup.config.toml` — profile with the `[model_providers.llmup]`
//!   block and a defensive `[features] multi_agent_v2 = false` (V1 is the
//!   default; we keep it explicit).
//! - `~/.codex/agents/llmup-<model>.toml` — custom sub-agent that pins
//!   `model_provider = "llmup"`. Deliberately omits `fork_turns`/`fork_context`
//!   (V1 no-fork is the default) and any `multi_agent_*` config.
//! - `~/.codex/llmup/state.json` — managed-file manifest. **Never** stores API
//!   keys.
//!
//! The generation logic is split into pure functions (tested directly) plus a
//! thin CLI/IO layer. Safe writes go through [`write_config_file_atomic`]
//! (migrated from `config_wizard.rs`): backup → temp → fsync → atomic rename.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use uuid::Uuid;

const PROVIDER_ID: &str = "llmup";
const ENV_KEY_NAME: &str = "LLMUP_PROXY_KEY";
const SCHEMA_VERSION: u32 = 1;
const DEFAULT_WIRE_API: &str = "responses";
const DEVELOPER_INSTRUCTIONS: &str =
    "Complete the delegated task. Return results to the parent agent.";

/// Inputs for file generation. `provider_key` is intentionally **not** stored;
/// it is used only for the optional live connection test.
#[derive(Debug, Clone)]
pub struct SetupInput {
    pub base_url: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub context_window: Option<u64>,
}

/// Action selected from argv.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    Generate,
    Status,
    Uninstall,
    Help,
}

/// Parsed CLI options.
#[derive(Debug, Clone, Default)]
pub struct CliOptions {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub provider_key: Option<String>,
    pub reasoning_effort: Option<String>,
    pub context_window: Option<u64>,
    pub action: Action,
}

/// Paths written by [`install`].
#[derive(Debug, Clone)]
pub struct GeneratedPaths {
    pub profile_path: PathBuf,
    pub agent_path: PathBuf,
    pub state_path: PathBuf,
}

// ===========================================================================
// Pure generation functions
// ===========================================================================

/// Strip a single trailing slash from a base URL so `<base>/models` joins
/// cleanly. Multiple trailing slashes collapse to none.
fn normalize_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim();
    trimmed.trim_end_matches('/').to_string()
}

/// File name for a model's custom agent, e.g. `llmup-gpt-5.toml`.
pub fn agent_file_name(model: &str) -> String {
    format!("llmup-{model}.toml")
}

/// Display name embedded in the agent TOML, e.g. `llmup_gpt-5`.
pub fn agent_name(model: &str) -> String {
    format!("llmup_{model}")
}

/// Generate the `[model_providers.llmup]` table.
pub fn generate_provider_block(base_url: &str) -> String {
    let normalized = normalize_base_url(base_url);
    format!(
        "[model_providers.{PROVIDER_ID}]\n\
         name = \"LLMUP\"\n\
         base_url = \"{normalized}\"\n\
         wire_api = \"{DEFAULT_WIRE_API}\"\n\
         requires_openai_auth = false\n\
         env_key = \"{ENV_KEY_NAME}\"\n"
    )
}

/// Generate the defensive `[features]` table pinning V1 as the default.
fn generate_features_block() -> String {
    "[features]\nmulti_agent_v2 = false\n".to_string()
}

/// Generate the full agent TOML. `fork_turns`/`fork_context`/`multi_agent_*`
/// are intentionally absent (V1 no-fork is the default).
pub fn generate_agent_content(
    model: &str,
    reasoning_effort: Option<&str>,
    context_window: Option<u64>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("name = \"{}\"\n", agent_name(model)));
    out.push_str(&format!("description = \"LLMUP sub-agent for {model}\"\n"));
    out.push_str(&format!("model = \"{model}\"\n"));
    out.push_str(&format!("model_provider = \"{PROVIDER_ID}\"\n"));
    out.push_str(&format!(
        "developer_instructions = \"{DEVELOPER_INSTRUCTIONS}\"\n"
    ));
    if let Some(effort) = reasoning_effort {
        out.push_str(&format!("model_reasoning_effort = \"{effort}\"\n"));
    }
    if let Some(window) = context_window {
        out.push_str(&format!("model_context_window = {window}\n"));
    }
    out
}

/// Build the profile (`llmup.config.toml`) content, merging the managed
/// `[model_providers.llmup]` and `[features]` tables into any existing content.
///
/// Unrelated top-level tables the user may have added are preserved; a re-run
/// does not duplicate the managed tables.
pub fn build_profile_content(existing: Option<&str>, base_url: &str) -> String {
    let provider_block = generate_provider_block(base_url);
    let features_block = generate_features_block();
    let stripped = strip_managed_tables(
        existing.unwrap_or(""),
        &["[model_providers.llmup]", "[features]"],
    );
    let mut out = stripped.trim_end().to_string();
    if !out.is_empty() {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&provider_block);
    out.push('\n');
    out.push_str(&features_block);
    out
}

/// Remove top-level TOML tables listed in `headers` from `content`. A table
/// spans from its header line up to (but excluding) the next line beginning
/// with `[` or the end of content. Non-matching lines are kept verbatim.
fn strip_managed_tables(content: &str, headers: &[&str]) -> String {
    let mut out = String::new();
    let mut skipping: Option<&str> = None;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            // Entering a new table: decide whether to skip it.
            skipping = headers
                .iter()
                .find(|header| trimmed.starts_with(*header))
                .copied();
        }
        match skipping {
            Some(_) => continue,
            None => out.push_str(line),
        }
    }
    out
}

/// Generate the state.json body. No API-key material is stored.
pub fn generate_state_content(
    codex_version: &str,
    managed_files: &[String],
    created_at: &str,
) -> String {
    // Hand-rolled for a stable, key-free field ordering.
    let mut files_buf = String::from("[");
    for (i, file) in managed_files.iter().enumerate() {
        if i > 0 {
            files_buf.push(',');
        }
        files_buf.push_str(&serde_json::Value::String(file.clone()).to_string());
    }
    files_buf.push(']');
    format!(
        "{{\n\
         \"schema_version\": {SCHEMA_VERSION},\n\
         \"codex_version\": \"{codex_version}\",\n\
         \"provider_id\": \"{PROVIDER_ID}\",\n\
         \"managed_files\": {files_buf},\n\
         \"created_at\": \"{created_at}\"\n\
         }}\n"
    )
}

// ===========================================================================
// Safe write (migrated from config_wizard.rs::write_config_file_atomic)
// ===========================================================================

/// Atomically write `contents` to `path`: back up any existing file to
/// `<path>.bak`, write a fresh temp file (mode 0o600 on unix, or the previous
/// permissions), fsync, then rename over the target.
pub fn write_config_file_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("failed to find parent directory for {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;

    // Backup any existing file before we touch it.
    if path.exists() {
        let backup = backup_path(path);
        fs::copy(path, &backup).map_err(|error| {
            format!(
                "failed to back up {} to {}: {error}",
                path.display(),
                backup.display()
            )
        })?;
    }

    let original_permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("config");
    let temp_path = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));

    let result = (|| {
        use std::fs::OpenOptions;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mode = original_permissions
                .as_ref()
                .map(|permissions| {
                    use std::os::unix::fs::PermissionsExt;
                    permissions.mode() & 0o777
                })
                .unwrap_or(0o600);
            options.mode(mode);
        }
        let mut file = options.open(&temp_path).map_err(|error| {
            format!(
                "failed to write temporary file {}: {error}",
                temp_path.display()
            )
        })?;
        if let Some(permissions) = original_permissions.clone() {
            fs::set_permissions(&temp_path, permissions).map_err(|error| {
                format!(
                    "failed to preserve permissions on {}: {error}",
                    temp_path.display()
                )
            })?;
        }
        file.write_all(contents.as_bytes()).map_err(|error| {
            format!(
                "failed to write temporary file {}: {error}",
                temp_path.display()
            )
        })?;
        file.flush().map_err(|error| {
            format!(
                "failed to flush temporary file {}: {error}",
                temp_path.display()
            )
        })?;
        file.sync_all().map_err(|error| {
            format!(
                "failed to sync temporary file {}: {error}",
                temp_path.display()
            )
        })?;
        drop(file);
        fs::rename(&temp_path, path).map_err(|error| {
            format!(
                "failed to replace {} with {}: {error}",
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

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".bak");
    path.with_file_name(name)
}

// ===========================================================================
// CLI parsing
// ===========================================================================

const HELP_TEXT: &str = "\
llm-universal-proxy codex-setup — configure Codex V1 hybrid sub-agents

USAGE:
    llm-universal-proxy codex-setup --base-url <url> --model <alias> --provider-key <key>
                                    [--reasoning-effort <effort>] [--context-window <N>]
    llm-universal-proxy codex-setup --status
    llm-universal-proxy codex-setup --uninstall
    llm-universal-proxy codex-setup --help

OPTIONS:
    --base-url <url>           llmup proxy base URL (required for install)
    --model <alias>            model alias/id routed through llmup (required for install)
    --provider-key <key>       provider key used for the connection test only;
                               the config references the LLMUP_PROXY_KEY env var
    --reasoning-effort <s>     optional model_reasoning_effort for the sub-agent
    --context-window <N>       optional model_context_window for the sub-agent
    --status                   show what codex-setup has installed
    --uninstall                remove files managed by codex-setup
    --help, -h                 show this help

Config is written under ~/.codex (or $CODEX_HOME). No API keys are stored.
";

pub fn parse_args(args: &[String]) -> Result<CliOptions, String> {
    let mut opts = CliOptions::default();
    let mut status = false;
    let mut uninstall = false;

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--help" | "-h" => {
                return Ok(CliOptions {
                    action: Action::Help,
                    ..Default::default()
                })
            }
            "--status" => status = true,
            "--uninstall" => uninstall = true,
            _ => {
                if let Some(value) = inline_value(arg, "--base-url") {
                    set_once(&mut opts.base_url, "--base-url", value)?;
                } else if arg == "--base-url" {
                    opts.base_url = Some(take_value(args, &mut index, "--base-url")?);
                } else if let Some(value) = inline_value(arg, "--model") {
                    set_once(&mut opts.model, "--model", value)?;
                } else if arg == "--model" {
                    opts.model = Some(take_value(args, &mut index, "--model")?);
                } else if let Some(value) = inline_value(arg, "--provider-key") {
                    set_once(&mut opts.provider_key, "--provider-key", value)?;
                } else if arg == "--provider-key" {
                    opts.provider_key = Some(take_value(args, &mut index, "--provider-key")?);
                } else if let Some(value) = inline_value(arg, "--reasoning-effort") {
                    set_once(&mut opts.reasoning_effort, "--reasoning-effort", value)?;
                } else if arg == "--reasoning-effort" {
                    opts.reasoning_effort =
                        Some(take_value(args, &mut index, "--reasoning-effort")?);
                } else if let Some(value) = inline_value(arg, "--context-window") {
                    opts.context_window = Some(parse_u64("--context-window", value)?);
                } else if arg == "--context-window" {
                    let value = take_value(args, &mut index, "--context-window")?;
                    opts.context_window = Some(parse_u64("--context-window", &value)?);
                } else {
                    return Err(format!("unknown argument `{arg}`"));
                }
            }
        }
        index += 1;
    }

    let action = if status && uninstall {
        return Err("--status and --uninstall are mutually exclusive".to_string());
    } else if status {
        Action::Status
    } else if uninstall {
        Action::Uninstall
    } else {
        Action::Generate
    };
    opts.action = action;

    if opts.action == Action::Generate {
        if opts.base_url.is_none() {
            return Err("--base-url is required".to_string());
        }
        if opts.model.is_none() {
            return Err("--model is required".to_string());
        }
        if opts.provider_key.is_none() {
            return Err("--provider-key is required".to_string());
        }
    }

    Ok(opts)
}

fn inline_value<'a>(arg: &'a str, flag: &str) -> Option<&'a str> {
    arg.strip_prefix(flag)?.strip_prefix('=')
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn set_once(target: &mut Option<String>, flag: &str, value: &str) -> Result<(), String> {
    if target.replace(value.to_string()).is_some() {
        return Err(format!("{flag} may only be provided once"));
    }
    Ok(())
}

fn parse_u64(flag: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{flag} must be a non-negative integer"))
}

// ===========================================================================
// Filesystem layout
// ===========================================================================

/// Resolve the Codex home directory (`$CODEX_HOME` or `~/.codex`).
pub fn codex_home() -> Result<PathBuf, String> {
    if let Some(custom) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(custom));
    }
    let home = crate::user_tools::home_dir_from_env()?;
    Ok(home.join(".codex"))
}

fn profile_path(codex_home: &Path) -> PathBuf {
    codex_home.join("llmup.config.toml")
}

fn agent_path(codex_home: &Path, model: &str) -> PathBuf {
    codex_home.join("agents").join(agent_file_name(model))
}

fn state_dir(codex_home: &Path) -> PathBuf {
    codex_home.join("llmup")
}

fn state_path(codex_home: &Path) -> PathBuf {
    state_dir(codex_home).join("state.json")
}

/// Best-effort detection of the installed Codex CLI version. Returns
/// `"unknown"` when `codex` is absent (tests / headless environments).
pub fn detect_codex_version() -> String {
    Command::new("codex")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .map(|raw| raw.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn timestamp_utc() -> String {
    // Use the system clock via std; format like an ISO-8601 UTC stamp. We
    // avoid pulling in chrono by delegating to the `date` command when
    // available, falling back to a Unix epoch seconds marker.
    if let Some(stamp) = Command::new("date")
        .arg("-u")
        .arg("+%Y-%m-%dT%H:%M:%SZ")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
    {
        return stamp;
    }
    format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    )
}

// ===========================================================================
// Install / status / uninstall
// ===========================================================================

/// Write all managed files under `codex_home`. Returns the written paths.
pub fn install(input: &SetupInput, codex_home: &Path) -> Result<GeneratedPaths, String> {
    let profile = profile_path(codex_home);
    let agent = agent_path(codex_home, &input.model);
    let state_file = state_path(codex_home);

    // Profile: merge into any existing tool-owned file.
    let existing_profile = fs::read_to_string(&profile).ok();
    let profile_content = build_profile_content(existing_profile.as_deref(), &input.base_url);
    write_config_file_atomic(&profile, &profile_content)?;

    // Agent: fully regenerated per model (tool-owned file).
    let agent_content = generate_agent_content(
        &input.model,
        input.reasoning_effort.as_deref(),
        input.context_window,
    );
    write_config_file_atomic(&agent, &agent_content)?;

    // State manifest (no keys).
    let codex_version = detect_codex_version();
    let created_at = timestamp_utc();
    let managed_files = vec![
        profile.to_string_lossy().into_owned(),
        agent.to_string_lossy().into_owned(),
    ];
    let state_content = generate_state_content(&codex_version, &managed_files, &created_at);
    write_config_file_atomic(&state_file, &state_content)?;

    Ok(GeneratedPaths {
        profile_path: profile,
        agent_path: agent,
        state_path: state_file,
    })
}

/// Print the current installation status. Exit code 0 in both installed and
/// not-installed cases.
pub fn run_status(codex_home: &Path, stdout: &mut dyn Write) -> Result<i32, String> {
    let state_file = state_path(codex_home);
    match fs::read_to_string(&state_file) {
        Ok(content) => {
            let value: serde_json::Value = serde_json::from_str(&content)
                .map_err(|error| format!("failed to parse {}: {error}", state_file.display()))?;
            writeln!(stdout, "codex-setup status").map_err(io_err)?;
            let version = value
                .get("codex_version")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let created = value
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            writeln!(stdout, "  codex_version: {version}").map_err(io_err)?;
            writeln!(stdout, "  created_at:    {created}").map_err(io_err)?;
            writeln!(stdout, "  managed_files:").map_err(io_err)?;
            if let Some(files) = value.get("managed_files").and_then(|v| v.as_array()) {
                for file in files {
                    if let Some(name) = file.as_str() {
                        writeln!(stdout, "    - {name}").map_err(io_err)?;
                    }
                }
            }
            writeln!(
                stdout,
                "Set {ENV_KEY_NAME}=<your-key> so Codex can authenticate the llmup provider."
            )
            .map_err(io_err)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            writeln!(stdout, "codex-setup status: not installed").map_err(io_err)?;
            writeln!(
                stdout,
                "  Nothing managed by codex-setup under {}.",
                codex_home.display()
            )
            .map_err(io_err)?;
        }
        Err(error) => {
            return Err(format!("failed to read {}: {error}", state_file.display()));
        }
    }
    Ok(0)
}

/// Remove every file listed in the state manifest, then the manifest itself.
pub fn run_uninstall(codex_home: &Path, stdout: &mut dyn Write) -> Result<i32, String> {
    let state_file = state_path(codex_home);
    let content = match fs::read_to_string(&state_file) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            writeln!(
                stdout,
                "codex-setup uninstall: nothing to remove (no state file)."
            )
            .map_err(io_err)?;
            return Ok(0);
        }
        Err(error) => {
            return Err(format!("failed to read {}: {error}", state_file.display()));
        }
    };
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|error| format!("failed to parse {}: {error}", state_file.display()))?;

    if let Some(files) = value.get("managed_files").and_then(|v| v.as_array()) {
        for file in files {
            if let Some(path) = file.as_str() {
                let path = Path::new(path);
                match fs::remove_file(path) {
                    Ok(()) => writeln!(stdout, "removed {}", path.display()).map_err(io_err)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("failed to remove {}: {error}", path.display()));
                    }
                }
            }
        }
    }

    // Remove the state manifest (and its directory if now empty).
    let _ = fs::remove_file(&state_file);
    let _ = fs::remove_dir(state_dir(codex_home));
    writeln!(stdout, "codex-setup uninstall: complete.").map_err(io_err)?;
    Ok(0)
}

fn io_err(error: std::io::Error) -> String {
    format!("failed to write status output: {error}")
}

// ===========================================================================
// Connection test + entry point
// ===========================================================================

/// Hit `<base_url>/models` with the provider key and the Codex user agent.
/// Returns the list of model ids on success.
pub async fn connection_test(base_url: &str, provider_key: &str) -> Result<Vec<String>, String> {
    let normalized = normalize_base_url(base_url);
    let url = format!("{normalized}/models");
    let response = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))?
        .get(&url)
        .header("Authorization", format!("Bearer {provider_key}"))
        .header("User-Agent", "codex/0.146.0")
        .send()
        .await
        .map_err(|error| format!("connection test to {url} failed: {error}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("connection test to {url} returned HTTP {status}"));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("connection test could not parse JSON from {url}: {error}"))?;
    let models = body
        .get("models")
        .and_then(|models| models.as_array())
        .ok_or_else(|| format!("connection test: no `models` array in response from {url}"))?;
    let ids = models
        .iter()
        .filter_map(|model| {
            model
                .get("id")
                .and_then(|id| id.as_str())
                .map(|id| id.to_string())
        })
        .collect();
    Ok(ids)
}

/// Entry point invoked from `main.rs` when `argv[1] == "codex-setup"`.
pub async fn run(args: &[String]) -> Result<i32, String> {
    let mut stdout = std::io::stdout();
    run_with(args, &mut stdout).await
}

async fn run_with(args: &[String], stdout: &mut dyn Write) -> Result<i32, String> {
    let opts = match parse_args(args) {
        Ok(opts) => opts,
        Err(message) => {
            writeln!(stdout, "{message}").map_err(io_err)?;
            writeln!(stdout, "{HELP_TEXT}").map_err(io_err)?;
            return Ok(2);
        }
    };

    match opts.action {
        Action::Help => {
            stdout.write_all(HELP_TEXT.as_bytes()).map_err(io_err)?;
            Ok(0)
        }
        Action::Status => {
            let home = codex_home()?;
            run_status(&home, stdout)
        }
        Action::Uninstall => {
            let home = codex_home()?;
            run_uninstall(&home, stdout)
        }
        Action::Generate => {
            let home = codex_home()?;
            let input = SetupInput {
                base_url: opts.base_url.clone().expect("validated"),
                model: opts.model.clone().expect("validated"),
                reasoning_effort: opts.reasoning_effort.clone(),
                context_window: opts.context_window,
            };
            let paths = install(&input, &home)?;

            writeln!(
                stdout,
                "codex-setup: wrote Codex V1 hybrid sub-agent config."
            )
            .map_err(io_err)?;
            writeln!(stdout, "  profile: {}", paths.profile_path.display()).map_err(io_err)?;
            writeln!(stdout, "  agent:   {}", paths.agent_path.display()).map_err(io_err)?;
            writeln!(stdout, "  state:   {}", paths.state_path.display()).map_err(io_err)?;

            // Live connection test (best-effort; install already succeeded).
            if let Some(key) = opts.provider_key.as_deref() {
                match connection_test(&input.base_url, key).await {
                    Ok(models) => {
                        writeln!(
                            stdout,
                            "connection test: OK ({} models reachable).",
                            models.len()
                        )
                        .map_err(io_err)?;
                    }
                    Err(error) => {
                        writeln!(stdout, "connection test: WARNING — {error}").map_err(io_err)?;
                        writeln!(
                            stdout,
                            "Config is installed; start the llmup proxy and re-run --status."
                        )
                        .map_err(io_err)?;
                    }
                }
            }

            writeln!(
                stdout,
                "Set {ENV_KEY_NAME}=<your-key>, then run: codex exec --profile llmup"
            )
            .map_err(io_err)?;
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_url_strips_trailing_slash() {
        assert_eq!(normalize_base_url("https://x.com/"), "https://x.com");
        assert_eq!(normalize_base_url("https://x.com///"), "https://x.com");
        assert_eq!(normalize_base_url("  https://x.com  "), "https://x.com");
        assert_eq!(normalize_base_url("https://x.com"), "https://x.com");
    }

    #[test]
    fn strip_managed_tables_removes_only_named_tables() {
        let content = "[model_providers.openai]\nname = \"OpenAI\"\n\n[model_providers.llmup]\nname = \"LLMUP\"\n\n[features]\nmulti_agent_v2 = true\n\n[other]\nx = 1\n";
        let stripped = strip_managed_tables(content, &["[model_providers.llmup]", "[features]"]);
        assert!(stripped.contains("[model_providers.openai]"));
        assert!(stripped.contains("name = \"OpenAI\""));
        assert!(stripped.contains("[other]"));
        assert!(stripped.contains("x = 1"));
        assert!(!stripped.contains("[model_providers.llmup]"));
        assert!(!stripped.contains("[features]"));
        assert!(!stripped.contains("multi_agent_v2"));
    }

    #[test]
    fn generate_agent_content_escapes_no_fork_keys() {
        let content = generate_agent_content("m", Some("high"), Some(1000));
        assert!(!content.to_lowercase().contains("fork_turns"));
        assert!(!content.to_lowercase().contains("fork_context"));
        assert!(content.contains("model_reasoning_effort = \"high\""));
        assert!(content.contains("model_context_window = 1000"));
    }

    #[test]
    fn generate_state_content_is_valid_json_without_keys() {
        let content = generate_state_content("1.0", &["a".into(), "b".into()], "t");
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["managed_files"][0], "a");
        let lower = content.to_lowercase();
        assert!(!lower.contains("key") && !lower.contains("token"));
    }
}
