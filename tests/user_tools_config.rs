use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use llm_universal_proxy::user_tools::config_wizard::{
    init_non_interactive, parse_config_args, run_cli, ConfigCommand, InitOptions, ProviderInterface,
};
use llm_universal_proxy::user_tools::env_file::parse_env_file_str;
use llm_universal_proxy::Config;
use serde_yaml::Value;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "llmup-user-tools-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(unix)]
unsafe extern "C" {
    fn umask(mask: std::os::raw::c_uint) -> std::os::raw::c_uint;
}

#[cfg(unix)]
struct UmaskGuard {
    previous: std::os::raw::c_uint,
}

#[cfg(unix)]
impl UmaskGuard {
    fn set(mask: u32) -> Self {
        let previous = unsafe { umask(mask as std::os::raw::c_uint) };
        Self { previous }
    }
}

#[cfg(unix)]
impl Drop for UmaskGuard {
    fn drop(&mut self) {
        unsafe {
            umask(self.previous);
        }
    }
}

struct EnvGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<Path>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value.as_ref());
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn seed_local_config(llmup_home: &Path, yaml: &str) -> (PathBuf, PathBuf, String) {
    seed_local_config_with_secrets(
        llmup_home,
        yaml,
        "LLM_UNIVERSAL_PROXY_KEY=local-proxy-key\nLLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret\n",
    )
}

fn seed_local_config_with_secrets(
    llmup_home: &Path,
    yaml: &str,
    secrets: &str,
) -> (PathBuf, PathBuf, String) {
    fs::create_dir_all(llmup_home).expect("create llmup home");
    let config_path = llmup_home.join("config.yaml");
    let secrets_path = llmup_home.join("secrets.env");
    fs::write(&config_path, yaml).expect("write config");
    fs::write(&secrets_path, secrets).expect("write secrets");
    (config_path, secrets_path, secrets.to_string())
}

fn yaml_get<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .as_mapping()
        .and_then(|mapping| mapping.get(Value::String(key.to_string())))
        .unwrap_or_else(|| panic!("missing YAML key {key}"))
}

#[cfg(unix)]
fn config_temp_file_modes(dir: &Path) -> Vec<(PathBuf, u32)> {
    use std::os::unix::fs::PermissionsExt;

    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if !name.starts_with(".config.yaml.tmp-") {
                        return None;
                    }
                    let mode = entry.metadata().ok()?.permissions().mode() & 0o777;
                    Some((entry.path(), mode))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn non_interactive_init_writes_valid_redacted_config_and_0600_secrets() {
    let temp = TempDir::new("config-init");
    let llmup_home = temp.path().join(".llmup");
    let options = InitOptions {
        llmup_home: llmup_home.clone(),
        codex_home: temp.path().join(".llmup-codex"),
        claude_config_dir: temp.path().join(".llmup-claude"),
        interface: ProviderInterface::OpenAiChatCompletions,
        model_service_url: "https://api.minimaxi.com/v1".to_string(),
        model_name: "MiniMax-M2.7-highspeed".to_string(),
        model_alias: "main".to_string(),
        force: false,
    };

    let result = init_non_interactive(options, "provider-secret-from-stdin")
        .expect("init should generate a usable config");

    let config_yaml = fs::read_to_string(&result.config_path).expect("read generated config");
    assert!(!config_yaml.contains("provider-secret-from-stdin"));
    assert!(config_yaml.contains("model_aliases"));
    assert!(config_yaml.contains("main: main:MiniMax-M2.7-highspeed"));
    assert!(!config_yaml.contains("DEFAULT"));
    assert!(!config_yaml.contains("default:"));
    assert!(config_yaml.contains("data_auth"));
    assert!(config_yaml.contains("LLM_UNIVERSAL_PROXY_KEY"));

    let config = Config::from_yaml_str(&config_yaml).expect("generated YAML should parse");
    config.validate().expect("generated YAML should validate");
    let resolved = config
        .resolve_model("main")
        .expect("main alias should resolve");
    assert_eq!(resolved.upstream_name, "main");
    assert_eq!(resolved.upstream_model, "MiniMax-M2.7-highspeed");

    let secrets = fs::read_to_string(&result.secrets_path).expect("read generated secrets");
    assert!(secrets.contains("LLM_UNIVERSAL_PROXY_KEY="));
    assert!(secrets.contains("LLMUP_PROVIDER_MAIN_API_KEY=provider-secret-from-stdin"));
    assert!(!secrets.contains("LLMUP_PROVIDER_DEFAULT_API_KEY"));
    parse_env_file_str(&secrets).expect("generated secrets.env should use the safe subset");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&result.secrets_path)
            .expect("secrets metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    assert!(result.codex_home.is_dir());
    assert!(result.claude_config_dir.is_dir());
    assert!(!result.summary.contains("provider-secret-from-stdin"));

    let overwrite = init_non_interactive(
        InitOptions {
            llmup_home,
            codex_home: result.codex_home.clone(),
            claude_config_dir: result.claude_config_dir.clone(),
            interface: ProviderInterface::OpenAiChatCompletions,
            model_service_url: "https://api.example.com/v1".to_string(),
            model_name: "other".to_string(),
            model_alias: "main".to_string(),
            force: false,
        },
        "new-secret",
    )
    .expect_err("existing config should not be overwritten without reconfigure");
    assert!(overwrite.contains("choose reconfigure"));
    assert!(!overwrite.contains("new-secret"));
}

#[test]
fn set_limits_for_string_alias_upgrades_alias_without_touching_secrets() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("set-limits-alias");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
"#,
    );

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(
        vec![
            OsString::from("set-limits"),
            OsString::from("--alias"),
            OsString::from("default"),
            OsString::from("--context-window"),
            OsString::from("200000"),
            OsString::from("--max-output-tokens"),
            OsString::from("128000"),
        ],
        &mut stdin,
        &mut stdout,
    )
    .expect("set-limits should update an existing string alias");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("alias default"));
    assert!(output.contains("200000"));
    assert!(output.contains("128000"));

    let rendered = fs::read_to_string(&config_path).expect("read updated config");
    let value: Value = serde_yaml::from_str(&rendered).expect("updated config should be YAML");
    let alias = yaml_get(
        yaml_get(yaml_get(&value, "model_aliases"), "default"),
        "target",
    );
    assert_eq!(alias.as_str(), Some("DEFAULT:test-model"));
    let limits = yaml_get(
        yaml_get(yaml_get(&value, "model_aliases"), "default"),
        "limits",
    );
    assert_eq!(yaml_get(limits, "context_window").as_u64(), Some(200_000));
    assert_eq!(
        yaml_get(limits, "max_output_tokens").as_u64(),
        Some(128_000)
    );
    let parsed = Config::from_yaml_str(&rendered).expect("updated YAML should parse");
    parsed.validate().expect("updated YAML should validate");
    assert_eq!(
        parsed.model_aliases["default"]
            .limits
            .as_ref()
            .and_then(|limits| limits.context_window),
        Some(200_000)
    );
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after set-limits"),
        original_secrets
    );
}

