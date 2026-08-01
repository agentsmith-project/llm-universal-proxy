use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, Response, StatusCode},
    response::IntoResponse,
    Extension, Json,
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use tracing::{debug, error, warn};

use crate::config::ResolvedModel;
use crate::debug_trace::{ConversationStateBridgeDebugTrace, DebugTraceContext};
use crate::downstream::DownstreamCancellation;
use crate::formats::UpstreamFormat;
use crate::hooks::{
    capture_headers, json_response_headers, new_request_id, now_timestamp_ms, sse_response_headers,
    HeaderEntry, HookRequestContext,
};
use crate::prompt_cache_controls::{
    synthesize_openai_family_prompt_cache_key_from_source,
    OpenAiFamilyPromptCacheKeySynthesisContext,
};
use crate::provider_state_controls::{
    provider_state_control_enabled, responses_stateful_request_controls,
};
use crate::request_processing::{
    classify_request_processing, PromptCacheRequestControl, RequestProcessing,
    RequestProcessingInput, StateBridgeModifier,
};
use crate::streaming::{
    needs_stream_translation, GuardedSseStream, ResourceLimitedStream, TranslateSseStream,
};
use crate::translate::{
    assess_request_translation_with_surface, translate_request_with_policy,
    translate_response_with_context, RequestTranslationPolicy, ResponseTranslationContext,
    TranslationDecision,
};
use crate::upstream;

use super::body_limits::{read_limited_json_request, JsonRequestBody};
use super::conversation_state_bridge::{
    BridgeRouteConfigFingerprint, ConversationStateBridgeStore, StoredBridgeResponse,
    LOCAL_REPLAY_SCHEMA_VERSION, LOCAL_RESPONSE_ID_PREFIX,
};
use super::data_auth::{self, RequestAuthContext};
use super::errors::{
    append_portability_warning_headers, classify_post_translation_non_stream_status,
    client_closed_response, error_response, format_upstream_unavailable_message,
    normalized_non_stream_upstream_error, streaming_error_response,
};
use super::headers::{
    append_raw_upstream_response_headers, append_raw_upstream_stream_response_headers,
    append_upstream_protocol_response_headers, apply_upstream_headers, build_auth_headers,
};
use super::public_boundary::{
    reject_internal_request_scoped_tool_bridge_context,
    validate_provider_forwarding_request_boundary, REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD,
};
use super::responses_resources::resolve_native_responses_stateful_route_or_error;
use super::secret_redaction::{
    redactor_for_request, RedactingSseObservationTransform, RedactingSseStream, SecretRedactor,
};
use super::state::{AppState, RuntimeNamespaceState, DEFAULT_NAMESPACE};
use super::tracked_body::TrackedBodyStream;

const TOOL_BRIDGE_CONTEXT_VERSION: u64 = 3;
const TOOL_BRIDGE_CONTEXT_PURPOSE: &str = "openai_responses_custom_tool_bridge";

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrustedToolBridgeContextEntry {
    stable_name: String,
    source_kind: String,
    transport_kind: String,
    wrapper_field: String,
    expected_canonical_shape: String,
}

impl TrustedToolBridgeContextEntry {
    fn from_value(stable_name: &str, value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let declared_stable_name = object.get("stable_name").and_then(Value::as_str)?;
        if declared_stable_name.is_empty() || declared_stable_name != stable_name {
            return None;
        }
        let source_kind = object.get("source_kind")?.as_str()?;
        let transport_kind = object.get("transport_kind")?.as_str()?;
        let wrapper_field = object.get("wrapper_field")?.as_str()?;
        let expected_canonical_shape = object.get("expected_canonical_shape")?.as_str()?;
        if !matches!(source_kind, "custom_text" | "custom_grammar")
            || transport_kind != "function_object_wrapper"
            || wrapper_field != "input"
            || expected_canonical_shape != "single_required_string"
        {
            return None;
        }
        Some(Self {
            stable_name: stable_name.to_string(),
            source_kind: source_kind.to_string(),
            transport_kind: transport_kind.to_string(),
            wrapper_field: wrapper_field.to_string(),
            expected_canonical_shape: expected_canonical_shape.to_string(),
        })
    }

