#[path = "common/forward_proxy.rs"]
mod forward_proxy;
#[path = "common/runtime_proxy.rs"]
mod runtime_proxy;

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json as AxumJson, Router,
};
use forward_proxy::spawn_http_forward_proxy;
use llm_universal_proxy::config::{
    Config, DebugTraceConfig, ModelAlias, ProxyConfig, RuntimeConfigPayload, UpstreamConfig,
};
use llm_universal_proxy::formats::UpstreamFormat;
use reqwest::{
    header::{HeaderMap as ReqwestHeaderMap, HeaderValue, CONTENT_TYPE},
    Client,
};
use runtime_proxy::{start_proxy, upstream_api_root};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

static UPSTREAM_PROXY_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
const TEST_PROVIDER_KEY: &str = "provider-secret";
const RAW_RESPONSE_CONTENT_TYPE: &str = "application/json; charset=utf-8; boundary=raw-canary";
const RAW_CHAT_REQUEST: &str = r#"{
  "model": "gpt-4o-mini",
  "messages": [
    { "role": "user", "content": "ping" }
  ],
  "prompt_cache_key": "stable-prefix",
  "temperature": 1e0
}"#;
const RAW_CHAT_STREAM_REQUEST: &str = r#"{
  "model": "gpt-4o-mini",
  "messages": [
    { "role": "user", "content": "ping" }
  ],
  "prompt_cache_key": "stable-prefix",
  "temperature": 1e0,
  "stream": true
}"#;
const RAW_RESPONSES_REQUEST: &str = r#"{
  "model": "gpt-4.1",
  "input": [
    {
      "role": "user",
      "content": [
        { "type": "input_text", "text": "ping" }
      ]
    }
  ],
  "prompt_cache_key": "responses-prefix",
  "temperature": 1e0
}"#;
const RAW_RESPONSES_STREAM_REQUEST: &str = r#"{
  "model": "gpt-4.1",
  "input": [
    {
      "role": "user",
      "content": [
        { "type": "input_text", "text": "ping" }
      ]
    }
  ],
  "prompt_cache_key": "responses-prefix",
  "temperature": 1e0,
  "stream": true
}"#;
const RAW_ANTHROPIC_REQUEST: &str = r#"{
  "model": "claude-3-5-sonnet",
  "max_tokens": 0,
  "messages": [
    {
      "role": "user",
      "content": [
        {
          "type": "text",
          "text": "ping",
          "cache_control": { "type": "ephemeral" }
        }
      ]
    }
  ],
  "temperature": 1e0
}"#;
const RAW_ANTHROPIC_STREAM_REQUEST: &str = r#"{
  "model": "claude-3-5-sonnet",
  "max_tokens": 0,
  "messages": [
    {
      "role": "user",
      "content": [
        {
          "type": "text",
          "text": "ping",
          "cache_control": { "type": "ephemeral" }
        }
      ]
    }
  ],
  "temperature": 1e0,
  "stream": true
}"#;
const RAW_CHAT_SUCCESS_RESPONSE: &str = r#"{
  "id" : "chatcmpl_raw_success",
  "object" : "chat.completion",
  "created" : 123,
  "model" : "gpt-4o-mini",
  "x_unknown" : { "field_order" : ["b", "a"], "scientific" : 1e0, "decimal" : 1.2300 },
  "choices" : [
    {
      "finish_reason" : "stop",
      "index" : 0,
      "message" : { "content" : "raw ok", "role" : "assistant", "extra" : "kept" }
    }
  ],
  "usage" : { "completion_tokens" : 1, "prompt_tokens" : 1, "total_tokens" : 2 }
}
"#;
const RAW_RESPONSES_SUCCESS_RESPONSE: &str = r#"{
  "id" : "resp_raw_success",
  "object" : "response",
  "created_at" : 123,
  "model" : "gpt-4.1",
  "x_unknown" : { "field_order" : ["b", "a"], "scientific" : 1e0, "decimal" : 1.2300 },
  "output" : [
    {
      "id" : "msg_raw",
      "type" : "message",
      "role" : "assistant",
      "content" : [
        { "type" : "output_text", "text" : "raw ok", "extra" : "kept" }
      ]
    }
  ],
  "usage" : { "input_tokens" : 1, "output_tokens" : 1, "total_tokens" : 2 }
}
"#;
const RAW_ANTHROPIC_SUCCESS_RESPONSE: &str = r#"{
  "id" : "msg_raw_success",
  "type" : "message",
  "role" : "assistant",
  "content" : [
    { "type" : "text", "text" : "raw ok", "extra" : "kept" }
  ],
  "model" : "claude-3-5-sonnet",
  "stop_reason" : "end_turn",
  "stop_sequence" : null,
  "x_unknown" : { "field_order" : ["b", "a"], "scientific" : 1e0, "decimal" : 1.2300 },
  "usage" : { "input_tokens" : 1, "output_tokens" : 1 }
}
"#;
const RAW_OPENAI_ERROR_RESPONSE: &str = r#"{
  "error" : {
    "message" : "raw provider error",
    "type" : "rate_limit_exceeded",
    "param" : null,
    "code" : "rate_limit",
    "x_unknown" : { "scientific" : 1e0, "decimal" : 1.2300 }
  },
  "top_unknown" : ["b", "a"]
}
"#;
const RAW_ANTHROPIC_ERROR_RESPONSE: &str = r#"{
  "type" : "error",
  "error" : {
    "type" : "rate_limit_error",
    "message" : "raw provider error",
    "x_unknown" : { "scientific" : 1e0, "decimal" : 1.2300 }
  },
  "top_unknown" : ["b", "a"]
}
"#;
const RAW_SSE_CONTENT_TYPE: &str = "text/event-stream; charset=utf-8; x-raw-canary=phase-3b";
const RAW_UPSTREAM_HOP_BY_HOP_HEADER: &str = "x-llmup-upstream-hop-by-hop-canary";
const RAW_UPSTREAM_HOP_BY_HOP_VALUE: &str = "phase-3b-hop-by-hop-must-not-forward";
const RAW_UPSTREAM_KEEP_ALIVE_VALUE: &str = "timeout=17; phase-3b-hop-by-hop=1";
const RAW_CHAT_STREAM_SUCCESS_RESPONSE: &str = concat!(
    ": chat comment canary scientific=1e0 decimal=1.2300\n",
    "id: chat-evt-1\n",
    "event: chat.completion.chunk\n",
    "retry: 1234\n",
    "data: { \"id\" : \"chatcmpl_sse_raw\", \"object\" : \"chat.completion.chunk\", \"created\" : 123, \"model\" : \"gpt-4o-mini\", \"choices\" : [ { \"index\" : 0, \"delta\" : { \"content\" : \"raw ok\", \"x_unknown\" : { \"scientific\" : 1e0, \"decimal\" : 1.2300 } }, \"finish_reason\" : null } ], \"x_top\" : [\"b\", \"a\"] }\n",
    "\n",
    "\n",
    ": heartbeat\n",
    "\n",
    "id: chat-usage\n",
    "event: completion.usage\n",
    "data: { \"id\" : \"chatcmpl_sse_raw\", \"object\" : \"chat.completion.chunk\", \"choices\" : [], \"usage\" : { \"prompt_tokens\" : 1, \"completion_tokens\" : 1, \"total_tokens\" : 2 }, \"x_usage_unknown\" : true }\n",
    "\n",
    "data: [DONE]\n",
    "\n",
);
const RAW_RESPONSES_STREAM_SUCCESS_RESPONSE: &str = concat!(
    ": responses comment canary scientific=1e0 decimal=1.2300\n",
    "id: resp-created\n",
    "event: response.created\n",
    "retry: 2345\n",
    "data: { \"type\" : \"response.created\", \"response\" : { \"id\" : \"resp_sse_raw\", \"object\" : \"response\", \"created_at\" : 123, \"model\" : \"gpt-4.1\", \"x_unknown\" : { \"scientific\" : 1e0, \"decimal\" : 1.2300 } } }\n",
    "\n",
    ": heartbeat\n",
    "\n",
    "id: resp-delta\n",
    "event: response.output_text.delta\n",
    "data: { \"type\" : \"response.output_text.delta\", \"item_id\" : \"msg_raw\", \"output_index\" : 0, \"content_index\" : 0, \"delta\" : \"raw ok\", \"x_order\" : [\"b\", \"a\"] }\n",
    "\n",
    "id: resp-completed\n",
    "event: response.completed\n",
    "data: { \"type\" : \"response.completed\", \"response\" : { \"id\" : \"resp_sse_raw\", \"object\" : \"response\", \"status\" : \"completed\", \"usage\" : { \"input_tokens\" : 1, \"output_tokens\" : 1, \"total_tokens\" : 2 }, \"x_usage\" : { \"scientific\" : 1e0, \"decimal\" : 1.2300 } } }\n",
    "\n",
);
const RAW_ANTHROPIC_STREAM_SUCCESS_RESPONSE: &str = concat!(
    ": anthropic comment canary scientific=1e0 decimal=1.2300\n",
    "id: msg-start\n",
    "event: message_start\n",
    "retry: 3456\n",
    "data: { \"type\" : \"message_start\", \"message\" : { \"id\" : \"msg_sse_raw\", \"type\" : \"message\", \"role\" : \"assistant\", \"model\" : \"claude-3-5-sonnet\", \"content\" : [], \"stop_reason\" : null, \"stop_sequence\" : null, \"usage\" : { \"input_tokens\" : 1, \"output_tokens\" : 0 }, \"x_unknown\" : { \"scientific\" : 1e0, \"decimal\" : 1.2300 } } }\n",
    "\n",
    "\n",
    ": heartbeat\n",
    "\n",
    "id: content-delta\n",
    "event: content_block_delta\n",
    "data: { \"type\" : \"content_block_delta\", \"index\" : 0, \"delta\" : { \"type\" : \"text_delta\", \"text\" : \"raw ok\", \"x_order\" : [\"b\", \"a\"] } }\n",
    "\n",
    "id: usage-delta\n",
    "event: message_delta\n",
    "data: { \"type\" : \"message_delta\", \"delta\" : { \"stop_reason\" : \"end_turn\", \"stop_sequence\" : null }, \"usage\" : { \"output_tokens\" : 1 }, \"x_usage\" : { \"scientific\" : 1e0, \"decimal\" : 1.2300 } }\n",
    "\n",
    "event: message_stop\n",
    "data: { \"type\" : \"message_stop\" }\n",
    "\n",
);
const RAW_OPENAI_CHAT_INTERNAL_ARTIFACT_STREAM_RESPONSE: &str = concat!(
    ": mutation-required path must not forward this raw fixture\n",
    "id: reserved-tool\n",
    "event: chat.completion.chunk\n",
    "data: { \"id\" : \"chatcmpl_reserved\", \"object\" : \"chat.completion.chunk\", \"created\" : 123, \"model\" : \"gpt-4o-mini\", \"_llmup_tool_bridge_context\" : { \"leak\" : true }, \"choices\" : [ { \"index\" : 0, \"delta\" : { \"tool_calls\" : [ { \"index\" : 0, \"id\" : \"call_reserved\", \"type\" : \"function\", \"function\" : { \"name\" : \"__llmup_custom__apply_patch\", \"arguments\" : \"{}\" } } ] }, \"finish_reason\" : null } ] }\n",
    "\n",
    "data: [DONE]\n",
    "\n",
);