#[test]
fn set_limits_for_structured_alias_preserves_target_and_surface_and_reports_unchanged() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("set-limits-structured-alias");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, _secrets_path, _original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  vision:
    target: DEFAULT:vision-model
    limits:
      context_window: 100000
      max_output_tokens: 8000
    surface:
      modalities:
        input: ["text", "image"]
        output: ["text"]
      tools:
        supports_search: true
        supports_view_image: true
"#,
    );

    for _ in 0..2 {
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let code = run_cli(
            vec![
                OsString::from("set-limits"),
                OsString::from("--alias"),
                OsString::from("vision"),
                OsString::from("--context-window"),
                OsString::from("200000"),
                OsString::from("--max-output-tokens"),
                OsString::from("64000"),
            ],
            &mut stdin,
            &mut stdout,
        )
        .expect("set-limits should update an existing structured alias");
        assert_eq!(code, 0);
    }

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(
        vec![
            OsString::from("set-limits"),
            OsString::from("--alias"),
            OsString::from("vision"),
            OsString::from("--context-window"),
            OsString::from("200000"),
            OsString::from("--max-output-tokens"),
            OsString::from("64000"),
        ],
        &mut stdin,
        &mut stdout,
    )
    .expect("idempotent set-limits should succeed");
    assert_eq!(code, 0);
    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("unchanged"));

    let rendered = fs::read_to_string(&config_path).expect("read updated config");
    let value: Value = serde_yaml::from_str(&rendered).expect("updated config should be YAML");
    let vision = yaml_get(yaml_get(&value, "model_aliases"), "vision");
    assert_eq!(
        yaml_get(vision, "target").as_str(),
        Some("DEFAULT:vision-model")
    );
    assert_eq!(
        yaml_get(yaml_get(vision, "surface"), "modalities")
            .as_mapping()
            .and_then(|mapping| mapping.get(Value::String("input".to_string())))
            .and_then(Value::as_sequence)
            .map(Vec::len),
        Some(2)
    );
    let limits = yaml_get(vision, "limits");
    assert_eq!(yaml_get(limits, "context_window").as_u64(), Some(200_000));
    assert_eq!(yaml_get(limits, "max_output_tokens").as_u64(), Some(64_000));
}

#[test]
fn set_limits_for_upstream_updates_upstream_without_upgrading_aliases() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("set-limits-upstream");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, _secrets_path, _original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
"#,
    );

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(
        vec![
            OsString::from("set-limits"),
            OsString::from("--upstream"),
            OsString::from("DEFAULT"),
            OsString::from("--context-window"),
            OsString::from("200000"),
            OsString::from("--max-output-tokens"),
            OsString::from("128000"),
        ],
        &mut stdin,
        &mut stdout,
    )
    .expect("set-limits should update an existing upstream");
    assert_eq!(code, 0);

    let rendered = fs::read_to_string(&config_path).expect("read updated config");
    let value: Value = serde_yaml::from_str(&rendered).expect("updated config should be YAML");
    let default_upstream = yaml_get(yaml_get(&value, "upstreams"), "DEFAULT");
    let limits = yaml_get(default_upstream, "limits");
    assert_eq!(yaml_get(limits, "context_window").as_u64(), Some(200_000));
    assert_eq!(
        yaml_get(limits, "max_output_tokens").as_u64(),
        Some(128_000)
    );
    assert_eq!(
        yaml_get(yaml_get(&value, "model_aliases"), "default").as_str(),
        Some("DEFAULT:test-model")
    );
}

#[cfg(unix)]
#[test]
fn set_limits_temp_config_is_private_before_contents_are_written() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Barrier};

    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("set-limits-temp-permissions");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let padding = "x".repeat(16 * 1024 * 1024);
    let (config_path, _secrets_path, _original_secrets) = seed_local_config(
        &llmup_home,
        &format!(
            r#"
listen: 127.0.0.1:8080
debug_trace:
  path: "{padding}"
  max_text_chars: 1
data_auth:
  mode: proxy_key
  proxy_key:
    inline: local-proxy-secret-in-config
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      inline: provider-secret-in-config
model_aliases:
  default: DEFAULT:test-model
"#
        ),
    );
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
        .expect("chmod seeded config");

    let stop = Arc::new(AtomicBool::new(false));
    let observed_group_or_other_mode = Arc::new(AtomicU32::new(0));
    let ready = Arc::new(Barrier::new(2));
    let monitor_dir = llmup_home.clone();
    let monitor_stop = Arc::clone(&stop);
    let monitor_bad_mode = Arc::clone(&observed_group_or_other_mode);
    let monitor_ready = Arc::clone(&ready);
    let monitor = std::thread::spawn(move || {
        monitor_ready.wait();
        while !monitor_stop.load(Ordering::SeqCst) {
            for (_path, mode) in config_temp_file_modes(&monitor_dir) {
                if mode & 0o077 != 0 {
                    monitor_bad_mode.store(mode, Ordering::SeqCst);
                    monitor_stop.store(true, Ordering::SeqCst);
                    return;
                }
            }
            std::hint::spin_loop();
        }
    });
    ready.wait();

    let _umask = UmaskGuard::set(0);
    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let result = run_cli(
        vec![
            OsString::from("set-limits"),
            OsString::from("--alias"),
            OsString::from("default"),
            OsString::from("--context-window"),
            OsString::from("200000"),
            OsString::from("--max-output-tokens"),
            OsString::from("128000"),
        ],
        &mut stdin,
        &mut stdout,
    );

    stop.store(true, Ordering::SeqCst);
    monitor
        .join()
        .expect("temp permission monitor should finish");

    let code = result.expect("set-limits should update sensitive config");
    assert_eq!(code, 0);

    let bad_mode = observed_group_or_other_mode.load(Ordering::SeqCst);
    assert_eq!(
        bad_mode & 0o077,
        0,
        "temporary config exposed group/other permissions: {bad_mode:03o}"
    );
    assert_eq!(
        fs::metadata(&config_path)
            .expect("updated config metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        config_temp_file_modes(&llmup_home).is_empty(),
        "temporary config files should not be left behind"
    );
}

