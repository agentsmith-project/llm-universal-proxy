use super::*;

fn models_snapshot_config(alias: &str) -> crate::config::Config {
    let upstream = redaction_upstream_config(
        "primary",
        "http://127.0.0.1:9/v1",
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
        None,
    );
    crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![upstream],
        model_aliases: BTreeMap::from([(
            alias.to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: format!("{alias}-upstream"),
                limits: None,
                surface: None,
            },
        )]),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    }
}

fn models_catalog_config(
    alias: &str,
    upstream_name: &str,
    upstream_model: &str,
    format: crate::formats::UpstreamFormat,
    provider_key: Option<&str>,
) -> crate::config::Config {
    let upstream = redaction_upstream_config(
        upstream_name,
        "http://127.0.0.1:9/v1",
        format,
        None,
        provider_key.map(|secret| crate::config::SecretSourceConfig {
            inline: Some(secret.to_string()),
            env: None,
        }),
    );
    crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![upstream],
        model_aliases: BTreeMap::from([(
            alias.to_string(),
            crate::config::ModelAlias {
                upstream_name: upstream_name.to_string(),
                upstream_model: upstream_model.to_string(),
                limits: None,
                surface: None,
            },
        )]),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    }
}

fn models_catalog_metadata_config(
    alias: &str,
    format: crate::formats::UpstreamFormat,
) -> crate::config::Config {
    let mut config = models_catalog_config(alias, "primary", "upstream-model", format, None);
    config.upstreams[0].limits = Some(crate::config::ModelLimits {
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
    });
    config.upstreams[0].surface_defaults = Some(crate::config::ModelSurfacePatch {
        modalities: Some(crate::config::ModelModalities {
            input: Some(vec![crate::config::ModelModality::Text]),
            output: Some(vec![crate::config::ModelModality::Text]),
        }),
        tools: Some(crate::config::ModelToolSurface {
            supports_search: Some(true),
            supports_view_image: Some(true),
            apply_patch_transport: Some(crate::config::ApplyPatchTransport::Function),
            supports_parallel_calls: Some(true),
        }),
    });

    let alias_config = config
        .model_aliases
        .get_mut(alias)
        .expect("metadata model alias");
    alias_config.limits = Some(crate::config::ModelLimits {
        context_window: None,
        max_output_tokens: Some(64_000),
    });
    alias_config.surface = Some(crate::config::ModelSurfacePatch {
        modalities: Some(crate::config::ModelModalities {
            input: Some(vec![
                crate::config::ModelModality::Text,
                crate::config::ModelModality::Image,
            ]),
            output: None,
        }),
        tools: Some(crate::config::ModelToolSurface {
            supports_search: Some(false),
            supports_view_image: None,
            apply_patch_transport: Some(crate::config::ApplyPatchTransport::Freeform),
            supports_parallel_calls: Some(false),
        }),
    });
    config
}

fn models_partial_limits_config(format: crate::formats::UpstreamFormat) -> crate::config::Config {
    let upstream =
        redaction_upstream_config("primary", "http://127.0.0.1:9/v1", format, None, None);
    crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![upstream],
        model_aliases: BTreeMap::from([
            (
                "context-only".to_string(),
                crate::config::ModelAlias {
                    upstream_name: "primary".to_string(),
                    upstream_model: "context-upstream".to_string(),
                    limits: Some(crate::config::ModelLimits {
                        context_window: Some(101_000),
                        max_output_tokens: None,
                    }),
                    surface: None,
                },
            ),
            (
                "output-only".to_string(),
                crate::config::ModelAlias {
                    upstream_name: "primary".to_string(),
                    upstream_model: "output-upstream".to_string(),
                    limits: Some(crate::config::ModelLimits {
                        context_window: None,
                        max_output_tokens: Some(8_192),
                    }),
                    surface: None,
                },
            ),
        ]),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    }
}

fn models_not_found_config(
    format: crate::formats::UpstreamFormat,
    provider_key: Option<&str>,
) -> crate::config::Config {
    crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![
            redaction_upstream_config(
                "left",
                "http://127.0.0.1:9/v1",
                format,
                None,
                provider_key.map(|secret| crate::config::SecretSourceConfig {
                    inline: Some(secret.to_string()),
                    env: None,
                }),
            ),
            redaction_upstream_config(
                "right",
                "http://127.0.0.1:9/v1",
                format,
                None,
                provider_key.map(|secret| crate::config::SecretSourceConfig {
                    inline: Some(secret.to_string()),
                    env: None,
                }),
            ),
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    }
}