fn direct_data_client() -> Client {
    let mut headers = ReqwestHeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {TEST_PROVIDER_KEY}")).unwrap(),
    );
    Client::builder()
        .no_proxy()
        .default_headers(headers)
        .build()
        .unwrap()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapturedUpstreamRequest {
    method: String,
    path: String,
    body: Option<Value>,
    raw_body: Option<Vec<u8>>,
}

#[derive(Clone, Default)]
struct CapturedUpstreamRequests {
    requests: Arc<Mutex<Vec<CapturedUpstreamRequest>>>,
}

impl CapturedUpstreamRequests {
    fn push(&self, request: CapturedUpstreamRequest) {
        self.requests.lock().unwrap().push(request);
    }

    fn snapshot(&self) -> Vec<CapturedUpstreamRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[derive(Clone)]
struct RawUpstreamResponse {
    status: StatusCode,
    content_type: &'static str,
    body: &'static str,
}

impl RawUpstreamResponse {
    fn success(body: &'static str) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: RAW_RESPONSE_CONTENT_TYPE,
            body,
        }
    }

    fn sse_success(body: &'static str) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: RAW_SSE_CONTENT_TYPE,
            body,
        }
    }

    fn provider_error(body: &'static str) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            content_type: RAW_RESPONSE_CONTENT_TYPE,
            body,
        }
    }
}

#[derive(Clone)]
struct RawResponseUpstreamState {
    captured: CapturedUpstreamRequests,
    response: RawUpstreamResponse,
}