#[test]
fn set_limits_rejects_bad_inputs_and_unknown_targets_without_writing() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("set-limits-rejects");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, _secrets_path, _original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
"#,
    );
    let original_config = fs::read_to_string(&config_path).expect("read original config");

    let bad_one_of = parse_config_args(vec![
        OsString::from("set-limits"),
        OsString::from("--alias"),
        OsString::from("default"),
        OsString::from("--upstream"),
        OsString::from("DEFAULT"),
        OsString::from("--context-window"),
        OsString::from("200000"),
        OsString::from("--max-output-tokens"),
        OsString::from("128000"),
    ])
    .expect_err("alias and upstream should be mutually exclusive");
    assert!(bad_one_of.contains("choose exactly one"));

    let bad_limits = parse_config_args(vec![
        OsString::from("set-limits"),
        OsString::from("--alias"),
        OsString::from("default"),
        OsString::from("--context-window"),
        OsString::from("1024"),
        OsString::from("--max-output-tokens"),
        OsString::from("1024"),
    ])
    .expect_err("max output must be lower than context");
    assert!(bad_limits.contains("less than"));

    let zero_context = parse_config_args(vec![
        OsString::from("set-limits"),
        OsString::from("--alias"),
        OsString::from("default"),
        OsString::from("--context-window"),
        OsString::from("0"),
        OsString::from("--max-output-tokens"),
        OsString::from("1"),
    ])
    .expect_err("zero context should fail");
    assert!(zero_context.contains("greater than zero"));

    let zero_max_output = parse_config_args(vec![
        OsString::from("set-limits"),
        OsString::from("--alias"),
        OsString::from("default"),
        OsString::from("--context-window"),
        OsString::from("1024"),
        OsString::from("--max-output-tokens"),
        OsString::from("0"),
    ])
    .expect_err("zero max output should fail");
    assert!(zero_max_output.contains("greater than zero"));

    let missing_target = parse_config_args(vec![
        OsString::from("set-limits"),
        OsString::from("--context-window"),
        OsString::from("1024"),
        OsString::from("--max-output-tokens"),
        OsString::from("1"),
    ])
    .expect_err("target should be required");
    assert!(missing_target.contains("choose exactly one"));

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let missing = run_cli(
        vec![
            OsString::from("set-limits"),
            OsString::from("--alias"),
            OsString::from("missing"),
            OsString::from("--context-window"),
            OsString::from("200000"),
            OsString::from("--max-output-tokens"),
            OsString::from("128000"),
        ],
        &mut stdin,
        &mut stdout,
    )
    .expect_err("unknown alias should fail");
    assert!(missing.contains("unknown alias"));
    assert_eq!(
        fs::read_to_string(&config_path).expect("read config after rejected update"),
        original_config
    );

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let missing = run_cli(
        vec![
            OsString::from("set-limits"),
            OsString::from("--upstream"),
            OsString::from("MISSING"),
            OsString::from("--context-window"),
            OsString::from("200000"),
            OsString::from("--max-output-tokens"),
            OsString::from("128000"),
        ],
        &mut stdin,
        &mut stdout,
    )
    .expect_err("unknown upstream should fail");
    assert!(missing.contains("unknown upstream"));
    assert_eq!(
        fs::read_to_string(&config_path).expect("read config after rejected upstream update"),
        original_config
    );
}

#[test]
fn set_limits_rejects_list_form_upstreams_without_writing() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("set-limits-list-upstreams");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, _secrets_path, _original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
upstreams:
  - name: DEFAULT
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
"#,
    );
    let original_config = fs::read_to_string(&config_path).expect("read original config");

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let err = run_cli(
        vec![
            OsString::from("set-limits"),
            OsString::from("--upstream"),
            OsString::from("DEFAULT"),
            OsString::from("--context-window"),
            OsString::from("200000"),
            OsString::from("--max-output-tokens"),
            OsString::from("128000"),
        ],
        &mut stdin,
        &mut stdout,
    )
    .expect_err("list-form upstreams should fail explicitly");
    assert!(err.contains("upstreams must be a YAML mapping"));
    assert_eq!(
        fs::read_to_string(&config_path).expect("read config after rejected list-form update"),
        original_config
    );
}

#[test]
fn config_help_mentions_set_limits_but_interactive_path_stays_unchanged() {
    let parsed = parse_config_args(vec![
        OsString::from("set-limits"),
        OsString::from("--upstream"),
        OsString::from("DEFAULT"),
        OsString::from("--context-window"),
        OsString::from("200000"),
        OsString::from("--max-output-tokens"),
        OsString::from("128000"),
    ])
    .expect("set-limits should parse");
    assert!(matches!(parsed, ConfigCommand::SetLimits(_)));

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(vec![OsString::from("--help")], &mut stdin, &mut stdout)
        .expect("help should succeed");
    assert_eq!(code, 0);
    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("llmup-config set-limits"));

    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("interactive-prompt-unchanged");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let mut stdin = Cursor::new(
        b"\nhttps://api.minimaxi.com/v1\nMiniMax-M2.7-highspeed\nprovider-secret-from-prompt\n"
            .to_vec(),
    );
    let mut stdout = Vec::new();

    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("interactive config should still succeed");
    assert_eq!(code, 0);
    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(!output.contains("context-window"));
    assert!(!output.contains("max-output-tokens"));
}

#[test]
fn interactive_config_wizard_creates_config_instead_of_printing_usage_only() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("interactive-config");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);

    let mut stdin = Cursor::new(
        b"\nhttps://api.minimaxi.com/v1\nMiniMax-M2.7-highspeed\nprovider-secret-from-prompt\n"
            .to_vec(),
    );
    let mut stdout = Vec::new();

    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("interactive config should succeed");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Wrote llmup config"));
    assert!(output.contains("Next: run llmup-codex or llmup-claude."));
    assert!(!output.contains("Usage:"));
    assert!(!output.contains("provider-secret-from-prompt"));

    let config_yaml =
        fs::read_to_string(llmup_home.join("config.yaml")).expect("read generated config");
    assert!(config_yaml.contains("api_root: https://api.minimaxi.com/v1"));
    assert!(config_yaml.contains("format: openai-chat-completions"));
    assert!(config_yaml.contains("main: main:MiniMax-M2.7-highspeed"));
    assert!(!config_yaml.contains("DEFAULT"));
    assert!(!config_yaml.contains("default:"));
    assert!(!config_yaml.contains("provider-secret-from-prompt"));

    let secrets = fs::read_to_string(llmup_home.join("secrets.env")).expect("read secrets");
    assert!(secrets.contains("LLMUP_PROVIDER_MAIN_API_KEY=provider-secret-from-prompt"));
    assert!(!secrets.contains("LLMUP_PROVIDER_DEFAULT_API_KEY"));
}