    fn to_value(&self) -> Value {
        serde_json::json!({
            "stable_name": self.stable_name,
            "source_kind": self.source_kind,
            "transport_kind": self.transport_kind,
            "wrapper_field": self.wrapper_field,
            "expected_canonical_shape": self.expected_canonical_shape
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrustedNamespaceBridgeContextEntry {
    namespace: String,
    child: String,
}

impl TrustedNamespaceBridgeContextEntry {
    fn from_value(flat_name: &str, value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let namespace = object.get("namespace").and_then(Value::as_str)?;
        let child = object.get("child").and_then(Value::as_str)?;
        if namespace.is_empty() || child.is_empty() {
            return None;
        }
        if flat_name != format!("{namespace}__{child}") {
            return None;
        }
        Some(Self {
            namespace: namespace.to_string(),
            child: child.to_string(),
        })
    }

    fn to_value(&self) -> Value {
        serde_json::json!({
            "namespace": self.namespace,
            "child": self.child
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrustedToolBridgeContext {
    version: u64,
    purpose: String,
    entries: BTreeMap<String, TrustedToolBridgeContextEntry>,
    namespace_entries: BTreeMap<String, TrustedNamespaceBridgeContextEntry>,
}

impl TrustedToolBridgeContext {
    fn from_value(value: Value) -> Option<Self> {
        let object = value.as_object()?;
        let version = object.get("version").and_then(Value::as_u64)?;
        if version != TOOL_BRIDGE_CONTEXT_VERSION {
            return None;
        }
        let purpose = object.get("purpose").and_then(Value::as_str)?;
        if purpose != TOOL_BRIDGE_CONTEXT_PURPOSE {
            return None;
        }
        let mut entries = BTreeMap::new();
        if let Some(entries_object) = object.get("entries").and_then(Value::as_object) {
            for (stable_name, entry_value) in entries_object {
                let entry = TrustedToolBridgeContextEntry::from_value(stable_name, entry_value)?;
                entries.insert(stable_name.clone(), entry);
            }
        }
        let mut namespace_entries = BTreeMap::new();
        if let Some(ns_object) = object.get("namespace_entries").and_then(Value::as_object) {
            for (flat_name, entry_value) in ns_object {
                let entry = TrustedNamespaceBridgeContextEntry::from_value(flat_name, entry_value)?;
                namespace_entries.insert(flat_name.clone(), entry);
            }
        }
        if entries.is_empty() && namespace_entries.is_empty() {
            return None;
        }
        Some(Self {
            version,
            purpose: purpose.to_string(),
            entries,
            namespace_entries,
        })
    }

    fn take_from_body(body: &mut Value) -> Option<Self> {
        let value = body
            .as_object_mut()?
            .remove(REQUEST_SCOPED_TOOL_BRIDGE_CONTEXT_FIELD)?;
        Self::from_value(value)
    }

    fn to_value(&self) -> Value {
        let entries = self
            .entries
            .iter()
            .map(|(stable_name, entry)| (stable_name.clone(), entry.to_value()))
            .collect::<serde_json::Map<String, Value>>();
        let namespace_entries = self
            .namespace_entries
            .iter()
            .map(|(flat_name, entry)| (flat_name.clone(), entry.to_value()))
            .collect::<serde_json::Map<String, Value>>();
        serde_json::json!({
            "version": self.version,
            "purpose": self.purpose,
            "entries": entries,
            "namespace_entries": namespace_entries
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RequestBoundaryDecision {
    Allow,
    AllowWithWarnings(Vec<String>),
    Reject(String),
}

#[derive(Debug, Clone)]
struct BridgeCaptureCandidate {
    namespace: String,
    owner_hash: String,
    client_model: String,
    resolved_model: ResolvedModel,
    route_config_fingerprint: BridgeRouteConfigFingerprint,
    request_items: Vec<Value>,
    ttl_seconds: u64,
    max_bytes: usize,
    local_response_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct BridgePreparation {
    capture_candidate: Option<BridgeCaptureCandidate>,
    debug_trace: Option<ConversationStateBridgeDebugTrace>,
}

type BridgeCommitFuture = Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send>>;

struct ConversationStateBridgeCaptureStream<S, E> {
    inner: TranslateSseStream<S, E>,
    store: Arc<ConversationStateBridgeStore>,
    candidate: Option<BridgeCaptureCandidate>,
    commit: Option<BridgeCommitFuture>,
    pending_after_commit: Option<Bytes>,
    done: bool,
}

impl<S, E> ConversationStateBridgeCaptureStream<S, E> {
    fn new(
        inner: TranslateSseStream<S, E>,
        store: Arc<ConversationStateBridgeStore>,
        candidate: BridgeCaptureCandidate,
    ) -> Self {
        Self {
            inner,
            store,
            candidate: Some(candidate),
            commit: None,
            pending_after_commit: None,
            done: false,
        }
    }
}

impl<S, E> Stream for ConversationStateBridgeCaptureStream<S, E>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: Into<Box<dyn std::error::Error + Send + Sync>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(commit) = this.commit.as_mut() {
                match commit.as_mut().poll(cx) {
                    Poll::Ready(result) => {
                        match result {
                            Ok(Some(local_id)) => {
                                debug!(
                                    "conversation_state_bridge captured streaming local response id={local_id}"
                                );
                            }
                            Ok(None) => {}
                            Err(message) => {
                                warn!(
                                    "conversation_state_bridge streaming capture skipped: {message}"
                                );
                            }
                        }
                        this.commit = None;
                        let Some(bytes) = this.pending_after_commit.take() else {
                            this.done = true;
                            return Poll::Ready(None);
                        };
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }

            if this.done {
                return Poll::Ready(None);
            }

            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    let Some(candidate) = this.candidate.take() else {
                        return Poll::Ready(Some(Ok(bytes)));
                    };
                    let Some(mut response) = this.inner.take_terminal_response() else {
                        this.candidate = Some(candidate);
                        return Poll::Ready(Some(Ok(bytes)));
                    };
                    let store = this.store.clone();
                    this.pending_after_commit = Some(bytes);
                    this.commit = Some(Box::pin(async move {
                        commit_conversation_state_bridge_capture(&store, candidate, &mut response)
                            .await
                    }));
                }
                Poll::Ready(Some(Err(error))) => {
                    this.candidate = None;
                    this.done = true;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    this.candidate = None;
                    this.done = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

pub(super) async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

pub(super) async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let runtime = state.runtime.read().await;
    let namespace_count = runtime.namespaces.len();
    if namespace_count == 0 {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "reason": "no namespaces configured",
                "namespace_count": namespace_count,
            })),
        )
    } else {
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ready",
                "namespace_count": namespace_count,
            })),
        )
    }
}

pub(super) async fn handle_openai_chat_completions(
    State(state): State<Arc<AppState>>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(
            UpstreamFormat::OpenAiChatCompletions,
        );
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::OpenAiChatCompletions,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_chat_completions_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_chat_completions_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(
            UpstreamFormat::OpenAiChatCompletions,
        );
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::OpenAiChatCompletions,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_chat_completions_inner(
        state,
        namespace,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_responses(
    State(state): State<Arc<AppState>>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_responses_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_openai_responses_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::OpenAiResponses);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::OpenAiResponses,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_openai_responses_inner(
        state,
        namespace,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_anthropic_messages(
    State(state): State<Arc<AppState>>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        DEFAULT_NAMESPACE,
        UpstreamFormat::Anthropic,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_anthropic_messages_inner(
        state,
        DEFAULT_NAMESPACE.to_string(),
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
        auth_context,
    )
    .await
}

pub(super) async fn handle_anthropic_messages_namespaced(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    downstream_cancellation: Option<Extension<DownstreamCancellation>>,
    request: Request,
) -> Response<Body> {
    let Some(auth_context) = data_auth::request_auth_context_from_request(&request) else {
        return data_auth::missing_request_auth_context_response(UpstreamFormat::Anthropic);
    };
    let (headers, body) = match read_limited_json_request(
        &state,
        &namespace,
        UpstreamFormat::Anthropic,
        &auth_context,
        request,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    handle_anthropic_messages_inner(
        state,
        namespace,
        downstream_cancellation
            .map(|Extension(cancellation)| cancellation)
            .unwrap_or_else(DownstreamCancellation::disabled),
        headers,
        body,
        auth_context,
    )
    .await
}

async fn handle_openai_chat_completions_inner(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: JsonRequestBody,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    let requested_model = body
        .parsed()
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    handle_request_core_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        "/openai/v1/chat/completions".to_string(),
        body,
        requested_model,
        UpstreamFormat::OpenAiChatCompletions,
        None,
        auth_context,
    )
    .await
}

async fn handle_openai_responses_inner(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: JsonRequestBody,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    let requested_model = body
        .parsed()
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    handle_request_core_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        "/openai/v1/responses".to_string(),
        body,
        requested_model,
        UpstreamFormat::OpenAiResponses,
        None,
        auth_context,
    )
    .await
}

async fn handle_anthropic_messages_inner(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    body: JsonRequestBody,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    let requested_model = body
        .parsed()
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    handle_request_core_with_downstream_cancellation(
        state,
        namespace,
        downstream_cancellation,
        headers,
        "/anthropic/v1/messages".to_string(),
        body,
        requested_model,
        UpstreamFormat::Anthropic,
        None,
        auth_context,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(super) async fn handle_request_core(
    state: Arc<AppState>,
    namespace: String,
    headers: HeaderMap,
    path: String,
    body: Value,
    requested_model: String,
    client_format: UpstreamFormat,
    forced_stream: Option<bool>,
) -> Response<Body> {
    let auth_context = trusted_test_request_auth_context(&state, &headers).await;
    handle_request_core_with_auth_context(
        state,
        TestRequestCoreRequest {
            namespace,
            headers,
            path,
            body,
            requested_model,
            client_format,
            forced_stream,
            auth_context,
        },
    )
    .await
}

#[cfg(test)]
pub(super) struct TestRequestCoreRequest {
    pub(super) namespace: String,
    pub(super) headers: HeaderMap,
    pub(super) path: String,
    pub(super) body: Value,
    pub(super) requested_model: String,
    pub(super) client_format: UpstreamFormat,
    pub(super) forced_stream: Option<bool>,
    pub(super) auth_context: RequestAuthContext,
}

#[cfg(test)]
pub(super) async fn handle_request_core_with_auth_context(
    state: Arc<AppState>,
    request: TestRequestCoreRequest,
) -> Response<Body> {
    handle_request_core_with_downstream_cancellation(
        state,
        request.namespace,
        DownstreamCancellation::disabled(),
        request.headers,
        request.path,
        JsonRequestBody::from_parsed_value(request.body),
        request.requested_model,
        request.client_format,
        request.forced_stream,
        request.auth_context,
    )
    .await
}

#[cfg(test)]
async fn trusted_test_request_auth_context(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> RequestAuthContext {
    let runtime = state.runtime.read().await.clone();
    let access = state.data_auth_policy.current_access().await;
    let (mode, authorization) = match &access {
        data_auth::DataAccess::ClientProviderKey => (
            crate::config::DataAuthMode::ClientProviderKey,
            data_auth::RequestAuthorization::ClientProviderKey {
                provider_key: test_client_provider_key_from_headers(headers)
                    .unwrap_or_else(|| "test-client-provider-key".to_string()),
            },
        ),
        data_auth::DataAccess::ProxyKey { .. } => (
            crate::config::DataAuthMode::ProxyKey,
            data_auth::RequestAuthorization::ProxyKey,
        ),
        data_auth::DataAccess::Unconfigured => (
            crate::config::DataAuthMode::ClientProviderKey,
            data_auth::RequestAuthorization::ClientProviderKey {
                provider_key: "test-client-provider-key".to_string(),
            },
        ),
        data_auth::DataAccess::Misconfigured(_) => (
            crate::config::DataAuthMode::ClientProviderKey,
            data_auth::RequestAuthorization::ClientProviderKey {
                provider_key: "test-client-provider-key".to_string(),
            },
        ),
    };
    RequestAuthContext::for_test("test-generation", mode, access, authorization, runtime)
}

#[cfg(test)]
fn test_client_provider_key_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .get(..7)
                .filter(|prefix| prefix.eq_ignore_ascii_case("Bearer "))
                .map(|_| value[7..].to_string())
        })
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            [
                "x-api-key",
                "x-goog-api-key",
                "api-key",
                "openai-api-key",
                "anthropic-api-key",
            ]
            .into_iter()
            .find_map(|name| {
                headers
                    .get(axum::http::HeaderName::from_static(name))
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| !value.trim().is_empty())
                    .map(ToString::to_string)
            })
        })
}

fn redact_header_entries(redactor: &SecretRedactor, headers: Vec<HeaderEntry>) -> Vec<HeaderEntry> {
    headers
        .into_iter()
        .map(|entry| HeaderEntry {
            name: entry.name,
            value: redactor.redact_text(&entry.value),
        })
        .collect()
}

struct RedactedRequestMetadata {
    path: String,
    client_model: String,
    upstream_name: String,
    upstream_model: String,
}

impl RedactedRequestMetadata {
    fn new(
        redactor: &SecretRedactor,
        path: &str,
        client_model: &str,
        upstream_name: &str,
        upstream_model: &str,
    ) -> Self {
        Self {
            path: redactor.redact_text(path),
            client_model: redactor.redact_text(client_model),
            upstream_name: redactor.redact_text(upstream_name),
            upstream_model: redactor.redact_text(upstream_model),
        }
    }
}

fn redacted_error_response(
    format: UpstreamFormat,
    status: StatusCode,
    message: &str,
    redactor: &SecretRedactor,
) -> Response<Body> {
    error_response(format, status, &redactor.redact_text(message))
}

fn redacted_streaming_error_response(
    format: UpstreamFormat,
    status: StatusCode,
    message: &str,
    redactor: &SecretRedactor,
) -> Response<Body> {
    streaming_error_response(format, status, &redactor.redact_text(message))
}

fn response_with_portability_warning_headers(
    mut response: Response<Body>,
    portability_warnings: &[String],
) -> Response<Body> {
    append_portability_warning_headers(&mut response, portability_warnings);
    response
}

/// Set the `openai-model` HTTP response header to the client-facing model
/// alias. Codex CLI (0.146.0+) reads the served model from this header rather
/// than the JSON body `model` field (see
/// `docs/engineering/2026-08-01-codex-compat-feedback-plan.md` §2.1), so it is
/// stamped on every OpenAI-family (Responses + Chat Completions) response —
/// streaming and non-streaming, success and inline error-passthrough.
///
/// The header is skipped for non-OpenAI clients, for an empty alias, or when
/// the alias contains bytes that are not valid in an HTTP header value, so an
/// unusual client `model` value can never break response construction.
fn set_openai_model_header(
    response: &mut Response<Body>,
    client_format: UpstreamFormat,
    model_alias: &str,
) {
    if !matches!(
        client_format,
        UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::OpenAiResponses
    ) {
        return;
    }
    if model_alias.is_empty() {
        return;
    }
    if let Ok(value) = axum::http::HeaderValue::from_str(model_alias) {
        response.headers_mut().insert("openai-model", value);
    }
}

fn upstream_request_body_with_synthesized_prompt_cache_key_redacted(
    upstream_request_body: &Value,
    key_fingerprint: &str,
) -> Value {
    let mut redacted = upstream_request_body.clone();
    if let Some(object) = redacted.as_object_mut() {
        if object.get("prompt_cache_key").is_some() {
            object.insert(
                "prompt_cache_key".to_string(),
                Value::String(format!("[SYNTHESIZED:{key_fingerprint}]")),
            );
        }
    }
    redacted
}

fn redact_text_with_prompt_cache_key(
    request_redactor: &SecretRedactor,
    prompt_cache_key_redactor: Option<&SecretRedactor>,
    value: &str,
) -> String {
    let redacted = request_redactor.redact_text(value);
    prompt_cache_key_redactor
        .map(|redactor| redactor.redact_text(&redacted))
        .unwrap_or(redacted)
}

fn redact_value_with_prompt_cache_key(
    request_redactor: &SecretRedactor,
    prompt_cache_key_redactor: Option<&SecretRedactor>,
    value: &Value,
) -> Value {
    let redacted = request_redactor.redact_value(value);
    prompt_cache_key_redactor
        .map(|redactor| redactor.redact_value(&redacted))
        .unwrap_or(redacted)
}

/// Redact known secrets in a zero-transform (raw passthrough) upstream error
/// body, mirroring the redaction the response headers already receive via
/// `append_raw_upstream_response_headers`. Only uncompressed bodies (no
/// non-identity `Content-Encoding`) are redacted so compressed payloads are
/// forwarded byte-for-byte; the upstream JSON shape is preserved and only
/// known secrets are replaced with `[REDACTED]`. A redacted body may differ in
/// length, but `matching_content_length` drops a mismatched `Content-Length`
/// so axum recomputes it.
fn redact_zero_transform_error_body(
    bytes: &Bytes,
    upstream_headers: &reqwest::header::HeaderMap,
    redactor: &SecretRedactor,
) -> Vec<u8> {
    let uncompressed = upstream_headers
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("identity"))
        .unwrap_or(true);
    if !uncompressed {
        return bytes.to_vec();
    }
    redactor
        .redact_text(&String::from_utf8_lossy(bytes))
        .into_bytes()
}

#[allow(clippy::too_many_arguments)]
async fn handle_request_core_with_downstream_cancellation(
    state: Arc<AppState>,
    namespace: String,
    downstream_cancellation: DownstreamCancellation,
    headers: HeaderMap,
    path: String,
    body: JsonRequestBody,
    mut requested_model: String,
    client_format: UpstreamFormat,
    forced_stream: Option<bool>,
    auth_context: RequestAuthContext,
) -> Response<Body> {
    let (raw_body_bytes, mut body) = body.into_parts();
    let request_id = new_request_id();
    let request_timestamp = now_timestamp_ms();
    let downstream_body = body.clone();
    let original_headers = capture_headers(&headers);
    let request_redactor = redactor_for_request(&auth_context, &headers);
    let redacted_original_body = request_redactor.redact_value(&downstream_body);
    let redacted_original_headers = redact_header_entries(&request_redactor, original_headers);
    let redacted_path = request_redactor.redact_text(&path);
    let redacted_requested_model = request_redactor.redact_text(&requested_model);
    let stream = forced_stream
        .unwrap_or_else(|| body.get("stream").and_then(Value::as_bool).unwrap_or(false));

    debug!("Request path: {}", redacted_path);
    debug!(
        "Request body: {}",
        request_redactor
            .redact_text(&serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()))
    );

    let namespace_state = {
        match auth_context.runtime().namespaces.get(&namespace) {
            Some(item) => item.clone(),
            None => {
                return redacted_error_response(
                    client_format,
                    StatusCode::NOT_FOUND,
                    &format!("namespace `{namespace}` is not configured"),
                    &request_redactor,
                );
            }
        }
    };

    let mut tracker = state.metrics.start_request(
        redacted_path.clone(),
        redacted_requested_model.clone(),
        stream,
    );
    if let Some(message) = reject_internal_request_scoped_tool_bridge_context(&downstream_body) {
        tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
        return redacted_error_response(
            client_format,
            StatusCode::BAD_REQUEST,
            &message,
            &request_redactor,
        );
    }

    let bridge_owner_hash = ConversationStateBridgeStore::owner_hash(&namespace, &auth_context);
    let preloaded_bridge_response = if conversation_state_bridge_can_preload(
        &namespace_state,
        client_format,
        &requested_model,
        &body,
    ) {
        match preload_local_bridge_response(
            &state.conversation_state_bridge,
            &body,
            &namespace,
            &bridge_owner_hash,
        )
        .await
        {
            Ok(entry) => {
                if requested_model.trim().is_empty() {
                    requested_model = entry.client_model.clone();
                }
                Some(entry)
            }
            Err(message) => {
                tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
                return redacted_error_response(
                    client_format,
                    StatusCode::BAD_REQUEST,
                    &message,
                    &request_redactor,
                );
            }
        }
    } else {
        None
    };

    let reusable_model_less_bridge_route = preloaded_bridge_response.as_ref().filter(|entry| {
        requested_model.trim().is_empty()
            && entry.client_model.trim().is_empty()
            && entry.route_config_fingerprint.schema_version == LOCAL_REPLAY_SCHEMA_VERSION
            && entry.route_config_fingerprint.namespace_revision == namespace_state.revision
    });
    let resolved_model = if let Some(entry) = reusable_model_less_bridge_route {
        entry.resolved_model.clone()
    } else {
        match resolve_request_model_or_error(
            &namespace_state,
            &requested_model,
            client_format,
            &body,
        ) {
            Ok(v) => v,
            Err(e) => {
                tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
                let message = if preloaded_bridge_response.is_some() {
                    conversation_state_bridge_route_config_changed_error()
                } else {
                    e.as_str()
                };
                return redacted_error_response(
                    client_format,
                    StatusCode::BAD_REQUEST,
                    message,
                    &request_redactor,
                );
            }
        }
    };
    if let Some(entry) = preloaded_bridge_response.as_ref() {
        if entry.route_config_fingerprint.schema_version != LOCAL_REPLAY_SCHEMA_VERSION
            || entry.route_config_fingerprint.namespace_revision != namespace_state.revision
            || entry.resolved_model != resolved_model
        {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return redacted_error_response(
                client_format,
                StatusCode::BAD_REQUEST,
                conversation_state_bridge_route_config_changed_error(),
                &request_redactor,
            );
        }
    }
    let upstream_state = match namespace_state.upstreams.get(&resolved_model.upstream_name) {
        Some(v) => v,
        None => {
            tracker.finish_error(StatusCode::INTERNAL_SERVER_ERROR.as_u16());
            return redacted_error_response(
                client_format,
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!(
                    "resolved upstream `{}` is not configured",
                    resolved_model.upstream_name
                ),
                &request_redactor,
            );
        }
    };
    let redacted_metadata = RedactedRequestMetadata::new(
        &request_redactor,
        &path,
        &requested_model,
        &resolved_model.upstream_name,
        &resolved_model.upstream_model,
    );
    tracker.set_upstream(
        redacted_metadata.upstream_name.clone(),
        redacted_metadata.upstream_model.clone(),
    );
    let request_translation_policy =
        request_translation_policy(&namespace_state.config, &requested_model, &resolved_model);
    if !upstream_state.availability.is_available() {
        tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
        return redacted_error_response(
            client_format,
            StatusCode::SERVICE_UNAVAILABLE,
            &format_upstream_unavailable_message(
                &resolved_model.upstream_name,
                &upstream_state.availability,
            ),
            &request_redactor,
        );
    }

    let Some(capability) = upstream_state.capability.as_ref() else {
        tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
        return redacted_error_response(
            client_format,
            StatusCode::SERVICE_UNAVAILABLE,
            &format_upstream_unavailable_message(
                &resolved_model.upstream_name,
                &upstream_state.availability,
            ),
            &request_redactor,
        );
    };
    let upstream_format = capability.upstream_format_for_request(client_format);
    if let Some(message) = reject_local_bridge_id_on_native_responses_passthrough(
        client_format,
        upstream_format,
        &body,
    ) {
        tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
        return redacted_error_response(
            client_format,
            StatusCode::BAD_REQUEST,
            message,
            &request_redactor,
        );
    }
    let bridge_route_config_fingerprint = conversation_state_bridge_route_config_fingerprint(
        &namespace_state,
        &resolved_model,
        upstream_format,
        &request_translation_policy,
    );
    if let Some(entry) = preloaded_bridge_response.as_ref() {
        if entry.route_config_fingerprint != bridge_route_config_fingerprint {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return redacted_error_response(
                client_format,
                StatusCode::BAD_REQUEST,
                conversation_state_bridge_route_config_changed_error(),
                &request_redactor,
            );
        }
    }
    if let Some(obj) = body.as_object_mut() {
        if let Some(forced_stream) = forced_stream {
            obj.insert("stream".to_string(), Value::Bool(forced_stream));
        }
    }

    let bridge_was_expanded = preloaded_bridge_response.is_some();
    let bridge_preparation = match prepare_conversation_state_bridge(
        &namespace_state,
        &namespace,
        &bridge_owner_hash,
        client_format,
        upstream_format,
        stream,
        &requested_model,
        &resolved_model,
        bridge_route_config_fingerprint,
        preloaded_bridge_response,
        &mut body,
    )
    .await
    {
        Ok(candidate) => candidate,
        Err(message) => {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return redacted_error_response(
                client_format,
                StatusCode::BAD_REQUEST,
                &message,
                &request_redactor,
            );
        }
    };
    let BridgePreparation {
        capture_candidate: bridge_capture_candidate,
        debug_trace: bridge_debug_trace,
    } = bridge_preparation;

    let original_body = body.clone();
    let stateful_responses_controls = responses_stateful_request_controls(&original_body);
    let state_bridge = if bridge_was_expanded {
        StateBridgeModifier::Expanded
    } else if bridge_capture_candidate.is_some() {
        StateBridgeModifier::CaptureCandidate
    } else {
        StateBridgeModifier::Off
    };
    // Step 3: resolve the upstream's reasoning dialect (if any) and the client's normalized
    // reasoning effort up front. Both feed the body-mutation gate below and the dialect-aware
    // reasoning emit pass applied to the final upstream body. When no dialect is configured,
    // `dialect_reasoning_emit_applies` and `dialect_echo_suppresses` are false and every
    // downstream branch is unchanged.
    let resolved_dialect = upstream_state
        .config
        .dialect
        .as_ref()
        .and_then(|dialect| dialect.resolve().ok());
    // The client's reasoning effort only matters when a dialect is configured (it feeds the
    // dialect-aware emit pass below), so skip parsing on the no-dialect hot path.
    let client_reasoning_effort = resolved_dialect.as_ref().and_then(|_| {
        crate::translate::parse_client_reasoning_effort(&original_body, client_format)
    });
    let dialect_reasoning_emit_applies = client_reasoning_effort.is_some();
    // Symmetric with the emit gate: when the dialect suppresses the reasoning echo
    // (`reasoning_echo: Some(false)`), force the JSON/translate path regardless of whether the
    // client sent a reasoning effort, so the response reaches `translate_response_with_context`
    // where the echo strip happens. Without this, a no-effort same-format request would take the
    // raw-passthrough path and surface thinking/reasoning_content despite echo:false.
    let dialect_echo_suppresses =
        resolved_dialect.as_ref().and_then(|d| d.reasoning_echo) == Some(false);

    let mut llmup = classify_request_processing(RequestProcessingInput {
        client_format,
        upstream_format,
        body: &original_body,
        requested_model: &requested_model,
        upstream_model: &resolved_model.upstream_model,
        stream,
        forced_stream: forced_stream.is_some(),
        route_policy_requires_body_mutation: request_translation_policy_requires_body_mutation(
            upstream_format,
            &original_body,
            &request_translation_policy,
        ) || dialect_reasoning_emit_applies
            || dialect_echo_suppresses,
        state_bridge,
    });
    tracker.set_request_processing(llmup);

    let mut portability_warnings = match classify_request_boundary_with_policy(
        client_format,
        upstream_format,
        &original_body,
        &request_translation_policy,
        &resolved_model.upstream_model,
        resolved_dialect.as_ref(),
    ) {
        RequestBoundaryDecision::Allow => Vec::new(),
        RequestBoundaryDecision::AllowWithWarnings(warnings) => warnings
            .into_iter()
            .map(|warning| request_redactor.redact_text(&warning))
            .collect(),
        RequestBoundaryDecision::Reject(message) => {
            tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
            return redacted_error_response(
                client_format,
                StatusCode::BAD_REQUEST,
                &message,
                &request_redactor,
            );
        }
    };
    for warning in &portability_warnings {
        warn!(
            "portability warning: client_format={} upstream_format={} warning={}",
            client_format, upstream_format, warning
        );
    }

    let (mut upstream_request_body, raw_upstream_request_body, request_scoped_tool_bridge_context) =
        if llmup.request_processing == RequestProcessing::RequestTransformationNotRequired {
            if let Err(e) =
                validate_provider_forwarding_request_boundary(client_format, &original_body)
            {
                let redacted_error = request_redactor.redact_text(&e);
                error!("Request boundary validation failed: {}", redacted_error);
                tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
                return response_with_portability_warning_headers(
                    error_response(client_format, StatusCode::BAD_REQUEST, &redacted_error),
                    &portability_warnings,
                );
            }
            (original_body.clone(), Some(raw_body_bytes), None)
        } else {
            if let Err(e) = translate_request_with_policy(
                client_format,
                upstream_format,
                &resolved_model.upstream_model,
                &mut body,
                request_translation_policy,
                stream,
            ) {
                let redacted_error = request_redactor.redact_text(&e);
                error!("Translation failed: {}", redacted_error);
                tracker.finish_error(StatusCode::BAD_REQUEST.as_u16());
                return response_with_portability_warning_headers(
                    error_response(client_format, StatusCode::BAD_REQUEST, &redacted_error),
                    &portability_warnings,
                );
            }

            if let Some(obj) = body.as_object_mut() {
                match upstream_format {
                    _ if client_format == UpstreamFormat::OpenAiResponses
                        && upstream_format == UpstreamFormat::OpenAiResponses
                        && requested_model.trim().is_empty()
                        && !stateful_responses_controls.is_empty()
                        && resolved_model.upstream_model.trim().is_empty() =>
                    {
                        obj.remove("model");
                    }
                    _ => {
                        obj.insert(
                            "model".to_string(),
                            Value::String(resolved_model.upstream_model.clone()),
                        );
                    }
                }
            }

            let request_scoped_tool_bridge_context =
                TrustedToolBridgeContext::take_from_body(&mut body);
            (body.clone(), None, request_scoped_tool_bridge_context)
        };

    // Step 3: dialect-switched reasoning-effort emit. The gate above forces the JSON/translate
    // path (raw-byte forwarding is off) whenever this applies, so mutating the finalized upstream
    // body here is guaranteed to reach the upstream send. Skipped entirely when no dialect is
    // configured — preserving byte-identical no-dialect behavior.
    if dialect_reasoning_emit_applies {
        let dialect = resolved_dialect
            .as_ref()
            .expect("dialect present when dialect_reasoning_emit_applies is true");
        let effort = client_reasoning_effort
            .expect("client effort present when dialect_reasoning_emit_applies is true");
        if let Some(warning) = crate::translate::apply_dialect_reasoning_emit(
            &mut upstream_request_body,
            upstream_format,
            dialect,
            effort,
        ) {
            let redacted = request_redactor.redact_text(&warning);
            warn!(
                "portability warning: client_format={} upstream_format={} warning={}",
                client_format, upstream_format, &redacted
            );
            portability_warnings.push(redacted);
        }
    }

    let prompt_cache_key_synthesis = if raw_upstream_request_body.is_none() {
        synthesize_openai_family_prompt_cache_key_from_source(
            OpenAiFamilyPromptCacheKeySynthesisContext {
                namespace: &namespace,
                upstream_name: &resolved_model.upstream_name,
                upstream_model: &resolved_model.upstream_model,
                upstream_format,
            },
            client_format,
            &original_body,
            &mut upstream_request_body,
        )
    } else {
        None
    };
    if let Some(synthesis) = prompt_cache_key_synthesis.as_ref() {
        llmup.provider_native_prompt_cache = PromptCacheRequestControl::Synthesized;
        tracker.set_request_processing(llmup);
        debug!(
            "Synthesized OpenAI-family prompt_cache_key fingerprint={}",
            synthesis.key_fingerprint()
        );
    }
    let prompt_cache_key_redactor = prompt_cache_key_synthesis
        .as_ref()
        .map(|synthesis| SecretRedactor::new([synthesis.key().to_string()]));
    let response_header_redactor = |value: &str| {
        redact_text_with_prompt_cache_key(
            &request_redactor,
            prompt_cache_key_redactor.as_ref(),
            value,
        )
    };

    let upstream_request_body_for_debug = prompt_cache_key_synthesis
        .as_ref()
        .map(|synthesis| {
            upstream_request_body_with_synthesized_prompt_cache_key_redacted(
                &upstream_request_body,
                synthesis.key_fingerprint(),
            )
        })
        .unwrap_or_else(|| upstream_request_body.clone());
    debug!(
        "Upstream request body: {}",
        request_redactor.redact_text(
            &serde_json::to_string_pretty(&upstream_request_body_for_debug)
                .unwrap_or_else(|_| upstream_request_body_for_debug.to_string())
        )
    );

    let (mut auth_headers, effective_credential) =
        match build_auth_headers(&headers, &auth_context, upstream_state, upstream_format) {
            Ok(value) => value,
            Err(message) => {
                tracker.finish_error(StatusCode::SERVICE_UNAVAILABLE.as_u16());
                return response_with_portability_warning_headers(
                    redacted_error_response(
                        client_format,
                        StatusCode::SERVICE_UNAVAILABLE,
                        &message,
                        &request_redactor,
                    ),
                    &portability_warnings,
                );
            }
        };
    apply_upstream_headers(
        &mut auth_headers,
        &upstream_state.config.upstream_headers,
        upstream_format,
    );
    if raw_upstream_request_body.is_some() {
        llmup.zero_transform_forwarding_active = true;
        tracker.set_request_processing(llmup);
    }
    let hook_ctx = namespace_state.hooks.as_ref().map(|_| HookRequestContext {
        request_id: request_id.clone(),
        timestamp_ms: request_timestamp,
        path: redacted_metadata.path.clone(),
        method: "POST".to_string(),
        stream,
        client_model: redacted_metadata.client_model.clone(),
        upstream_name: redacted_metadata.upstream_name.clone(),
        upstream_model: redacted_metadata.upstream_model.clone(),
        client_format,
        upstream_format,
        llmup,
        credential_source: effective_credential.source,
        credential_fingerprint: effective_credential.fingerprint.clone(),
        client_request_headers: redacted_original_headers,
        client_request_body: redacted_original_body.clone(),
    });
    let debug_ctx = namespace_state
        .debug_trace
        .as_ref()
        .map(|_| DebugTraceContext {
            request_id: request_id.clone(),
            timestamp_ms: request_timestamp,
            path: redacted_metadata.path.clone(),
            stream,
            client_model: redacted_metadata.client_model.clone(),
            upstream_name: redacted_metadata.upstream_name.clone(),
            upstream_model: redacted_metadata.upstream_model.clone(),
            client_format,
            upstream_format,
            llmup,
        });
    if let (Some(recorder), Some(ctx)) = (namespace_state.debug_trace.as_ref(), debug_ctx.as_ref())
    {
        let redacted_upstream_request_body = redact_value_with_prompt_cache_key(
            &request_redactor,
            prompt_cache_key_redactor.as_ref(),
            &upstream_request_body_for_debug,
        );
        recorder.record_request_with_upstream_and_bridge_trace(
            ctx,
            &redacted_original_body,
            &redacted_upstream_request_body,
            bridge_debug_trace.as_ref(),
        );
    }

    let url = upstream::upstream_url(
        &namespace_state.config,
        &upstream_state.config,
        upstream_format,
        None,
        stream,
    );
    debug!(
        "Calling upstream URL: {}",
        request_redactor.redact_text(&url)
    );
    let upstream_client = if stream && llmup.zero_transform_forwarding_active {
        upstream_state
            .no_auto_decompression_streaming_client
            .clone()
    } else if stream {
        upstream_state.streaming_client.clone()
    } else if llmup.zero_transform_forwarding_active {
        upstream_state.no_auto_decompression_client.clone()
    } else {
        upstream_state.client.clone()
    };
    let upstream_body = match raw_upstream_request_body.as_ref() {
        Some(raw_body) => upstream::UpstreamRequestBody::RawJson(raw_body),
        None => upstream::UpstreamRequestBody::Json(&upstream_request_body),
    };
    let res = match upstream::call_upstream_with_cancellation(
        &upstream_client,
        &url,
        upstream_body,
        stream,
        &auth_headers,
        stream.then_some(namespace_state.config.upstream_timeout),
        &downstream_cancellation,
    )
    .await
    {
        Ok(r) => r,
        Err(upstream::DownstreamAwareError::Inner(e)) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            let message = redact_text_with_prompt_cache_key(
                &request_redactor,
                prompt_cache_key_redactor.as_ref(),
                &e.to_string(),
            );
            let response = if stream {
                streaming_error_response(client_format, StatusCode::BAD_GATEWAY, &message)
            } else {
                error_response(client_format, StatusCode::BAD_GATEWAY, &message)
            };
            return response_with_portability_warning_headers(response, &portability_warnings);
        }
        Err(upstream::DownstreamAwareError::DownstreamCancelled) => {
            tracker.finish_cancelled();
            return client_closed_response(client_format);
        }
    };
    let preserve_native_upstream_protocol_headers = upstream_format == client_format;

    if stream {
        let status = res.status();
        let upstream_response_headers = res.headers().clone();
        debug!("Upstream streaming response status: {}", status);
        if llmup.zero_transform_forwarding_active {
            if !status.is_success() {
                let bytes = match tokio::time::timeout(
                    namespace_state.config.upstream_timeout,
                    upstream::read_response_bytes_limited_with_cancellation(
                        res,
                        namespace_state
                            .config
                            .resource_limits
                            .max_upstream_error_body_bytes,
                        &downstream_cancellation,
                    ),
                )
                .await
                {
                    Ok(Ok(body)) => body,
                    Ok(Err(upstream::DownstreamAwareError::Inner(
                        upstream::ResponseBodyLimitError::LimitExceeded { limit },
                    ))) => {
                        tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                        return response_with_portability_warning_headers(
                            redacted_streaming_error_response(
                                client_format,
                                StatusCode::BAD_GATEWAY,
                                &format!(
                                    "upstream error body exceeded resource limit of {limit} bytes"
                                ),
                                &request_redactor,
                            ),
                            &portability_warnings,
                        );
                    }
                    Ok(Err(upstream::DownstreamAwareError::Inner(
                        upstream::ResponseBodyLimitError::Inner(error),
                    ))) => {
                        tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                        return response_with_portability_warning_headers(
                            redacted_streaming_error_response(
                                client_format,
                                StatusCode::BAD_GATEWAY,
                                &format!("failed to read upstream error body: {error}"),
                                &request_redactor,
                            ),
                            &portability_warnings,
                        );
                    }
                    Ok(Err(upstream::DownstreamAwareError::DownstreamCancelled)) => {
                        tracker.finish_cancelled();
                        return client_closed_response(client_format);
                    }
                    // The streaming client only bounds connect + headers, so a
                    // stalled upstream (crash / LB timeout mid-response) would
                    // otherwise hang the proxy task on the error-body read
                    // forever. Bound it with the same budget used for headers and
                    // treat an elapsed read as an upstream read failure.
                    Err(_elapsed) => {
                        tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                        return response_with_portability_warning_headers(
                            redacted_streaming_error_response(
                                client_format,
                                StatusCode::BAD_GATEWAY,
                                &format!(
                                    "failed to read upstream error body: timed out after {:?}",
                                    namespace_state.config.upstream_timeout
                                ),
                                &request_redactor,
                            ),
                            &portability_warnings,
                        );
                    }
                };
                tracker.finish_error(status.as_u16());
                let forwarded_bytes = Bytes::from(redact_zero_transform_error_body(
                    &bytes,
                    &upstream_response_headers,
                    &request_redactor,
                ));
                let body_len = forwarded_bytes.len();
                let mut response = Response::builder()
                    .status(status)
                    .body(Body::from(forwarded_bytes))
                    .unwrap();
                append_raw_upstream_response_headers(
                    &mut response,
                    &upstream_response_headers,
                    body_len,
                    &response_header_redactor,
                );
                set_openai_model_header(&mut response, client_format, &requested_model);
                return response_with_portability_warning_headers(response, &portability_warnings);
            }

            if !response_is_event_stream(&upstream_response_headers) {
                tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                return response_with_portability_warning_headers(
                    redacted_streaming_error_response(
                        client_format,
                        StatusCode::BAD_GATEWAY,
                        "upstream returned non-SSE response for streaming request",
                        &request_redactor,
                    ),
                    &portability_warnings,
                );
            }

            let upstream_stream = res
                .bytes_stream()
                .map(|result| result.map_err(std::io::Error::other));
            let observation_max_sse_frame_bytes =
                namespace_state.config.resource_limits.max_sse_frame_bytes;
            let mut body_stream: Pin<
                Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
            > = Box::pin(upstream_stream);
            if let (Some(dispatcher), Some(ctx)) = (namespace_state.hooks.clone(), hook_ctx.clone())
            {
                body_stream = Box::pin(dispatcher.wrap_stream_with_observation_transform(
                    body_stream,
                    ctx,
                    status.as_u16(),
                    sse_response_headers(),
                    Box::new(RedactingSseObservationTransform::new_bounded(
                        request_redactor.clone(),
                        observation_max_sse_frame_bytes,
                    )),
                ));
            }
            if let (Some(recorder), Some(ctx)) =
                (namespace_state.debug_trace.as_ref(), debug_ctx.clone())
            {
                body_stream = Box::pin(recorder.wrap_stream_with_observation_transform(
                    body_stream,
                    ctx,
                    status.as_u16(),
                    Box::new(RedactingSseObservationTransform::new_bounded(
                        request_redactor.clone(),
                        observation_max_sse_frame_bytes,
                    )),
                ));
            }
            // Apply the idle/max-duration resource timers to the raw upstream
            // stream so a stalled zero-transform upstream cannot hang the
            // connection/task indefinitely (C1). This deliberately wraps with
            // the timer-only `ResourceLimitedStream` rather than
            // `GuardedSseStream`, which would re-canonicalize/sanitize SSE
            // frames and corrupt raw passthrough bytes.
            body_stream = Box::pin(ResourceLimitedStream::new(
                body_stream,
                namespace_state.config.resource_limits.clone(),
            ));
            let body = Body::from_stream(TrackedBodyStream::new(
                body_stream,
                tracker,
                status.as_u16(),
                downstream_cancellation.cancellation_token(),
            ));
            let mut response = Response::builder()
                .status(status)
                .header("Cache-Control", "no-cache")
                .header("Connection", "keep-alive")
                .body(body)
                .unwrap();
            append_raw_upstream_stream_response_headers(
                &mut response,
                &upstream_response_headers,
                &response_header_redactor,
            );
            set_openai_model_header(&mut response, client_format, &requested_model);
            return response_with_portability_warning_headers(response, &portability_warnings);
        }
        if !status.is_success() {
            let error_body = match tokio::time::timeout(
                namespace_state.config.upstream_timeout,
                upstream::read_response_text_limited_with_cancellation(
                    res,
                    namespace_state
                        .config
                        .resource_limits
                        .max_upstream_error_body_bytes,
                    &downstream_cancellation,
                ),
            )
            .await
            {
                Ok(Ok(body)) => body,
                Ok(Err(upstream::DownstreamAwareError::Inner(
                    upstream::ResponseBodyLimitError::LimitExceeded { limit },
                ))) => {
                    tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
                    return response_with_portability_warning_headers(
                        redacted_streaming_error_response(
                            client_format,
                            StatusCode::BAD_GATEWAY,
                            &format!(
                                "upstream error body exceeded resource limit of {limit} bytes"
                            ),
                            &request_redactor,
                        ),
                        &portability_warnings,
                    );
                }
                Ok(Err(upstream::DownstreamAwareError::Inner(
                    upstream::ResponseBodyLimitError::Inner(_),
                ))) => "Unknown error".to_string(),
                Ok(Err(upstream::DownstreamAwareError::DownstreamCancelled)) => {
                    tracker.finish_cancelled();
                    return client_closed_response(client_format);
                }
                // Bound the streaming error-body read with the same budget used
                // for headers; a stalled upstream would otherwise hang the proxy
                // task on the read forever. Treat an elapsed read as an upstream
                // read failure.
                Err(_elapsed) => format!(
                    "failed to read upstream error body: timed out after {:?}",
                    namespace_state.config.upstream_timeout
                ),
            };
            let redacted_error_body = redact_text_with_prompt_cache_key(
                &request_redactor,
                prompt_cache_key_redactor.as_ref(),
                &error_body,
            );
            error!(
                "Upstream returned error for streaming request: {} - {}",
                status, redacted_error_body
            );
            tracker.finish_error(status.as_u16());
            let public_error_body = if serde_json::from_str::<Value>(&error_body).is_ok() {
                error_body
            } else {
                format!("upstream streaming error body: {error_body}")
            };
            let public_error_body = redact_text_with_prompt_cache_key(
                &request_redactor,
                prompt_cache_key_redactor.as_ref(),
                &public_error_body,
            );
            let mut response = streaming_error_response(
                client_format,
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                &public_error_body,
            );
            if preserve_native_upstream_protocol_headers {
                append_upstream_protocol_response_headers(
                    &mut response,
                    &upstream_response_headers,
                    &response_header_redactor,
                );
            }
            return response_with_portability_warning_headers(response, &portability_warnings);
        }
        if !response_is_event_stream(&upstream_response_headers) {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return response_with_portability_warning_headers(
                redacted_streaming_error_response(
                    client_format,
                    StatusCode::BAD_GATEWAY,
                    "upstream returned non-SSE response for streaming request",
                    &request_redactor,
                ),
                &portability_warnings,
            );
        }
        let upstream_stream = res.bytes_stream();
        let mut body_stream: Pin<
            Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
        > = if needs_stream_translation(upstream_format, client_format) {
            let stream_capture_candidate = bridge_capture_candidate.clone();
            let response_id_override = stream_capture_candidate
                .as_ref()
                .and_then(|candidate| candidate.local_response_id.clone());
            let translated =
                TranslateSseStream::new(upstream_stream, upstream_format, client_format)
                    .with_resource_limits(namespace_state.config.resource_limits.clone())
                    .with_request_scoped_tool_bridge_context(
                        request_scoped_tool_bridge_context
                            .as_ref()
                            .map(TrustedToolBridgeContext::to_value),
                    )
                    .with_responses_response_id_override(response_id_override);
            if let Some(candidate) = stream_capture_candidate {
                Box::pin(ConversationStateBridgeCaptureStream::new(
                    translated,
                    state.conversation_state_bridge.clone(),
                    candidate,
                ))
            } else {
                Box::pin(translated)
            }
        } else {
            let guarded = GuardedSseStream::new(upstream_stream, client_format)
                .with_resource_limits(namespace_state.config.resource_limits.clone());
            Box::pin(guarded)
        };
        body_stream = Box::pin(RedactingSseStream::new(
            body_stream,
            request_redactor.clone(),
        ));
        if let Some(redactor) = prompt_cache_key_redactor.clone() {
            body_stream = Box::pin(RedactingSseStream::new(body_stream, redactor));
        }
        if let (Some(dispatcher), Some(ctx)) = (namespace_state.hooks.clone(), hook_ctx.clone()) {
            body_stream = Box::pin(dispatcher.wrap_stream(
                body_stream,
                ctx,
                status.as_u16(),
                sse_response_headers(),
            ));
        }
        if let (Some(recorder), Some(ctx)) =
            (namespace_state.debug_trace.as_ref(), debug_ctx.clone())
        {
            body_stream = Box::pin(recorder.wrap_stream(body_stream, ctx, status.as_u16()));
        }
        let body = Body::from_stream(TrackedBodyStream::new(
            body_stream,
            tracker,
            status.as_u16(),
            downstream_cancellation.cancellation_token(),
        ));
        let mut response = Response::builder()
            .status(status)
            .header("Content-Type", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .header("Connection", "keep-alive")
            .body(body)
            .unwrap();
        append_upstream_protocol_response_headers(
            &mut response,
            &upstream_response_headers,
            &response_header_redactor,
        );
        set_openai_model_header(&mut response, client_format, &requested_model);
        return response_with_portability_warning_headers(response, &portability_warnings);
    }

    let status = res.status();
    let upstream_response_headers = res.headers().clone();
    let response_body_limit = if status.is_success() {
        namespace_state
            .config
            .resource_limits
            .max_non_stream_response_bytes
    } else {
        namespace_state
            .config
            .resource_limits
            .max_upstream_error_body_bytes
    };
    let bytes = match upstream::read_response_bytes_limited_with_cancellation(
        res,
        response_body_limit,
        &downstream_cancellation,
    )
    .await
    {
        Ok(b) => b,
        Err(upstream::DownstreamAwareError::Inner(
            upstream::ResponseBodyLimitError::LimitExceeded { limit },
        )) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            let message = if status.is_success() {
                format!("upstream response body exceeded resource limit of {limit} bytes")
            } else {
                format!("upstream error body exceeded resource limit of {limit} bytes")
            };
            return response_with_portability_warning_headers(
                redacted_error_response(
                    client_format,
                    StatusCode::BAD_GATEWAY,
                    &message,
                    &request_redactor,
                ),
                &portability_warnings,
            );
        }
        Err(upstream::DownstreamAwareError::Inner(upstream::ResponseBodyLimitError::Inner(e))) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return response_with_portability_warning_headers(
                redacted_error_response(
                    client_format,
                    StatusCode::BAD_GATEWAY,
                    &e.to_string(),
                    &request_redactor,
                ),
                &portability_warnings,
            );
        }
        Err(upstream::DownstreamAwareError::DownstreamCancelled) => {
            tracker.finish_cancelled();
            return client_closed_response(client_format);
        }
    };
    if llmup.zero_transform_forwarding_active {
        if status.is_success() {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(upstream_body_for_observability) => {
                    let public_out =
                        request_redactor.redact_value(&upstream_body_for_observability);
                    if let (Some(dispatcher), Some(ctx)) =
                        (namespace_state.hooks.as_ref(), hook_ctx.clone())
                    {
                        dispatcher.emit_non_stream(
                            ctx,
                            status.as_u16(),
                            json_response_headers(),
                            public_out.clone(),
                        );
                    }
                    if let (Some(recorder), Some(ctx)) =
                        (namespace_state.debug_trace.as_ref(), debug_ctx.as_ref())
                    {
                        recorder.record_non_stream_response(ctx, status.as_u16(), &public_out);
                    }
                }
                Err(_) => {
                    warn!(
                        "zero-transform upstream response was not valid JSON; forwarding raw body without non-stream response hook/debug capture"
                    );
                }
            }
            tracker.finish_success(status.as_u16());
        } else {
            tracker.finish_error(status.as_u16());
        }
        let forwarded_bytes = if status.is_success() {
            bytes
        } else {
            Bytes::from(redact_zero_transform_error_body(
                &bytes,
                &upstream_response_headers,
                &request_redactor,
            ))
        };
        let body_len = forwarded_bytes.len();
        let mut response = Response::builder()
            .status(status)
            .body(Body::from(forwarded_bytes))
            .unwrap();
        append_raw_upstream_response_headers(
            &mut response,
            &upstream_response_headers,
            body_len,
            &response_header_redactor,
        );
        set_openai_model_header(&mut response, client_format, &requested_model);
        return response_with_portability_warning_headers(response, &portability_warnings);
    }
    if !status.is_success() {
        error!("Upstream returned non-success status: {}", status);
        let redacted_upstream_body = redact_text_with_prompt_cache_key(
            &request_redactor,
            prompt_cache_key_redactor.as_ref(),
            &String::from_utf8_lossy(&bytes),
        );
        error!("Upstream response body: {}", redacted_upstream_body);
        tracker.finish_error(status.as_u16());
        let upstream_error_body = String::from_utf8_lossy(&bytes);
        let public_error_body = if serde_json::from_str::<Value>(&upstream_error_body).is_ok() {
            upstream_error_body.to_string()
        } else {
            format!("upstream error body: {upstream_error_body}")
        };
        let public_error_body = redact_text_with_prompt_cache_key(
            &request_redactor,
            prompt_cache_key_redactor.as_ref(),
            &public_error_body,
        );
        let mut response = error_response(
            client_format,
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            &public_error_body,
        );
        if preserve_native_upstream_protocol_headers {
            append_upstream_protocol_response_headers(
                &mut response,
                &upstream_response_headers,
                &response_header_redactor,
            );
        }
        return response_with_portability_warning_headers(response, &portability_warnings);
    }
    let upstream_body: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            let redacted_upstream_body = redact_text_with_prompt_cache_key(
                &request_redactor,
                prompt_cache_key_redactor.as_ref(),
                &String::from_utf8_lossy(&bytes),
            );
            error!(
                "Upstream returned invalid JSON body: {}",
                redacted_upstream_body
            );
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            return response_with_portability_warning_headers(
                redacted_error_response(
                    client_format,
                    StatusCode::BAD_GATEWAY,
                    "upstream returned invalid JSON",
                    &request_redactor,
                ),
                &portability_warnings,
            );
        }
    };
    if let Some((status, message)) =
        normalized_non_stream_upstream_error(upstream_format, client_format, &upstream_body)
    {
        tracker.finish_error(status.as_u16());
        let message = redact_text_with_prompt_cache_key(
            &request_redactor,
            prompt_cache_key_redactor.as_ref(),
            &message,
        );
        let mut response = error_response(client_format, status, &message);
        if preserve_native_upstream_protocol_headers {
            append_upstream_protocol_response_headers(
                &mut response,
                &upstream_response_headers,
                &response_header_redactor,
            );
        }
        return response_with_portability_warning_headers(response, &portability_warnings);
    }
    let response_translation_context = ResponseTranslationContext::default()
        .with_request_scoped_tool_bridge_context_value(
            request_scoped_tool_bridge_context
                .as_ref()
                .map(TrustedToolBridgeContext::to_value),
        )
        .with_reasoning_echo(resolved_dialect.as_ref().and_then(|d| d.reasoning_echo));
    let mut out = match translate_response_with_context(
        upstream_format,
        client_format,
        &upstream_body,
        response_translation_context,
    ) {
        Ok(v) => v,
        Err(e) => {
            tracker.finish_error(StatusCode::BAD_GATEWAY.as_u16());
            let message = redact_text_with_prompt_cache_key(
                &request_redactor,
                prompt_cache_key_redactor.as_ref(),
                &e,
            );
            return response_with_portability_warning_headers(
                error_response(client_format, StatusCode::BAD_GATEWAY, &message),
                &portability_warnings,
            );
        }
    };
    let response_status = classify_post_translation_non_stream_status(client_format, &out);
    if response_status.is_success() {
        if let Some(candidate) = bridge_capture_candidate {
            match commit_conversation_state_bridge_capture(
                &state.conversation_state_bridge,
                candidate,
                &mut out,
            )
            .await
            {
                Ok(Some(local_id)) => {
                    debug!("conversation_state_bridge captured local response id={local_id}");
                }
                Ok(None) => {}
                Err(message) => {
                    warn!("conversation_state_bridge capture skipped: {message}");
                    portability_warnings.push(message);
                }
            }
        }
    }
    let public_out = redact_value_with_prompt_cache_key(
        &request_redactor,
        prompt_cache_key_redactor.as_ref(),
        &out,
    );
    if let (Some(dispatcher), Some(ctx)) = (namespace_state.hooks.as_ref(), hook_ctx) {
        dispatcher.emit_non_stream(
            ctx,
            response_status.as_u16(),
            json_response_headers(),
            public_out.clone(),
        );
    }
    if let (Some(recorder), Some(ctx)) = (namespace_state.debug_trace.as_ref(), debug_ctx.as_ref())
    {
        recorder.record_non_stream_response(ctx, response_status.as_u16(), &public_out);
    }
    if response_status.is_success() {
        tracker.finish_success(response_status.as_u16());
    } else {
        tracker.finish_error(response_status.as_u16());
    }
    let mut response = Response::builder()
        .status(response_status)
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&public_out).unwrap_or_else(|_| b"{}".to_vec()),
        ))
        .unwrap();
    if preserve_native_upstream_protocol_headers {
        append_upstream_protocol_response_headers(
            &mut response,
            &upstream_response_headers,
            &response_header_redactor,
        );
    }
    set_openai_model_header(&mut response, client_format, &requested_model);
    response_with_portability_warning_headers(response, &portability_warnings)
}