#[derive(Clone, Copy)]
struct RawForwardingCase {
    name: &'static str,
    format: UpstreamFormat,
    llmup_path: &'static str,
    upstream_path: &'static str,
    request_body: &'static str,
    stream_request_body: &'static str,
    success_body: &'static str,
    stream_success_body: &'static str,
    error_body: &'static str,
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<String>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<str>) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value.as_ref());
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn openai_auto_discovery_config(upstream_base: &str) -> Config {
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        proxy: None,
        upstreams: vec![UpstreamConfig {
            name: "default".to_string(),
            api_root: upstream_api_root(upstream_base, UpstreamFormat::OpenAiCompletion),
            fixed_upstream_format: None,
            provider_key_env: None,
            provider_key: None,
            upstream_headers: Vec::new(),
            proxy: None,
            limits: None,
            surface_defaults: None,
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    }
}

fn fixed_format_config(upstream_base: &str, format: UpstreamFormat) -> Config {
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        proxy: Some(ProxyConfig::Direct),
        upstreams: vec![UpstreamConfig {
            name: "default".to_string(),
            api_root: upstream_api_root(upstream_base, format),
            fixed_upstream_format: Some(format),
            provider_key_env: None,
            provider_key: None,
            upstream_headers: Vec::new(),
            proxy: None,
            limits: None,
            surface_defaults: None,
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    }
}

async fn spawn_openai_capture_upstream() -> (
    String,
    tokio::task::JoinHandle<()>,
    CapturedUpstreamRequests,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let captured = CapturedUpstreamRequests::default();
    let app = Router::new()
        .route("/v1/chat/completions", post(openai_chat_handler))
        .route("/v1/responses", post(openai_responses_create_handler))
        .route("/v1/responses/:id", get(openai_responses_get_handler))
        .route("/v1/messages", post(anthropic_messages_handler))
        .with_state(captured.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, captured)
}

async fn spawn_raw_response_capture_upstream(
    response: RawUpstreamResponse,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    CapturedUpstreamRequests,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let captured = CapturedUpstreamRequests::default();
    let state = RawResponseUpstreamState {
        captured: captured.clone(),
        response,
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(raw_openai_chat_handler))
        .route("/v1/responses", post(raw_openai_responses_create_handler))
        .route("/v1/messages", post(raw_anthropic_messages_handler))
        .with_state(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, captured)
}

async fn openai_chat_handler(
    State(captured): State<CapturedUpstreamRequests>,
    method: Method,
    headers: HeaderMap,
    raw_body: Bytes,
) -> Response {
    let (body, raw_body) = parse_captured_json_body(raw_body);
    capture_request(
        &captured,
        method,
        "/v1/chat/completions",
        Some(body.clone()),
        Some(raw_body),
    );
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from("data: [DONE]\n\n"))
            .unwrap();
    }
    let mut response = (
        StatusCode::OK,
        AxumJson(json!({
            "id": "chatcmpl-proxy-test",
            "object": "chat.completion",
            "created": 1,
            "model": body.get("model").cloned().unwrap_or_else(|| json!("mock")),
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })),
    )
        .into_response();
    if let Some(value) = headers.get("x-proxy-test-id") {
        response
            .headers_mut()
            .insert("x-proxy-test-id", value.clone());
    }
    response
}

async fn raw_openai_chat_handler(
    State(state): State<RawResponseUpstreamState>,
    method: Method,
    raw_body: Bytes,
) -> Response {
    raw_response_with_capture(state, method, "/v1/chat/completions", raw_body)
}

async fn openai_responses_create_handler(
    State(captured): State<CapturedUpstreamRequests>,
    method: Method,
    raw_body: Bytes,
) -> Response {
    let (body, raw_body) = parse_captured_json_body(raw_body);
    capture_request(
        &captured,
        method,
        "/v1/responses",
        Some(body.clone()),
        Some(raw_body),
    );
    (
        StatusCode::OK,
        AxumJson(json!({
            "id": "resp_proxy_create",
            "object": "response",
            "model": body.get("model").cloned().unwrap_or_else(|| json!("gpt-4o")),
            "output": [{
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "hi"
                }]
            }]
        })),
    )
        .into_response()
}

async fn raw_openai_responses_create_handler(
    State(state): State<RawResponseUpstreamState>,
    method: Method,
    raw_body: Bytes,
) -> Response {
    raw_response_with_capture(state, method, "/v1/responses", raw_body)
}

async fn anthropic_messages_handler(
    State(captured): State<CapturedUpstreamRequests>,
    method: Method,
    raw_body: Bytes,
) -> Response {
    let (body, raw_body) = parse_captured_json_body(raw_body);
    capture_request(
        &captured,
        method,
        "/v1/messages",
        Some(body.clone()),
        Some(raw_body),
    );
    (
        StatusCode::OK,
        AxumJson(json!({
            "id": "msg_proxy_matrix",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "text", "text": "hi" }],
            "model": body.get("model").cloned().unwrap_or_else(|| json!("claude-3-5-sonnet")),
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })),
    )
        .into_response()
}

async fn raw_anthropic_messages_handler(
    State(state): State<RawResponseUpstreamState>,
    method: Method,
    raw_body: Bytes,
) -> Response {
    raw_response_with_capture(state, method, "/v1/messages", raw_body)
}

fn raw_response_with_capture(
    state: RawResponseUpstreamState,
    method: Method,
    path: &str,
    raw_body: Bytes,
) -> Response {
    let (body, raw_body) = parse_captured_json_body(raw_body);
    capture_request(
        &state.captured,
        method,
        path,
        Some(body.clone()),
        Some(raw_body),
    );

    Response::builder()
        .status(state.response.status)
        .header("Content-Type", state.response.content_type)
        .header(
            "Content-Length",
            state.response.body.as_bytes().len().to_string(),
        )
        .header("request-id", "req_raw_phase_3a")
        .header("x-request-id", "xreq_raw_phase_3a")
        .header("openai-processing-ms", "42")
        .header("retry-after", "3")
        .header("x-ratelimit-remaining-requests", "17")
        .header("connection", RAW_UPSTREAM_HOP_BY_HOP_HEADER)
        .header(
            RAW_UPSTREAM_HOP_BY_HOP_HEADER,
            RAW_UPSTREAM_HOP_BY_HOP_VALUE,
        )
        .header("keep-alive", RAW_UPSTREAM_KEEP_ALIVE_VALUE)
        .header("set-cookie", "session=must-not-forward")
        .body(Body::from(state.response.body))
        .unwrap()
}