#[test]
fn interactive_config_wizard_allows_protocol_format_selection() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("interactive-format");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);

    let mut stdin = Cursor::new(
        b"anthropic-messages\nhttps://api.anthropic.example/v1\nclaude-compatible-model\nprovider-secret-from-prompt\n"
            .to_vec(),
    );
    let mut stdout = Vec::new();

    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("interactive config should succeed");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Model service type"));
    assert!(!output.contains("provider-secret-from-prompt"));

    let config_yaml =
        fs::read_to_string(llmup_home.join("config.yaml")).expect("read generated config");
    assert!(config_yaml.contains("api_root: https://api.anthropic.example/v1"));
    assert!(config_yaml.contains("format: anthropic-messages"));
    assert!(config_yaml.contains("main: main:claude-compatible-model"));
    assert!(!config_yaml.contains("DEFAULT"));
    assert!(!config_yaml.contains("provider-secret-from-prompt"));
}

#[test]
fn config_wizard_rejects_ambiguous_protocol_short_names() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("interactive-rejects-short-interface");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);

    let mut stdin = Cursor::new(
        b"anthropic\nhttps://api.anthropic.example/v1\nclaude-compatible-model\nprovider-secret-from-prompt\n"
            .to_vec(),
    );
    let mut stdout = Vec::new();
    let err = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect_err("ambiguous short protocol name should fail");

    assert!(err.contains("unsupported model service type `anthropic`"));
    assert!(err.contains("openai-chat-completions"));
    assert!(err.contains("openai-responses"));
    assert!(err.contains("anthropic-messages"));
    assert!(!llmup_home.join("config.yaml").exists());
    assert!(!llmup_home.join("secrets.env").exists());
}

#[test]
fn interactive_config_wizard_allows_openai_responses_format_selection() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("interactive-responses-format");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);

    let mut stdin = Cursor::new(
        b"openai-responses\nhttps://api.responses.example/v1\nresponses-model\nprovider-secret-from-prompt\n"
            .to_vec(),
    );
    let mut stdout = Vec::new();

    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("interactive config should succeed");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("openai-chat-completions"));
    assert!(output.contains("openai-responses"));
    assert!(output.contains("anthropic-messages"));
    assert!(!output.contains("provider-secret-from-prompt"));

    let config_yaml =
        fs::read_to_string(llmup_home.join("config.yaml")).expect("read generated config");
    assert!(config_yaml.contains("api_root: https://api.responses.example/v1"));
    assert!(config_yaml.contains("format: openai-responses"));
    assert!(config_yaml.contains("main: main:responses-model"));
    assert!(!config_yaml.contains("DEFAULT"));
    assert!(!config_yaml.contains("provider-secret-from-prompt"));
}

#[test]
fn existing_config_offers_keep_reconfigure_doctor_and_redacted_summary() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("existing-config-summary");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);

    init_non_interactive(
        InitOptions {
            llmup_home: llmup_home.clone(),
            codex_home: temp.path().join(".llmup-codex"),
            claude_config_dir: temp.path().join(".llmup-claude"),
            interface: ProviderInterface::OpenAiChatCompletions,
            model_service_url: "https://api.minimaxi.com/v1".to_string(),
            model_name: "MiniMax-M2.7-highspeed".to_string(),
            model_alias: "main".to_string(),
            force: false,
        },
        "provider-secret-existing",
    )
    .expect("seed config");

    let mut stdin = Cursor::new(b"\n".to_vec());
    let mut stdout = Vec::new();
    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("existing config keep should succeed");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Current llmup config"));
    assert!(output.contains(&format!(
        "config: {}",
        llmup_home.join("config.yaml").display()
    )));
    assert!(output.contains(&format!(
        "secrets: {}",
        llmup_home.join("secrets.env").display()
    )));
    assert!(output.contains("Local models:"));
    assert!(output.contains("main -> main:MiniMax-M2.7-highspeed"));
    assert!(output.contains("Model services:"));
    assert!(output.contains("main"));
    assert!(output.contains("openai-chat-completions"));
    assert!(output.contains("https://api.minimaxi.com/v1"));
    assert!(output.contains("Provider API keys: all configured"));
    assert!(output.contains("Press Enter to finish"));
    assert!(output.contains("add-model"));
    assert!(!output.contains("type add-service, add-alias"));
    assert!(output.contains("Keeping existing config."));
    assert!(!output.contains("provider-secret-existing"));
}

#[test]
fn existing_config_add_model_enters_second_level_model_menu_for_existing_service() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("existing-config-add-model");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  main:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
model_aliases:
  main: main:provider-main
"#,
        "LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key\nLLMUP_PROVIDER_MAIN_API_KEY=keep-main-provider-secret\n",
    );

    let mut stdin =
        Cursor::new(b"add-model\nexisting-service\nmain\nprovider-fast\nfast\n".to_vec());
    let mut stdout = Vec::new();
    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("interactive add-model existing service should succeed");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Press Enter to finish"));
    assert!(output.contains("add-model"));
    assert!(!output.contains("type add-service, add-alias"));
    assert!(output.contains("type new-service or existing-service"));
    assert!(output.contains("Added local model fast -> main:provider-fast"));

    let rendered = fs::read_to_string(config_path).expect("read updated config");
    assert!(rendered.contains("fast: main:provider-fast"));
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after interactive alias add"),
        original_secrets
    );
}

#[test]
fn existing_config_rejects_hidden_legacy_add_model_shortcuts() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("existing-config-rejects-legacy-shortcuts");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  main:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
model_aliases:
  main: main:provider-main
"#,
        "LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key\nLLMUP_PROVIDER_MAIN_API_KEY=keep-main-provider-secret\n",
    );
    let original_config = fs::read_to_string(&config_path).expect("read original config");

    let mut stdin = Cursor::new(
        b"add-service\nbackup\nanthropic-messages\nhttps://backup.example.com/v1\nprovider-sonnet\nsonnet\nsecret\n"
            .to_vec(),
    );
    let mut stdout = Vec::new();
    let err = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect_err("top-level add-service should not be accepted");
    assert!(err.contains("unknown choice `add-service`"));
    assert_eq!(
        fs::read_to_string(&config_path).expect("read config after rejected add-service"),
        original_config
    );
    assert_eq!(
        fs::read_to_string(&secrets_path).expect("read secrets after rejected add-service"),
        original_secrets
    );

    let mut stdin = Cursor::new(b"add-model\nadd-alias\nmain\nprovider-fast\nfast\n".to_vec());
    let mut stdout = Vec::new();
    let err = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect_err("second-level add-alias should not be accepted");
    assert!(err.contains("unknown add-model choice `add-alias`"));
    assert_eq!(
        fs::read_to_string(&config_path).expect("read config after rejected add-alias"),
        original_config
    );
    assert_eq!(
        fs::read_to_string(&secrets_path).expect("read secrets after rejected add-alias"),
        original_secrets
    );
}