async fn state_for_models_config(
    config: crate::config::Config,
    access: data_auth::DataAccess,
) -> Arc<AppState> {
    let runtime = crate::server::state::build_runtime_state(config.clone(), &access)
        .await
        .expect("build models snapshot runtime");

    Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        admin_update_lock: Arc::new(Mutex::new(())),
        metrics: crate::telemetry::RuntimeMetrics::new(&config),
        admin_access: AdminAccess::LoopbackOnly,
        data_auth_policy: data_auth::RuntimeConfigValidationPolicy::new(
            "127.0.0.1:0".parse().expect("loopback socket addr"),
            access,
        ),
        conversation_state_bridge: Arc::new(
            crate::server::conversation_state_bridge::ConversationStateBridgeStore::new(),
        ),
    })
}

async fn state_for_models_snapshot(alias: &str) -> Arc<AppState> {
    let config = models_snapshot_config(alias);
    let access = data_auth::DataAccess::ClientProviderKey;
    state_for_models_config(config, access).await
}

async fn models_response_text(response: Response<Body>) -> String {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map(|bytes| String::from_utf8(bytes.to_vec()).expect("models response utf8"))
        .expect("models response body")
}

async fn models_response_json(response: Response<Body>) -> Value {
    let body_text = models_response_text(response).await;
    serde_json::from_str(&body_text).expect("models response JSON")
}

fn model_by_id<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["data"]
        .as_array()
        .expect("models data array")
        .iter()
        .find(|model| model.get("id").and_then(Value::as_str) == Some(id))
        .expect("model by id")
}

fn assert_anthropic_top_level_limits_match_llmup_limits(model: &Value) {
    assert_eq!(
        model["max_input_tokens"], model["llmup"]["limits"]["context_window"],
        "model = {model:?}"
    );
    assert_eq!(
        model["max_tokens"], model["llmup"]["limits"]["max_output_tokens"],
        "model = {model:?}"
    );
    assert_eq!(model["max_input_tokens"], 200_000, "model = {model:?}");
    assert_eq!(model["max_tokens"], 64_000, "model = {model:?}");
    assert_eq!(
        model["llmup"]["surface"]["limits"]["context_window"], 200_000,
        "model = {model:?}"
    );
    assert_eq!(
        model["llmup"]["surface"]["limits"]["max_output_tokens"], 64_000,
        "model = {model:?}"
    );
}

fn assert_models_response_redacted(body_text: &str, secrets: &[&str], context: &str) {
    for secret in secrets {
        assert!(
            !body_text.contains(secret),
            "{context} leaked {secret}: {body_text}"
        );
    }
    assert!(
        body_text.contains("[REDACTED]"),
        "{context} should show redacted placeholder: {body_text}"
    );
}

async fn proxy_mode_models_auth_context(state: &Arc<AppState>) -> data_auth::RequestAuthContext {
    let runtime = state.runtime.read().await.clone();
    request_auth_context_for_runtime(
        runtime,
        data_auth::DataAccess::ProxyKey {
            key: PROXY_INLINE_REDACTION_SECRET.to_string(),
        },
        data_auth::RequestAuthorization::ProxyKey,
    )
}

