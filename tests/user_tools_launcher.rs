use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use llm_universal_proxy::user_tools::agent_launcher::{
    build_client_argv, build_client_environment, parse_launcher_args, prepare_profile_projection,
    run_cli, validate_native_model_flags, write_runtime_config_for_port, AgentKind, LauncherHomes,
    ProfileProjection, ProxyMode,
};
use llm_universal_proxy::user_tools::agent_model_profile::AgentModelCatalog;
use llm_universal_proxy::user_tools::env_file::{parse_env_file_str, EnvFile};
use llm_universal_proxy::Config;

struct TempDir {
    path: PathBuf,
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value.into());
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
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

fn launcher_profile_config() -> Config {
    Config::from_yaml_str(
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
    limits:
      context_window: 200000
      max_output_tokens: 128000
    surface_defaults:
      modalities:
        input: ["text"]
        output: ["text"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false
model_aliases:
  main: DEFAULT:test-model
  haiku: DEFAULT:test-haiku-model
  opus: DEFAULT:test-opus-model
  sonnet: DEFAULT:test-sonnet-model
  vision:
    target: DEFAULT:vision-model
    surface:
      modalities:
        input: ["text", "image"]
      tools:
        supports_search: true
"#,
    )
    .expect("profile config should parse")
}

fn managed_mode(port: u16) -> ProxyMode {
    ProxyMode::Managed {
        port,
        proxy_key: "local-proxy-key".to_string(),
        secrets: EnvFile::default(),
    }
}

fn enabled_projection(alias: &str) -> ProfileProjection {
    ProfileProjection::Enabled {
        model_catalog: AgentModelCatalog::from_config(&launcher_profile_config(), alias)
            .expect("model catalog should resolve"),
        codex_catalog_path: None,
    }
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
fn launcher_model_flags_parse_and_validate_before_native_args() {
    let parsed = parse_launcher_args(os_vec(&[
        "--llmup-model",
        "vision",
        "--llmup-port=31337",
        "resume",
    ]))
    .expect("model value should parse");
    assert_eq!(parsed.control.model_alias.as_deref(), Some("vision"));
    assert!(!parsed.control.no_profile_projection);
    assert_eq!(parsed.native_argv, os_vec(&["resume"]));

    let equals =
        parse_launcher_args(os_vec(&["--llmup-model=default"])).expect("model= should parse");
    assert_eq!(equals.control.model_alias.as_deref(), Some("default"));

    let no_profile = parse_launcher_args(os_vec(&[
        "--llmup-no-profile-projection",
        "--",
        "--model",
        "native",
    ]))
    .expect("no-profile should parse");
    assert!(no_profile.control.no_profile_projection);
    assert_eq!(no_profile.native_argv, os_vec(&["--model", "native"]));

    for args in [
        os_vec(&["--llmup-model"]),
        os_vec(&["--llmup-model="]),
        vec![OsString::from("--llmup-model"), OsString::new()],
    ] {
        let err = parse_launcher_args(args).expect_err("missing or empty model should fail");
        assert!(err.contains("--llmup-model"), "unexpected error: {err}");
    }

    let err = parse_launcher_args(os_vec(&[
        "--llmup-model",
        "default",
        "--llmup-no-profile-projection",
    ]))
    .expect_err("model and no-profile should conflict");
    assert!(err.contains("--llmup-model"));
    assert!(err.contains("--llmup-no-profile-projection"));

    let err = parse_launcher_args(os_vec(&["--llmup-model=default", "--llmup-no-proxy"]))
        .expect_err("model and no-proxy should conflict");
    assert!(err.contains("--llmup-model"));
    assert!(err.contains("--llmup-no-proxy"));
}

#[test]
fn managed_projection_rejects_native_model_flags_unless_profile_projection_is_disabled() {
    let projection = enabled_projection("main");

    for native in [
        os_vec(&["-m", "native"]),
        os_vec(&["--model", "native"]),
        os_vec(&["--model=native"]),
        os_vec(&["-c", "model=\"native\""]),
        os_vec(&["-cmodel=\"native\""]),
        os_vec(&["-c=model_provider=\"openai\""]),
        os_vec(&["--config", "model_provider=\"openai\""]),
        os_vec(&["-c", "openai_base_url=\"https://api.openai.com/v1\""]),
        os_vec(&["-c", "model_catalog_json=\"/tmp/native-catalog.json\""]),
        os_vec(&["--config=model_providers.proxy.base_url=\"http://127.0.0.1:1/openai/v1\""]),
        os_vec(&["--oss"]),
        os_vec(&["--local-provider", "ollama"]),
        os_vec(&["--local-provider=ollama"]),
        os_vec(&["--profile", "native-profile"]),
        os_vec(&["--profile=native-profile"]),
    ] {
        let err = validate_native_model_flags(AgentKind::Codex, &projection, &native)
            .expect_err("codex native projection override flags should conflict");
        assert!(err.contains("--llmup-model"), "unexpected error: {err}");
        assert!(
            err.contains("--llmup-no-profile-projection"),
            "unexpected error: {err}"
        );
    }

    for native in [os_vec(&["--model", "native"]), os_vec(&["--model=native"])] {
        let err = validate_native_model_flags(AgentKind::Claude, &projection, &native)
            .expect_err("claude native model flags should conflict");
        assert!(err.contains("--llmup-model"), "unexpected error: {err}");
    }

    validate_native_model_flags(
        AgentKind::Codex,
        &ProfileProjection::Disabled,
        &os_vec(&[
            "-m",
            "native",
            "--model=other",
            "-c",
            "model_provider=\"native\"",
            "--oss",
            "--profile",
            "native-profile",
        ]),
    )
    .expect("no-profile should pass native codex projection override flags through");
    validate_native_model_flags(
        AgentKind::Claude,
        &ProfileProjection::Disabled,
        &os_vec(&["--model", "native"]),
    )
    .expect("no-profile should pass native claude model flags through");
}

#[test]
fn managed_projection_preflights_codex_catalog_before_starting_proxy() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let temp = TempDir::new("codex-preflight");
    let llmup_home = temp.path().join("llmup");
    let config_path = temp.path().join("config.yaml");
    let env_file_path = temp.path().join("secrets.env");
    fs::write(
        &config_path,
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
    limits:
      context_window: 200000
      max_output_tokens: 64000
model_aliases:
  __llmup_custom__bad: DEFAULT:test-model
"#,
    )
    .expect("write config");
    fs::write(
        &env_file_path,
        "LLM_UNIVERSAL_PROXY_KEY=local-proxy-key\nLLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret\n",
    )
    .expect("write env file");
    let _guards = [
        EnvGuard::set("HOME", temp.path().as_os_str()),
        EnvGuard::set("LLMUP_HOME", llmup_home.as_os_str()),
        EnvGuard::set(
            "LLMUP_CODEX_HOME",
            temp.path().join("codex-home").as_os_str(),
        ),
        EnvGuard::set(
            "LLMUP_CLAUDE_CONFIG_DIR",
            temp.path().join("claude-config").as_os_str(),
        ),
    ];

    let mut stdout = Vec::new();
    let err = run_cli(
        AgentKind::Codex,
        vec![
            OsString::from("--llmup-config"),
            config_path.into_os_string(),
            OsString::from("--llmup-env-file"),
            env_file_path.into_os_string(),
            OsString::from("--llmup-model"),
            OsString::from("__llmup_custom__bad"),
            OsString::from("--llmup-port"),
            OsString::from("19005"),
        ],
        &mut stdout,
    )
    .expect_err("catalog projection should fail before proxy startup");

    assert!(
        err.contains("codex model catalog") && err.contains("reserved internal tool artifact"),
        "unexpected error: {err}"
    );
}

#[test]
fn managed_projection_injects_codex_multi_alias_catalog_and_selected_model_argv() {
    let temp = TempDir::new("codex-profile");
    let projection = prepare_profile_projection(
        AgentKind::Codex,
        AgentModelCatalog::from_config(&launcher_profile_config(), "vision")
            .expect("model catalog should resolve"),
        temp.path(),
    )
    .expect("codex projection should prepare");
    let codex = build_client_argv(
        AgentKind::Codex,
        &managed_mode(31337),
        &projection,
        &os_vec(&["--help"]),
    );

    assert_eq!(&codex[0..2], os_vec(&["-c", "model_provider=\"proxy\""]));
    assert!(codex.contains(&OsString::from(
        "model_providers.proxy.base_url=\"http://127.0.0.1:31337/openai/v1\""
    )));
    assert!(codex.windows(2).any(|pair| {
        pair == os_vec(&["-c", "openai_base_url=\"http://127.0.0.1:31337/openai/v1\""])
    }));
    assert!(codex
        .windows(2)
        .any(|pair| pair == os_vec(&["-m", "vision"])));
    assert!(codex
        .iter()
        .any(|arg| { arg.to_string_lossy().starts_with("model_catalog_json=\"") }));
    assert!(!codex
        .windows(2)
        .any(|pair| pair == os_vec(&["-c", "tools.web_search=false"])));
    let joined = codex
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(!joined.contains("web_search=\"disabled\""));
    assert!(!joined.contains("tools.view_image=false"));
    assert!(codex.ends_with(&os_vec(&["--help"])));

    let ProfileProjection::Enabled {
        codex_catalog_path: Some(catalog_path),
        ..
    } = &projection
    else {
        panic!("codex projection should include a catalog path");
    };
    let catalog: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(catalog_path).expect("read generated catalog"))
            .expect("catalog should be JSON");
    let models = catalog["models"]
        .as_array()
        .expect("catalog models should be array");
    assert_eq!(models.len(), 5);
    assert_eq!(
        models
            .iter()
            .map(|entry| entry["slug"].as_str().expect("slug should be string"))
            .collect::<Vec<_>>(),
        vec!["haiku", "main", "opus", "sonnet", "vision"]
    );
    let entry = models
        .iter()
        .find(|entry| entry["slug"] == "main")
        .expect("main entry");
    assert_eq!(entry["display_name"], "main");
    assert_eq!(entry["shell_type"], "shell_command");
    assert_eq!(entry["visibility"], "list");
    assert_eq!(entry["supported_in_api"], true);
    assert_eq!(entry["supports_reasoning_summaries"], false);
    assert_eq!(entry["support_verbosity"], false);
    assert_eq!(entry["truncation_policy"]["mode"], "bytes");
    assert_eq!(entry["truncation_policy"]["limit"], 10000);
    assert_eq!(entry["apply_patch_tool_type"], "freeform");
    assert_eq!(entry["supports_parallel_tool_calls"], false);
    assert!(entry["supported_reasoning_levels"].is_array());
    assert!(entry["base_instructions"].as_str().is_some());
    assert_eq!(entry["experimental_supported_tools"], serde_json::json!([]));
    assert_eq!(entry["context_window"], 200000);
    assert_eq!(entry["auto_compact_token_limit"], 61200);
    assert_eq!(entry["input_modalities"], serde_json::json!(["text"]));
    assert_eq!(entry["supports_search_tool"], false);
    let vision = models
        .iter()
        .find(|entry| entry["slug"] == "vision")
        .expect("vision entry");
    assert_eq!(
        vision["input_modalities"],
        serde_json::json!(["text", "image"])
    );
    assert_eq!(vision["supports_search_tool"], true);

    let no_proxy = build_client_argv(
        AgentKind::Codex,
        &ProxyMode::NoProxy,
        &ProfileProjection::Disabled,
        &os_vec(&["--version", "--model", "native"]),
    );
    assert_eq!(no_proxy, os_vec(&["--version", "--model", "native"]));

    let no_profile = build_client_argv(
        AgentKind::Codex,
        &managed_mode(31338),
        &ProfileProjection::Disabled,
        &os_vec(&["--model", "native", "--help"]),
    );
    assert!(no_profile.contains(&OsString::from(
        "model_providers.proxy.base_url=\"http://127.0.0.1:31338/openai/v1\""
    )));
    assert!(!no_profile
        .windows(2)
        .any(|pair| pair == os_vec(&["-m", "main"])));
    assert!(!no_profile
        .iter()
        .any(|arg| { arg.to_string_lossy().starts_with("model_catalog_json=\"") }));
    assert!(no_profile.ends_with(&os_vec(&["--model", "native", "--help"])));
}