#[test]
fn existing_config_add_model_new_service_writes_second_service_alias_and_secret() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("existing-config-add-new-service");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, _original_secrets) = seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  main:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
model_aliases:
  main: main:provider-main
"#,
        "LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key\nLLMUP_PROVIDER_MAIN_API_KEY=keep-main-provider-secret\n",
    );

    let mut stdin = Cursor::new(
        b"add-model\nnew-service\nbackup\nanthropic-messages\nhttps://backup.example.com/v1\nprovider-sonnet\nsonnet\nbackup-provider-secret\n"
            .to_vec(),
    );
    let mut stdout = Vec::new();
    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("interactive add-model new service should succeed");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("type new-service or existing-service"));
    assert!(output.contains("Added model service backup"));
    assert!(output.contains("sonnet -> backup:provider-sonnet"));
    assert!(!output.contains("backup-provider-secret"));

    let rendered = fs::read_to_string(&config_path).expect("read updated config");
    assert!(rendered.contains("backup:"));
    assert!(rendered.contains("env: LLMUP_PROVIDER_BACKUP_API_KEY"));
    assert!(rendered.contains("sonnet: backup:provider-sonnet"));
    assert!(rendered.contains("main: main:provider-main"));
    assert!(!rendered.contains("backup-provider-secret"));
    let parsed = Config::from_yaml_str(&rendered).expect("updated YAML should parse");
    parsed.validate().expect("updated YAML should validate");

    let secrets = fs::read_to_string(secrets_path).expect("read updated secrets");
    assert!(secrets.contains("LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key"));
    assert!(secrets.contains("LLMUP_PROVIDER_MAIN_API_KEY=keep-main-provider-secret"));
    assert!(secrets.contains("LLMUP_PROVIDER_BACKUP_API_KEY=backup-provider-secret"));
}

#[test]
fn config_list_outputs_redacted_multi_model_summary_without_secret_values() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("config-list");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  main:
    api_root: https://api.example.com/v1?token=secret#fragment
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
  backup:
    api_root: https://backup.example.com/v1
    format: anthropic-messages
    provider_key:
      env: LLMUP_PROVIDER_BACKUP_API_KEY
model_aliases:
  main: main:provider-main
  sonnet: backup:provider-sonnet
"#,
        "LLM_UNIVERSAL_PROXY_KEY=local-proxy-key\nLLMUP_PROVIDER_MAIN_API_KEY=main-provider-secret\nLLMUP_PROVIDER_BACKUP_API_KEY=backup-provider-secret\n",
    );

    let parsed = parse_config_args(vec![OsString::from("list")]).expect("list parses");
    assert_eq!(parsed, ConfigCommand::List);

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(vec![OsString::from("list")], &mut stdin, &mut stdout)
        .expect("list should summarize local files");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Current llmup config"));
    assert!(output.contains("Model services:"));
    assert!(output.contains("main"));
    assert!(output.contains("backup"));
    assert!(output.contains("openai-chat-completions"));
    assert!(output.contains("anthropic-messages"));
    assert!(output.contains("Local models:"));
    assert!(output.contains("main -> main:provider-main"));
    assert!(output.contains("sonnet -> backup:provider-sonnet"));
    assert!(output.contains("Provider API keys: all configured"));
    assert!(output.contains("https://api.example.com/v1?redacted=true#redacted"));
    assert!(!output.contains("token=secret"));
    assert!(!output.contains("main-provider-secret"));
    assert!(!output.contains("backup-provider-secret"));
}

#[test]
fn add_model_new_service_creates_upstream_alias_and_secret_without_overwriting_existing_secrets() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("add-model-new-service");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, _original_secrets) = seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  main:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
model_aliases:
  main: main:provider-main
"#,
        "LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key\nLLMUP_PROVIDER_MAIN_API_KEY=keep-main-provider-secret\n",
    );

    let mut stdin = Cursor::new(b"backup-provider-secret\n".to_vec());
    let mut stdout = Vec::new();
    let code = run_cli(
        vec![
            OsString::from("add-model"),
            OsString::from("--new-service"),
            OsString::from("--service-name"),
            OsString::from("backup"),
            OsString::from("--interface"),
            OsString::from("anthropic-messages"),
            OsString::from("--url"),
            OsString::from("https://backup.example.com/v1"),
            OsString::from("--model"),
            OsString::from("provider-sonnet"),
            OsString::from("--alias"),
            OsString::from("sonnet"),
            OsString::from("--api-key-stdin"),
        ],
        &mut stdin,
        &mut stdout,
    )
    .expect("add-model --new-service should succeed");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Added model service backup"));
    assert!(output.contains("sonnet -> backup:provider-sonnet"));
    assert!(!output.contains("backup-provider-secret"));

    let rendered = fs::read_to_string(&config_path).expect("read updated config");
    assert!(rendered.contains("backup:"));
    assert!(rendered.contains("format: anthropic-messages"));
    assert!(rendered.contains("env: LLMUP_PROVIDER_BACKUP_API_KEY"));
    assert!(rendered.contains("sonnet: backup:provider-sonnet"));
    assert!(rendered.contains("main: main:provider-main"));
    assert!(!rendered.contains("backup-provider-secret"));
    let parsed = Config::from_yaml_str(&rendered).expect("updated YAML should parse");
    parsed.validate().expect("updated YAML should validate");
    let resolved = parsed.resolve_model("sonnet").expect("new alias resolves");
    assert_eq!(resolved.upstream_name, "backup");
    assert_eq!(resolved.upstream_model, "provider-sonnet");

    let secrets = fs::read_to_string(secrets_path).expect("read updated secrets");
    assert!(secrets.contains("LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key"));
    assert!(secrets.contains("LLMUP_PROVIDER_MAIN_API_KEY=keep-main-provider-secret"));
    assert!(secrets.contains("LLMUP_PROVIDER_BACKUP_API_KEY=backup-provider-secret"));
}