async fn client_mode_models_auth_context(state: &Arc<AppState>) -> data_auth::RequestAuthContext {
    let runtime = state.runtime.read().await.clone();
    request_auth_context_for_runtime(
        runtime,
        data_auth::DataAccess::ClientProviderKey,
        data_auth::RequestAuthorization::ClientProviderKey {
            provider_key: CLIENT_PROVIDER_REDACTION_SECRET.to_string(),
        },
    )
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_models_list_and_object_promote_effective_limits_from_llmup_limits() {
    let state = state_for_models_config(
        models_catalog_metadata_config("haiku", crate::formats::UpstreamFormat::Anthropic),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let auth_context = client_mode_models_auth_context(&state).await;

    let response = crate::server::models::handle_anthropic_models(
        State(state.clone()),
        Some(axum::Extension(auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let body = models_response_json(response).await;
    let model = model_by_id(&body, "haiku");
    assert_anthropic_top_level_limits_match_llmup_limits(model);
    assert!(model.get("capabilities").is_none(), "model = {model:?}");
    assert_eq!(
        model["llmup"]["surface"]["tools"]["apply_patch_transport"],
        "freeform"
    );

    let auth_context = client_mode_models_auth_context(&state).await;
    let response = crate::server::models::handle_anthropic_model(
        State(state),
        Path("haiku".to_string()),
        Some(axum::Extension(auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let model = models_response_json(response).await;
    assert_anthropic_top_level_limits_match_llmup_limits(&model);
    assert!(model.get("capabilities").is_none(), "model = {model:?}");
    assert_eq!(
        model["llmup"]["surface"]["tools"]["apply_patch_transport"],
        "freeform"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_models_omit_top_level_limit_fields_without_effective_values() {
    let state = state_for_models_config(
        models_partial_limits_config(crate::formats::UpstreamFormat::Anthropic),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let auth_context = client_mode_models_auth_context(&state).await;

    let response = crate::server::models::handle_anthropic_models(
        State(state),
        Some(axum::Extension(auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let body = models_response_json(response).await;
    let context_only = model_by_id(&body, "context-only");
    assert_eq!(context_only["max_input_tokens"], 101_000);
    assert!(
        context_only.get("max_tokens").is_none(),
        "model = {context_only:?}"
    );
    assert_eq!(context_only["llmup"]["limits"]["context_window"], 101_000);
    assert!(
        context_only["llmup"]["limits"]
            .get("max_output_tokens")
            .is_none(),
        "model = {context_only:?}"
    );

    let output_only = model_by_id(&body, "output-only");
    assert!(
        output_only.get("max_input_tokens").is_none(),
        "model = {output_only:?}"
    );
    assert_eq!(output_only["max_tokens"], 8_192);
    assert!(
        output_only["llmup"]["limits"]
            .get("context_window")
            .is_none(),
        "model = {output_only:?}"
    );
    assert_eq!(output_only["llmup"]["limits"]["max_output_tokens"], 8_192);
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_models_do_not_synthesize_capabilities_from_surface_metadata() {
    let state = state_for_models_config(
        models_catalog_metadata_config("haiku", crate::formats::UpstreamFormat::Anthropic),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let auth_context = client_mode_models_auth_context(&state).await;

    let response = crate::server::models::handle_anthropic_model(
        State(state),
        Path("haiku".to_string()),
        Some(axum::Extension(auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let model = models_response_json(response).await;
    assert_eq!(
        model["llmup"]["surface"]["tools"]["apply_patch_transport"],
        "freeform"
    );
    assert_eq!(model["llmup"]["surface"]["tools"]["supports_search"], false);
    assert_eq!(
        model["llmup"]["surface"]["tools"]["supports_view_image"],
        true
    );
    assert_eq!(
        model["llmup"]["surface"]["tools"]["supports_parallel_calls"],
        false
    );
    assert_eq!(model["llmup"]["surface"]["modalities"]["input"][1], "image");
    assert!(
        !model.to_string().contains("capabilities"),
        "model = {model:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn openai_models_do_not_expose_anthropic_top_level_limit_fields() {
    let state = state_for_models_config(
        models_catalog_metadata_config(
            "sonnet",
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
        ),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let auth_context = client_mode_models_auth_context(&state).await;

    let response = crate::server::models::handle_openai_models(
        State(state.clone()),
        Some(axum::Extension(auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let body = models_response_json(response).await;
    let model = model_by_id(&body, "sonnet");
    assert!(model.get("max_input_tokens").is_none(), "model = {model:?}");
    assert!(model.get("max_tokens").is_none(), "model = {model:?}");
    assert_eq!(model["llmup"]["limits"]["context_window"], 200_000);
    assert_eq!(model["llmup"]["limits"]["max_output_tokens"], 64_000);

    let auth_context = client_mode_models_auth_context(&state).await;
    let response = crate::server::models::handle_openai_model(
        State(state),
        Path("sonnet".to_string()),
        Some(axum::Extension(auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let model = models_response_json(response).await;
    assert!(model.get("max_input_tokens").is_none(), "model = {model:?}");
    assert!(model.get("max_tokens").is_none(), "model = {model:?}");
    assert_eq!(model["llmup"]["limits"]["context_window"], 200_000);
    assert_eq!(model["llmup"]["limits"]["max_output_tokens"], 64_000);
}

#[tokio::test(flavor = "current_thread")]
async fn protected_openai_models_use_request_runtime_snapshot_after_auth_race() {
    let state = state_for_models_snapshot("old-model").await;
    let old_runtime = state.runtime.read().await.clone();
    let auth_context = request_auth_context_for_runtime(
        old_runtime,
        data_auth::DataAccess::ClientProviderKey,
        data_auth::RequestAuthorization::ClientProviderKey {
            provider_key: "client-model-key".to_string(),
        },
    );
    replace_runtime_and_data_auth(
        &state,
        models_snapshot_config("new-model"),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;

    let response = crate::server::models::handle_openai_models(
        State(state),
        Some(axum::Extension(auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map(|bytes| String::from_utf8(bytes.to_vec()).expect("models response utf8"))
        .expect("models response body");
    assert!(body_text.contains("old-model"), "{body_text}");
    assert!(!body_text.contains("new-model"), "{body_text}");
}

#[tokio::test(flavor = "current_thread")]
async fn openai_models_list_redacts_alias_and_metadata_known_secrets() {
    let provider_alias =
        format!("alias-{PROVIDER_INLINE_REDACTION_SECRET}-{PROXY_INLINE_REDACTION_SECRET}");
    let provider_upstream =
        format!("upstream-{PROVIDER_INLINE_REDACTION_SECRET}-{PROXY_INLINE_REDACTION_SECRET}");
    let provider_model =
        format!("model-{PROVIDER_INLINE_REDACTION_SECRET}-{PROXY_INLINE_REDACTION_SECRET}");
    let proxy_access = data_auth::DataAccess::ProxyKey {
        key: PROXY_INLINE_REDACTION_SECRET.to_string(),
    };
    let proxy_state = state_for_models_config(
        models_catalog_config(
            &provider_alias,
            &provider_upstream,
            &provider_model,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            Some(PROVIDER_INLINE_REDACTION_SECRET),
        ),
        proxy_access.clone(),
    )
    .await;
    let proxy_runtime = proxy_state.runtime.read().await.clone();
    let proxy_auth_context = request_auth_context_for_runtime(
        proxy_runtime,
        proxy_access,
        data_auth::RequestAuthorization::ProxyKey,
    );

    let response = crate::server::models::handle_openai_models(
        State(proxy_state),
        Some(axum::Extension(proxy_auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = models_response_text(response).await;
    assert_models_response_redacted(
        &body_text,
        &[
            PROVIDER_INLINE_REDACTION_SECRET,
            PROXY_INLINE_REDACTION_SECRET,
        ],
        "OpenAI models list proxy mode",
    );
    let body: Value = serde_json::from_str(&body_text).expect("models list JSON");
    let model = &body["data"][0];
    assert_eq!(model["object"], "model");
    assert!(
        model["id"].as_str().unwrap_or("").contains("[REDACTED]"),
        "{body_text}"
    );
    assert!(
        model["llmup"]["upstream_name"]
            .as_str()
            .unwrap_or("")
            .contains("[REDACTED]"),
        "{body_text}"
    );
    assert!(
        model["llmup"]["upstream_model"]
            .as_str()
            .unwrap_or("")
            .contains("[REDACTED]"),
        "{body_text}"
    );

    let client_alias = format!("alias-{CLIENT_PROVIDER_REDACTION_SECRET}");
    let client_upstream = format!("upstream-{CLIENT_PROVIDER_REDACTION_SECRET}");
    let client_model = format!("model-{CLIENT_PROVIDER_REDACTION_SECRET}");
    let client_state = state_for_models_config(
        models_catalog_config(
            &client_alias,
            &client_upstream,
            &client_model,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        ),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let client_auth_context = client_mode_models_auth_context(&client_state).await;

    let response = crate::server::models::handle_openai_models(
        State(client_state),
        Some(axum::Extension(client_auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = models_response_text(response).await;
    assert_models_response_redacted(
        &body_text,
        &[CLIENT_PROVIDER_REDACTION_SECRET],
        "OpenAI models list client mode",
    );
}

#[tokio::test(flavor = "current_thread")]
async fn openai_model_not_found_redacts_client_and_server_keys() {
    let provider_state = state_for_models_config(
        models_not_found_config(
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            Some(PROVIDER_INLINE_REDACTION_SECRET),
        ),
        data_auth::DataAccess::ProxyKey {
            key: PROXY_INLINE_REDACTION_SECRET.to_string(),
        },
    )
    .await;
    let provider_auth_context = proxy_mode_models_auth_context(&provider_state).await;
    let missing_id =
        format!("missing-{PROVIDER_INLINE_REDACTION_SECRET}-{PROXY_INLINE_REDACTION_SECRET}");

    let response = crate::server::models::handle_openai_model(
        State(provider_state),
        Path(missing_id),
        Some(axum::Extension(provider_auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body_text = models_response_text(response).await;
    assert_models_response_redacted(
        &body_text,
        &[
            PROVIDER_INLINE_REDACTION_SECRET,
            PROXY_INLINE_REDACTION_SECRET,
        ],
        "OpenAI model not found server keys",
    );

    let client_state = state_for_models_config(
        models_not_found_config(crate::formats::UpstreamFormat::OpenAiChatCompletions, None),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let client_auth_context = client_mode_models_auth_context(&client_state).await;
    let missing_id = format!("missing-{CLIENT_PROVIDER_REDACTION_SECRET}");

    let response = crate::server::models::handle_openai_model(
        State(client_state),
        Path(missing_id),
        Some(axum::Extension(client_auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body_text = models_response_text(response).await;
    assert_models_response_redacted(
        &body_text,
        &[CLIENT_PROVIDER_REDACTION_SECRET],
        "OpenAI model not found client key",
    );
}