#[test]
fn codex_projection_rejects_invalid_input_budget_but_claude_projection_allows_it() {
    let config = Config::from_yaml_str(
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
    limits:
      context_window: 200000
      max_output_tokens: 200000
model_aliases:
  main: DEFAULT:test-model
"#,
    )
    .expect("profile config should parse");
    config.validate().expect("config should validate");
    let model_catalog = AgentModelCatalog::from_config(&config, "main")
        .expect("generic model catalog should allow Claude-compatible limits");
    let temp = TempDir::new("projection-invalid-budget");

    let err = prepare_profile_projection(AgentKind::Codex, model_catalog.clone(), temp.path())
        .expect_err("Codex projection should fail before startup");
    assert!(err.contains("max_output_tokens"));
    assert!(err.contains("context_window"));

    let projection = prepare_profile_projection(AgentKind::Claude, model_catalog, temp.path())
        .expect("Claude projection should not run Codex input-budget checks");
    let ProfileProjection::Enabled {
        codex_catalog_path, ..
    } = projection
    else {
        panic!("Claude projection should remain enabled");
    };
    assert!(codex_catalog_path.is_none());
}

#[test]
fn claude_managed_projection_uses_main_and_family_model_env_without_capabilities_or_discovery() {
    let homes = LauncherHomes {
        llmup_home: PathBuf::from("/tmp/llmup"),
        codex_home: PathBuf::from("/tmp/llmup-codex"),
        claude_config_dir: PathBuf::from("/tmp/llmup-claude"),
    };
    let projection = enabled_projection("main");

    let claude = build_client_argv(
        AgentKind::Claude,
        &managed_mode(31338),
        &projection,
        &os_vec(&["auth", "--help"]),
    );
    assert_eq!(claude, os_vec(&["auth", "--help"]));

    let mut parent = BTreeMap::from([
        (OsString::from("PATH"), OsString::from("/bin")),
        (
            OsString::from("ANTHROPIC_MODEL"),
            OsString::from("parent-model"),
        ),
        (
            OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION"),
            OsString::from("parent-option"),
        ),
        (
            OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"),
            OsString::from("parent-name"),
        ),
        (
            OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"),
            OsString::from("parent-description"),
        ),
        (
            OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES"),
            OsString::from("thinking"),
        ),
        (
            OsString::from("CLAUDE_CODE_MAX_OUTPUT_TOKENS"),
            OsString::from("42"),
        ),
        (
            OsString::from("CLAUDE_CODE_AUTO_COMPACT_WINDOW"),
            OsString::from("42"),
        ),
        (
            OsString::from("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            OsString::from("42"),
        ),
        (
            OsString::from("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"),
            OsString::from("1"),
        ),
    ]);
    let profile_env_keys = [
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL_NAME",
        "ANTHROPIC_SMALL_FAST_MODEL_DESCRIPTION",
        "ANTHROPIC_SMALL_FAST_MODEL_SUPPORTED_CAPABILITIES",
    ];
    for key in profile_env_keys {
        parent.insert(OsString::from(key), OsString::from(format!("parent-{key}")));
    }
    let env = build_client_environment(
        AgentKind::Claude,
        parent,
        &managed_mode(19002),
        &homes,
        &projection,
    )
    .expect("claude env should build");

    assert_eq!(
        env.get(&OsString::from("ANTHROPIC_MODEL")),
        Some(&OsString::from("main"))
    );
    assert_eq!(
        env.get(&OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION")),
        Some(&OsString::from("main"))
    );
    assert_eq!(
        env.get(&OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME")),
        Some(&OsString::from("main"))
    );
    assert_eq!(
        env.get(&OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION")),
        Some(&OsString::from("llmup proxy model main"))
    );
    assert_eq!(
        env.get(&OsString::from("CLAUDE_CODE_SUBAGENT_MODEL")),
        Some(&OsString::from("main"))
    );
    assert_eq!(
        env.get(&OsString::from("CLAUDE_CODE_MAX_OUTPUT_TOKENS")),
        Some(&OsString::from("128000"))
    );
    assert_eq!(
        env.get(&OsString::from("CLAUDE_CODE_AUTO_COMPACT_WINDOW")),
        Some(&OsString::from("200000"))
    );
    assert!(!env.contains_key(&OsString::from(
        "ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES"
    )));
    assert!(!env.contains_key(&OsString::from("CLAUDE_CODE_MAX_CONTEXT_TOKENS")));
    assert!(!env.contains_key(&OsString::from(
        "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"
    )));
    for family in ["HAIKU", "SONNET", "OPUS"] {
        let alias = family.to_ascii_lowercase();
        assert_eq!(
            env.get(&OsString::from(format!("ANTHROPIC_DEFAULT_{family}_MODEL"))),
            Some(&OsString::from(alias.as_str()))
        );
        assert_eq!(
            env.get(&OsString::from(format!(
                "ANTHROPIC_DEFAULT_{family}_MODEL_NAME"
            ))),
            Some(&OsString::from(alias.as_str()))
        );
        assert_eq!(
            env.get(&OsString::from(format!(
                "ANTHROPIC_DEFAULT_{family}_MODEL_DESCRIPTION"
            ))),
            Some(&OsString::from(format!("llmup proxy model {alias}")))
        );
        assert!(
            !env.contains_key(&OsString::from(format!(
                "ANTHROPIC_DEFAULT_{family}_MODEL_SUPPORTED_CAPABILITIES"
            ))),
            "{family} capabilities must not be injected"
        );
    }
    assert!(!env.contains_key(&OsString::from("ANTHROPIC_SMALL_FAST_MODEL")));
    assert!(!env.contains_key(&OsString::from("ANTHROPIC_SMALL_FAST_MODEL_NAME")));
    assert!(!env.contains_key(&OsString::from("ANTHROPIC_SMALL_FAST_MODEL_DESCRIPTION")));
    assert!(!env.contains_key(&OsString::from(
        "ANTHROPIC_SMALL_FAST_MODEL_SUPPORTED_CAPABILITIES"
    )));
}

#[test]
fn claude_managed_projection_does_not_inject_family_model_env_without_family_aliases() {
    let homes = LauncherHomes {
        llmup_home: PathBuf::from("/tmp/llmup"),
        codex_home: PathBuf::from("/tmp/llmup-codex"),
        claude_config_dir: PathBuf::from("/tmp/llmup-claude"),
    };
    let config = Config::from_yaml_str(
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  main: DEFAULT:test-model
"#,
    )
    .expect("config should parse");
    config.validate().expect("config should validate");
    let projection = ProfileProjection::Enabled {
        model_catalog: AgentModelCatalog::from_config(&config, "main")
            .expect("model catalog should resolve"),
        codex_catalog_path: None,
    };
    let parent = BTreeMap::from([
        (
            OsString::from("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            OsString::from("parent-haiku"),
        ),
        (
            OsString::from("ANTHROPIC_DEFAULT_SONNET_MODEL"),
            OsString::from("parent-sonnet"),
        ),
        (
            OsString::from("ANTHROPIC_DEFAULT_OPUS_MODEL"),
            OsString::from("parent-opus"),
        ),
    ]);

    let env = build_client_environment(
        AgentKind::Claude,
        parent,
        &managed_mode(19002),
        &homes,
        &projection,
    )
    .expect("claude env should build");

    for family in ["HAIKU", "SONNET", "OPUS"] {
        assert!(!env.contains_key(&OsString::from(format!("ANTHROPIC_DEFAULT_{family}_MODEL"))));
        assert!(!env.contains_key(&OsString::from(format!(
            "ANTHROPIC_DEFAULT_{family}_MODEL_NAME"
        ))));
        assert!(!env.contains_key(&OsString::from(format!(
            "ANTHROPIC_DEFAULT_{family}_MODEL_DESCRIPTION"
        ))));
        assert!(!env.contains_key(&OsString::from(format!(
            "ANTHROPIC_DEFAULT_{family}_MODEL_SUPPORTED_CAPABILITIES"
        ))));
    }
}

#[test]
fn claude_managed_projection_controls_subagent_model_and_attribution_env() {
    let homes = LauncherHomes {
        llmup_home: PathBuf::from("/tmp/llmup"),
        codex_home: PathBuf::from("/tmp/llmup-codex"),
        claude_config_dir: PathBuf::from("/tmp/llmup-claude"),
    };
    let projection = enabled_projection("vision");
    let parent = BTreeMap::from([
        (
            OsString::from("CLAUDE_CODE_SUBAGENT_MODEL"),
            OsString::from("parent-subagent-model"),
        ),
        (
            OsString::from("CLAUDE_CODE_ATTRIBUTION_HEADER"),
            OsString::from("1"),
        ),
    ]);

    let env = build_client_environment(
        AgentKind::Claude,
        parent,
        &managed_mode(19002),
        &homes,
        &projection,
    )
    .expect("claude env should build");

    assert_eq!(
        env.get(&OsString::from("CLAUDE_CODE_SUBAGENT_MODEL")),
        Some(&OsString::from("vision"))
    );
    assert_eq!(
        env.get(&OsString::from("CLAUDE_CODE_ATTRIBUTION_HEADER")),
        Some(&OsString::from("0"))
    );
}

#[test]
fn managed_no_profile_does_not_inject_profile_specific_claude_env() {
    let homes = LauncherHomes {
        llmup_home: PathBuf::from("/tmp/llmup"),
        codex_home: PathBuf::from("/tmp/llmup-codex"),
        claude_config_dir: PathBuf::from("/tmp/llmup-claude"),
    };
    let parent = BTreeMap::from([(
        OsString::from("ANTHROPIC_MODEL"),
        OsString::from("parent-model"),
    )]);

    let env = build_client_environment(
        AgentKind::Claude,
        parent,
        &managed_mode(19002),
        &homes,
        &ProfileProjection::Disabled,
    )
    .expect("claude env should build");
    assert_eq!(
        env.get(&OsString::from("ANTHROPIC_MODEL")),
        Some(&OsString::from("parent-model"))
    );
    assert!(!env.contains_key(&OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION")));
    assert!(!env.contains_key(&OsString::from("CLAUDE_CODE_MAX_OUTPUT_TOKENS")));
}

#[test]
fn claude_managed_no_profile_keeps_user_subagent_model_but_disables_attribution() {
    let homes = LauncherHomes {
        llmup_home: PathBuf::from("/tmp/llmup"),
        codex_home: PathBuf::from("/tmp/llmup-codex"),
        claude_config_dir: PathBuf::from("/tmp/llmup-claude"),
    };
    let parent = BTreeMap::from([
        (
            OsString::from("CLAUDE_CODE_SUBAGENT_MODEL"),
            OsString::from("parent-subagent-model"),
        ),
        (
            OsString::from("CLAUDE_CODE_ATTRIBUTION_HEADER"),
            OsString::from("1"),
        ),
    ]);

    let env = build_client_environment(
        AgentKind::Claude,
        parent,
        &managed_mode(19002),
        &homes,
        &ProfileProjection::Disabled,
    )
    .expect("claude env should build");

    assert_eq!(
        env.get(&OsString::from("CLAUDE_CODE_SUBAGENT_MODEL")),
        Some(&OsString::from("parent-subagent-model"))
    );
    assert_eq!(
        env.get(&OsString::from("CLAUDE_CODE_ATTRIBUTION_HEADER")),
        Some(&OsString::from("0"))
    );
}

#[test]
fn no_proxy_keeps_client_environment_and_argv_fully_native() {
    let homes = LauncherHomes {
        llmup_home: PathBuf::from("/tmp/llmup"),
        codex_home: PathBuf::from("/tmp/llmup-codex"),
        claude_config_dir: PathBuf::from("/tmp/llmup-claude"),
    };
    let parent = BTreeMap::from([
        (OsString::from("HOME"), OsString::from("/real-home")),
        (
            OsString::from("OPENAI_API_KEY"),
            OsString::from("parent-openai-key"),
        ),
        (
            OsString::from("ANTHROPIC_API_KEY"),
            OsString::from("parent-anthropic-key"),
        ),
        (
            OsString::from("ANTHROPIC_BEDROCK_TOKEN"),
            OsString::from("parent-bedrock-token"),
        ),
        (
            OsString::from("CLAUDE_CODE_SUBAGENT_MODEL"),
            OsString::from("parent-subagent-model"),
        ),
        (
            OsString::from("CLAUDE_CODE_ATTRIBUTION_HEADER"),
            OsString::from("1"),
        ),
    ]);

    let codex_argv = build_client_argv(
        AgentKind::Codex,
        &ProxyMode::NoProxy,
        &ProfileProjection::Disabled,
        &os_vec(&["--help", "--model", "native"]),
    );
    assert_eq!(codex_argv, os_vec(&["--help", "--model", "native"]));

    let codex_env = build_client_environment(
        AgentKind::Codex,
        parent.clone(),
        &ProxyMode::NoProxy,
        &homes,
        &ProfileProjection::Disabled,
    )
    .expect("codex no-proxy env should build");
    assert!(!codex_env.contains_key(&OsString::from("CODEX_HOME")));
    assert_eq!(
        codex_env.get(&OsString::from("OPENAI_API_KEY")),
        Some(&OsString::from("parent-openai-key"))
    );
    assert!(!codex_env.contains_key(&OsString::from("OPENAI_BASE_URL")));
    assert_eq!(
        codex_env.get(&OsString::from("HOME")),
        Some(&OsString::from("/real-home"))
    );

    let claude_argv = build_client_argv(
        AgentKind::Claude,
        &ProxyMode::NoProxy,
        &ProfileProjection::Disabled,
        &os_vec(&["auth", "--help"]),
    );
    assert_eq!(claude_argv, os_vec(&["auth", "--help"]));

    let claude_env = build_client_environment(
        AgentKind::Claude,
        parent,
        &ProxyMode::NoProxy,
        &homes,
        &ProfileProjection::Disabled,
    )
    .expect("claude no-proxy env should build");
    assert!(!claude_env.contains_key(&OsString::from("CLAUDE_CONFIG_DIR")));
    assert_eq!(
        claude_env.get(&OsString::from("ANTHROPIC_API_KEY")),
        Some(&OsString::from("parent-anthropic-key"))
    );
    assert_eq!(
        claude_env.get(&OsString::from("ANTHROPIC_BEDROCK_TOKEN")),
        Some(&OsString::from("parent-bedrock-token"))
    );
    assert!(!claude_env.contains_key(&OsString::from("ANTHROPIC_BASE_URL")));
    assert!(!claude_env.contains_key(&OsString::from("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB")));
    assert_eq!(
        claude_env.get(&OsString::from("CLAUDE_CODE_SUBAGENT_MODEL")),
        Some(&OsString::from("parent-subagent-model"))
    );
    assert_eq!(
        claude_env.get(&OsString::from("CLAUDE_CODE_ATTRIBUTION_HEADER")),
        Some(&OsString::from("1"))
    );
    assert_eq!(
        claude_env.get(&OsString::from("HOME")),
        Some(&OsString::from("/real-home"))
    );
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
        &ProxyMode::Managed {
            port: 19001,
            proxy_key: "local-proxy-key".to_string(),
            secrets: secrets.clone(),
        },
        &homes,
        &ProfileProjection::Disabled,
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
        &ProxyMode::Managed {
            port: 19002,
            proxy_key: "local-proxy-key".to_string(),
            secrets,
        },
        &homes,
        &ProfileProjection::Disabled,
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
fn claude_managed_environment_scrubs_provider_routing_and_auth_helpers() {
    let homes = LauncherHomes {
        llmup_home: PathBuf::from("/tmp/llmup"),
        codex_home: PathBuf::from("/tmp/llmup-codex"),
        claude_config_dir: PathBuf::from("/tmp/llmup-claude"),
    };
    let secrets = parse_env_file_str(
        r#"
LLM_UNIVERSAL_PROXY_KEY=local-proxy-key
LLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret
"#,
    )
    .expect("secrets should parse");

    let scrubbed = [
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
        "ANTHROPIC_BEDROCK_TOKEN",
        "ANTHROPIC_VERTEX_PROJECT",
        "ANTHROPIC_FOUNDRY_ENDPOINT",
        "ANTHROPIC_AWS_REGION",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
    ];
    let mut parent = BTreeMap::from([
        (
            OsString::from("ANTHROPIC_API_KEY"),
            OsString::from("parent-anthropic-key"),
        ),
        (
            OsString::from("ANTHROPIC_BASE_URL"),
            OsString::from("https://parent.example"),
        ),
        (OsString::from("KEEP_ME"), OsString::from("yes")),
    ]);
    for key in scrubbed {
        parent.insert(OsString::from(key), OsString::from(format!("{key}-value")));
    }

    let env = build_client_environment(
        AgentKind::Claude,
        parent,
        &ProxyMode::Managed {
            port: 19004,
            proxy_key: "local-proxy-key".to_string(),
            secrets,
        },
        &homes,
        &ProfileProjection::Disabled,
    )
    .expect("claude env should build");

    for key in scrubbed {
        assert!(
            !env.contains_key(&OsString::from(key)),
            "{key} should be scrubbed"
        );
    }
    assert_eq!(
        env.get(&OsString::from("ANTHROPIC_API_KEY")),
        Some(&OsString::from("local-proxy-key"))
    );
    assert_eq!(
        env.get(&OsString::from("ANTHROPIC_BASE_URL")),
        Some(&OsString::from("http://127.0.0.1:19004/anthropic"))
    );
    assert_eq!(
        env.get(&OsString::from("KEEP_ME")),
        Some(&OsString::from("yes"))
    );
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
    format: openai-chat-completions
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

#[test]
fn launcher_default_main_fails_fast_when_only_legacy_default_alias_exists() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let temp = TempDir::new("legacy-default-fail-fast");
    let config_path = temp.path().join("config.yaml");
    let artifact_dir = temp.path().join("artifacts");
    fs::write(
        &config_path,
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:legacy-upstream-model
"#,
    )
    .expect("write config");

    let _guards = [
        EnvGuard::set("HOME", temp.path().as_os_str()),
        EnvGuard::set("LLMUP_INTERNAL_LAUNCH_PLAN", "1"),
    ];

    let mut stdout = Vec::new();
    let err = run_cli(
        AgentKind::Codex,
        vec![
            OsString::from("--llmup-internal-launch-plan-json"),
            OsString::from("--llmup-internal-proxy-base"),
            OsString::from("http://matrix-proxy.local:19090"),
            OsString::from("--llmup-internal-proxy-key"),
            OsString::from("matrix-proxy-key"),
            OsString::from("--llmup-internal-artifact-dir"),
            artifact_dir.into_os_string(),
            OsString::from("--llmup-config"),
            config_path.into_os_string(),
        ],
        &mut stdout,
    )
    .expect_err("launcher should not fall back from main to legacy default");

    assert!(stdout.is_empty());
    assert!(err.contains("main"), "unexpected error: {err}");
    assert!(err.contains("default"), "unexpected error: {err}");
    assert!(err.contains("llmup-config"), "unexpected error: {err}");
    assert!(err.contains("list"), "unexpected error: {err}");
    assert!(err.contains("rename"), "unexpected error: {err}");
}

#[test]
fn claude_family_alias_launch_plans_select_alias_and_proxy_config_resolves_upstream_model() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let temp = TempDir::new("claude-family-launch-plan");
    let config_path = temp.path().join("config.yaml");
    let env_file_path = temp.path().join("secrets.env");
    let artifact_dir = temp.path().join("artifacts");
    let yaml = r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  main: DEFAULT:upstream-main-model
  haiku: DEFAULT:upstream-haiku-model
  sonnet: DEFAULT:upstream-sonnet-model
  opus: DEFAULT:upstream-opus-model
"#;
    fs::write(&config_path, yaml).expect("write config");
    fs::write(
        &env_file_path,
        "LLM_UNIVERSAL_PROXY_KEY=env-file-proxy-key\nLLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret-plan\n",
    )
    .expect("write env file");

    let config = Config::from_yaml_str(yaml).expect("config should parse");
    config.validate().expect("config should validate");
    for (alias, upstream_model) in [
        ("haiku", "upstream-haiku-model"),
        ("sonnet", "upstream-sonnet-model"),
        ("opus", "upstream-opus-model"),
    ] {
        assert_eq!(
            config
                .resolve_model(alias)
                .expect("alias should resolve")
                .upstream_model,
            upstream_model
        );
    }

    let _guards = [
        EnvGuard::set("HOME", temp.path().as_os_str()),
        EnvGuard::set("LLMUP_INTERNAL_LAUNCH_PLAN", "1"),
        EnvGuard::set("LLMUP_CLAUDE_BIN", "/opt/native/claude"),
    ];

    for alias in ["haiku", "sonnet", "opus"] {
        let mut stdout = Vec::new();
        let code = run_cli(
            AgentKind::Claude,
            vec![
                OsString::from("--llmup-internal-launch-plan-json"),
                OsString::from("--llmup-internal-proxy-base"),
                OsString::from("http://matrix-proxy.local:19091"),
                OsString::from("--llmup-internal-proxy-key"),
                OsString::from("matrix-proxy-key"),
                OsString::from("--llmup-internal-artifact-dir"),
                artifact_dir.join(alias).into_os_string(),
                OsString::from("--llmup-config"),
                config_path.clone().into_os_string(),
                OsString::from("--llmup-env-file"),
                env_file_path.clone().into_os_string(),
                OsString::from("--llmup-model"),
                OsString::from(alias),
            ],
            &mut stdout,
        )
        .expect("launch plan should be generated");
        assert_eq!(code, 0);

        let text = String::from_utf8(stdout).expect("launch plan should be utf8");
        let plan: serde_json::Value = serde_json::from_str(&text).expect("plan should be JSON");
        assert_eq!(plan["projection"]["profile"]["alias"], alias);
        assert_eq!(plan["env"]["ANTHROPIC_MODEL"], serde_json::json!(alias));
        assert_eq!(
            plan["env"]["ANTHROPIC_CUSTOM_MODEL_OPTION"],
            serde_json::json!(alias)
        );
        assert_eq!(
            plan["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
            serde_json::json!(alias)
        );
        for family in ["HAIKU", "SONNET", "OPUS"] {
            let family_alias = family.to_ascii_lowercase();
            let model_key = format!("ANTHROPIC_DEFAULT_{family}_MODEL");
            let name_key = format!("ANTHROPIC_DEFAULT_{family}_MODEL_NAME");
            let description_key = format!("ANTHROPIC_DEFAULT_{family}_MODEL_DESCRIPTION");
            let capabilities_key =
                format!("ANTHROPIC_DEFAULT_{family}_MODEL_SUPPORTED_CAPABILITIES");
            assert_eq!(
                plan["env"][model_key.as_str()],
                serde_json::json!(family_alias)
            );
            assert_eq!(
                plan["env"][name_key.as_str()],
                serde_json::json!(family_alias)
            );
            assert_eq!(
                plan["env"][description_key.as_str()],
                serde_json::json!(format!("llmup proxy model {family_alias}"))
            );
            assert!(plan["env"].get(capabilities_key.as_str()).is_none());
        }
        assert!(plan["env"]
            .get("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY")
            .is_none());
        assert!(plan["artifacts"]["codex_model_catalog"].is_null());
    }
}

#[test]
fn internal_launch_plan_requires_env_gate_and_emits_sanitized_codex_projection_json() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let temp = TempDir::new("launch-plan-codex");
    let config_path = temp.path().join("config.yaml");
    let env_file_path = temp.path().join("secrets.env");
    let artifact_dir = temp.path().join("artifacts");
    fs::write(
        &config_path,
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
    limits:
      context_window: 200000
      max_output_tokens: 128000
    surface_defaults:
      modalities:
        input: ["text"]
        output: ["text"]
      tools:
        supports_search: false
model_aliases:
  main: DEFAULT:test-model
  vision: DEFAULT:vision-model
"#,
    )
    .expect("write config");
    fs::write(
        &env_file_path,
        "LLM_UNIVERSAL_PROXY_KEY=env-file-proxy-key\nLLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret-plan\n",
    )
    .expect("write env file");

    let _guards = [
        EnvGuard::set("HOME", temp.path().as_os_str()),
        EnvGuard::set("LLMUP_CODEX_BIN", "/opt/native/codex"),
        EnvGuard::set("LLMUP_PROVIDER_DEFAULT_API_KEY", "provider-secret-plan"),
        EnvGuard::set("UNRELATED_SECRET_COPY", "provider-secret-plan"),
        EnvGuard::set("OPENAI_API_KEY", "parent-openai-key"),
    ];
    std::env::remove_var("LLMUP_INTERNAL_LAUNCH_PLAN");

    let args = vec![
        OsString::from("--llmup-internal-launch-plan-json"),
        OsString::from("--llmup-internal-proxy-base"),
        OsString::from("http://matrix-proxy.local:19090"),
        OsString::from("--llmup-internal-proxy-key"),
        OsString::from("matrix-proxy-key"),
        OsString::from("--llmup-internal-artifact-dir"),
        artifact_dir.clone().into_os_string(),
        OsString::from("--llmup-config"),
        config_path.clone().into_os_string(),
        OsString::from("--llmup-env-file"),
        env_file_path.clone().into_os_string(),
        OsString::from("--"),
        OsString::from("resume"),
        OsString::from("--last"),
    ];

    let mut stdout = Vec::new();
    let err = run_cli(AgentKind::Codex, args.clone(), &mut stdout)
        .expect_err("internal launch plan should require env gate");
    assert!(
        err.contains("LLMUP_INTERNAL_LAUNCH_PLAN"),
        "unexpected error: {err}"
    );

    let _gate = EnvGuard::set("LLMUP_INTERNAL_LAUNCH_PLAN", "1");
    let mut stdout = Vec::new();
    let code =
        run_cli(AgentKind::Codex, args, &mut stdout).expect("launch plan should be generated");
    assert_eq!(code, 0);

    let text = String::from_utf8(stdout).expect("launch plan should be utf8");
    assert!(!text.contains("provider-secret-plan"));
    assert!(!text.contains("env-file-proxy-key"));
    assert!(!text.contains("parent-openai-key"));

    let plan: serde_json::Value = serde_json::from_str(&text).expect("plan should be JSON");
    assert_eq!(plan["agent"], "codex");
    assert_eq!(plan["program"], "/opt/native/codex");
    assert_eq!(plan["projection"]["enabled"], true);
    assert_eq!(plan["projection"]["profile"]["alias"], "main");
    assert_eq!(
        plan["env"]["OPENAI_API_KEY"],
        serde_json::json!("matrix-proxy-key")
    );
    assert_eq!(
        plan["env"]["OPENAI_BASE_URL"],
        serde_json::json!("http://matrix-proxy.local:19090/openai/v1")
    );
    assert_eq!(
        plan["env"]["CODEX_HOME"],
        serde_json::json!(temp.path().join(".llmup-codex").display().to_string())
    );
    assert!(plan["env"].get("LLMUP_PROVIDER_DEFAULT_API_KEY").is_none());
    assert!(plan["env"].get("UNRELATED_SECRET_COPY").is_none());

    let argv = plan["argv"].as_array().expect("argv should be an array");
    assert!(argv
        .windows(2)
        .any(|pair| pair[0] == "-m" && pair[1] == "main"));
    assert!(argv.iter().any(|arg| {
        arg.as_str()
            .is_some_and(|value| value == "model_provider=\"proxy\"")
    }));
    assert!(argv.iter().any(|arg| {
        arg.as_str().is_some_and(|value| {
            value == "model_providers.proxy.base_url=\"http://matrix-proxy.local:19090/openai/v1\""
        })
    }));
    assert!(argv.windows(2).any(|pair| {
        pair[0] == "-c"
            && pair[1] == "openai_base_url=\"http://matrix-proxy.local:19090/openai/v1\""
    }));
    assert!(argv.iter().any(|arg| {
        arg.as_str()
            .is_some_and(|value| value.starts_with("model_catalog_json=\""))
    }));
    assert!(argv.ends_with(&[serde_json::json!("resume"), serde_json::json!("--last")]));

    let catalog_path = plan["artifacts"]["codex_model_catalog"]
        .as_str()
        .expect("codex artifact path should be present");
    assert!(catalog_path.starts_with(&artifact_dir.display().to_string()));
    assert!(Path::new(catalog_path).exists());
    let catalog: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(catalog_path).expect("read catalog"))
            .expect("catalog should parse");
    assert_eq!(
        catalog["models"]
            .as_array()
            .expect("models should be array")
            .iter()
            .map(|entry| entry["slug"].as_str().expect("slug should be string"))
            .collect::<Vec<_>>(),
        vec!["main", "vision"]
    );
}

#[test]
fn internal_launch_plan_supports_claude_projection_without_codex_artifacts() {
    let _lock = ENV_LOCK.lock().expect("env lock");
    let temp = TempDir::new("launch-plan-claude");
    let config_path = temp.path().join("config.yaml");
    let env_file_path = temp.path().join("secrets.env");
    let artifact_dir = temp.path().join("artifacts");
    fs::write(
        &config_path,
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
    limits:
      context_window: 200000
      max_output_tokens: 128000
model_aliases:
  main: DEFAULT:test-model
"#,
    )
    .expect("write config");
    fs::write(
        &env_file_path,
        "LLM_UNIVERSAL_PROXY_KEY=env-file-proxy-key\nLLMUP_PROVIDER_DEFAULT_API_KEY=provider-secret-plan\n",
    )
    .expect("write env file");

    let _guards = [
        EnvGuard::set("HOME", temp.path().as_os_str()),
        EnvGuard::set("LLMUP_INTERNAL_LAUNCH_PLAN", "1"),
        EnvGuard::set("LLMUP_CLAUDE_BIN", "/opt/native/claude"),
        EnvGuard::set("ANTHROPIC_API_KEY", "parent-anthropic-key"),
    ];

    let mut stdout = Vec::new();
    let code = run_cli(
        AgentKind::Claude,
        vec![
            OsString::from("--llmup-internal-launch-plan-json"),
            OsString::from("--llmup-internal-proxy-base"),
            OsString::from("http://matrix-proxy.local:19091/"),
            OsString::from("--llmup-internal-proxy-key"),
            OsString::from("matrix-proxy-key"),
            OsString::from("--llmup-internal-artifact-dir"),
            artifact_dir.clone().into_os_string(),
            OsString::from("--llmup-config"),
            config_path.into_os_string(),
            OsString::from("--llmup-env-file"),
            env_file_path.into_os_string(),
            OsString::from("--"),
            OsString::from("--resume"),
            OsString::from("session"),
        ],
        &mut stdout,
    )
    .expect("launch plan should be generated");
    assert_eq!(code, 0);

    let text = String::from_utf8(stdout).expect("launch plan should be utf8");
    assert!(!text.contains("provider-secret-plan"));
    assert!(!text.contains("env-file-proxy-key"));
    assert!(!text.contains("parent-anthropic-key"));

    let plan: serde_json::Value = serde_json::from_str(&text).expect("plan should be JSON");
    assert_eq!(plan["agent"], "claude");
    assert_eq!(plan["program"], "/opt/native/claude");
    assert_eq!(plan["argv"], serde_json::json!(["--resume", "session"]));
    assert_eq!(plan["projection"]["enabled"], true);
    assert_eq!(plan["projection"]["profile"]["alias"], "main");
    assert_eq!(
        plan["env"]["ANTHROPIC_API_KEY"],
        serde_json::json!("matrix-proxy-key")
    );
    assert_eq!(
        plan["env"]["ANTHROPIC_BASE_URL"],
        serde_json::json!("http://matrix-proxy.local:19091/anthropic")
    );
    assert_eq!(
        plan["env"]["CLAUDE_CONFIG_DIR"],
        serde_json::json!(temp.path().join(".llmup-claude").display().to_string())
    );
    assert_eq!(plan["env"]["ANTHROPIC_MODEL"], serde_json::json!("main"));
    assert_eq!(
        plan["env"]["ANTHROPIC_CUSTOM_MODEL_OPTION"],
        serde_json::json!("main")
    );
    assert_eq!(
        plan["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
        serde_json::json!("main")
    );
    assert!(plan["artifacts"]["codex_model_catalog"].is_null());
    assert!(!artifact_dir.join("codex").exists());
}