#[test]
fn add_model_existing_service_only_adds_alias_without_touching_secrets() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("add-model-existing-service");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  main:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
model_aliases:
  main: main:provider-main
"#,
        "LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key\nLLMUP_PROVIDER_MAIN_API_KEY=keep-main-provider-secret\n",
    );

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(
        vec![
            OsString::from("add-model"),
            OsString::from("--service"),
            OsString::from("main"),
            OsString::from("--model"),
            OsString::from("provider-fast"),
            OsString::from("--alias"),
            OsString::from("fast"),
        ],
        &mut stdin,
        &mut stdout,
    )
    .expect("add-model --service should succeed");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Added local model fast -> main:provider-fast"));
    let rendered = fs::read_to_string(&config_path).expect("read updated config");
    assert!(rendered.contains("fast: main:provider-fast"));
    let parsed = Config::from_yaml_str(&rendered).expect("updated YAML should parse");
    parsed.validate().expect("updated YAML should validate");
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after alias add"),
        original_secrets
    );
}

#[test]
fn add_model_existing_service_allows_legacy_and_handwritten_upstream_names() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("add-model-existing-legacy-service");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  DEFAULT:
    api_root: https://default.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
  openai_chat:
    api_root: https://chat.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_OPENAI_CHAT_API_KEY
  foo.bar:
    api_root: https://dot.example.com/v1
    format: anthropic-messages
    provider_key:
      env: LLMUP_PROVIDER_FOO_BAR_API_KEY
model_aliases:
  legacy-main: DEFAULT:provider-main
"#,
        "LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key\nLLMUP_PROVIDER_DEFAULT_API_KEY=keep-default-provider-secret\nLLMUP_PROVIDER_OPENAI_CHAT_API_KEY=keep-chat-provider-secret\nLLMUP_PROVIDER_FOO_BAR_API_KEY=keep-dot-provider-secret\n",
    );

    for (service, model, alias) in [
        ("DEFAULT", "provider-fast", "legacy-default"),
        ("openai_chat", "provider-chat", "legacy-chat"),
        ("foo.bar", "provider-dot", "legacy-dot"),
    ] {
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let code = run_cli(
            vec![
                OsString::from("add-model"),
                OsString::from("--service"),
                OsString::from(service),
                OsString::from("--model"),
                OsString::from(model),
                OsString::from("--alias"),
                OsString::from(alias),
            ],
            &mut stdin,
            &mut stdout,
        )
        .expect("existing handwritten service names should be accepted");
        assert_eq!(code, 0);
    }

    let rendered = fs::read_to_string(&config_path).expect("read updated config");
    assert!(rendered.contains("legacy-default: DEFAULT:provider-fast"));
    assert!(rendered.contains("legacy-chat: openai_chat:provider-chat"));
    assert!(rendered.contains("legacy-dot: foo.bar:provider-dot"));
    let parsed = Config::from_yaml_str(&rendered).expect("updated YAML should parse");
    parsed.validate().expect("updated YAML should validate");
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after legacy service alias adds"),
        original_secrets
    );
}

#[test]
fn add_model_rejects_reserved_empty_or_ambiguous_names_without_writing() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("add-model-invalid-names");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  main:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
model_aliases:
  main: main:provider-main
"#,
        "LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key\nLLMUP_PROVIDER_MAIN_API_KEY=keep-main-provider-secret\n",
    );
    let original_config = fs::read_to_string(&config_path).expect("read original config");

    let invalid_new_services = [
        ("", "empty service name"),
        ("DEFAULT", "reserved service name"),
        ("Foo", "case-variant service name"),
        ("foo bar", "space in service name"),
        ("foo:bar", "colon in service name"),
        ("foo_bar", "underscore in service name"),
        ("openai_chat", "underscore in service name"),
        ("foo.bar", "dot in service name"),
    ];
    for (service_name, label) in invalid_new_services {
        let mut stdin = Cursor::new(b"new-secret\n".to_vec());
        let mut stdout = Vec::new();
        let err = run_cli(
            vec![
                OsString::from("add-model"),
                OsString::from("--new-service"),
                OsString::from("--service-name"),
                OsString::from(service_name),
                OsString::from("--interface"),
                OsString::from("openai-chat-completions"),
                OsString::from("--url"),
                OsString::from("https://new.example.com/v1"),
                OsString::from("--model"),
                OsString::from("provider-new"),
                OsString::from("--alias"),
                OsString::from("new-alias"),
                OsString::from("--api-key-stdin"),
            ],
            &mut stdin,
            &mut stdout,
        )
        .expect_err(label);
        assert!(
            err.contains("service-name") || err.contains("service"),
            "unexpected error for {label}: {err}"
        );
    }

    let invalid_aliases = [
        ("", "empty alias"),
        ("default", "reserved alias"),
        ("Foo", "case-variant alias"),
        ("foo bar", "space in alias"),
        ("foo:bar", "colon in alias"),
        ("foo_bar", "underscore in alias"),
        ("foo.bar", "dot in alias"),
    ];
    for (alias, label) in invalid_aliases {
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let err = run_cli(
            vec![
                OsString::from("add-model"),
                OsString::from("--service"),
                OsString::from("main"),
                OsString::from("--model"),
                OsString::from("provider-new"),
                OsString::from("--alias"),
                OsString::from(alias),
            ],
            &mut stdin,
            &mut stdout,
        )
        .expect_err(label);
        assert!(
            err.contains("alias") || err.contains("model name"),
            "unexpected error for {label}: {err}"
        );
    }

    assert_eq!(
        fs::read_to_string(&config_path).expect("read config after rejected add-model"),
        original_config
    );
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after rejected add-model"),
        original_secrets
    );
}

#[test]
fn add_model_rejects_case_and_normalization_collisions_with_legacy_names() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("add-model-collisions");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  main:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
  FOO:
    api_root: https://foo.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_FOO_API_KEY
  foo.bar:
    api_root: https://foobar.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_FOOBAR_API_KEY
model_aliases:
  main: main:provider-main
  CaseAlias: main:provider-case
  foo_bar: main:provider-foo
