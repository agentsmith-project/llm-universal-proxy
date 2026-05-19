use llm_universal_proxy::config::ModelModality;
use llm_universal_proxy::user_tools::agent_model_profile::{
    build_codex_model_catalog, AgentModelProfile,
};
use llm_universal_proxy::Config;

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
