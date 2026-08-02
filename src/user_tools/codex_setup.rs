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
/// Local mirror of the 8-level reasoning-effort vocabulary for user-facing
/// validation and error messages. The single source of truth for the enum is
/// `config::dialect::ReasoningLevel`; this slice is intentionally CLI-local so
/// `dialect.rs` stays untouched.
const VALID_REASONING_EFFORTS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
/// Default wizard base URL. The proxy exposes no bare `/models` route, only
/// `/openai/v1/models`, so the `/openai/v1` suffix is required for both the
/// connection test and the Codex runtime.
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080/openai/v1";

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
    /// `--interactive` / `--tui`: launch the sequential prompt wizard. A
    /// generate-mode variant — the same flags pre-fill the wizard defaults.
    Interactive,
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
    /// `--force-v1`: derive a per-model catalog pinning V1 (opt-in).
    pub force_v1: bool,
}

/// Paths written by [`install`].
#[derive(Debug, Clone)]
pub struct GeneratedPaths {
    pub profile_path: PathBuf,
    pub agent_path: PathBuf,
    pub state_path: PathBuf,
    /// Path of the derived V1 catalog, present only when `--force-v1`
    /// successfully produced one.
    pub catalog_path: Option<PathBuf>,
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

/// Escape a value for safe interpolation into a TOML basic (double-quoted)
/// string: `\` → `\\`, `"` → `\"`, newline/tab/CR → `\n`/`\t`/`\r`, and any
/// remaining control character becomes a `\uXXXX` escape. Prevents TOML
/// injection via user-supplied `base_url`/`model`/`reasoning_effort`.
fn escape_toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Return the HTTP-key-leak warning when `base_url` uses an unencrypted
/// `http://` scheme, else `None`. Pure so the decision is unit-testable.
fn base_url_http_warning(base_url: &str) -> Option<&'static str> {
    let trimmed = base_url.trim().to_ascii_lowercase();
    if trimmed.starts_with("http://") {
        Some("Warning: provider key will be sent over unencrypted HTTP")
    } else {
        None
    }
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
    let escaped = escape_toml_basic_string(&normalized);
    format!(
        "[model_providers.{PROVIDER_ID}]\n\
         name = \"LLMUP\"\n\
         base_url = \"{escaped}\"\n\
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
    let esc_model = escape_toml_basic_string(model);
    let esc_name = escape_toml_basic_string(&agent_name(model));
    let mut out = String::new();
    out.push_str(&format!("name = \"{esc_name}\"\n"));
    out.push_str(&format!(
        "description = \"LLMUP sub-agent for {esc_model}\"\n"
    ));
    out.push_str(&format!("model = \"{esc_model}\"\n"));
    out.push_str(&format!("model_provider = \"{PROVIDER_ID}\"\n"));
    out.push_str(&format!(
        "developer_instructions = \"{DEVELOPER_INSTRUCTIONS}\"\n"
    ));
    if let Some(effort) = reasoning_effort {
        let esc_effort = escape_toml_basic_string(effort);
        out.push_str(&format!("model_reasoning_effort = \"{esc_effort}\"\n"));
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
///
/// When `catalog_json_path` is `Some` (the `--force-v1` path), a top-level
/// `model_catalog_json = "<path>"` key is emitted. Top-level keys must precede
/// any TOML table header, so it is written first.
pub fn build_profile_content(
    existing: Option<&str>,
    base_url: &str,
    catalog_json_path: Option<&str>,
) -> String {
    let provider_block = generate_provider_block(base_url);
    let features_block = generate_features_block();
    let stripped = strip_managed(
        existing.unwrap_or(""),
        &["[model_providers.llmup]", "[features]"],
        &["model_catalog_json"],
    );
    let mut out = String::new();
    if let Some(path) = catalog_json_path {
        let esc_path = escape_toml_basic_string(path);
        out.push_str(&format!("model_catalog_json = \"{esc_path}\"\n"));
    }
    let stripped = stripped.trim_end();
    if !stripped.is_empty() {
        out.push_str(stripped);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&provider_block);
    out.push('\n');
    out.push_str(&features_block);
    out
}

/// Remove managed TOML content from `content`:
/// - **Tables** listed in `headers` — each spans from its header line up to (but
///   excluding) the next line beginning with `[` or the end of content. Matching
///   is an **exact** comparison against the header text (after trimming) so that
///   user-owned subsections such as `[features.advanced]` or
///   `[model_providers.llmup.auth]` are preserved rather than wiped on re-run.
/// - **Bare top-level keys** listed in `bare_keys` — any line whose trimmed
///   content is `<key> =` (whitespace around `=` tolerated) is dropped. This
///   keeps the re-run idempotent for managed top-level scalars such as
///   `model_catalog_json` while preserving user-owned keys like `env_key`.
///
/// Non-matching lines are kept verbatim.
fn strip_managed(content: &str, headers: &[&str], bare_keys: &[&str]) -> String {
    let mut out = String::new();
    let mut skipping_table = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            // Entering a new table: skip only on an exact header match.
            skipping_table = headers.contains(&trimmed);
            if skipping_table {
                continue;
            }
        } else if skipping_table {
            // Still inside a managed table body.
            continue;
        }
        // Drop managed bare top-level keys (e.g. `model_catalog_json = "..."`).
        let is_managed_key = bare_keys.iter().any(|key| {
            trimmed
                .strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        });
        if is_managed_key {
            continue;
        }
        out.push_str(line);
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
// --force-v1 catalog derivation
// ===========================================================================

/// Pure patcher: force every entry in `catalog["models"]` to
/// `multi_agent_version = "v1"`, preserving all other fields. A model with the
/// field absent gets it added; one set to `"v2"` or `null` is overwritten to
/// `"v1"`. The input is returned cloned (never mutated).
///
/// If the catalog shape is unexpected (`models` missing / not an array, or any
/// entry not an object), this prints a warning and returns the input unchanged
/// rather than silently no-op'ing.
pub fn patch_catalog_v1(catalog: &serde_json::Value) -> serde_json::Value {
    const SHAPE_WARNING: &str =
        "Warning: bundled catalog has unexpected shape; --force-v1 may not have taken effect";

    let Some(models) = catalog.get("models") else {
        eprintln!("{SHAPE_WARNING}");
        return catalog.clone();
    };
    if !models.is_array() {
        eprintln!("{SHAPE_WARNING}");
        return catalog.clone();
    }
    // If any entry is not an object, bail with the warning (return unchanged).
    if !models.as_array().unwrap().iter().all(|m| m.is_object()) {
        eprintln!("{SHAPE_WARNING}");
        return catalog.clone();
    }

    let mut out = catalog.clone();
    for model in out
        .get_mut("models")
        .and_then(|m| m.as_array_mut())
        .unwrap()
        .iter_mut()
    {
        model.as_object_mut().unwrap().insert(
            "multi_agent_version".to_string(),
            serde_json::Value::String("v1".to_string()),
        );
    }
    out
}

/// Run `codex debug models --bundled` and return its stdout (the official
/// catalog JSON). Not unit-tested — requires the `codex` binary; the pure
/// [`patch_catalog_v1`] is tested instead.
fn run_codex_debug_models_bundled() -> Result<String, String> {
    let output = Command::new("codex")
        .args(["debug", "models", "--bundled"])
        .output()
        .map_err(|error| format!("failed to spawn `codex`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`codex debug models --bundled` exited non-zero: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("`codex` catalog stdout was not valid UTF-8: {error}"))
}

/// Location of the tool-owned derived V1 catalog (`<codex_home>/llmup/...`).
fn derived_catalog_path(codex_home: &Path) -> PathBuf {
    state_dir(codex_home).join("model-catalog.json")
}

/// Parse a raw bundled-catalog JSON, patch every model to V1, and atomically
/// write the derived catalog. Returns the path written.
fn derive_and_write_catalog(codex_home: &Path, raw_json: &str) -> Result<PathBuf, String> {
    let catalog: serde_json::Value = serde_json::from_str(raw_json)
        .map_err(|error| format!("failed to parse codex bundled catalog JSON: {error}"))?;
    let patched = patch_catalog_v1(&catalog);
    let body = serde_json::to_string_pretty(&patched)
        .map_err(|error| format!("failed to serialize derived V1 catalog: {error}"))?;
    let path = derived_catalog_path(codex_home);
    write_config_file_atomic(&path, &body)?;
    Ok(path)
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
    llm-universal-proxy codex-setup --interactive
    llm-universal-proxy codex-setup --status
    llm-universal-proxy codex-setup --uninstall
    llm-universal-proxy codex-setup --help

OPTIONS:
    --base-url <url>           llmup proxy base URL (required for install)
    --model <alias>            model alias/id routed through llmup (required for install)
    --provider-key <key>       provider key used for the connection test only;
                               the config references the LLMUP_PROXY_KEY env var
    --reasoning-effort <none|minimal|low|medium|high|xhigh|max|ultra>
                               optional model_reasoning_effort for the sub-agent; if
                               omitted, Codex's own default applies
    --context-window <N>       optional model_context_window for the sub-agent
    --force-v1                 derive a per-model catalog pinning multi_agent_version = \"v1\"
                               (requires the codex CLI; falls back to feature-flag-only otherwise)
    --interactive, --tui       launch a sequential prompt-based wizard (requires a terminal);
                               the flags above act as pre-filled defaults in the wizard.
                               Mutually exclusive with --status / --uninstall
    --status                   show what codex-setup has installed
    --uninstall                remove files managed by codex-setup
    --help, -h                 show this help

Config is written under ~/.codex (or $CODEX_HOME). No API keys are stored.
";

pub fn parse_args(args: &[String]) -> Result<CliOptions, String> {
    let mut opts = CliOptions::default();
    let mut status = false;
    let mut uninstall = false;
    let mut interactive = false;

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
            "--interactive" | "--tui" => interactive = true,
            "--force-v1" => opts.force_v1 = true,
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
    } else if interactive && (status || uninstall) {
        return Err(
            "--interactive is mutually exclusive with --status and --uninstall".to_string(),
        );
    } else if status {
        Action::Status
    } else if uninstall {
        Action::Uninstall
    } else if interactive {
        Action::Interactive
    } else {
        Action::Generate
    };
    opts.action = action;

    // Feature 1: validate --reasoning-effort against the 8-level union
    // vocabulary. Invalid values are rejected up front with a message that
    // enumerates every option so the user sees the menu. Omitting the flag is
    // always valid (Codex's default applies).
    if let Some(effort) = &opts.reasoning_effort {
        if !VALID_REASONING_EFFORTS.contains(&effort.as_str()) {
            return Err(format!(
                "invalid --reasoning-effort `{effort}`; valid: none|minimal|low|medium|high|xhigh|max|ultra"
            ));
        }
    }

    if opts.action == Action::Generate {
        if opts.base_url.is_none() {
            return Err("--base-url is required".to_string());
        }
        if opts.model.is_none() {
            return Err("--model is required".to_string());
        }
        // N4: reject path traversal / shell metacharacters in the model alias.
        if let Some(model) = &opts.model {
            if !is_valid_model_name(model) {
                return Err(format!(
                    "--model contains invalid characters (allowed: letters, digits, '.', '_', '-'): `{model}`"
                ));
            }
        }
        // --provider-key is optional for Generate (N7): it only gates the live
        // connection test, so an omitted key is accepted.
    }

    Ok(opts)
}

/// A valid model alias contains only safe characters `[A-Za-z0-9._-]` and is
/// non-empty, preventing path traversal and shell-injection via `--model`.
fn is_valid_model_name(model: &str) -> bool {
    !model.is_empty()
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
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

/// Read the `managed_files` list from a prior `state.json`, if any. Returns an
/// empty vec when the file is absent or unparseable so that a fresh install
/// starts clean while a re-install can union in previously tracked files.
fn read_previous_managed_files(state_file: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(state_file) else {
        return Vec::new();
    };
    let Ok(value): serde_json::Result<serde_json::Value> = serde_json::from_str(&content) else {
        return Vec::new();
    };
    value
        .get("managed_files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| f.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
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
///
/// Pass `bundled_catalog_json = Some(raw)` (the stdout of
/// `codex debug models --bundled`) to enable `--force-v1` catalog derivation:
/// every model's `multi_agent_version` is pinned to `"v1"`, the derived
/// catalog is written, the profile gains `model_catalog_json`, and the catalog
/// path is recorded in `state.json` for uninstall. `None` keeps the default
/// feature-flag-only V1 defense.
pub fn install(
    input: &SetupInput,
    codex_home: &Path,
    bundled_catalog_json: Option<&str>,
) -> Result<GeneratedPaths, String> {
    let profile = profile_path(codex_home);
    let agent = agent_path(codex_home, &input.model);
    let state_file = state_path(codex_home);

    // Optional V1 catalog derivation (--force-v1 with a successful codex read).
    let catalog_path: Option<PathBuf> = match bundled_catalog_json {
        Some(raw) => Some(derive_and_write_catalog(codex_home, raw)?),
        None => None,
    };

    // Profile: merge into any existing tool-owned file.
    let existing_profile = fs::read_to_string(&profile).ok();
    let catalog_str = catalog_path.as_ref().and_then(|p| p.to_str());
    let profile_content =
        build_profile_content(existing_profile.as_deref(), &input.base_url, catalog_str);
    write_config_file_atomic(&profile, &profile_content)?;

    // Agent: fully regenerated per model (tool-owned file).
    let agent_content = generate_agent_content(
        &input.model,
        input.reasoning_effort.as_deref(),
        input.context_window,
    );
    write_config_file_atomic(&agent, &agent_content)?;

    // State manifest (no keys). UNION with any previously tracked files so a
    // re-install with a different model does not orphan prior agent files.
    let codex_version = detect_codex_version();
    let created_at = timestamp_utc();
    let mut managed_files = vec![
        profile.to_string_lossy().into_owned(),
        agent.to_string_lossy().into_owned(),
    ];
    if let Some(catalog) = &catalog_path {
        managed_files.push(catalog.to_string_lossy().into_owned());
    }
    for prev in read_previous_managed_files(&state_file) {
        if !managed_files.iter().any(|f| f == &prev) {
            managed_files.push(prev);
        }
    }
    let state_content = generate_state_content(&codex_version, &managed_files, &created_at);
    write_config_file_atomic(&state_file, &state_content)?;

    Ok(GeneratedPaths {
        profile_path: profile,
        agent_path: agent,
        state_path: state_file,
        catalog_path,
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

/// Extract model identifiers from a `/models` response `models` array.
///
/// Reads the `slug` field first (the Codex-UA catalog key) and falls back to
/// `id` (the conventional OpenAI shape), so the connection test is correct for
/// both the llmup proxy catalog and any id-style upstream. Entries lacking both
/// fields, or with a non-string value, are dropped.
pub fn extract_model_ids(models: &[serde_json::Value]) -> Vec<String> {
    models
        .iter()
        .filter_map(|model| model.get("slug").or_else(|| model.get("id")))
        .filter_map(|value| value.as_str().map(|s| s.to_string()))
        .collect()
}

// ===========================================================================
// Wizard decision logic (Feature 2)
// ===========================================================================
//
// The wizard prompts are thin I/O wrappers over `dialoguer` (verified via the
// binary, not unit tests). The *decisions* the wizard makes are factored into
// the two pure helpers below so the logic is unit-testable.

/// Resolve the final model for the wizard given everything known:
/// - `discovered`: models returned by the connection test (may be empty);
/// - `pre_filled`: the `--model` flag value, if any (a wizard pre-fill);
/// - `free_text`: the model the user explicitly typed or selected this session.
///
/// Priority: an explicit current choice (`free_text`) beats the flag default
/// (`pre_filled`), which beats the first discovered model. Empty strings are
/// treated as absent. Returns an empty `String` when nothing is available (the
/// caller validates with [`is_valid_model_name`] and aborts on failure).
fn resolve_wizard_model(
    discovered: &[String],
    pre_filled: Option<&str>,
    free_text: Option<&str>,
) -> String {
    free_text
        .filter(|s| !s.is_empty())
        .or(pre_filled.filter(|s| !s.is_empty()))
        .or(discovered.first().map(String::as_str))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Map a `Select` index from the reasoning-effort menu to an effort level.
/// The menu is `[ "default (omit)", <8 levels...> ]`, so index `0` → `None`
/// (omit the key) and indices `1..=8` map to [`VALID_REASONING_EFFORTS`].
/// Out-of-range indices gracefully yield `None` (no panic).
fn resolve_wizard_effort(selection: usize) -> Option<String> {
    if selection == 0 {
        return None;
    }
    VALID_REASONING_EFFORTS
        .get(selection - 1)
        .map(|s| s.to_string())
}

/// Hit `<base_url>/models` with the provider key and the Codex user agent.
/// Returns the list of model ids on success.
pub async fn connection_test(base_url: &str, provider_key: &str) -> Result<Vec<String>, String> {
    let normalized = normalize_base_url(base_url);
    let url = format!("{normalized}/models");
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
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
    Ok(extract_model_ids(models))
}

// ===========================================================================
// Interactive wizard (Feature 2)
// ===========================================================================

/// Run the 9-step sequential wizard using `dialoguer`. Reuses the existing
/// [`install`] / [`connection_test`] / [`build_profile_content`] /
/// [`generate_agent_content`] / [`run_codex_debug_models_bundled`] — no new
/// write logic. The caller ([`run_with`]) gates this on stdin being a TTY.
///
/// Cancellation safety: every prompt returns before [`install`] is called, so
/// ESC / Ctrl-C at any step writes nothing. The provider key is used only for
/// the step-3 connection test and is never persisted ([`SetupInput`] has no key
/// field).
async fn run_wizard(opts: &CliOptions, home: &Path, stdout: &mut dyn Write) -> Result<i32, String> {
    use dialoguer::{Confirm, Input, Password, Select};

    // Any prompt error (ESC / Ctrl-C / EOF) is a clean abort: nothing has been
    // written yet because [`install`] runs only at the final confirm.
    macro_rules! prompt {
        ($expr:expr) => {
            match $expr {
                Ok(value) => value,
                Err(_) => {
                    writeln!(stdout, "Wizard aborted; no files were written.").map_err(io_err)?;
                    return Ok(0);
                }
            }
        };
    }

    // Step 1 — base_url (Input, pre-fill from --base-url).
    let base_url: String = prompt!(Input::new()
        .with_prompt("llmup proxy base URL")
        .default(
            opts.base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
        )
        .interact_text());

    // Step 2 — provider_key (Password, masked, optional: empty = skip).
    let provider_key: String = prompt!(Password::new()
        .with_prompt("Provider key (connection test only; never stored — enter to skip)")
        .allow_empty_password(true)
        .interact());

    // Step 3 — connection test (automatic; only when a key was entered).
    let discovered: Vec<String> = if provider_key.trim().is_empty() {
        writeln!(stdout, "Skipping connection test (no key entered).").map_err(io_err)?;
        Vec::new()
    } else {
        if let Some(warning) = base_url_http_warning(&base_url) {
            writeln!(stdout, "{warning}").map_err(io_err)?;
        }
        match connection_test(&base_url, &provider_key).await {
            Ok(models) if models.is_empty() => {
                writeln!(
                    stdout,
                    "connection test: WARNING — reachable but 0 models discovered \
                     (check base_url includes /openai/v1)."
                )
                .map_err(io_err)?;
                Vec::new()
            }
            Ok(models) => {
                writeln!(
                    stdout,
                    "connection test: OK ({} models reachable).",
                    models.len()
                )
                .map_err(io_err)?;
                models
            }
            Err(error) => {
                writeln!(stdout, "connection test: WARNING — {error}").map_err(io_err)?;
                Vec::new()
            }
        }
    };

    // Step 4 — model: Select when models were discovered, else free-text Input.
    let chosen: Option<String> = if discovered.is_empty() {
        let default = opts.model.as_deref().unwrap_or("").to_string();
        let entered: String = prompt!(Input::new()
            .with_prompt("Model alias/id")
            .default(default)
            .interact_text());
        Some(entered)
    } else {
        let items: Vec<&str> = discovered.iter().map(String::as_str).collect();
        let default_idx = opts
            .model
            .as_deref()
            .and_then(|m| discovered.iter().position(|d| d == m))
            .unwrap_or(0);
        let idx = prompt!(Select::new()
            .with_prompt("Select model")
            .items(&items)
            .default(default_idx)
            .interact());
        Some(discovered.get(idx).cloned().unwrap_or_default())
    };
    let model = resolve_wizard_model(&discovered, opts.model.as_deref(), chosen.as_deref());
    if !is_valid_model_name(&model) {
        writeln!(
            stdout,
            "Invalid model name `{model}` (allowed: letters, digits, '.', '_', '-'); aborting."
        )
        .map_err(io_err)?;
        return Ok(2);
    }

    // Step 5 — reasoning_effort (Select: "default (omit)" + 8 levels).
    let effort_items: Vec<&str> = std::iter::once("default (omit)")
        .chain(VALID_REASONING_EFFORTS.iter().copied())
        .collect();
    let default_effort_idx = opts
        .reasoning_effort
        .as_deref()
        .and_then(|e| VALID_REASONING_EFFORTS.iter().position(|v| *v == e))
        .map(|p| p + 1)
        .unwrap_or(0);
    let selection = prompt!(Select::new()
        .with_prompt("Reasoning effort")
        .items(&effort_items)
        .default(default_effort_idx)
        .interact());
    let reasoning_effort = resolve_wizard_effort(selection);

    // Step 6 — context_window (optional numeric Input; blank = omit).
    let ctx_entered: String = prompt!(Input::new()
        .with_prompt("Context window (blank to omit)")
        .default(String::new())
        .interact_text());
    let context_window: Option<u64> = if ctx_entered.trim().is_empty() {
        None
    } else {
        match ctx_entered.trim().parse::<u64>() {
            Ok(n) => Some(n),
            Err(_) => {
                writeln!(
                    stdout,
                    "Context window must be a non-negative integer; aborting."
                )
                .map_err(io_err)?;
                return Ok(2);
            }
        }
    };

    // Step 7 — force-v1 (Confirm).
    let force_v1 = prompt!(Confirm::new()
        .with_prompt("Force V1 for all official models? (requires codex CLI)")
        .default(opts.force_v1)
        .interact());

    // Step 8 — preview (print the exact bodies that would be written).
    let agent_content = generate_agent_content(&model, reasoning_effort.as_deref(), context_window);
    let profile_content = build_profile_content(None, &base_url, None);
    writeln!(stdout, "\n--- preview: agent TOML ---\n{agent_content}").map_err(io_err)?;
    writeln!(stdout, "--- preview: profile TOML ---\n{profile_content}").map_err(io_err)?;
    if force_v1 {
        writeln!(
            stdout,
            "(--force-v1: a model_catalog_json key will be derived and added to the profile.)"
        )
        .map_err(io_err)?;
    }

    // Step 9 — confirm install (Confirm). No → exit without writing.
    let proceed = prompt!(Confirm::new()
        .with_prompt("Proceed with installation?")
        .default(true)
        .interact());
    if !proceed {
        writeln!(stdout, "Aborted; no files were written.").map_err(io_err)?;
        return Ok(0);
    }

    // Install reuses the exact same path as flag mode.
    let input = SetupInput {
        base_url: base_url.clone(),
        model: model.clone(),
        reasoning_effort: reasoning_effort.clone(),
        context_window,
    };
    let bundled_catalog = if force_v1 {
        match run_codex_debug_models_bundled() {
            Ok(raw) => Some(raw),
            Err(_) => {
                writeln!(
                    stdout,
                    "WARNING: --force-v1 requires the codex CLI; \
                     falling back to feature-flag-only V1 defense."
                )
                .map_err(io_err)?;
                None
            }
        }
    } else {
        None
    };
    let paths = install(&input, home, bundled_catalog.as_deref())?;
    writeln!(
        stdout,
        "codex-setup: wrote Codex V1 hybrid sub-agent config."
    )
    .map_err(io_err)?;
    writeln!(stdout, "  profile: {}", paths.profile_path.display()).map_err(io_err)?;
    writeln!(stdout, "  agent:   {}", paths.agent_path.display()).map_err(io_err)?;
    writeln!(stdout, "  state:   {}", paths.state_path.display()).map_err(io_err)?;
    writeln!(
        stdout,
        "Set {ENV_KEY_NAME}=<your-key>, then run: codex exec --profile llmup"
    )
    .map_err(io_err)?;
    Ok(0)
}

/// Entry point invoked from `main.rs` when `argv[1] == "codex-setup"`.
pub async fn run(args: &[String]) -> Result<i32, String> {
    let mut stdout = std::io::stdout();
    run_with(args, &mut stdout).await
}

/// Testable entry point mirroring [`run`] but writing to a caller-supplied
/// buffer. `pub` so integration tests can exercise the full action dispatch
/// (including the optional-`--provider-key` skip path).
pub async fn run_with(args: &[String], stdout: &mut dyn Write) -> Result<i32, String> {
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
        Action::Interactive => {
            // Non-TTY stdin (CI / piped / nohup) makes dialoguer read EOF and
            // misbehave; reject with guidance instead of hanging.
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() {
                writeln!(
                    stdout,
                    "--interactive requires a terminal; use flags for scripting."
                )
                .map_err(io_err)?;
                return Ok(2);
            }
            let home = codex_home()?;
            run_wizard(&opts, &home, stdout).await
        }
        Action::Generate => {
            let home = codex_home()?;
            let input = SetupInput {
                base_url: opts.base_url.clone().expect("validated"),
                model: opts.model.clone().expect("validated"),
                reasoning_effort: opts.reasoning_effort.clone(),
                context_window: opts.context_window,
            };
            // --force-v1: read the official catalog and derive a V1-pinned copy.
            // Failure is non-fatal — we fall back to the feature-flag-only V1
            // defense already baked into every profile.
            let bundled_catalog = if opts.force_v1 {
                match run_codex_debug_models_bundled() {
                    Ok(raw) => Some(raw),
                    Err(_) => {
                        writeln!(
                            stdout,
                            "WARNING: --force-v1 requires the codex CLI; \
                             falling back to feature-flag-only V1 defense."
                        )
                        .map_err(io_err)?;
                        None
                    }
                }
            } else {
                None
            };
            let paths = install(&input, &home, bundled_catalog.as_deref())?;

            writeln!(
                stdout,
                "codex-setup: wrote Codex V1 hybrid sub-agent config."
            )
            .map_err(io_err)?;
            writeln!(stdout, "  profile: {}", paths.profile_path.display()).map_err(io_err)?;
            writeln!(stdout, "  agent:   {}", paths.agent_path.display()).map_err(io_err)?;
            writeln!(stdout, "  state:   {}", paths.state_path.display()).map_err(io_err)?;

            // Live connection test (best-effort; install already succeeded).
            // --provider-key is optional (N7): skip the test entirely when not
            // supplied instead of erroring out.
            if let Some(key) = opts.provider_key.as_deref() {
                if let Some(warning) = base_url_http_warning(&input.base_url) {
                    writeln!(stdout, "{warning}").map_err(io_err)?;
                }
                match connection_test(&input.base_url, key).await {
                    Ok(models) if models.is_empty() => {
                        // Reachable but no models: not a success state. The usual
                        // cause is a missing `/openai/v1` suffix on the base URL
                        // (the proxy exposes no bare `/models` route).
                        writeln!(
                            stdout,
                            "connection test: WARNING — reachable but 0 models discovered \
                             (check base_url includes /openai/v1)."
                        )
                        .map_err(io_err)?;
                    }
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
            } else {
                writeln!(stdout, "Skipping connection test (no --provider-key)").map_err(io_err)?;
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
        let stripped = strip_managed(content, &["[model_providers.llmup]", "[features]"], &[]);
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

    // ---------- M3: TOML basic-string escaping ----------

    #[test]
    fn escape_toml_basic_string_escapes_special_chars() {
        assert_eq!(escape_toml_basic_string(r#"a"b"#), r#"a\"b"#);
        assert_eq!(escape_toml_basic_string(r"a\b"), r"a\\b");
        assert_eq!(escape_toml_basic_string("a\nb"), r"a\nb");
        assert_eq!(escape_toml_basic_string("a\tb"), r"a\tb");
        assert_eq!(escape_toml_basic_string("a\rb"), r"a\rb");
        // Other control chars fall back to a TOML \uXXXX escape.
        let escaped_ctrl = escape_toml_basic_string("\u{0001}");
        assert!(
            escaped_ctrl.starts_with('\\') && escaped_ctrl.contains('u'),
            "control char should be \\u-escaped: {escaped_ctrl}"
        );
        // Plain values pass through untouched.
        assert_eq!(escape_toml_basic_string("gpt-5.2"), "gpt-5.2");
    }

    // ---------- N2: HTTP-scheme warning helper ----------

    #[test]
    fn base_url_http_warning_detects_http_scheme() {
        assert!(base_url_http_warning("http://example.com").is_some());
        assert!(base_url_http_warning("https://example.com").is_none());
        assert!(base_url_http_warning("not a url").is_none());
    }

    // ---------- Feature 2: wizard decision logic ----------

    #[test]
    fn resolve_wizard_model_prefers_explicit_free_text() {
        // The user typed/selected something this session — that wins over any
        // pre-filled flag default or discovered model.
        let discovered = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            resolve_wizard_model(&discovered, Some("a"), Some("gpt-5")),
            "gpt-5"
        );
        // Even with no discovery, free-text wins over the pre-filled default.
        assert_eq!(
            resolve_wizard_model(&[], Some("prefilled"), Some("typed")),
            "typed"
        );
    }

    #[test]
    fn resolve_wizard_model_falls_back_to_prefilled() {
        // No explicit choice → use the --model flag default.
        assert_eq!(
            resolve_wizard_model(&[], Some("flag-model"), None),
            "flag-model"
        );
        // Pre-filled beats the first discovered model.
        let discovered = vec!["first".to_string(), "second".to_string()];
        assert_eq!(
            resolve_wizard_model(&discovered, Some("flag-model"), None),
            "flag-model"
        );
    }

    #[test]
    fn resolve_wizard_model_falls_back_to_first_discovered() {
        // Nothing explicit and no flag → first discovered model is the default.
        let discovered = vec!["first".to_string(), "second".to_string()];
        assert_eq!(resolve_wizard_model(&discovered, None, None), "first");
    }

    #[test]
    fn resolve_wizard_model_empty_when_nothing_available() {
        assert_eq!(resolve_wizard_model(&[], None, None), "");
        // Empty strings are treated as absent (e.g. user pressed Enter on a
        // blank free-text prompt with no pre-fill).
        assert_eq!(resolve_wizard_model(&[], Some(""), Some("")), "");
    }

    #[test]
    fn resolve_wizard_effort_default_index_is_none() {
        // Index 0 is the leading "default (omit)" menu choice.
        assert_eq!(resolve_wizard_effort(0), None);
    }

    #[test]
    fn resolve_wizard_effort_maps_levels() {
        assert_eq!(resolve_wizard_effort(1), Some("none".to_string()));
        assert_eq!(resolve_wizard_effort(2), Some("minimal".to_string()));
        assert_eq!(resolve_wizard_effort(5), Some("high".to_string()));
        assert_eq!(resolve_wizard_effort(8), Some("ultra".to_string()));
    }

    #[test]
    fn resolve_wizard_effort_out_of_range_is_none() {
        // Past the end of the 8-level menu → graceful None (no panic).
        assert_eq!(resolve_wizard_effort(9), None);
        assert_eq!(resolve_wizard_effort(usize::MAX), None);
    }

    #[test]
    fn parse_args_interactive_flag_sets_action() {
        let opts = parse_args(&["--interactive".into()]).expect("interactive");
        assert_eq!(opts.action, Action::Interactive);
    }

    #[test]
    fn parse_args_tui_alias_sets_action() {
        let opts = parse_args(&["--tui".into()]).expect("tui alias");
        assert_eq!(opts.action, Action::Interactive);
    }

    #[test]
    fn parse_args_interactive_does_not_require_generate_flags() {
        // --interactive gathers base-url/model via the wizard, so they must NOT
        // be required at parse time.
        let opts = parse_args(&["--interactive".into()]).expect("interactive ok");
        assert_eq!(opts.action, Action::Interactive);
        assert_eq!(opts.base_url, None);
        assert_eq!(opts.model, None);
    }

    #[test]
    fn parse_args_interactive_and_status_mutually_exclusive() {
        let err = parse_args(&["--interactive".into(), "--status".into()]).unwrap_err();
        assert!(
            err.contains("mutually exclusive") && err.contains("--interactive"),
            "expected interactive+status mutual-exclusion error: {err}"
        );
    }

    #[test]
    fn parse_args_interactive_and_uninstall_mutually_exclusive() {
        let err = parse_args(&["--uninstall".into(), "--tui".into()]).unwrap_err();
        assert!(
            err.contains("mutually exclusive") && err.contains("--interactive"),
            "expected interactive+uninstall mutual-exclusion error: {err}"
        );
    }

    #[test]
    fn parse_args_interactive_accepts_prefilled_generate_flags() {
        // Generate flags alongside --interactive act as wizard pre-fills.
        let opts = parse_args(&[
            "--interactive".into(),
            "--base-url".into(),
            "https://proxy.example.com".into(),
            "--model".into(),
            "gpt-5".into(),
            "--reasoning-effort".into(),
            "high".into(),
        ])
        .expect("interactive + pre-fills");
        assert_eq!(opts.action, Action::Interactive);
        assert_eq!(opts.base_url.as_deref(), Some("https://proxy.example.com"));
        assert_eq!(opts.model.as_deref(), Some("gpt-5"));
        assert_eq!(opts.reasoning_effort.as_deref(), Some("high"));
    }
}
