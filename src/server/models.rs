use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
    response::IntoResponse,
    Extension, Json,
};
use serde_json::Value;

use crate::config::Config;
use crate::formats::UpstreamFormat;

use super::data_auth::{self, RequestAuthContext};
use super::errors::error_response;
use super::secret_redaction::{redactor_for_request, SecretRedactor};
use super::state::{AppState, DEFAULT_NAMESPACE};

const PUBLIC_MODEL_NAMESPACE: &str = "llmup";

pub(super) async fn handle_openai_models(
    State(_state): State<Arc<AppState>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(
            UpstreamFormat::OpenAiChatCompletions,
        );
    };
    handle_openai_models_inner(&auth_context, DEFAULT_NAMESPACE, &headers).await
}

pub(super) async fn handle_openai_models_namespaced(
    State(_state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(
            UpstreamFormat::OpenAiChatCompletions,
        );
    };
    handle_openai_models_inner(&auth_context, &namespace, &headers).await
}

pub(super) async fn handle_openai_model(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(
            UpstreamFormat::OpenAiChatCompletions,
        );
    };
    handle_openai_model_inner(&auth_context, DEFAULT_NAMESPACE, &id).await
}

pub(super) async fn handle_openai_model_namespaced(
    State(_state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(
            UpstreamFormat::OpenAiChatCompletions,
        );
    };
    handle_openai_model_inner(&auth_context, &namespace, &id).await
}

pub(super) async fn handle_anthropic_models(
    State(_state): State<Arc<AppState>>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    handle_anthropic_models_inner(&auth_context, DEFAULT_NAMESPACE, &headers).await
}

pub(super) async fn handle_anthropic_models_namespaced(
    State(_state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    handle_anthropic_models_inner(&auth_context, &namespace, &headers).await
}

pub(super) async fn handle_anthropic_model(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    handle_anthropic_model_inner(&auth_context, DEFAULT_NAMESPACE, &id).await
}

pub(super) async fn handle_anthropic_model_namespaced(
    State(_state): State<Arc<AppState>>,
    Path((namespace, id)): Path<(String, String)>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> impl IntoResponse {
    let Some(auth_context) = data_auth::request_auth_context_from_extension(auth_context) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    handle_anthropic_model_inner(&auth_context, &namespace, &id).await
}

async fn handle_openai_models_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
    headers: &HeaderMap,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    match namespace_config(auth_context, namespace) {
        Some(config) => {
            let body = if is_codex_user_agent(headers) {
                codex_models_catalog(&config)
            } else {
                openai_model_list(&config)
            };
            redacted_json_response(StatusCode::OK, body, &request_redactor)
        }
        None => redacted_error_response(
            UpstreamFormat::OpenAiChatCompletions,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        ),
    }
}

async fn handle_openai_model_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
    id: &str,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    let Some(config) = namespace_config(auth_context, namespace) else {
        return redacted_error_response(
            UpstreamFormat::OpenAiChatCompletions,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        );
    };
    match openai_model_object(&config, id) {
        Some(model) => redacted_json_response(StatusCode::OK, model, &request_redactor),
        None => redacted_error_response(
            UpstreamFormat::OpenAiChatCompletions,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
            &request_redactor,
        ),
    }
}

async fn handle_anthropic_models_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
    headers: &HeaderMap,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    match namespace_config(auth_context, namespace) {
        Some(config) => {
            let body = if is_codex_user_agent(headers) {
                codex_models_catalog(&config)
            } else {
                anthropic_model_list(&config)
            };
            redacted_json_response(StatusCode::OK, body, &request_redactor)
        }
        None => redacted_error_response(
            UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        ),
    }
}

async fn handle_anthropic_model_inner(
    auth_context: &RequestAuthContext,
    namespace: &str,
    id: &str,
) -> Response<Body> {
    let request_redactor = redactor_for_model_request(auth_context);
    let Some(config) = namespace_config(auth_context, namespace) else {
        return redacted_error_response(
            UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            "namespace not found",
            &request_redactor,
        );
    };
    match anthropic_model_object(&config, id) {
        Some(model) => redacted_json_response(StatusCode::OK, model, &request_redactor),
        None => redacted_error_response(
            UpstreamFormat::Anthropic,
            StatusCode::NOT_FOUND,
            &format!("model `{id}` not found"),
            &request_redactor,
        ),
    }
}

fn redactor_for_model_request(auth_context: &RequestAuthContext) -> SecretRedactor {
    redactor_for_request(auth_context, &HeaderMap::new())
}

fn redacted_json_response(
    status: StatusCode,
    body: Value,
    redactor: &SecretRedactor,
) -> Response<Body> {
    (status, Json(redactor.redact_value(&body))).into_response()
}

fn redacted_error_response(
    format: UpstreamFormat,
    status: StatusCode,
    message: &str,
    redactor: &SecretRedactor,
) -> Response<Body> {
    error_response(format, status, &redactor.redact_text(message))
}

