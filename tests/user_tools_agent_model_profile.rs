use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use llm_universal_proxy::config::ModelModality;
use llm_universal_proxy::formats::UpstreamFormat;
use llm_universal_proxy::user_tools::agent_model_profile::{
    build_codex_model_catalog, build_codex_model_catalog_for_profiles,
    write_codex_model_catalog_for_profiles, AgentModelCatalog, AgentModelProfile,
};
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
            "llmup-agent-model-profile-{label}-{}-{nanos}",
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

fn config_from_yaml(raw: &str) -> Config {
    let config = Config::from_yaml_str(raw).expect("config should parse");
    config.validate().expect("config should validate");
    config
}

#[test]
fn agent_model_profile_resolves_effective_limits_surface_and_codex_auto_compact() {
    let config = config_from_yaml(
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
    surface_defaults:
      modalities:
        input: ["text"]
        output: ["text"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: true
model_aliases:
  minimax:
    target: DEFAULT:MiniMax
    limits:
      max_output_tokens: 128000
    surface:
      modalities:
        input: ["text", "image"]
      tools:
        supports_parallel_calls: false
"#,
    );

    let profile =
        AgentModelProfile::from_config(&config, "minimax").expect("profile should resolve alias");

    assert_eq!(profile.alias, "minimax");
    assert_eq!(
        profile.upstream_format,
        Some(UpstreamFormat::OpenAiResponses)
    );
    let limits = profile.limits.as_ref().expect("limits should merge");
    assert_eq!(limits.context_window, Some(200000));
    assert_eq!(limits.max_output_tokens, Some(128000));
    assert_eq!(profile.codex_auto_compact_token_limit, Some(61200));
    assert_eq!(
        profile
            .surface
            .modalities
            .as_ref()
            .and_then(|item| item.input.as_ref())
            .map(Vec::as_slice),
        Some([ModelModality::Text, ModelModality::Image,].as_slice())
    );
    let tools = profile.surface.tools.as_ref().expect("tools should merge");
    assert_eq!(tools.supports_search, Some(false));
    assert_eq!(tools.supports_view_image, Some(false));
    assert_eq!(tools.supports_parallel_calls, Some(false));
}

#[test]
fn agent_model_profile_errors_for_unknown_alias() {
    let unknown_config = config_from_yaml(
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  default: DEFAULT:test-model
"#,
    );
    let err = AgentModelProfile::from_config(&unknown_config, "missing")
        .expect_err("unknown alias should fail");
    assert!(err.contains("unknown model alias `missing`"));
}

#[test]
fn agent_model_catalog_resolves_selected_alias_without_limiting_all_profiles() {
    let config = config_from_yaml(
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
  main: DEFAULT:main-upstream
  haiku:
    target: DEFAULT:haiku-upstream
    limits:
      max_output_tokens: 32000
  vision:
    target: DEFAULT:vision-upstream
    surface:
      modalities:
        input: ["text", "image"]
      tools:
        supports_search: true
        supports_parallel_calls: true
"#,
    );

    let catalog =
        AgentModelCatalog::from_config(&config, "vision").expect("catalog should resolve");

    assert_eq!(catalog.selected.alias, "vision");
    assert_eq!(
        catalog
            .profiles
            .iter()
            .map(|profile| profile.alias.as_str())
            .collect::<Vec<_>>(),
        vec!["haiku", "main", "vision"]
    );

    let payload = build_codex_model_catalog_for_profiles(&catalog.profiles)
        .expect("multi-alias catalog should build");
    let models = payload["models"]
        .as_array()
        .expect("codex catalog should keep models array shape");
    assert_eq!(models.len(), 3);

    let slugs = models
        .iter()
        .map(|entry| entry["slug"].as_str().expect("slug should be string"))
        .collect::<Vec<_>>();
    assert_eq!(slugs, vec!["haiku", "main", "vision"]);

    let main = models
        .iter()
        .find(|entry| entry["slug"] == "main")
        .expect("main entry");
    assert_eq!(main["context_window"], 200000);
    assert_eq!(main["auto_compact_token_limit"], 115600);
    assert_eq!(main["input_modalities"], serde_json::json!(["text"]));
    assert_eq!(main["supports_search_tool"], false);
    assert_eq!(main["apply_patch_tool_type"], "freeform");
    assert_eq!(main["supports_parallel_tool_calls"], false);

    let haiku = models
        .iter()
        .find(|entry| entry["slug"] == "haiku")
        .expect("haiku entry");
    assert_eq!(haiku["context_window"], 200000);
    assert_eq!(haiku["auto_compact_token_limit"], 142800);

    let vision = models
        .iter()
        .find(|entry| entry["slug"] == "vision")
        .expect("vision entry");
    assert_eq!(
        vision["input_modalities"],
        serde_json::json!(["text", "image"])
    );
    assert_eq!(vision["supports_search_tool"], true);
    assert_eq!(vision["supports_parallel_tool_calls"], true);
}

#[test]
fn write_codex_model_catalog_for_profiles_writes_multi_alias_debug_models_shape() {
    let config = config_from_yaml(
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
model_aliases:
  main: DEFAULT:main-upstream
  sonnet: DEFAULT:sonnet-upstream
  opus: DEFAULT:opus-upstream
"#,
    );
    let catalog = AgentModelCatalog::from_config(&config, "main").expect("catalog should resolve");
    let temp = TempDir::new("write-codex-catalog");

    let path = write_codex_model_catalog_for_profiles(&catalog.profiles, temp.path())
        .expect("catalog should write");
    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).expect("read written catalog"))
            .expect("catalog should parse as JSON");

    assert!(
        payload.as_object().is_some_and(|object| object.len() == 1),
        "codex debug models compatible catalog should have only the top-level models key"
    );
    let models = payload["models"]
        .as_array()
        .expect("models should be array");
    assert_eq!(models.len(), 3);
    assert_eq!(
        models
            .iter()
            .map(|entry| entry["slug"].as_str().expect("slug should be string"))
            .collect::<Vec<_>>(),
        vec!["main", "opus", "sonnet"]
    );
}

