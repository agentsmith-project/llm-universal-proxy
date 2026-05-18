use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use llm_universal_proxy::user_tools::config_wizard::{
    init_non_interactive, parse_config_args, run_cli, ApiKeySource, ConfigCommand, InitOptions,
    ProviderInterface,
};
use llm_universal_proxy::user_tools::env_file::parse_env_file_str;
use llm_universal_proxy::Config;

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

#[test]
fn non_interactive_init_writes_valid_redacted_config_and_0600_secrets() {
    let temp = TempDir::new("config-init");
    let llmup_home = temp.path().join(".llmup");
    let options = InitOptions {
        llmup_home: llmup_home.clone(),
        codex_home: temp.path().join(".llmup-codex"),
        claude_config_dir: temp.path().join(".llmup-claude"),
        interface: ProviderInterface::OpenAi,
        model_service_url: "https://api.minimaxi.com/v1".to_string(),
        model_name: "MiniMax-M2.7-highspeed".to_string(),
        model_alias: "default".to_string(),
        force: false,
    };

    let result = init_non_interactive(options, "provider-secret-from-stdin")
        .expect("init should generate a usable config");

    let config_yaml = fs::read_to_string(&result.config_path).expect("read generated config");
    assert!(!config_yaml.contains("provider-secret-from-stdin"));
    assert!(config_yaml.contains("model_aliases"));
    assert!(config_yaml.contains("default: DEFAULT:MiniMax-M2.7-highspeed"));
    assert!(config_yaml.contains("data_auth"));
    assert!(config_yaml.contains("LLM_UNIVERSAL_PROXY_KEY"));

    let config = Config::from_yaml_str(&config_yaml).expect("generated YAML should parse");
    config.validate().expect("generated YAML should validate");
    let resolved = config
        .resolve_model("default")
        .expect("default alias should resolve");
    assert_eq!(resolved.upstream_name, "DEFAULT");
    assert_eq!(resolved.upstream_model, "MiniMax-M2.7-highspeed");

    let secrets = fs::read_to_string(&result.secrets_path).expect("read generated secrets");
    assert!(secrets.contains("LLM_UNIVERSAL_PROXY_KEY="));
    assert!(secrets.contains("LLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret-from-stdin"));
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
            interface: ProviderInterface::OpenAi,
            model_service_url: "https://api.example.com/v1".to_string(),
            model_name: "other".to_string(),
            model_alias: "default".to_string(),
            force: false,
        },
        "new-secret",
    )
    .expect_err("existing config should not be overwritten without --force");
    assert!(overwrite.contains("--force"));
    assert!(!overwrite.contains("new-secret"));
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
        b"https://api.minimaxi.com/v1\nMiniMax-M2.7-highspeed\nprovider-secret-from-prompt\n"
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
    assert!(config_yaml.contains("format: openai-completion"));
    assert!(config_yaml.contains("default: DEFAULT:MiniMax-M2.7-highspeed"));
    assert!(!config_yaml.contains("provider-secret-from-prompt"));

    let secrets = fs::read_to_string(llmup_home.join("secrets.env")).expect("read secrets");
    assert!(secrets.contains("LLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret-from-prompt"));
}

#[test]
fn config_cli_hides_init_from_help_but_parses_hidden_noninteractive_sources() {
    let help = parse_config_args(vec![OsString::from("--help")]).expect("help parses");
    assert_eq!(help, ConfigCommand::Help);

    let parsed = parse_config_args(vec![
        OsString::from("init"),
        OsString::from("--non-interactive"),
        OsString::from("--interface"),
        OsString::from("anthropic"),
        OsString::from("--model-service-url"),
        OsString::from("https://api.anthropic.example/v1"),
        OsString::from("--model-name"),
        OsString::from("claude-test"),
        OsString::from("--model-alias"),
        OsString::from("default"),
        OsString::from("--api-key-env"),
        OsString::from("TEST_PROVIDER_KEY"),
    ])
    .expect("hidden init should parse for automation");

    let ConfigCommand::Init(init) = parsed else {
        panic!("expected hidden init command");
    };
    assert_eq!(init.interface, ProviderInterface::Anthropic);
    assert_eq!(init.model_alias, "default");
    assert_eq!(
        init.api_key_source,
        ApiKeySource::Env("TEST_PROVIDER_KEY".to_string())
    );

    let err = parse_config_args(vec![
        OsString::from("init"),
        OsString::from("--non-interactive"),
        OsString::from("--api-key"),
        OsString::from("plaintext-secret"),
    ])
    .expect_err("--api-key value must stay unsupported");
    assert!(err.contains("--api-key-stdin"));
    assert!(err.contains("--api-key-env"));
    assert!(!err.contains("plaintext-secret"));
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