fn namespace_config(auth_context: &RequestAuthContext, namespace: &str) -> Option<Config> {
    auth_context
        .runtime()
        .namespaces
        .get(namespace)
        .map(|item| item.config.clone())
}

fn configured_aliases(config: &Config) -> Vec<(&String, &crate::config::ModelAlias)> {
    config.model_aliases.iter().collect()
}

fn synthetic_model_alias(config: &Config, id: &str) -> Option<(String, crate::config::ModelAlias)> {
    if let Some(target) = config.model_aliases.get(id) {
        return Some((id.to_string(), target.clone()));
    }

    let resolved = config.resolve_model(id).ok()?;
    Some((
        id.to_string(),
        crate::config::ModelAlias {
            upstream_name: resolved.upstream_name,
            upstream_model: resolved.upstream_model,
            limits: None,
            surface: None,
        },
    ))
}

fn effective_limits(
    config: &Config,
    target: &crate::config::ModelAlias,
) -> Option<crate::config::ModelLimits> {
    config.effective_model_limits(target)
}

fn effective_surface(
    config: &Config,
    target: &crate::config::ModelAlias,
) -> crate::config::ModelSurface {
    config.effective_model_surface(target)
}

fn public_model_metadata(
    config: &Config,
    target: &crate::config::ModelAlias,
) -> (
    Option<crate::config::ModelLimits>,
    crate::config::ModelSurface,
    Value,
) {
    let limits = effective_limits(config, target);
    let surface = effective_surface(config, target);
    let metadata = serde_json::json!({
        "upstream_name": target.upstream_name,
        "upstream_model": target.upstream_model,
        "limits": limits,
        "surface": surface,
    });
    (limits, surface, metadata)
}

/// Codex CLI (and clients whose User-Agent contains "codex") fetch `/models`
/// and expect `ModelsResponse { models: [ModelInfo] }` rather than the
/// standard OpenAI `{object:"list", data:[...]}` shape. See
/// `docs/engineering/2026-08-01-codex-compat-feedback-plan.md` §3 P2.
fn is_codex_user_agent(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|ua| ua.to_ascii_lowercase().contains("codex"))
}

/// Build the Codex `ModelsResponse { models: [...] }` body for `config` by
/// reusing the shared catalog builder (`build_codex_model_catalog_for_config`),
/// which produces `ModelInfo`-shaped entries from the runtime config's model
/// aliases + their merged `surface_defaults`/`limits` metadata.
fn codex_models_catalog(config: &Config) -> Value {
    crate::user_tools::agent_model_profile::build_codex_model_catalog_for_config(config)
}

fn openai_model_list(config: &Config) -> Value {
    serde_json::json!({
        "object": "list",
        "data": configured_aliases(config)
            .into_iter()
            .map(|(alias, target)| openai_model_value(config, alias, target))
            .collect::<Vec<_>>()
    })
}

fn openai_model_object(config: &Config, id: &str) -> Option<Value> {
    let (model_id, target) = synthetic_model_alias(config, id)?;
    Some(openai_model_value(config, &model_id, &target))
}

fn openai_model_value(config: &Config, id: &str, target: &crate::config::ModelAlias) -> Value {
    let (_limits, _surface, metadata) = public_model_metadata(config, target);
    serde_json::json!({
        "id": id,
        "object": "model",
        "created": 0,
        "owned_by": PUBLIC_MODEL_NAMESPACE,
        "llmup": metadata
    })
}

fn anthropic_model_list(config: &Config) -> Value {
    let data = configured_aliases(config)
        .into_iter()
        .map(|(alias, target)| anthropic_model_value(config, alias, target))
        .collect::<Vec<_>>();
    let first_id = data
        .first()
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let last_id = data
        .last()
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    serde_json::json!({
        "data": data,
        "has_more": false,
        "first_id": first_id,
        "last_id": last_id
    })
}

fn anthropic_model_object(config: &Config, id: &str) -> Option<Value> {
    let (model_id, target) = synthetic_model_alias(config, id)?;
    Some(anthropic_model_value(config, &model_id, &target))
}

fn anthropic_model_value(config: &Config, id: &str, target: &crate::config::ModelAlias) -> Value {
    let (limits, _surface, metadata) = public_model_metadata(config, target);
    let mut model = serde_json::json!({
        "id": id,
        "type": "model",
        "display_name": id,
        "created_at": "1970-01-01T00:00:00Z",
        "llmup": metadata
    });

    if let Some(limits) = limits {
        if let Some(context_window) = limits.context_window {
            model["max_input_tokens"] = serde_json::json!(context_window);
        }
        if let Some(max_output_tokens) = limits.max_output_tokens {
            model["max_tokens"] = serde_json::json!(max_output_tokens);
        }
    }

    model
}