fn response_is_event_stream(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"))
        .unwrap_or(false)
}

fn conversation_state_bridge_can_preload(
    namespace_state: &RuntimeNamespaceState,
    client_format: UpstreamFormat,
    requested_model: &str,
    body: &Value,
) -> bool {
    if client_format != UpstreamFormat::OpenAiResponses {
        return false;
    }
    let Some(previous_response_id) = body.get("previous_response_id").and_then(Value::as_str)
    else {
        return false;
    };
    if !previous_response_id.starts_with(LOCAL_RESPONSE_ID_PREFIX) {
        return false;
    }
    if requested_model.trim().is_empty() {
        return true;
    }

    let Ok(resolved_model) = namespace_state.config.resolve_model(requested_model) else {
        return true;
    };
    let Some(upstream) = namespace_state.upstreams.get(&resolved_model.upstream_name) else {
        return true;
    };
    let fixed_native =
        upstream.config.fixed_upstream_format == Some(UpstreamFormat::OpenAiResponses);
    let discovered_native = upstream
        .capability
        .as_ref()
        .map(|capability| {
            capability.upstream_format_for_request(UpstreamFormat::OpenAiResponses)
                == UpstreamFormat::OpenAiResponses
        })
        .unwrap_or(false);
    !(fixed_native || discovered_native)
}