async fn openai_responses_get_handler(
    State(captured): State<CapturedUpstreamRequests>,
    method: Method,
    Path(id): Path<String>,
) -> Response {
    capture_request(
        &captured,
        method,
        &format!("/v1/responses/{id}"),
        None,
        None,
    );
    (
        StatusCode::OK,
        AxumJson(json!({
            "id": id,
            "object": "response",
            "model": "gpt-4o",
            "output": [{
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "resource ok"
                }]
            }]
        })),
    )
        .into_response()
}

fn parse_captured_json_body(raw_body: Bytes) -> (Value, Vec<u8>) {
    let raw_body = raw_body.to_vec();
    let body = serde_json::from_slice(&raw_body).expect("upstream request body should be JSON");
    (body, raw_body)
}

fn capture_request(
    captured: &CapturedUpstreamRequests,
    method: Method,
    path: &str,
    body: Option<Value>,
    raw_body: Option<Vec<u8>>,
) {
    captured.push(CapturedUpstreamRequest {
        method: method.to_string(),
        path: path.to_string(),
        body,
        raw_body,
    });
}

async fn wait_for_upstream_path(
    captured: &CapturedUpstreamRequests,
    path: &str,
    attempts: usize,
) -> Vec<CapturedUpstreamRequest> {
    for _ in 0..attempts {
        let snapshot = captured.snapshot();
        if snapshot.iter().any(|request| request.path == path) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    captured.snapshot()
}

async fn wait_for_upstream_request_count(
    captured: &CapturedUpstreamRequests,
    minimum: usize,
    attempts: usize,
) -> Vec<CapturedUpstreamRequest> {
    for _ in 0..attempts {
        let snapshot = captured.snapshot();
        if snapshot.len() >= minimum {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    captured.snapshot()
}

async fn post_raw_json(client: &Client, url: String, raw_json: &str) -> reqwest::Response {
    client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .body(raw_json.to_string())
        .send()
        .await
        .unwrap()
}

fn raw_forwarding_cases() -> [RawForwardingCase; 3] {
    [
        RawForwardingCase {
            name: "openai chat",
            format: UpstreamFormat::OpenAiCompletion,
            llmup_path: "/openai/v1/chat/completions",
            upstream_path: "/v1/chat/completions",
            request_body: RAW_CHAT_REQUEST,
            stream_request_body: RAW_CHAT_STREAM_REQUEST,
            success_body: RAW_CHAT_SUCCESS_RESPONSE,
            stream_success_body: RAW_CHAT_STREAM_SUCCESS_RESPONSE,
            error_body: RAW_OPENAI_ERROR_RESPONSE,
        },
        RawForwardingCase {
            name: "openai responses",
            format: UpstreamFormat::OpenAiResponses,
            llmup_path: "/openai/v1/responses",
            upstream_path: "/v1/responses",
            request_body: RAW_RESPONSES_REQUEST,
            stream_request_body: RAW_RESPONSES_STREAM_REQUEST,
            success_body: RAW_RESPONSES_SUCCESS_RESPONSE,
            stream_success_body: RAW_RESPONSES_STREAM_SUCCESS_RESPONSE,
            error_body: RAW_OPENAI_ERROR_RESPONSE,
        },
        RawForwardingCase {
            name: "anthropic messages",
            format: UpstreamFormat::Anthropic,
            llmup_path: "/anthropic/v1/messages",
            upstream_path: "/v1/messages",
            request_body: RAW_ANTHROPIC_REQUEST,
            stream_request_body: RAW_ANTHROPIC_STREAM_REQUEST,
            success_body: RAW_ANTHROPIC_SUCCESS_RESPONSE,
            stream_success_body: RAW_ANTHROPIC_STREAM_SUCCESS_RESPONSE,
            error_body: RAW_ANTHROPIC_ERROR_RESPONSE,
        },
    ]
}

fn header_str<'a>(headers: &'a ReqwestHeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn header_values<'a>(headers: &'a ReqwestHeaderMap, name: &str) -> Vec<&'a str> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect()
}

fn assert_upstream_hop_by_hop_headers_not_forwarded(headers: &ReqwestHeaderMap) {
    assert!(
        !header_values(headers, "connection").iter().any(|value| {
            value.split(',').any(|token| {
                token
                    .trim()
                    .eq_ignore_ascii_case(RAW_UPSTREAM_HOP_BY_HOP_HEADER)
            })
        }),
        "upstream Connection hop-by-hop canary must not be forwarded"
    );
    assert!(
        headers.get(RAW_UPSTREAM_HOP_BY_HOP_HEADER).is_none(),
        "upstream Connection-nominated hop-by-hop header must not be forwarded"
    );
    assert!(
        !header_values(headers, "keep-alive")
            .iter()
            .any(|value| *value == RAW_UPSTREAM_KEEP_ALIVE_VALUE),
        "upstream Keep-Alive hop-by-hop header must not be forwarded"
    );
}

fn assert_raw_response_headers(headers: &ReqwestHeaderMap) {
    assert_eq!(
        header_str(headers, CONTENT_TYPE.as_str()),
        Some(RAW_RESPONSE_CONTENT_TYPE)
    );
    assert_eq!(header_str(headers, "request-id"), Some("req_raw_phase_3a"));
    assert_eq!(
        header_str(headers, "x-request-id"),
        Some("xreq_raw_phase_3a")
    );
    assert_eq!(header_str(headers, "openai-processing-ms"), Some("42"));
    assert_eq!(header_str(headers, "retry-after"), Some("3"));
    assert_eq!(
        header_str(headers, "x-ratelimit-remaining-requests"),
        Some("17")
    );
    assert!(
        headers.get("set-cookie").is_none(),
        "sensitive upstream response header must not be forwarded"
    );
    assert_upstream_hop_by_hop_headers_not_forwarded(headers);
}

fn assert_raw_stream_response_headers(headers: &ReqwestHeaderMap) {
    assert_eq!(
        header_str(headers, CONTENT_TYPE.as_str()),
        Some(RAW_SSE_CONTENT_TYPE)
    );
    assert_eq!(header_str(headers, "cache-control"), Some("no-cache"));
    assert_eq!(header_str(headers, "connection"), Some("keep-alive"));
    assert_eq!(header_str(headers, "request-id"), Some("req_raw_phase_3a"));
    assert_eq!(
        header_str(headers, "x-request-id"),
        Some("xreq_raw_phase_3a")
    );
    assert_eq!(header_str(headers, "openai-processing-ms"), Some("42"));
    assert_eq!(header_str(headers, "retry-after"), Some("3"));
    assert_eq!(
        header_str(headers, "x-ratelimit-remaining-requests"),
        Some("17")
    );
    assert!(
        headers.get("content-length").is_none(),
        "2xx SSE stream must not forward Content-Length"
    );
    // The downstream HTTP stack may add Transfer-Encoding for streaming bodies.
    assert!(
        headers.get("set-cookie").is_none(),
        "sensitive upstream response header must not be forwarded"
    );
    assert_upstream_hop_by_hop_headers_not_forwarded(headers);
}

#[tokio::test]
async fn same_format_non_stream_success_response_forwards_exact_upstream_bytes() {
    for case in raw_forwarding_cases() {
        let (upstream_base, _upstream, captured_upstream) =
            spawn_raw_response_capture_upstream(RawUpstreamResponse::success(case.success_body))
                .await;
        let config = fixed_format_config(&upstream_base, case.format);
        let (llmup_base, _llmup) = start_proxy(config).await;
        let client = direct_data_client();

        let response = post_raw_json(
            &client,
            format!("{llmup_base}{}", case.llmup_path),
            case.request_body,
        )
        .await;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.unwrap();

        assert_eq!(status, StatusCode::OK, "case = {}", case.name);
        assert_eq!(
            body.as_ref(),
            case.success_body.as_bytes(),
            "case = {}",
            case.name
        );
        assert_raw_response_headers(&headers);

        let requests = wait_for_upstream_path(&captured_upstream, case.upstream_path, 80).await;
        let request = requests
            .iter()
            .find(|request| request.path == case.upstream_path)
            .unwrap_or_else(|| panic!("{} request should reach upstream", case.name));
        assert_eq!(
            request.raw_body.as_deref(),
            Some(case.request_body.as_bytes()),
            "case = {}",
            case.name
        );
    }
}

#[tokio::test]
async fn same_format_non_stream_provider_error_response_forwards_exact_upstream_bytes() {
    for case in raw_forwarding_cases() {
        let (upstream_base, _upstream, captured_upstream) = spawn_raw_response_capture_upstream(
            RawUpstreamResponse::provider_error(case.error_body),
        )
        .await;
        let config = fixed_format_config(&upstream_base, case.format);
        let (llmup_base, _llmup) = start_proxy(config).await;
        let client = direct_data_client();

        let response = post_raw_json(
            &client,
            format!("{llmup_base}{}", case.llmup_path),
            case.request_body,
        )
        .await;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.unwrap();

        assert_eq!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "case = {}",
            case.name
        );
        assert_eq!(
            body.as_ref(),
            case.error_body.as_bytes(),
            "case = {}",
            case.name
        );
        assert_raw_response_headers(&headers);

        let requests = wait_for_upstream_path(&captured_upstream, case.upstream_path, 80).await;
        assert!(
            requests
                .iter()
                .any(|request| request.path == case.upstream_path),
            "{} request should reach upstream: {requests:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn same_format_stream_success_response_forwards_exact_upstream_sse_bytes() {
    for case in raw_forwarding_cases() {
        let (upstream_base, _upstream, captured_upstream) = spawn_raw_response_capture_upstream(
            RawUpstreamResponse::sse_success(case.stream_success_body),
        )
        .await;
        let config = fixed_format_config(&upstream_base, case.format);
        let (llmup_base, _llmup) = start_proxy(config).await;
        let client = direct_data_client();

        let response = post_raw_json(
            &client,
            format!("{llmup_base}{}", case.llmup_path),
            case.stream_request_body,
        )
        .await;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.unwrap();

        assert_eq!(status, StatusCode::OK, "case = {}", case.name);
        assert_eq!(
            body.as_ref(),
            case.stream_success_body.as_bytes(),
            "case = {}",
            case.name
        );
        assert_raw_stream_response_headers(&headers);

        let requests = wait_for_upstream_path(&captured_upstream, case.upstream_path, 80).await;
        let request = requests
            .iter()
            .find(|request| request.path == case.upstream_path)
            .unwrap_or_else(|| panic!("{} request should reach upstream", case.name));
        assert_eq!(
            request.raw_body.as_deref(),
            Some(case.stream_request_body.as_bytes()),
            "case = {}",
            case.name
        );
    }
}

#[tokio::test]
async fn same_format_stream_provider_error_response_forwards_exact_upstream_bytes() {
    for case in raw_forwarding_cases() {
        let (upstream_base, _upstream, captured_upstream) = spawn_raw_response_capture_upstream(
            RawUpstreamResponse::provider_error(case.error_body),
        )
        .await;
        let config = fixed_format_config(&upstream_base, case.format);
        let (llmup_base, _llmup) = start_proxy(config).await;
        let client = direct_data_client();

        let response = post_raw_json(
            &client,
            format!("{llmup_base}{}", case.llmup_path),
            case.stream_request_body,
        )
        .await;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.unwrap();

        assert_eq!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "case = {}",
            case.name
        );
        assert_eq!(
            body.as_ref(),
            case.error_body.as_bytes(),
            "case = {}",
            case.name
        );
        assert_raw_response_headers(&headers);

        let requests = wait_for_upstream_path(&captured_upstream, case.upstream_path, 80).await;
        let request = requests
            .iter()
            .find(|request| request.path == case.upstream_path)
            .unwrap_or_else(|| panic!("{} request should reach upstream", case.name));
        assert_eq!(
            request.raw_body.as_deref(),
            Some(case.stream_request_body.as_bytes()),
            "case = {}",
            case.name
        );
    }
}

#[tokio::test]
async fn mutation_required_stream_request_does_not_activate_raw_sse_forwarding() {
    let (upstream_base, _upstream, captured_upstream) = spawn_raw_response_capture_upstream(
        RawUpstreamResponse::sse_success(RAW_OPENAI_CHAT_INTERNAL_ARTIFACT_STREAM_RESPONSE),
    )
    .await;
    let mut config = fixed_format_config(&upstream_base, UpstreamFormat::OpenAiCompletion);
    config.model_aliases.insert(
        "alias-chat".to_string(),
        ModelAlias {
            upstream_name: "default".to_string(),
            upstream_model: "gpt-4o-mini".to_string(),
            limits: None,
            surface: None,
        },
    );
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();
    let raw_json = r#"{
  "model": "alias-chat",
  "messages": [
    { "role": "user", "content": "ping" }
  ],
  "temperature": 1.2300,
  "stream": true
}"#;

    let response = post_raw_json(
        &client,
        format!("{llmup_base}/openai/v1/chat/completions"),
        raw_json,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let downstream_body = response.bytes().await.unwrap();
    assert_ne!(
        downstream_body.as_ref(),
        RAW_OPENAI_CHAT_INTERNAL_ARTIFACT_STREAM_RESPONSE.as_bytes(),
        "mutation-required request must not activate raw SSE forwarding"
    );
    let downstream_body_text = String::from_utf8_lossy(&downstream_body);
    assert!(
        !downstream_body_text.contains("_llmup_tool_bridge_context"),
        "internal bridge context leaked: {downstream_body_text}"
    );
    assert!(
        !downstream_body_text.contains("__llmup_custom__"),
        "internal custom tool prefix leaked: {downstream_body_text}"
    );

    let requests = wait_for_upstream_path(&captured_upstream, "/v1/chat/completions", 80).await;
    let request = requests
        .iter()
        .find(|request| request.path == "/v1/chat/completions")
        .expect("chat request should reach upstream");
    assert_ne!(request.raw_body.as_deref(), Some(raw_json.as_bytes()));
    assert_eq!(
        request
            .body
            .as_ref()
            .and_then(|body| body.get("model"))
            .and_then(Value::as_str),
        Some("gpt-4o-mini")
    );
}

#[tokio::test]
async fn alias_model_rewrite_prevents_raw_non_stream_response_forwarding() {
    let (upstream_base, _upstream, captured_upstream) = spawn_raw_response_capture_upstream(
        RawUpstreamResponse::success(RAW_CHAT_SUCCESS_RESPONSE),
    )
    .await;
    let mut config = fixed_format_config(&upstream_base, UpstreamFormat::OpenAiCompletion);
    config.model_aliases.insert(
        "alias-chat".to_string(),
        ModelAlias {
            upstream_name: "default".to_string(),
            upstream_model: "gpt-4o-mini".to_string(),
            limits: None,
            surface: None,
        },
    );
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();
    let raw_json = r#"{
  "model": "alias-chat",
  "messages": [
    { "role": "user", "content": "ping" }
  ],
  "temperature": 1.2300
}"#;

    let response = post_raw_json(
        &client,
        format!("{llmup_base}/openai/v1/chat/completions"),
        raw_json,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let downstream_body = response.bytes().await.unwrap();
    assert_ne!(
        downstream_body.as_ref(),
        RAW_CHAT_SUCCESS_RESPONSE.as_bytes(),
        "alias rewrite must not activate raw response forwarding"
    );

    let requests = wait_for_upstream_path(&captured_upstream, "/v1/chat/completions", 80).await;
    let request = requests
        .iter()
        .find(|request| request.path == "/v1/chat/completions")
        .expect("chat request should reach upstream");
    assert_ne!(request.raw_body.as_deref(), Some(raw_json.as_bytes()));
    assert_eq!(
        request
            .body
            .as_ref()
            .and_then(|body| body.get("model"))
            .and_then(Value::as_str),
        Some("gpt-4o-mini")
    );
}

#[tokio::test]
async fn openai_chat_same_format_eligible_request_forwards_exact_raw_body_bytes() {
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let config = fixed_format_config(&upstream_base, UpstreamFormat::OpenAiCompletion);
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();
    let raw_json = r#"{
  "model": "gpt-4o-mini",
  "messages": [
    { "role": "user", "content": "ping" }
  ],
  "prompt_cache_key": "stable-prefix",
  "x_provider_native": { "kept": true, "n": 1.2300 },
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "describe_reserved_prefix",
        "description": "Plain schema text may mention __llmup_custom__apply_patch.",
        "parameters": {
          "type": "object",
          "properties": {
            "literal": {
              "type": "string",
              "description": "__llmup_custom__apply_patch is only example text"
            }
          }
        }
      }
    }
  ],
  "temperature": 1e0
}"#;

    let response = post_raw_json(
        &client,
        format!("{llmup_base}/openai/v1/chat/completions"),
        raw_json,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let requests = wait_for_upstream_path(&captured_upstream, "/v1/chat/completions", 80).await;
    let request = requests
        .iter()
        .find(|request| request.path == "/v1/chat/completions")
        .expect("chat request should reach upstream");
    assert_eq!(request.raw_body.as_deref(), Some(raw_json.as_bytes()));
    assert_eq!(
        request
            .body
            .as_ref()
            .and_then(|body| body.get("prompt_cache_key"))
            .and_then(Value::as_str),
        Some("stable-prefix")
    );
}