"#,
        "LLM_UNIVERSAL_PROXY_KEY=keep-local-proxy-key\nLLMUP_PROVIDER_MAIN_API_KEY=keep-main-provider-secret\nLLMUP_PROVIDER_FOO_API_KEY=foo-secret\nLLMUP_PROVIDER_FOOBAR_API_KEY=foobar-secret\n",
    );
    let original_config = fs::read_to_string(&config_path).expect("read original config");

    for alias in ["casealias", "foo-bar"] {
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let err = run_cli(
            vec![
                OsString::from("add-model"),
                OsString::from("--service"),
                OsString::from("main"),
                OsString::from("--model"),
                OsString::from("provider-new"),
                OsString::from("--alias"),
                OsString::from(alias),
            ],
            &mut stdin,
            &mut stdout,
        )
        .expect_err("alias normalization collision should fail");
        assert!(err.contains("collides"), "unexpected error: {err}");
    }

    for service_name in ["foo", "foo-bar"] {
        let mut stdin = Cursor::new(b"new-secret\n".to_vec());
        let mut stdout = Vec::new();
        let err = run_cli(
            vec![
                OsString::from("add-model"),
                OsString::from("--new-service"),
                OsString::from("--service-name"),
                OsString::from(service_name),
                OsString::from("--interface"),
                OsString::from("openai-chat-completions"),
                OsString::from("--url"),
                OsString::from("https://new.example.com/v1"),
                OsString::from("--model"),
                OsString::from("provider-new"),
                OsString::from("--alias"),
                OsString::from("new-alias"),
                OsString::from("--api-key-stdin"),
            ],
            &mut stdin,
            &mut stdout,
        )
        .expect_err("service normalization collision should fail");
        assert!(err.contains("collides"), "unexpected error: {err}");
    }

    assert_eq!(
        fs::read_to_string(&config_path).expect("read config after rejected collisions"),
        original_config
    );
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after rejected collisions"),
        original_secrets
    );
}

#[test]
fn legacy_default_alias_gets_rename_hint_without_automatic_migration() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("legacy-default-hint");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, _original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
"#,
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&secrets_path, fs::Permissions::from_mode(0o600))
            .expect("secure legacy secrets");
    }
    let original_config = fs::read_to_string(&config_path).expect("read original config");

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(vec![OsString::from("list")], &mut stdin, &mut stdout)
        .expect("legacy config should still list");
    assert_eq!(code, 0);
    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("default -> DEFAULT:test-model"));
    assert!(output.contains("Action needed"));
    assert!(output.contains("rename"));
    assert!(output.contains("main"));
    assert!(output.contains("llmup launchers use `main`"));

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(vec![OsString::from("doctor")], &mut stdin, &mut stdout)
        .expect("legacy config should still doctor");
    assert_eq!(code, 0);
    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Action needed"));
    assert!(output.contains("rename"));
    assert!(output.contains("main"));
    assert_eq!(
        fs::read_to_string(config_path).expect("read config after hint"),
        original_config
    );
}

#[test]
fn legacy_default_alias_pressing_enter_renames_to_main() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("legacy-default-enter-renames");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
"#,
    );

    let mut stdin = Cursor::new(b"\n".to_vec());
    let mut stdout = Vec::new();
    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("pressing enter should take the recommended legacy rename path");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Press Enter to rename it to `main`"));
    assert!(output.contains("Renamed local model default -> main"));
    assert!(!output.contains("Next: run llmup-codex or llmup-claude"));

    let rendered = fs::read_to_string(&config_path).expect("read renamed config");
    assert!(rendered.contains("main: DEFAULT:test-model"));
    assert!(!rendered.contains("default: DEFAULT:test-model"));
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after enter rename"),
        original_secrets
    );
}

#[test]
fn legacy_default_alias_default_action_renames_to_main_without_touching_secrets() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("legacy-default-rename-string");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
"#,
    );

    let mut stdin = Cursor::new(b"\n".to_vec());
    let mut stdout = Vec::new();
    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("default legacy rename should succeed");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Press Enter to rename it to `main`"));
    assert!(output.contains("Renamed local model default -> main"));

    let rendered = fs::read_to_string(&config_path).expect("read renamed config");
    assert!(rendered.contains("main: DEFAULT:test-model"));
    assert!(!rendered.contains("default: DEFAULT:test-model"));
    let parsed = Config::from_yaml_str(&rendered).expect("renamed YAML should parse");
    parsed.validate().expect("renamed YAML should validate");
    assert!(parsed.model_aliases.contains_key("main"));
    assert!(!parsed.model_aliases.contains_key("default"));
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after rename"),
        original_secrets
    );
}

#[test]
fn legacy_structured_default_alias_rename_preserves_alias_object() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("legacy-default-rename-structured");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default:
    target: DEFAULT:test-model
    limits:
      context_window: 200000
      max_output_tokens: 64000
    surface:
      modalities:
        input: ["text", "image"]
        output: ["text"]
"#,
    );

    let mut stdin = Cursor::new(b"\n".to_vec());
    let mut stdout = Vec::new();
    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("default structured legacy rename should succeed");
    assert_eq!(code, 0);

    let rendered = fs::read_to_string(&config_path).expect("read renamed config");
    let value: Value = serde_yaml::from_str(&rendered).expect("renamed config should be YAML");
    let aliases = yaml_get(&value, "model_aliases");
    assert!(aliases
        .as_mapping()
        .expect("aliases should be mapping")
        .get(Value::String("default".to_string()))
        .is_none());
    let main = yaml_get(aliases, "main");
    assert_eq!(
        yaml_get(main, "target").as_str(),
        Some("DEFAULT:test-model")
    );
    assert_eq!(
        yaml_get(yaml_get(main, "limits"), "context_window").as_u64(),
        Some(200_000)
    );
    assert_eq!(
        yaml_get(yaml_get(main, "limits"), "max_output_tokens").as_u64(),
        Some(64_000)
    );
    assert_eq!(
        yaml_get(yaml_get(yaml_get(main, "surface"), "modalities"), "input")
            .as_sequence()
            .map(Vec::len),
        Some(2)
    );
    let parsed = Config::from_yaml_str(&rendered).expect("renamed YAML should parse");
    parsed.validate().expect("renamed YAML should validate");
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after structured rename"),
        original_secrets
    );
}

#[test]
fn legacy_default_alias_is_left_alone_when_main_exists() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("legacy-default-rename-main-exists");
    let llmup_home = temp.path().join(".llmup");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let (config_path, secrets_path, original_secrets) = seed_local_config(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
  main: DEFAULT:main-model
"#,
    );
    let original_config = fs::read_to_string(&config_path).expect("read original config");

    let mut stdin = Cursor::new(b"\n".to_vec());
    let mut stdout = Vec::new();
    let code = run_cli(Vec::<OsString>::new(), &mut stdin, &mut stdout)
        .expect("main config should keep existing aliases");
    assert_eq!(code, 0);
    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("Keeping existing config."));
    assert!(!output.contains("Renamed local model default -> main"));
    assert_eq!(
        fs::read_to_string(&config_path).expect("read config after failed rename"),
        original_config
    );
    assert_eq!(
        fs::read_to_string(secrets_path).expect("read secrets after failed rename"),
        original_secrets
    );
}