fn reject_local_bridge_id_on_native_responses_passthrough(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> Option<&'static str> {
    if client_format != UpstreamFormat::OpenAiResponses
        || upstream_format != UpstreamFormat::OpenAiResponses
    {
        return None;
    }

    let previous_response_id = body.get("previous_response_id").and_then(Value::as_str)?;
    if !previous_response_id.starts_with(LOCAL_RESPONSE_ID_PREFIX) {
        return None;
    }

    Some(
        "Responses `previous_response_id` is a local conversation_state_bridge id and cannot be used for provider-native same-wire handling; use a route that can perform local replay/request construction",
    )
}

async fn preload_local_bridge_response(
    store: &ConversationStateBridgeStore,
    body: &Value,
    namespace: &str,
    owner_hash: &str,
) -> Result<StoredBridgeResponse, String> {
    let response_id = body
        .get("previous_response_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "Responses `previous_response_id` is required for local replay".to_string()
        })?;
    if !response_id.starts_with(LOCAL_RESPONSE_ID_PREFIX) {
        return Err(format!(
            "Responses `previous_response_id` `{response_id}` is not an llmup local conversation_state_bridge id"
        ));
    }
    store
        .get(response_id, namespace, owner_hash)
        .await
        .map_err(|error| error.public_message(response_id))
}