#[tokio::test]
async fn raw_eligible_openai_chat_rejects_reserved_legacy_function_name_without_upstream_call() {
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let config = fixed_format_config(&upstream_base, UpstreamFormat::OpenAiCompletion);
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();
    let raw_json = r#"{
  "model": "gpt-4o-mini",
  "messages": [
    { "role": "user", "content": "ping" }
  ],
  "functions": [
    {
      "name": "__llmup_custom__legacy_exec",
      "parameters": { "type": "object", "properties": {} }
    }
  ],
  "temperature": 1e0
}"#;

    let response = post_raw_json(
        &client,
        format!("{llmup_base}/openai/v1/chat/completions"),
        raw_json,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body_text = response.text().await.unwrap();
    assert!(
        !body_text.contains("__llmup_custom__"),
        "reserved prefix should not leak: {body_text}"
    );
    assert!(
        captured_upstream.snapshot().is_empty(),
        "reserved function name should fail before upstream call: {:?}",
        captured_upstream.snapshot()
    );
}

#[tokio::test]
async fn openai_chat_same_format_rejects_anthropic_cache_extension_without_upstream_call() {
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let config = fixed_format_config(&upstream_base, UpstreamFormat::OpenAiCompletion);
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();
    let raw_json = r#"{
  "model": "gpt-4o-mini",
  "messages": [
    { "role": "user", "content": "ping" }
  ],
  "extra_body": {
    "anthropic": {
      "cache_control": { "type": "ephemeral", "ttl": "5m" }
    }
  },
  "temperature": 1e0
}"#;

    let response = post_raw_json(
        &client,
        format!("{llmup_base}/openai/v1/chat/completions"),
        raw_json,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body_text = response.text().await.unwrap();
    assert!(
        body_text.contains("extra_body.anthropic.cache_control"),
        "body = {body_text}"
    );
    assert!(body_text.contains("target Anthropic"), "body = {body_text}");
    assert!(
        captured_upstream.snapshot().is_empty(),
        "bad prompt-cache extension should fail before upstream call: {:?}",
        captured_upstream.snapshot()
    );
}

