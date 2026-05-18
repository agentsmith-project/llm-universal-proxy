use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use llm_universal_proxy::user_tools::agent_launcher::{
    build_client_argv, build_client_environment, parse_launcher_args,
    write_runtime_config_for_port, AgentKind, LauncherHomes, ProxyMode,
};
use llm_universal_proxy::user_tools::env_file::{parse_env_file_str, EnvFile};
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

fn os_vec(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

#[test]
fn launcher_routing_preserves_os_args_and_only_consumes_first_delimiter_prefix() {
    let parsed = parse_launcher_args(os_vec(&[
        "--llmup-no-proxy",
        "resume",
        "--last",
        "--",
        "--llmup-port",
        "1234",
        "--",
        "path with spaces",
    ]))
    .expect("launcher args should parse");

    assert!(parsed.control.no_proxy);
    assert_eq!(
        parsed.native_argv,
        os_vec(&[
            "resume",
            "--last",
            "--llmup-port",
            "1234",
            "--",
            "path with spaces",
        ])
    );

    let literal_delimiter =
        parse_launcher_args(os_vec(&["--", "--"])).expect("literal delimiter should parse");
    assert_eq!(literal_delimiter.native_argv, os_vec(&["--"]));

    let err = parse_launcher_args(os_vec(&["--llmup-unknown"]))
        .expect_err("unknown llmup flags before delimiter should fail");
    assert!(err.contains("--llmup-unknown"));

    let after_delimiter = parse_launcher_args(os_vec(&["--", "--llmup-unknown"]))
        .expect("unknown llmup flag after delimiter belongs to native argv");
    assert_eq!(after_delimiter.native_argv, os_vec(&["--llmup-unknown"]));
}

#[test]
fn managed_injection_is_fixed_and_does_not_scan_native_model_or_provider_flags() {
    let native = os_vec(&[
        "--help",
        "-m",
        "user-model",
        "--oss",
        "--profile",
        "local",
        "-c",
        "model_provider=\"other\"",
    ]);

    let codex = build_client_argv(
        AgentKind::Codex,
        ProxyMode::Managed {
            port: 31337,
            proxy_key: "local-proxy".to_string(),
            secrets: EnvFile::default(),
        },
        &native,
    );

    assert_eq!(&codex[0..2], os_vec(&["-c", "model_provider=\"proxy\""]));
    assert!(codex.contains(&OsString::from(
        "model_providers.proxy.base_url=\"http://127.0.0.1:31337/openai/v1\""
    )));
    assert!(codex
        .windows(2)
        .any(|pair| pair == os_vec(&["-m", "default"])));
    assert!(codex.ends_with(&native));

    let no_proxy = build_client_argv(
        AgentKind::Codex,
        ProxyMode::NoProxy,
        &os_vec(&["--version", "--model", "native"]),
    );
    assert_eq!(no_proxy, os_vec(&["--version", "--model", "native"]));

    let claude = build_client_argv(
        AgentKind::Claude,
        ProxyMode::Managed {
            port: 31338,
            proxy_key: "local-proxy".to_string(),
            secrets: EnvFile::default(),
        },
        &os_vec(&["auth", "--help"]),
    );
    assert_eq!(claude, os_vec(&["--model", "default", "auth", "--help"]));
}

#[test]
fn client_environment_removes_secret_env_names_and_overrides_provider_keys() {
    let secrets = parse_env_file_str(
        r#"
LLM_UNIVERSAL_PROXY_KEY=local-proxy-key
LLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret
"#,
    )
    .expect("secrets should parse");
    let homes = LauncherHomes {
        llmup_home: PathBuf::from("/tmp/llmup"),
        codex_home: PathBuf::from("/tmp/llmup-codex"),
        claude_config_dir: PathBuf::from("/tmp/llmup-claude"),
    };
    let parent = BTreeMap::from([
        (OsString::from("PATH"), OsString::from("/bin")),
        (
            OsString::from("OPENAI_API_KEY"),
            OsString::from("parent-openai"),
        ),
        (
            OsString::from("ANTHROPIC_API_KEY"),
            OsString::from("parent-anthropic"),
        ),
        (
            OsString::from("LLMUP_PROVIDER_DEFAULT_API_KEY"),
            OsString::from("provider-secret"),
        ),
        (
            OsString::from("UNRELATED_SECRET_COPY"),
            OsString::from("provider-secret"),
        ),
        (
            OsString::from("ANTHROPIC_BEDROCK_TOKEN"),
            OsString::from("bedrock"),
        ),
        (
            OsString::from("CLAUDE_CODE_USE_VERTEX"),
            OsString::from("1"),
        ),
        (OsString::from("KEEP_ME"), OsString::from("yes")),
    ]);

    let codex_env = build_client_environment(
        AgentKind::Codex,
        parent.clone(),
        ProxyMode::Managed {
            port: 19001,
            proxy_key: "local-proxy-key".to_string(),
            secrets: secrets.clone(),
        },
        &homes,
    )
    .expect("codex env should build");
    assert_eq!(
        codex_env.get(&OsString::from("OPENAI_API_KEY")),
        Some(&OsString::from("local-proxy-key"))
    );
    assert_eq!(
        codex_env.get(&OsString::from("CODEX_HOME")),
        Some(&homes.codex_home.clone().into_os_string())
    );
    assert!(!codex_env.contains_key(&OsString::from("LLMUP_PROVIDER_DEFAULT_API_KEY")));
    assert!(!codex_env.contains_key(&OsString::from("UNRELATED_SECRET_COPY")));
    assert_eq!(
        codex_env.get(&OsString::from("KEEP_ME")),
        Some(&OsString::from("yes"))
    );

    let claude_env = build_client_environment(
        AgentKind::Claude,
        parent,
        ProxyMode::Managed {
            port: 19002,
            proxy_key: "local-proxy-key".to_string(),
            secrets,
        },
        &homes,
    )
    .expect("claude env should build");
    assert_eq!(
        claude_env.get(&OsString::from("ANTHROPIC_API_KEY")),
        Some(&OsString::from("local-proxy-key"))
    );
    assert_eq!(
        claude_env.get(&OsString::from("ANTHROPIC_BASE_URL")),
        Some(&OsString::from("http://127.0.0.1:19002/anthropic"))
    );
    assert_eq!(
        claude_env.get(&OsString::from("CLAUDE_CONFIG_DIR")),
        Some(&homes.claude_config_dir.clone().into_os_string())
    );
    assert_eq!(
        claude_env.get(&OsString::from("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB")),
        Some(&OsString::from("1"))
    );
    assert!(!claude_env.contains_key(&OsString::from("ANTHROPIC_BEDROCK_TOKEN")));
    assert!(!claude_env.contains_key(&OsString::from("CLAUDE_CODE_USE_VERTEX")));
}

#[test]
fn runtime_config_overrides_listen_without_rewriting_user_config_or_dropping_data_auth() {
    let temp = TempDir::new("runtime-yaml");
    let config_path = temp.path().join("config.yaml");
    let run_dir = temp.path().join("run");
    let user_yaml = r#"
listen: 127.0.0.1:8080
upstream_timeout_secs: 120
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-completion
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
"#;
    fs::write(&config_path, user_yaml).expect("write user config");

    let runtime =
        write_runtime_config_for_port(&config_path, &run_dir, 19003).expect("runtime YAML");

    assert_eq!(
        fs::read_to_string(&config_path).expect("read original"),
        user_yaml,
        "launcher must not rewrite the user's config.yaml in place"
    );
    assert!(runtime.yaml.contains("listen: 127.0.0.1:19003"));
    assert!(runtime.yaml.contains("data_auth"));
    assert!(runtime.yaml.contains("LLM_UNIVERSAL_PROXY_KEY"));

    let parsed = Config::from_yaml_str(&runtime.yaml).expect("runtime YAML should parse");
    parsed.validate().expect("runtime YAML should validate");
    assert_eq!(parsed.listen, "127.0.0.1:19003");
    assert!(parsed.data_auth.is_some());
}