#[allow(clippy::too_many_arguments)]
async fn prepare_conversation_state_bridge(
    namespace_state: &RuntimeNamespaceState,
    namespace: &str,
    owner_hash: &str,
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    stream: bool,
    requested_model: &str,
    resolved_model: &ResolvedModel,
    route_config_fingerprint: BridgeRouteConfigFingerprint,
    preloaded_response: Option<StoredBridgeResponse>,
    body: &mut Value,
) -> Result<BridgePreparation, String> {
    if client_format != UpstreamFormat::OpenAiResponses
        || upstream_format == UpstreamFormat::OpenAiResponses
    {
        return Ok(BridgePreparation::default());
    }

    if provider_state_control_enabled(body.get("store")) {
        return Err(responses_store_requires_native_responses_message(
            upstream_format,
        ));
    }
    let store_disabled = matches!(
        body.get("store"),
        Some(Value::Null) | Some(Value::Bool(false))
    );
    let has_previous_response_id = body.get("previous_response_id").is_some();

    if stream && preloaded_response.is_some() {
        return Err(
            "conversation_state_bridge currently supports local `previous_response_id` replay only for non-streaming OpenAI Responses translation"
                .to_string(),
        );
    }

    let mut debug_trace = None;
    let request_items = if let Some(previous) = preloaded_response {
        let stored_item_count = previous.transcript_items.len();
        let current_items = responses_bridge_continuation_items_from_body(body)?;
        let current_item_count = current_items.len();
        validate_bridge_continuation_items(&previous.transcript_items, &current_items)?;
        let mut expanded = previous.transcript_items;
        expanded.extend(current_items);
        let expanded_item_count = expanded.len();
        set_responses_input_items(body, expanded.clone())?;
        remove_local_bridge_state_controls(body);
        debug_trace = Some(ConversationStateBridgeDebugTrace::replay_hit(
            stored_item_count,
            current_item_count,
            expanded_item_count,
        ));
        expanded
    } else if has_previous_response_id {
        return Ok(BridgePreparation::default());
    } else {
        match responses_bridge_input_items_from_body(body) {
            Ok(items) => {
                if bridge_items_include_tool_output(&items) {
                    remove_local_bridge_state_controls(body);
                    return Ok(BridgePreparation::default());
                }
                remove_local_bridge_state_controls(body);
                items
            }
            Err(_) => {
                remove_local_bridge_state_controls(body);
                return Ok(BridgePreparation::default());
            }
        }
    };

    if store_disabled {
        return Ok(BridgePreparation {
            capture_candidate: None,
            debug_trace,
        });
    }

    let max_bytes = namespace_state.config.conversation_state_bridge.max_bytes;
    if debug_trace.is_none() {
        debug_trace = Some(ConversationStateBridgeDebugTrace::capture_candidate(
            request_items.len(),
            max_bytes,
        ));
    }
    Ok(BridgePreparation {
        capture_candidate: Some(BridgeCaptureCandidate {
            namespace: namespace.to_string(),
            owner_hash: owner_hash.to_string(),
            client_model: requested_model.to_string(),
            resolved_model: resolved_model.clone(),
            route_config_fingerprint,
            request_items,
            ttl_seconds: namespace_state.config.conversation_state_bridge.ttl_seconds,
            max_bytes,
            local_response_id: stream.then(ConversationStateBridgeStore::mint_response_id),
        }),
        debug_trace,
    })
}