#[tokio::test]
async fn openai_responses_same_format_eligible_request_forwards_exact_raw_body_bytes() {
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let config = fixed_format_config(&upstream_base, UpstreamFormat::OpenAiResponses);
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();
    let raw_json = r#"{
  "model": "gpt-4.1",
  "input": [
    {
      "role": "user",
      "content": [
        { "type": "input_text", "text": "ping" }
      ]
    }
  ],
  "prompt_cache_key": "responses-prefix",
  "metadata": { "unknown": "provider-field", "score": 1.2300 },
  "temperature": 1e0
}"#;

    let response = post_raw_json(
        &client,
        format!("{llmup_base}/openai/v1/responses"),
        raw_json,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let requests = wait_for_upstream_path(&captured_upstream, "/v1/responses", 80).await;
    let request = requests
        .iter()
        .find(|request| request.path == "/v1/responses")
        .expect("responses request should reach upstream");
    assert_eq!(request.raw_body.as_deref(), Some(raw_json.as_bytes()));
}

#[tokio::test]
async fn anthropic_same_format_rejects_openai_cache_extension_without_upstream_call() {
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let config = fixed_format_config(&upstream_base, UpstreamFormat::Anthropic);
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();
    let raw_json = r#"{
  "model": "claude-3-5-sonnet",
  "max_tokens": 0,
  "messages": [
    {
      "role": "user",
      "content": [
        { "type": "text", "text": "ping" }
      ]
    }
  ],
  "extra_body": {
    "openai": {
      "prompt_cache_key": "stable-prefix"
    }
  },
  "temperature": 1e0
}"#;

    let response = post_raw_json(
        &client,
        format!("{llmup_base}/anthropic/v1/messages"),
        raw_json,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body_text = response.text().await.unwrap();
    assert!(
        body_text.contains("extra_body.openai"),
        "body = {body_text}"
    );
    assert!(body_text.contains("target OpenAI"), "body = {body_text}");
    assert!(
        captured_upstream.snapshot().is_empty(),
        "bad prompt-cache extension should fail before upstream call: {:?}",
        captured_upstream.snapshot()
    );
}