#[test]
fn config_doctor_warns_missing_clients_but_validates_files() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("config-doctor");
    let llmup_home = temp.path().join(".llmup");
    let empty_path = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_path).expect("create empty PATH dir");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let _path = EnvGuard::set("PATH", &empty_path);

    init_non_interactive(
        InitOptions {
            llmup_home,
            codex_home: temp.path().join(".llmup-codex"),
            claude_config_dir: temp.path().join(".llmup-claude"),
            interface: ProviderInterface::OpenAiChatCompletions,
            model_service_url: "https://api.example.com/v1".to_string(),
            model_name: "test-model".to_string(),
            model_alias: "main".to_string(),
            force: false,
        },
        "provider-secret-for-doctor",
    )
    .expect("seed config");

    let parsed = parse_config_args(vec![OsString::from("doctor")]).expect("doctor parses");
    assert_eq!(parsed, ConfigCommand::Doctor);
    assert!(parse_config_args(vec![OsString::from("check")]).is_err());
    assert!(parse_config_args(vec![OsString::from("summary")]).is_err());

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(vec![OsString::from("doctor")], &mut stdin, &mut stdout)
        .expect("doctor should succeed for valid config and secrets");
    assert_eq!(code, 0);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("llmup config doctor"));
    assert!(output.contains("OK config YAML parses and validates"));
    assert!(output.contains("OK secrets.env parses"));
    assert!(output.contains("OK required secrets are configured"));
    #[cfg(unix)]
    assert!(output.contains("OK secrets permissions"));
    assert!(output.contains("WARNING codex not found in PATH"));
    assert!(output.contains("WARNING claude not found in PATH"));
    assert!(!output.contains("provider-secret-for-doctor"));
}

#[test]
fn config_doctor_checks_all_provider_key_envs_for_multiple_upstreams() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned");
    let temp = TempDir::new("config-doctor-multi-upstream");
    let llmup_home = temp.path().join(".llmup");
    let empty_path = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_path).expect("create empty PATH dir");
    let _home = EnvGuard::set("HOME", temp.path());
    let _llmup_home = EnvGuard::set("LLMUP_HOME", &llmup_home);
    let _path = EnvGuard::set("PATH", &empty_path);
    seed_local_config_with_secrets(
        &llmup_home,
        r#"
listen: 127.0.0.1:8080
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  main:
    api_root: https://api.example.com/v1
    format: openai-chat-completions
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
  backup:
    api_root: https://backup.example.com/v1
    format: anthropic-messages
    provider_key:
      env: LLMUP_PROVIDER_BACKUP_API_KEY
model_aliases:
  main: main:provider-main
  sonnet: backup:provider-sonnet
"#,
        "LLM_UNIVERSAL_PROXY_KEY=local-proxy-key\nLLMUP_PROVIDER_MAIN_API_KEY=main-provider-secret\n",
    );

    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(vec![OsString::from("doctor")], &mut stdin, &mut stdout)
        .expect("doctor should report missing provider envs without throwing");
    assert_eq!(code, 1);

    let output = String::from_utf8(stdout).expect("stdout should be utf-8");
    assert!(output.contains("ERROR required secrets missing or empty"));
    assert!(output.contains("LLMUP_PROVIDER_BACKUP_API_KEY"));
    assert!(!output.contains("main-provider-secret"));
}

#[test]
fn config_cli_rejects_hidden_init_surface() {
    let help = parse_config_args(vec![OsString::from("--help")]).expect("help parses");
    assert_eq!(help, ConfigCommand::Help);
    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let code = run_cli(vec![OsString::from("--help")], &mut stdin, &mut stdout)
        .expect("help should render");
    assert_eq!(code, 0);
    let output = String::from_utf8(stdout).expect("help should be utf-8");
    assert!(!output.contains("llmup-config init"));

    let err = parse_config_args(vec![
        OsString::from("init"),
        OsString::from("--non-interactive"),
        OsString::from("--interface"),
        OsString::from("anthropic-messages"),
        OsString::from("--model-service-url"),
        OsString::from("https://api.anthropic.example/v1"),
        OsString::from("--model-name"),
        OsString::from("claude-test"),
        OsString::from("--model-alias"),
        OsString::from("main"),
        OsString::from("--api-key-env"),
        OsString::from("TEST_PROVIDER_KEY"),
    ])
    .expect_err("hidden init must not parse");
    assert!(err.contains("unknown llmup-config command `init`"));

    let err = parse_config_args(vec![
        OsString::from("init"),
        OsString::from("--non-interactive"),
        OsString::from("--interface"),
        OsString::from("openai-compatible"),
        OsString::from("--model-service-url"),
        OsString::from("https://api.example.com/v1"),
        OsString::from("--model-name"),
        OsString::from("test-model"),
        OsString::from("--api-key-env"),
        OsString::from("TEST_PROVIDER_KEY"),
    ])
    .expect_err("init must fail before parsing hidden arguments");
    assert!(err.contains("unknown llmup-config command `init`"));
    assert!(!err.contains("openai-compatible"));
}

#[test]
fn env_file_parser_accepts_safe_key_values_and_rejects_shell_semantics() {
    let parsed = parse_env_file_str(
        r#"
# local-only secrets
LLM_UNIVERSAL_PROXY_KEY=local-proxy-key
LLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret
KEY_WITH_DASHES=abc-123_./+
"#,
    )
    .expect("safe env file should parse");

    assert_eq!(
        parsed.get("LLM_UNIVERSAL_PROXY_KEY").map(String::as_str),
        Some("local-proxy-key")
    );
    assert_eq!(
        parsed
            .get("LLMUP_PROVIDER_DEFAULT_API_KEY")
            .map(String::as_str),
        Some("provider-secret")
    );

    for bad in [
        "1BAD=value",
        "BAD-KEY=value",
        "export GOOD=value",
        "GOOD=$(cat /tmp/secret)",
        "GOOD=`cat /tmp/secret`",
        "GOOD=${SECRET}",
        "GOOD=value # shell comment",
        "GOOD",
        " GOOD=value",
    ] {
        let err = parse_env_file_str(bad).expect_err("unsafe env syntax should be rejected");
        assert!(
            err.contains("env") || err.contains("line") || err.contains("shell"),
            "unexpected error for {bad:?}: {err}"
        );
    }
}