fn responses_store_requires_native_responses_message(upstream_format: UpstreamFormat) -> String {
    format!(
        "Responses request control `store` requires a native OpenAI Responses upstream and cannot be translated to {upstream_format}; the proxy does not reconstruct provider state"
    )
}

fn conversation_state_bridge_route_config_changed_error() -> &'static str {
    "conversation_state_bridge replay failed closed because route/config owner/config changed for this local previous_response_id"
}

fn conversation_state_bridge_route_config_fingerprint(
    namespace_state: &RuntimeNamespaceState,
    resolved_model: &ResolvedModel,
    upstream_format: UpstreamFormat,
    request_translation_policy: &RequestTranslationPolicy,
) -> BridgeRouteConfigFingerprint {
    BridgeRouteConfigFingerprint::new(
        namespace_state.revision.clone(),
        resolved_model.clone(),
        upstream_format,
        request_translation_policy.surface.clone(),
    )
}

fn remove_local_bridge_state_controls(body: &mut Value) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("previous_response_id");
        obj.remove("store");
    }
}

fn set_responses_input_items(body: &mut Value, items: Vec<Value>) -> Result<(), String> {
    let Some(obj) = body.as_object_mut() else {
        return Err("OpenAI Responses request body must be a JSON object".to_string());
    };
    obj.insert("input".to_string(), Value::Array(items));
    Ok(())
}

/// Continuation items for a local-replay turn. Unlike the first-request path in
/// `prepare_conversation_state_bridge`, a continuation that carries
/// `previous_response_id` may legitimately omit `input` to continue with the
/// stored context only; a missing `input` is treated as an empty item list so a
/// valid no-new-input continuation replays the stored transcript instead of
/// failing with BAD_REQUEST. A present-but-unparseable `input` still errors.
fn responses_bridge_continuation_items_from_body(body: &Value) -> Result<Vec<Value>, String> {
    if body.get("input").is_none() {
        return Ok(Vec::new());
    }
    responses_bridge_input_items_from_body(body)
}

fn responses_bridge_input_items_from_body(body: &Value) -> Result<Vec<Value>, String> {
    let input = body.get("input").ok_or_else(|| {
        "conversation_state_bridge replay requires OpenAI Responses `input`".to_string()
    })?;
    match input {
        Value::String(text) => Ok(vec![responses_text_message_item(
            "user",
            "input_text",
            text,
        )]),
        Value::Array(items) => items
            .iter()
            .map(responses_bridge_input_item)
            .collect::<Result<Vec<_>, _>>(),
        _ => Err("conversation_state_bridge replay only supports text OpenAI Responses `input` or portable input items".to_string()),
    }
}

fn responses_bridge_input_item(item: &Value) -> Result<Value, String> {
    match responses_bridge_item_type(item) {
        Some("message") => responses_text_input_item(item),
        Some("function_call_output") => responses_function_call_output_item(item),
        Some("custom_tool_call_output") => responses_custom_tool_call_output_item(item),
        Some("function_call" | "custom_tool_call") => Err(
            "conversation_state_bridge memory replay only accepts assistant `function_call` or `custom_tool_call` items from captured upstream output"
                .to_string(),
        ),
        _ => Err(
            "conversation_state_bridge replay only supports text message, `function_call_output`, and `custom_tool_call_output` input items"
                .to_string(),
        ),
    }
}

fn responses_text_input_item(item: &Value) -> Result<Value, String> {
    let item_type = responses_bridge_item_type(item);
    if item_type != Some("message") {
        return Err(
            "conversation_state_bridge MVP only replays text message input items".to_string(),
        );
    }
    let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
    if !matches!(role, "user" | "assistant") {
        return Err(
            "conversation_state_bridge MVP only replays user/assistant text messages".to_string(),
        );
    }
    let text_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    let text = responses_text_content(item.get("content"))?;
    Ok(responses_text_message_item(role, text_type, &text))
}

fn responses_text_content(content: Option<&Value>) -> Result<String, String> {
    match content {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(Value::Array(parts)) => {
            let mut text = String::new();
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text") => {
                        if part
                            .get("annotations")
                            .and_then(Value::as_array)
                            .is_some_and(|annotations| !annotations.is_empty())
                        {
                            return Err(
                                "conversation_state_bridge MVP only supports plain text content"
                                    .to_string(),
                            );
                        }
                        text.push_str(part.get("text").and_then(Value::as_str).unwrap_or(""));
                    }
                    Some("refusal") => {
                        text.push_str(part.get("refusal").and_then(Value::as_str).unwrap_or(""));
                    }
                    _ => {
                        return Err(
                            "conversation_state_bridge MVP only supports text content".to_string()
                        );
                    }
                }
            }
            Ok(text)
        }
        _ => Err("conversation_state_bridge MVP only supports text content".to_string()),
    }
}