#[tokio::test]
async fn anthropic_same_format_eligible_request_forwards_exact_raw_body_bytes() {
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let config = fixed_format_config(&upstream_base, UpstreamFormat::Anthropic);
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();
    let raw_json = r#"{
  "model": "claude-3-5-sonnet",
  "max_tokens": 0,
  "system": [
    {
      "type": "text",
      "text": "stable system",
      "cache_control": { "type": "ephemeral" }
    }
  ],
  "messages": [
    {
      "role": "user",
      "content": [
        {
          "type": "text",
          "text": "ping",
          "cache_control": { "type": "ephemeral" }
        }
      ]
    }
  ],
  "metadata": { "provider_unknown": true, "ratio": 1.2300 },
  "temperature": 1e0
}"#;

    let response = post_raw_json(
        &client,
        format!("{llmup_base}/anthropic/v1/messages"),
        raw_json,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let requests = wait_for_upstream_path(&captured_upstream, "/v1/messages", 80).await;
    let request = requests
        .iter()
        .find(|request| request.path == "/v1/messages")
        .expect("anthropic request should reach upstream");
    assert_eq!(request.raw_body.as_deref(), Some(raw_json.as_bytes()));
}

#[tokio::test]
async fn alias_model_rewrite_does_not_forward_original_raw_body_bytes() {
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let mut config = fixed_format_config(&upstream_base, UpstreamFormat::OpenAiCompletion);
    config.model_aliases.insert(
        "alias-chat".to_string(),
        ModelAlias {
            upstream_name: "default".to_string(),
            upstream_model: "gpt-4o-mini".to_string(),
            limits: None,
            surface: None,
        },
    );
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();
    let raw_json = r#"{
  "model": "alias-chat",
  "messages": [
    { "role": "user", "content": "ping" }
  ],
  "temperature": 1.2300
}"#;

    let response = post_raw_json(
        &client,
        format!("{llmup_base}/openai/v1/chat/completions"),
        raw_json,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let requests = wait_for_upstream_path(&captured_upstream, "/v1/chat/completions", 80).await;
    let request = requests
        .iter()
        .find(|request| request.path == "/v1/chat/completions")
        .expect("chat request should reach upstream");
    assert_ne!(request.raw_body.as_deref(), Some(raw_json.as_bytes()));
    assert_eq!(
        request
            .body
            .as_ref()
            .and_then(|body| body.get("model"))
            .and_then(Value::as_str),
        Some("gpt-4o-mini")
    );
}

#[test]
fn yaml_proxy_config_round_trip_preserves_namespace_and_per_upstream_override_layers() {
    let config = Config::from_yaml_str(
        r#"
listen: 127.0.0.1:0
proxy: direct
upstreams:
  OPENAI:
    api_root: http://example.com/v1
    format: openai-completion
    proxy:
      url: http://upstream-proxy.local:8080
"#,
    )
    .unwrap();

    let round_trip = serde_json::to_value(RuntimeConfigPayload::from(&config)).unwrap();

    assert_eq!(round_trip["proxy"], "direct");
    assert_eq!(
        round_trip["upstreams"][0]["proxy"]["url"],
        "http://upstream-proxy.local:8080"
    );
}