#[test]
fn agent_model_profile_allows_invalid_codex_input_budget_until_catalog_build() {
    let invalid_limits = config_from_yaml(
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
  default: DEFAULT:test-model
"#,
    );

    let profile = AgentModelProfile::from_config(&invalid_limits, "default")
        .expect("generic profile parsing should not enforce Codex input budget");
    let limits = profile.limits.as_ref().expect("limits should resolve");
    assert_eq!(limits.context_window, Some(200000));
    assert_eq!(limits.max_output_tokens, Some(200000));
    assert_eq!(profile.codex_auto_compact_token_limit, None);

    let err = build_codex_model_catalog(&profile)
        .expect_err("Codex catalog should reject a non-positive input budget");
    assert!(err.contains("max_output_tokens"));
    assert!(err.contains("context_window"));
}

#[test]
fn codex_catalog_uses_full_shape_and_context_when_output_limit_is_missing() {
    let config = config_from_yaml(
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
    surface_defaults:
      modalities:
        input: ["text", "image"]
      tools:
        supports_search: true
        apply_patch_transport: freeform
        supports_parallel_calls: true
model_aliases:
  vision: DEFAULT:vision-model
"#,
    );
    let profile =
        AgentModelProfile::from_config(&config, "vision").expect("profile should resolve");
    assert_eq!(profile.codex_auto_compact_token_limit, Some(170000));

    let catalog = build_codex_model_catalog(&profile).expect("catalog should build");
    let entry = &catalog["models"][0];

    assert_eq!(entry["slug"], "vision");
    assert_eq!(entry["display_name"], "vision");
    assert_eq!(entry["description"], "llmup proxy model vision");
    assert_eq!(entry["supported_reasoning_levels"][3]["effort"], "xhigh");
    assert_eq!(entry["shell_type"], "shell_command");
    assert_eq!(entry["visibility"], "list");
    assert_eq!(entry["supported_in_api"], true);
    assert_eq!(entry["context_window"], 200000);
    assert_eq!(entry["auto_compact_token_limit"], 170000);
    assert_eq!(
        entry["input_modalities"],
        serde_json::json!(["text", "image"])
    );
    assert_eq!(entry["supports_search_tool"], true);
    assert_eq!(entry["apply_patch_tool_type"], "freeform");
    assert_eq!(entry["supports_parallel_tool_calls"], true);
    assert_eq!(entry["experimental_supported_tools"], serde_json::json!([]));
}

#[test]
fn codex_catalog_defaults_to_text_only_when_view_image_is_false_without_modalities() {
    let config = config_from_yaml(
        r#"
listen: 127.0.0.1:8080
upstreams:
  DEFAULT:
    api_root: https://api.example.com/v1
    format: openai-responses
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
    surface_defaults:
      tools:
        supports_view_image: false
model_aliases:
  text-only: DEFAULT:text-model
"#,
    );
    let profile =
        AgentModelProfile::from_config(&config, "text-only").expect("profile should resolve");
    assert!(profile.surface.modalities.is_none());
    assert_eq!(
        profile
            .surface
            .tools
            .as_ref()
            .and_then(|tools| tools.supports_view_image),
        Some(false)
    );

    let catalog = build_codex_model_catalog(&profile).expect("catalog should build");
    let entry = &catalog["models"][0];

    assert_eq!(entry["input_modalities"], serde_json::json!(["text"]));
}