fn responses_text_message_item(role: &str, text_type: &str, text: &str) -> Value {
    serde_json::json!({
        "type": "message",
        "role": role,
        "content": [{ "type": text_type, "text": text }]
    })
}

fn responses_bridge_item_type(item: &Value) -> Option<&str> {
    item.get("type")
        .and_then(Value::as_str)
        .or_else(|| item.get("role").and_then(Value::as_str).map(|_| "message"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BridgeToolCallKind {
    Function,
    Custom,
}

impl BridgeToolCallKind {
    fn call_item_type(self) -> &'static str {
        match self {
            Self::Function => "function_call",
            Self::Custom => "custom_tool_call",
        }
    }

    fn output_item_type(self) -> &'static str {
        match self {
            Self::Function => "function_call_output",
            Self::Custom => "custom_tool_call_output",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PendingBridgeToolCall {
    call_id: String,
    kind: BridgeToolCallKind,
}

fn responses_function_call_output_item(item: &Value) -> Result<Value, String> {
    responses_tool_call_output_item(item, "function_call_output", false)
}

fn responses_custom_tool_call_output_item(item: &Value) -> Result<Value, String> {
    responses_tool_call_output_item(item, "custom_tool_call_output", true)
}

fn responses_tool_call_output_item(
    item: &Value,
    item_type: &str,
    text_only_output: bool,
) -> Result<Value, String> {
    if item.get("namespace").is_some() {
        return Err(format!(
            "conversation_state_bridge memory replay does not support namespaced `{item_type}` items"
        ));
    }
    if item.get("proxied_tool_kind").is_some() {
        return Err(format!(
            "conversation_state_bridge memory replay does not support proxied `{item_type}` replay"
        ));
    }
    let call_id = non_empty_string_field(item, "call_id", item_type)?;
    let output = item.get("output").ok_or_else(|| {
        format!("conversation_state_bridge memory replay requires `{item_type}.output`")
    })?;
    let output = responses_portable_tool_output(output, item_type, text_only_output)?;
    Ok(serde_json::json!({
        "type": item_type,
        "call_id": call_id,
        "output": output
    }))
}

fn responses_tool_call_output_call_id(item: &Value) -> Result<&str, String> {
    let item_type = responses_bridge_item_type(item).unwrap_or("tool_call_output");
    non_empty_string_field(item, "call_id", item_type)
}

fn bridge_tool_call_kind_for_item_type(item_type: &str) -> Option<BridgeToolCallKind> {
    match item_type {
        "function_call" => Some(BridgeToolCallKind::Function),
        "custom_tool_call" => Some(BridgeToolCallKind::Custom),
        _ => None,
    }
}

fn bridge_tool_output_kind_for_item_type(item_type: &str) -> Option<BridgeToolCallKind> {
    match item_type {
        "function_call_output" => Some(BridgeToolCallKind::Function),
        "custom_tool_call_output" => Some(BridgeToolCallKind::Custom),
        _ => None,
    }
}

fn responses_bridge_tool_output_kind(item: &Value) -> Option<BridgeToolCallKind> {
    responses_bridge_item_type(item).and_then(bridge_tool_output_kind_for_item_type)
}

fn bridge_items_include_tool_output(items: &[Value]) -> bool {
    items
        .iter()
        .any(|item| responses_bridge_tool_output_kind(item).is_some())
}

fn first_bridge_tool_output_type(items: &[Value]) -> &'static str {
    items
        .iter()
        .find_map(|item| {
            responses_bridge_tool_output_kind(item).map(|kind| kind.output_item_type())
        })
        .unwrap_or("tool_call_output")
}

fn validate_bridge_continuation_items(
    stored_items: &[Value],
    current_items: &[Value],
) -> Result<(), String> {
    let pending = pending_bridge_tool_calls(stored_items)?;
    let current_output_count = current_items
        .iter()
        .filter(|item| responses_bridge_tool_output_kind(item).is_some())
        .count();

    if pending.is_empty() {
        if current_output_count == 0 {
            return Ok(());
        }
        let output_type = first_bridge_tool_output_type(current_items);
        let call_type = bridge_tool_output_kind_for_item_type(output_type)
            .map(BridgeToolCallKind::call_item_type)
            .unwrap_or("tool_call");
        return Err(format!(
            "conversation_state_bridge replay received `{output_type}` but the local previous_response_id has no pending `{call_type}`"
        ));
    }

    if current_items.len() < pending.len() {
        return Err(format!(
            "conversation_state_bridge replay has pending tool calls {pending:?}; the continuation submitted {current_output_count} tool output item(s)"
        ));
    }

    let pending_set = pending.iter().cloned().collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    for item in current_items.iter().take(pending.len()) {
        let Some(kind) = responses_bridge_tool_output_kind(item) else {
            return Err(format!(
                "conversation_state_bridge replay has pending tool calls {:?}; the continuation must begin with matching {} item(s)",
                pending,
                bridge_pending_output_type_summary(&pending)
            ));
        };
        let output_type = kind.output_item_type();
        let call_id = responses_tool_call_output_call_id(item)?;
        let pending_call = PendingBridgeToolCall {
            call_id: call_id.to_string(),
            kind,
        };
        if !seen.insert(pending_call.clone()) {
            return Err(format!(
                "conversation_state_bridge replay received duplicate `{output_type}` for pending call_id `{call_id}`"
            ));
        }
        if !pending_set.contains(&pending_call) {
            if let Some(expected) = pending.iter().find(|pending| pending.call_id == call_id) {
                return Err(format!(
                    "conversation_state_bridge replay received `{output_type}` for call_id `{call_id}`, but pending `{}` requires `{}`",
                    expected.kind.call_item_type(),
                    expected.kind.output_item_type()
                ));
            }
            return Err(format!(
                "conversation_state_bridge replay received `{output_type}` for call_id `{call_id}`, which does not match pending local tool call(s) {pending:?}"
            ));
        }
    }

    for item in current_items.iter().skip(pending.len()) {
        if let Some(kind) = responses_bridge_tool_output_kind(item) {
            let output_type = kind.output_item_type();
            let call_id = responses_tool_call_output_call_id(item)?;
            return Err(format!(
                "conversation_state_bridge replay received extra `{output_type}` for call_id `{call_id}` after the pending output prefix"
            ));
        }
        if responses_bridge_item_type(item) != Some("message") {
            return Err(
                "conversation_state_bridge replay only allows text message input after pending tool output prefix"
                    .to_string(),
            );
        }
    }

    if let Some(missing) = pending.iter().find(|pending| !seen.contains(*pending)) {
        return Err(format!(
            "conversation_state_bridge replay is missing required `{}` for pending call_id `{}`",
            missing.kind.output_item_type(),
            missing.call_id
        ));
    }
    Ok(())
}

fn bridge_pending_output_type_summary(pending: &[PendingBridgeToolCall]) -> String {
    let output_types = pending
        .iter()
        .map(|pending| pending.kind.output_item_type())
        .collect::<BTreeSet<_>>();
    output_types
        .into_iter()
        .map(|item_type| format!("`{item_type}`"))
        .collect::<Vec<_>>()
        .join(" or ")
}

fn pending_bridge_tool_calls(items: &[Value]) -> Result<Vec<PendingBridgeToolCall>, String> {
    let mut pending = Vec::new();
    for item in items {
        match responses_bridge_item_type(item) {
            Some("function_call" | "custom_tool_call") => {
                let item_type = responses_bridge_item_type(item).unwrap_or("tool_call");
                let kind = bridge_tool_call_kind_for_item_type(item_type).unwrap();
                let call_id = non_empty_string_field(item, "call_id", item_type)?;
                let pending_call = PendingBridgeToolCall {
                    call_id: call_id.to_string(),
                    kind,
                };
                if pending.iter().any(|pending| pending == &pending_call) {
                    return Err(format!(
                        "conversation_state_bridge replay state contains duplicate pending {item_type} call_id `{call_id}`"
                    ));
                }
                pending.push(pending_call);
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                let item_type = responses_bridge_item_type(item).unwrap_or("tool_call_output");
                let kind = bridge_tool_output_kind_for_item_type(item_type).unwrap();
                let call_id = responses_tool_call_output_call_id(item)?;
                if !pending.iter().any(|pending| pending.call_id == call_id) {
                    return Err(format!(
                        "conversation_state_bridge replay state contains `{item_type}` for unknown call_id `{call_id}`"
                    ));
                }
                let Some(index) = pending
                    .iter()
                    .position(|pending| pending.call_id == call_id && pending.kind == kind)
                else {
                    let expected = pending.iter().find(|pending| pending.call_id == call_id);
                    let expected_output_type = expected
                        .map(|pending| pending.kind.output_item_type())
                        .unwrap_or("tool_call_output");
                    return Err(format!(
                        "conversation_state_bridge replay state contains `{item_type}` for call_id `{call_id}`, but pending call requires `{expected_output_type}`"
                    ));
                };
                pending.remove(index);
            }
            Some("message") => {}
            Some("reasoning") => {}
            Some(other) => {
                return Err(format!(
                    "conversation_state_bridge replay state contains unsupported `{other}` item"
                ));
            }
            None => {
                return Err(
                    "conversation_state_bridge replay state contains an input item without `type`"
                        .to_string(),
                );
            }
        }
    }
    Ok(pending)
}

fn responses_portable_tool_output(
    output: &Value,
    item_type: &str,
    text_only_output: bool,
) -> Result<Value, String> {
    match output {
        Value::String(_) => Ok(output.clone()),
        Value::Array(items) => {
            let mut portable = Vec::with_capacity(items.len());
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text") => {
                        portable.push(serde_json::json!({
                            "type": item.get("type").and_then(Value::as_str).unwrap_or("input_text"),
                            "text": item.get("text").and_then(Value::as_str).unwrap_or("")
                        }));
                    }
                    Some(other) => {
                        return Err(format!(
                            "conversation_state_bridge memory replay cannot store `{item_type}.output` array item type `{other}`"
                        ));
                    }
                    None => {
                        return Err(format!(
                            "conversation_state_bridge memory replay requires typed text items in `{item_type}.output` arrays"
                        ));
                    }
                }
            }
            Ok(Value::Array(portable))
        }
        _ if text_only_output => Err(format!(
            "conversation_state_bridge memory replay requires `{item_type}.output` to be a string or text array"
        )),
        _ => Ok(output.clone()),
    }
}

fn non_empty_string_field<'a>(
    item: &'a Value,
    field: &str,
    item_type: &str,
) -> Result<&'a str, String> {
    item.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!("conversation_state_bridge memory replay requires `{item_type}.{field}`")
        })
}

async fn commit_conversation_state_bridge_capture(
    store: &ConversationStateBridgeStore,
    candidate: BridgeCaptureCandidate,
    response: &mut Value,
) -> Result<Option<String>, String> {
    if response.get("status").and_then(Value::as_str) != Some("completed") {
        return Ok(None);
    }
    let output_items = responses_bridge_output_items_from_response(response)?;
    if output_items.is_empty() && candidate.local_response_id.is_none() {
        return Ok(None);
    }

    let mut transcript_items = candidate.request_items;
    transcript_items.extend(output_items);
    let entry = StoredBridgeResponse::new(
        candidate.namespace,
        candidate.owner_hash,
        candidate.client_model,
        candidate.resolved_model,
        candidate.route_config_fingerprint,
        transcript_items,
    );
    let ttl = Duration::from_secs(candidate.ttl_seconds);
    let local_id = if let Some(local_response_id) = candidate.local_response_id {
        store
            .put_with_id(local_response_id, entry, ttl, candidate.max_bytes)
            .await?
    } else {
        store.put(entry, ttl, candidate.max_bytes).await?
    };
    if let Some(obj) = response.as_object_mut() {
        obj.insert("id".to_string(), Value::String(local_id.clone()));
    }
    Ok(Some(local_id))
}