#[tokio::test]
async fn env_proxy_is_used_consistently_for_discovery_request_and_resource_paths() {
    let _env_lock = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let (proxy_base, _forward_proxy, captured_proxy) = spawn_http_forward_proxy().await;
    let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &proxy_base);
    let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &proxy_base);
    let _all_proxy = ScopedEnvVar::remove("ALL_PROXY");
    let _all_proxy_lower = ScopedEnvVar::remove("all_proxy");
    let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
    let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
    let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

    let config = openai_auto_discovery_config(&upstream_base);
    let (llmup_base, _proxy_handle) = start_proxy(config).await;
    let client = direct_data_client();

    let response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let resource = client
        .get(format!("{llmup_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert_eq!(resource.status(), StatusCode::OK);

    let proxied = captured_proxy
        .wait_for_count(3, Duration::from_secs(2))
        .await;
    let joined_uris = proxied
        .iter()
        .map(|item| item.uri.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined_uris.contains("/v1/chat/completions"),
        "proxy did not observe request path traffic: {joined_uris}"
    );
    assert!(
        joined_uris.contains("/v1/responses"),
        "proxy did not observe discovery or resource traffic: {joined_uris}"
    );
    assert!(
        joined_uris.contains("/v1/responses/resp_123"),
        "proxy did not observe resource path traffic: {joined_uris}"
    );

    let upstream_requests =
        wait_for_upstream_path(&captured_upstream, "/v1/responses/resp_123", 80).await;
    assert!(
        upstream_requests
            .iter()
            .any(|request| request.path == "/v1/chat/completions"),
        "upstream did not receive request path traffic: {upstream_requests:?}"
    );
    assert!(
        upstream_requests
            .iter()
            .any(|request| request.path == "/v1/responses/resp_123"),
        "upstream did not receive resource path traffic: {upstream_requests:?}"
    );
    assert!(
        upstream_requests.iter().any(|request| {
            request.path == "/v1/responses"
                && request
                    .body
                    .as_ref()
                    .and_then(|body| body.get("input"))
                    .is_some()
        }),
        "upstream did not receive discovery probe for responses: {upstream_requests:?}"
    );
}

#[tokio::test]
async fn per_upstream_override_should_override_namespace_and_env_for_request_path() {
    let _env_lock = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (upstream_base, _upstream, _captured_upstream) = spawn_openai_capture_upstream().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let (namespace_proxy_base, _namespace_proxy, captured_namespace_proxy) =
        spawn_http_forward_proxy().await;
    let (override_proxy_base, _override_proxy, captured_override_proxy) =
        spawn_http_forward_proxy().await;
    let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &env_proxy_base);
    let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &env_proxy_base);
    let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
    let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
    let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

    let yaml = format!(
        r#"
listen: 127.0.0.1:0
proxy:
  url: {namespace_proxy_base}
upstreams:
  OPENAI:
    api_root: {api_root}
    format: openai-completion
    proxy:
      url: {override_proxy_base}
"#,
        api_root = upstream_api_root(&upstream_base, UpstreamFormat::OpenAiCompletion),
    );
    let config = Config::from_yaml_str(&yaml).unwrap();
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();

    let response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let override_seen = captured_override_proxy
        .wait_for_count(1, Duration::from_secs(1))
        .await;
    let namespace_seen = captured_namespace_proxy.snapshot();
    let env_seen = captured_env_proxy.snapshot();

    assert_eq!(override_seen.len(), 1, "per-upstream override should win");
    assert!(
        namespace_seen.is_empty(),
        "namespace proxy should be shadowed by upstream proxy: {namespace_seen:?}"
    );
    assert!(
        env_seen.is_empty(),
        "env proxy should be shadowed by upstream proxy: {env_seen:?}"
    );
}

#[tokio::test]
async fn explicit_direct_should_cut_env_proxy() {
    let _env_lock = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &env_proxy_base);
    let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &env_proxy_base);
    let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
    let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
    let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

    let yaml = format!(
        r#"
listen: 127.0.0.1:0
proxy: direct
upstreams:
  OPENAI:
    api_root: {api_root}
    format: openai-completion
"#,
        api_root = upstream_api_root(&upstream_base, UpstreamFormat::OpenAiCompletion),
    );
    let config = Config::from_yaml_str(&yaml).unwrap();
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();

    let response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let upstream_requests =
        wait_for_upstream_path(&captured_upstream, "/v1/chat/completions", 80).await;
    let env_requests = captured_env_proxy.snapshot();

    assert!(
        upstream_requests
            .iter()
            .any(|request| request.path == "/v1/chat/completions"),
        "direct mode should still reach the upstream: {upstream_requests:?}"
    );
    assert!(
        env_requests.is_empty(),
        "direct mode should bypass env proxy entirely: {env_requests:?}"
    );
}

#[tokio::test]
async fn per_upstream_direct_should_bypass_namespace_and_env_for_request_resource_and_streaming_paths(
) {
    let _env_lock = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let (namespace_proxy_base, _namespace_proxy, captured_namespace_proxy) =
        spawn_http_forward_proxy().await;
    let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &env_proxy_base);
    let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &env_proxy_base);
    let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
    let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
    let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

    let yaml = format!(
        r#"
listen: 127.0.0.1:0
proxy:
  url: {namespace_proxy_base}
upstreams:
  OPENAI:
    api_root: {api_root}
    proxy: direct
"#,
        api_root = upstream_api_root(&upstream_base, UpstreamFormat::OpenAiCompletion),
    );
    let config = Config::from_yaml_str(&yaml).unwrap();
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();

    let request_response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(request_response.status(), StatusCode::OK);

    let resource_response = client
        .get(format!("{llmup_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert_eq!(resource_response.status(), StatusCode::OK);

    let streaming_response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "stream": true,
            "messages": [{ "role": "user", "content": "stream ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(streaming_response.status(), StatusCode::OK);
    assert_eq!(
        streaming_response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let streaming_body = streaming_response.text().await.unwrap();
    assert!(streaming_body.contains("[DONE]"));

    let upstream_requests = wait_for_upstream_request_count(&captured_upstream, 4, 80).await;
    let chat_requests = upstream_requests
        .iter()
        .filter(|request| request.path == "/v1/chat/completions")
        .collect::<Vec<_>>();

    assert!(
        upstream_requests
            .iter()
            .any(|request| request.path == "/v1/responses/resp_123"),
        "resource path should reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        upstream_requests.iter().any(|request| {
            request.path == "/v1/responses"
                && request
                    .body
                    .as_ref()
                    .and_then(|body| body.get("input"))
                    .is_some()
        }),
        "resource discovery should reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        chat_requests.len() >= 2,
        "request and streaming paths should both reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        chat_requests.iter().any(|request| {
            request
                .body
                .as_ref()
                .and_then(|body| body.get("stream"))
                .and_then(Value::as_bool)
                == Some(true)
        }),
        "streaming path should reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        chat_requests.iter().any(|request| {
            request
                .body
                .as_ref()
                .and_then(|body| body.get("stream"))
                .and_then(Value::as_bool)
                != Some(true)
        }),
        "request path should reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        captured_namespace_proxy.snapshot().is_empty(),
        "namespace proxy should be bypassed by per-upstream direct: {:?}",
        captured_namespace_proxy.snapshot()
    );
    assert!(
        captured_env_proxy.snapshot().is_empty(),
        "env proxy should be bypassed by per-upstream direct: {:?}",
        captured_env_proxy.snapshot()
    );
}