fn responses_bridge_output_items_from_response(response: &Value) -> Result<Vec<Value>, String> {
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "conversation_state_bridge capture requires OpenAI Responses output array".to_string()
        })?;
    let mut items = Vec::with_capacity(output.len());
    for item in output {
        if let Some(item) = responses_bridge_output_item(item)? {
            items.push(item);
        }
    }
    Ok(items)
}

fn responses_bridge_output_item(item: &Value) -> Result<Option<Value>, String> {
    match responses_bridge_item_type(item) {
        Some("message") => responses_text_output_item(item).map(Some),
        Some("reasoning") => responses_reasoning_output_item(item),
        Some("function_call") => responses_function_call_item(item).map(Some),
        Some("custom_tool_call") => responses_custom_tool_call_item(item).map(Some),
        _ => Err(
            "conversation_state_bridge memory capture only supports assistant text message output, visible `reasoning` summary, `function_call`, and `custom_tool_call` output"
                .to_string(),
        ),
    }
}

fn responses_text_output_item(item: &Value) -> Result<Value, String> {
    if item.get("type").and_then(Value::as_str) != Some("message") {
        return Err(
            "conversation_state_bridge MVP only captures assistant text message output".to_string(),
        );
    }
    let role = item
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");
    if role != "assistant" {
        return Err(
            "conversation_state_bridge MVP only captures assistant text message output".to_string(),
        );
    }
    let text = responses_text_content(item.get("content"))?;
    Ok(responses_text_message_item(
        "assistant",
        "output_text",
        &text,
    ))
}

fn responses_reasoning_output_item(item: &Value) -> Result<Option<Value>, String> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return Err(
            "conversation_state_bridge memory capture only captures `reasoning` output items"
                .to_string(),
        );
    }
    let Some(summary) = item.get("summary").and_then(Value::as_array) else {
        return Ok(None);
    };
    let summary = summary
        .iter()
        .filter_map(|part| {
            if part.get("type").and_then(Value::as_str) != Some("summary_text") {
                return None;
            }
            let text = part.get("text").and_then(Value::as_str)?;
            Some(serde_json::json!({
                "type": "summary_text",
                "text": text
            }))
        })
        .collect::<Vec<_>>();
    if summary.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::json!({
        "type": "reasoning",
        "summary": summary
    })))
}

fn responses_function_call_item(item: &Value) -> Result<Value, String> {
    if item.get("namespace").is_some() {
        return Err(
            "conversation_state_bridge memory capture does not support namespaced `function_call` output"
                .to_string(),
        );
    }
    if item.get("proxied_tool_kind").is_some() {
        return Err(
            "conversation_state_bridge memory capture does not support proxied tool call replay"
                .to_string(),
        );
    }
    let call_id = non_empty_string_field(item, "call_id", "function_call")?;
    let name = non_empty_string_field(item, "name", "function_call")?;
    let arguments = non_empty_string_field(item, "arguments", "function_call")?;
    validate_json_object_arguments(arguments)?;
    Ok(serde_json::json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments
    }))
}

fn responses_custom_tool_call_item(item: &Value) -> Result<Value, String> {
    if item.get("namespace").is_some() {
        return Err(
            "conversation_state_bridge memory capture does not support namespaced `custom_tool_call` output"
                .to_string(),
        );
    }
    if item.get("proxied_tool_kind").is_some() {
        return Err(
            "conversation_state_bridge memory capture does not support proxied custom tool call replay"
                .to_string(),
        );
    }
    let call_id = non_empty_string_field(item, "call_id", "custom_tool_call")?;
    let name = non_empty_string_field(item, "name", "custom_tool_call")?;
    let input = item.get("input").and_then(Value::as_str).ok_or_else(|| {
        "conversation_state_bridge memory capture requires `custom_tool_call.input`".to_string()
    })?;
    Ok(serde_json::json!({
        "type": "custom_tool_call",
        "call_id": call_id,
        "name": name,
        "input": input
    }))
}

fn validate_json_object_arguments(arguments: &str) -> Result<(), String> {
    let value: Value = serde_json::from_str(arguments).map_err(|error| {
        format!(
            "conversation_state_bridge memory capture requires `function_call.arguments` to be valid JSON object text: {error}"
        )
    })?;
    if !value.is_object() {
        return Err(
            "conversation_state_bridge memory capture requires `function_call.arguments` to decode to a JSON object"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn classify_request_boundary(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> RequestBoundaryDecision {
    classify_request_boundary_with_policy(
        client_format,
        upstream_format,
        body,
        &RequestTranslationPolicy {
            surface: crate::config::ModelSurface::default(),
        },
        "",
        None,
    )
}

fn classify_request_boundary_with_policy(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
    policy: &RequestTranslationPolicy,
    resolved_upstream_model: &str,
    dialect: Option<&crate::config::DialectBlock>,
) -> RequestBoundaryDecision {
    match assess_request_translation_with_surface(
        client_format,
        upstream_format,
        body,
        &policy.surface,
        resolved_upstream_model,
        dialect,
    )
    .decision()
    {
        TranslationDecision::Allow => RequestBoundaryDecision::Allow,
        TranslationDecision::AllowWithWarnings(warnings) => {
            RequestBoundaryDecision::AllowWithWarnings(warnings)
        }
        TranslationDecision::Reject(message) => RequestBoundaryDecision::Reject(message),
    }
}

pub(super) fn resolve_requested_model_or_error(
    namespace_config: &crate::config::Config,
    requested_model: &str,
    client_format: UpstreamFormat,
    body: &Value,
) -> Result<crate::config::ResolvedModel, String> {
    if requested_model.trim().is_empty() && namespace_config.upstreams.len() > 1 {
        if client_format == UpstreamFormat::OpenAiResponses
            && body.get("previous_response_id").is_some()
        {
            return Err(
                "Responses requests with `previous_response_id` must also include a routable `model` when this namespace has multiple upstreams; the proxy does not reconstruct response-to-upstream state"
                    .to_string(),
            );
        }

        return Err(
            "request must include a routable `model` when this namespace has multiple upstreams; use `upstream:model` or configure `model_aliases`"
                .to_string(),
        );
    }

    namespace_config.resolve_model(requested_model)
}

fn resolve_request_model_or_error(
    namespace_state: &RuntimeNamespaceState,
    requested_model: &str,
    client_format: UpstreamFormat,
    body: &Value,
) -> Result<crate::config::ResolvedModel, String> {
    if let Some(resolved) = resolve_native_responses_stateful_route_or_error(
        namespace_state,
        requested_model,
        client_format,
        body,
    )? {
        return Ok(resolved);
    }

    resolve_requested_model_or_error(
        &namespace_state.config,
        requested_model,
        client_format,
        body,
    )
}

fn request_translation_policy(
    namespace_config: &crate::config::Config,
    requested_model: &str,
    resolved_model: &crate::config::ResolvedModel,
) -> RequestTranslationPolicy {
    let surface = namespace_config
        .model_aliases
        .get(requested_model)
        .map(|alias| namespace_config.effective_model_surface(alias))
        .unwrap_or_else(|| {
            namespace_config.effective_model_surface(&crate::config::ModelAlias {
                upstream_name: resolved_model.upstream_name.clone(),
                upstream_model: resolved_model.upstream_model.clone(),
                limits: None,
                surface: None,
            })
        });

    RequestTranslationPolicy { surface }
}

pub(super) fn request_translation_policy_requires_body_mutation(
    target_format: UpstreamFormat,
    body: &Value,
    policy: &RequestTranslationPolicy,
) -> bool {
    request_translation_policy_default_output_limit_would_apply(target_format, body, policy)
        || request_translation_policy_parallel_tool_gate_would_apply(target_format, body, policy)
}

fn request_translation_policy_default_output_limit_would_apply(
    target_format: UpstreamFormat,
    body: &Value,
    policy: &RequestTranslationPolicy,
) -> bool {
    policy
        .surface
        .limits
        .as_ref()
        .and_then(|limits| limits.max_output_tokens)
        .is_some()
        && !request_body_has_explicit_output_limit(target_format, body)
}

fn request_translation_policy_parallel_tool_gate_would_apply(
    target_format: UpstreamFormat,
    body: &Value,
    policy: &RequestTranslationPolicy,
) -> bool {
    policy
        .surface
        .tools
        .as_ref()
        .and_then(|tools| tools.supports_parallel_calls)
        == Some(false)
        && !request_body_has_explicit_parallel_tool_calls_preference(target_format, body)
        && request_body_has_tool_definitions(target_format, body)
}

fn request_body_has_explicit_output_limit(target_format: UpstreamFormat, body: &Value) -> bool {
    let Some(obj) = body.as_object() else {
        return false;
    };

    match target_format {
        UpstreamFormat::Anthropic => obj.get("max_tokens").is_some(),
        UpstreamFormat::OpenAiChatCompletions => {
            obj.get("max_completion_tokens").is_some() || obj.get("max_tokens").is_some()
        }
        UpstreamFormat::OpenAiResponses => obj.get("max_output_tokens").is_some(),
    }
}

fn request_body_has_explicit_parallel_tool_calls_preference(
    target_format: UpstreamFormat,
    body: &Value,
) -> bool {
    match target_format {
        UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::OpenAiResponses => body
            .get("parallel_tool_calls")
            .and_then(Value::as_bool)
            .is_some(),
        UpstreamFormat::Anthropic => body
            .get("tool_choice")
            .and_then(Value::as_object)
            .and_then(|tool_choice| tool_choice.get("disable_parallel_tool_use"))
            .and_then(Value::as_bool)
            .is_some(),
    }
}

fn request_body_has_tool_definitions(target_format: UpstreamFormat, body: &Value) -> bool {
    match target_format {
        UpstreamFormat::OpenAiChatCompletions
        | UpstreamFormat::OpenAiResponses
        | UpstreamFormat::Anthropic => body
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty()),
    }
}

#[cfg(test)]
mod zero_transform_redaction_tests {
    use super::redact_zero_transform_error_body;
    use bytes::Bytes;

    fn redactor_with_secret(secret: &str) -> crate::server::secret_redaction::SecretRedactor {
        crate::server::secret_redaction::SecretRedactor::new([secret.to_string()])
    }

    #[test]
    fn redacts_known_secret_in_zero_transform_error_body() {
        let provider_key = "sk-test-provider-key-123";
        let redactor = redactor_with_secret(provider_key);
        let body = format!(
            "{{\"error\":{{\"message\":\"diagnostic: rejected key {provider_key} upstream\"}}}}"
        );
        let bytes = Bytes::from(body);
        let headers = reqwest::header::HeaderMap::new();

        let redacted = redact_zero_transform_error_body(&bytes, &headers, &redactor);
        let text = String::from_utf8(redacted).expect("utf8 body");

        assert!(!text.contains(provider_key), "provider key leaked: {text}");
        assert!(
            text.contains("[REDACTED]"),
            "expected redaction marker: {text}"
        );
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(
            value["error"]["message"],
            "diagnostic: rejected key [REDACTED] upstream"
        );
    }

    #[test]
    fn leaves_secret_free_zero_transform_error_body_unchanged() {
        let redactor = redactor_with_secret("sk-test-provider-key-123");
        let body = "{\"error\":{\"message\":\"model not found\"}}";
        let bytes = Bytes::from(body);
        let headers = reqwest::header::HeaderMap::new();

        let redacted = redact_zero_transform_error_body(&bytes, &headers, &redactor);

        assert_eq!(redacted.as_slice(), body.as_bytes());
    }

    #[test]
    fn skips_redaction_when_content_encoding_is_present() {
        let provider_key = "sk-test-provider-key-123";
        let redactor = redactor_with_secret(provider_key);
        let body = format!("error body echoes {provider_key}");
        let bytes = Bytes::from(body);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_ENCODING,
            reqwest::header::HeaderValue::from_static("gzip"),
        );

        let forwarded = redact_zero_transform_error_body(&bytes, &headers, &redactor);

        assert_eq!(forwarded.as_slice(), bytes.as_ref());
    }
}
