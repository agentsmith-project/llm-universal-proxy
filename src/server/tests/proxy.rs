use super::*;
use crate::server::proxy::request_translation_policy_requires_body_mutation;

const INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD: &str = "_llmup_tool_bridge_context";

#[derive(Clone, Default)]
struct CapturedTraceWriter {
    buffer: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl CapturedTraceWriter {
    fn contents(&self) -> String {
        let bytes = self.buffer.lock().expect("trace buffer lock").clone();
        String::from_utf8(bytes).expect("trace logs utf8")
    }
}

struct CapturedTraceSink {
    buffer: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl std::io::Write for CapturedTraceSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buffer
            .lock()
            .expect("trace buffer lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedTraceWriter {
    type Writer = CapturedTraceSink;

    fn make_writer(&'writer self) -> Self::Writer {
        CapturedTraceSink {
            buffer: self.buffer.clone(),
        }
    }
}

async fn capture_error_logs<F, T>(future: F) -> (T, String)
where
    F: std::future::Future<Output = T>,
{
    capture_logs(tracing::Level::ERROR, future).await
}

async fn capture_debug_logs<F, T>(future: F) -> (T, String)
where
    F: std::future::Future<Output = T>,
{
    capture_logs(tracing::Level::DEBUG, future).await
}

async fn capture_logs<F, T>(level: tracing::Level, future: F) -> (T, String)
where
    F: std::future::Future<Output = T>,
{
    let writer = CapturedTraceWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(writer.clone())
        .with_ansi(false)
        .without_time()
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let guard = tracing::dispatcher::set_default(&dispatch);
    let output = future.await;
    drop(guard);
    (output, writer.contents())
}

async fn response_text(response: Response<Body>) -> String {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body bytes");
    String::from_utf8(body.to_vec()).expect("response body utf8")
}

fn assert_no_secret_leak(text: &str, secrets: &[&str], context: &str) {
    for secret in secrets {
        assert!(!text.contains(secret), "{context} leaked {secret}: {text}");
    }
}

async fn enable_debug_trace_for_default_namespace(state: &Arc<AppState>) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "llmup-success-redaction-trace-{}.jsonl",
        uuid::Uuid::new_v4()
    ));
    let recorder = crate::debug_trace::DebugTraceRecorder::new(&crate::config::DebugTraceConfig {
        path: Some(path.to_string_lossy().to_string()),
        max_text_chars: 4096,
    })
    .expect("debug trace recorder");
    state
        .runtime
        .write()
        .await
        .namespaces
        .get_mut(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .debug_trace = Some(recorder);
    path
}

async fn wait_for_debug_trace_response(path: &std::path::Path) -> String {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut contents = String::new();
    while tokio::time::Instant::now() < deadline {
        contents = std::fs::read_to_string(path).unwrap_or_default();
        if contents.contains("\"phase\":\"response\"") {
            return contents;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    contents
}

fn debug_trace_request_entry(trace: &str) -> Value {
    trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|entry| entry.get("phase").and_then(Value::as_str) == Some("request"))
        .expect("trace should contain request entry")
}

fn assert_debug_trace_omits_stable_prefix(trace: &str, request_entry: &Value) {
    let request_entry_text =
        serde_json::to_string(request_entry).expect("request entry serializes");
    assert!(
        !request_entry_text.contains("stable-prefix"),
        "debug trace request entry must not contain prompt-cache key values: {request_entry_text}"
    );
    assert!(
        !trace.contains("stable-prefix"),
        "debug trace must not contain prompt-cache key values: {trace}"
    );
}

async fn wait_for_debug_trace_request_count(path: &std::path::Path, count: usize) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut entries = Vec::new();
    while tokio::time::Instant::now() < deadline {
        entries = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| entry.get("phase").and_then(Value::as_str) == Some("request"))
            .collect::<Vec<_>>();
        if entries.len() >= count {
            return entries;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    entries
}

fn json_contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(object) => object
            .iter()
            .any(|(candidate, value)| candidate == key || json_contains_key(value, key)),
        Value::Array(items) => items.iter().any(|item| json_contains_key(item, key)),
        _ => false,
    }
}

fn portability_warning_headers(response: &Response<Body>) -> Vec<String> {
    response
        .headers()
        .get_all("x-llmup-portability-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(ToString::to_string)
        .collect()
}

fn assert_portability_warning_headers_contain(response: &Response<Body>, expected: &[&str]) {
    let warnings = portability_warning_headers(response);
    assert!(response.headers().get("x-proxy-compat-warning").is_none());
    for expected in expected {
        assert!(
            warnings.iter().any(|warning| warning.contains(expected)),
            "expected warning containing {expected}, warnings = {warnings:?}"
        );
    }
}

fn assert_cache_control_portability_warning_headers(response: &Response<Body>) {
    let warnings = portability_warning_headers(response);
    assert!(response.headers().get("x-proxy-compat-warning").is_none());
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("cache_control")),
        "warnings = {warnings:?}"
    );
}

#[derive(Clone, Default)]
struct CapturedHookPayloads {
    payloads: Arc<Mutex<Vec<Value>>>,
}

impl CapturedHookPayloads {
    async fn push(&self, payload: Value) {
        self.payloads.lock().await.push(payload);
    }

    async fn snapshot(&self) -> Vec<Value> {
        self.payloads.lock().await.clone()
    }
}

async fn spawn_hook_capture_mock() -> (String, CapturedHookPayloads, tokio::task::JoinHandle<()>) {
    async fn exchange_handler(
        State(captured): State<CapturedHookPayloads>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        captured.push(body).await;
        (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
    }

    let captured = CapturedHookPayloads::default();
    let app = Router::new()
        .route("/exchange", post(exchange_handler))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind hook capture mock");
    let addr = listener.local_addr().expect("hook capture local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("hook capture mock server");
    });

    (format!("http://{addr}"), captured, server)
}

async fn spawn_exchange_usage_hook_capture_mock() -> (
    String,
    CapturedHookPayloads,
    CapturedHookPayloads,
    tokio::task::JoinHandle<()>,
) {
    async fn exchange_handler(
        State((exchange, _usage)): State<(CapturedHookPayloads, CapturedHookPayloads)>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        exchange.push(body).await;
        (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
    }

    async fn usage_handler(
        State((_exchange, usage)): State<(CapturedHookPayloads, CapturedHookPayloads)>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        usage.push(body).await;
        (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
    }

    let exchange = CapturedHookPayloads::default();
    let usage = CapturedHookPayloads::default();
    let app = Router::new()
        .route("/exchange", post(exchange_handler))
        .route("/usage", post(usage_handler))
        .with_state((exchange.clone(), usage.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind exchange/usage hook capture mock");
    let addr = listener.local_addr().expect("hook capture local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("exchange/usage hook capture mock server");
    });

    (format!("http://{addr}"), exchange, usage, server)
}

async fn enable_exchange_hook_for_default_namespace(state: &Arc<AppState>, hook_base: &str) {
    let dispatcher = crate::hooks::HookDispatcher::new(&crate::config::HookConfig {
        max_pending_bytes: 4 * 1024 * 1024,
        timeout: std::time::Duration::from_secs(5),
        failure_threshold: 3,
        cooldown: std::time::Duration::from_secs(1),
        exchange: Some(crate::config::HookEndpointConfig {
            url: format!("{hook_base}/exchange"),
            authorization: None,
        }),
        usage: None,
    })
    .expect("hook dispatcher");
    state
        .runtime
        .write()
        .await
        .namespaces
        .get_mut(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .hooks = Some(dispatcher);
}

async fn enable_exchange_usage_hooks_for_default_namespace(state: &Arc<AppState>, hook_base: &str) {
    let dispatcher = crate::hooks::HookDispatcher::new(&crate::config::HookConfig {
        max_pending_bytes: 4 * 1024 * 1024,
        timeout: std::time::Duration::from_secs(5),
        failure_threshold: 3,
        cooldown: std::time::Duration::from_secs(1),
        exchange: Some(crate::config::HookEndpointConfig {
            url: format!("{hook_base}/exchange"),
            authorization: None,
        }),
        usage: Some(crate::config::HookEndpointConfig {
            url: format!("{hook_base}/usage"),
            authorization: None,
        }),
    })
    .expect("hook dispatcher");
    state
        .runtime
        .write()
        .await
        .namespaces
        .get_mut(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .hooks = Some(dispatcher);
}

async fn wait_for_hook_payload(captured: &CapturedHookPayloads) -> Value {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Some(payload) = captured.snapshot().await.into_iter().next() {
            return payload;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("expected hook payload to arrive")
}

async fn wait_for_hook_payload_count(captured: &CapturedHookPayloads, count: usize) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut payloads = Vec::new();
    while tokio::time::Instant::now() < deadline {
        payloads = captured.snapshot().await;
        if payloads.len() >= count {
            return payloads;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("expected {count} hook payload(s), got {}", payloads.len())
}

fn parse_sse_events(body: &[u8]) -> Vec<Value> {
    let mut buffer = body.to_vec();
    let mut events = Vec::new();
    while let Some(event) = crate::streaming::take_one_sse_event(&mut buffer) {
        events.push(event);
    }
    events
}

async fn spawn_openai_completion_stream_mock_with_events(
    response_events: Vec<Value>,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    use bytes::Bytes;
    use futures_util::stream;

    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_events: Vec<Value>,
    }

    async fn handle_chat_completions(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        state.requests.lock().await.push(body);

        let pieces = state
            .response_events
            .iter()
            .flat_map(|event| {
                let event_bytes =
                    serde_json::to_vec(event).expect("serialize OpenAI streaming payload");
                vec![
                    Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: ")),
                    Ok(Bytes::from(event_bytes)),
                    Ok(Bytes::from_static(b"\n\n")),
                ]
            })
            .chain(std::iter::once(Ok(Bytes::from_static(b"data: [DONE]\n\n"))))
            .collect::<Vec<_>>();
        let body_stream = stream::iter(pieces);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(body_stream))
            .expect("streaming response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(MockState {
            requests: requests.clone(),
            response_events,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind OpenAI stream mock upstream");
    let addr = listener
        .local_addr()
        .expect("OpenAI stream mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("OpenAI stream mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn spawn_openai_responses_mock(
    response_body: Value,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_body: Value,
    }

    async fn handle_responses(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.requests.lock().await.push(body);
        Json(state.response_body)
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/responses", post(handle_responses))
        .with_state(MockState {
            requests: requests.clone(),
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind OpenAI Responses mock upstream");
    let addr = listener
        .local_addr()
        .expect("OpenAI Responses mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("OpenAI Responses mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn spawn_openai_completion_raw_mock(
    status: StatusCode,
    response_body: String,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        status: StatusCode,
        response_body: String,
    }

    async fn handle_chat_completions(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        state.requests.lock().await.push(body);
        Response::builder()
            .status(state.status)
            .header("Content-Type", "application/json")
            .body(Body::from(state.response_body))
            .expect("raw OpenAI completion mock response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(MockState {
            requests: requests.clone(),
            status,
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw OpenAI completion mock upstream");
    let addr = listener
        .local_addr()
        .expect("raw OpenAI completion mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("raw OpenAI completion mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn spawn_openai_completion_prompt_cache_echo_error_mock(
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
    }

    async fn handle_chat_completions(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        let prompt_cache_key = body
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .unwrap_or("<missing>")
            .to_string();
        state.requests.lock().await.push(body);
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "error": {
                        "message": format!("upstream echoed prompt_cache_key {prompt_cache_key}"),
                        "prompt_cache_key": prompt_cache_key
                    }
                })
                .to_string(),
            ))
            .expect("prompt-cache echo error mock response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(MockState {
            requests: requests.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind prompt-cache echo OpenAI mock upstream");
    let addr = listener
        .local_addr()
        .expect("prompt-cache echo OpenAI mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("prompt-cache echo OpenAI mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn spawn_openai_completion_prompt_cache_echo_header_mock(
    status: StatusCode,
    response_body: Value,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        status: StatusCode,
        response_body: Value,
    }

    async fn handle_chat_completions(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        let prompt_cache_key = body
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .unwrap_or("<missing>")
            .to_string();
        state.requests.lock().await.push(body);
        Response::builder()
            .status(state.status)
            .header("Content-Type", "application/json")
            .header("x-request-id", format!("upstream-echo-{prompt_cache_key}"))
            .body(Body::from(state.response_body.to_string()))
            .expect("prompt-cache echo header mock response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(MockState {
            requests: requests.clone(),
            status,
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind prompt-cache echo header OpenAI mock upstream");
    let addr = listener
        .local_addr()
        .expect("prompt-cache echo header OpenAI mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("prompt-cache echo header OpenAI mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn spawn_openai_raw_mock(
    status: StatusCode,
    response_body: String,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        status: StatusCode,
        response_body: String,
    }

    async fn handle_openai_endpoint(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        state.requests.lock().await.push(body);
        Response::builder()
            .status(state.status)
            .header("Content-Type", "application/json")
            .body(Body::from(state.response_body))
            .expect("raw OpenAI mock response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_openai_endpoint))
        .route("/responses", post(handle_openai_endpoint))
        .with_state(MockState {
            requests: requests.clone(),
            status,
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw OpenAI mock upstream");
    let addr = listener.local_addr().expect("raw OpenAI mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("raw OpenAI mock server");
    });

    (format!("http://{addr}"), requests, server)
}

async fn spawn_openai_auth_recording_mock(
    status: StatusCode,
    response_body: String,
) -> (
    String,
    Arc<Mutex<Vec<Option<String>>>>,
    tokio::task::JoinHandle<()>,
) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Option<String>>>>,
        status: StatusCode,
        response_body: String,
    }

    async fn handle_openai_endpoint(
        State(state): State<MockState>,
        headers: HeaderMap,
        Json(_body): Json<Value>,
    ) -> Response<Body> {
        state.requests.lock().await.push(
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
        );
        Response::builder()
            .status(state.status)
            .header("Content-Type", "application/json")
            .body(Body::from(state.response_body))
            .expect("auth recording OpenAI mock response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_openai_endpoint))
        .route("/responses", post(handle_openai_endpoint))
        .with_state(MockState {
            requests: requests.clone(),
            status,
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind auth recording OpenAI mock upstream");
    let addr = listener
        .local_addr()
        .expect("auth recording OpenAI mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("auth recording OpenAI mock server");
    });

    (format!("http://{addr}"), requests, server)
}

fn snapshot_race_config(
    api_root: &str,
    upstream_format: crate::formats::UpstreamFormat,
    provider_key: Option<&str>,
) -> crate::config::Config {
    crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![redaction_upstream_config(
            "primary",
            api_root,
            upstream_format,
            provider_key.map(|key| crate::config::SecretSourceConfig {
                inline: Some(key.to_string()),
                env: None,
            }),
        )],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    }
}

async fn spawn_header_delayed_openai_completion_stream_mock(
    header_delay: std::time::Duration,
    sentinel_body: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct SlowHeaderState {
        header_delay: std::time::Duration,
        sentinel_body: &'static str,
    }

    async fn handle_chat_completions(
        State(state): State<SlowHeaderState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        assert_eq!(body.get("stream").and_then(Value::as_bool), Some(true));
        tokio::time::sleep(state.header_delay).await;

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from(format!(
                "data: {{\"sentinel\":\"{}\"}}\n\n",
                state.sentinel_body
            )))
            .expect("delayed header streaming response")
    }

    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(SlowHeaderState {
            header_delay,
            sentinel_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind header-delayed mock upstream");
    let addr = listener.local_addr().expect("header-delayed local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("header-delayed mock server");
    });

    (format!("http://{addr}"), server)
}

async fn set_resource_limits(state: &Arc<AppState>, limits: crate::config::ResourceLimits) {
    let mut runtime = state.runtime.write().await;
    runtime
        .namespaces
        .get_mut(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .config
        .resource_limits = limits;
}

async fn app_state_with_all_provider_redaction_sources(
    api_root: &str,
    upstream_format: crate::formats::UpstreamFormat,
    data_access: data_auth::DataAccess,
) -> Arc<AppState> {
    app_state_for_redaction_upstreams(
        vec![
            redaction_upstream_config(
                "inline",
                api_root,
                upstream_format,
                Some(crate::config::SecretSourceConfig {
                    inline: Some(PROVIDER_INLINE_REDACTION_SECRET.to_string()),
                    env: None,
                }),
            ),
            redaction_upstream_config(
                "env",
                api_root,
                upstream_format,
                Some(crate::config::SecretSourceConfig {
                    inline: None,
                    env: Some(PROVIDER_ENV_REDACTION_ENV.to_string()),
                }),
            ),
            redaction_upstream_config(
                "structured-env",
                api_root,
                upstream_format,
                Some(crate::config::SecretSourceConfig {
                    inline: None,
                    env: Some(PROVIDER_STRUCTURED_ENV_REDACTION_ENV.to_string()),
                }),
            ),
        ],
        data_access,
    )
    .await
}

fn data_access_from_static_env_proxy_key() -> data_auth::DataAccess {
    let data_auth = crate::config::DataAuthConfig {
        mode: crate::config::DataAuthMode::ProxyKey,
        proxy_key: Some(crate::config::SecretSourceConfig {
            inline: None,
            env: Some(PROXY_ENV_REDACTION_ENV.to_string()),
        }),
    };
    data_auth::RuntimeDataAuthState::from_static_config(Some(&data_auth))
        .access()
        .clone()
}

fn data_access_from_default_env_proxy_key() -> data_auth::DataAccess {
    data_auth::RuntimeDataAuthState::from_static_config(None)
        .access()
        .clone()
}

fn upstream_error_body_with_secrets(proxy_secret: &str, extra_secret: Option<&str>) -> String {
    let mut message = format!(
        "upstream denied provider inline {PROVIDER_INLINE_REDACTION_SECRET}, provider env {PROVIDER_ENV_REDACTION_SECRET}, structured provider env {PROVIDER_STRUCTURED_ENV_REDACTION_SECRET}, proxy key {proxy_secret}"
    );
    if let Some(extra_secret) = extra_secret {
        message.push_str(&format!(", request key {extra_secret}"));
    }
    serde_json::json!({ "error": { "message": message } }).to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn stale_client_provider_key_context_does_not_use_new_proxy_server_key() {
    let client_key = "old-client-provider-key";
    let new_server_key = "new-server-provider-key";
    let upstream_body = serde_json::json!({
        "id": "chatcmpl_snapshot",
        "object": "chat.completion",
        "created": 0,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }]
    })
    .to_string();
    let (mock_base, auth_headers, server) =
        spawn_openai_auth_recording_mock(StatusCode::OK, upstream_body).await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "primary",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        )],
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let old_runtime = state.runtime.read().await.clone();
    let auth_context = request_auth_context_for_runtime(
        old_runtime,
        data_auth::DataAccess::ClientProviderKey,
        data_auth::RequestAuthorization::ClientProviderKey {
            provider_key: client_key.to_string(),
        },
    );
    replace_runtime_and_data_auth(
        &state,
        snapshot_race_config(
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            Some(new_server_key),
        ),
        data_auth::DataAccess::ProxyKey {
            key: "new-proxy-key".to_string(),
        },
    )
    .await;

    let response = handle_request_core_with_auth_context(
        state,
        TestRequestCoreRequest {
            namespace: DEFAULT_NAMESPACE.to_string(),
            headers: HeaderMap::new(),
            path: "/openai/v1/chat/completions".to_string(),
            body: serde_json::json!({
                "model": "primary:gpt-4o-mini",
                "messages": [{ "role": "user", "content": "Hi" }],
                "stream": false
            }),
            requested_model: "primary:gpt-4o-mini".to_string(),
            client_format: crate::formats::UpstreamFormat::OpenAiChatCompletions,
            forced_stream: None,
            auth_context,
        },
    )
    .await;

    assert!(
        response.status().is_success(),
        "stale client snapshot response status={}",
        response.status()
    );
    assert_eq!(
        auth_headers.lock().await.as_slice(),
        &[Some(format!("Bearer {client_key}"))]
    );
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn stale_proxy_key_context_does_not_drop_or_replace_old_server_key() {
    let old_server_key = "old-server-provider-key";
    let upstream_body = serde_json::json!({
        "id": "chatcmpl_snapshot",
        "object": "chat.completion",
        "created": 0,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }]
    })
    .to_string();
    let (mock_base, auth_headers, server) =
        spawn_openai_auth_recording_mock(StatusCode::OK, upstream_body).await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "primary",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            Some(crate::config::SecretSourceConfig {
                inline: Some(old_server_key.to_string()),
                env: None,
            }),
        )],
        data_auth::DataAccess::ProxyKey {
            key: "old-proxy-key".to_string(),
        },
    )
    .await;
    let old_runtime = state.runtime.read().await.clone();
    let auth_context = request_auth_context_for_runtime(
        old_runtime,
        data_auth::DataAccess::ProxyKey {
            key: "old-proxy-key".to_string(),
        },
        data_auth::RequestAuthorization::ProxyKey,
    );
    replace_runtime_and_data_auth(
        &state,
        snapshot_race_config(
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        ),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;

    let response = handle_request_core_with_auth_context(
        state,
        TestRequestCoreRequest {
            namespace: DEFAULT_NAMESPACE.to_string(),
            headers: HeaderMap::new(),
            path: "/openai/v1/chat/completions".to_string(),
            body: serde_json::json!({
                "model": "primary:gpt-4o-mini",
                "messages": [{ "role": "user", "content": "Hi" }],
                "stream": false
            }),
            requested_model: "primary:gpt-4o-mini".to_string(),
            client_format: crate::formats::UpstreamFormat::OpenAiChatCompletions,
            forced_stream: None,
            auth_context,
        },
    )
    .await;

    assert!(
        response.status().is_success(),
        "stale proxy snapshot response status={}",
        response.status()
    );
    assert_eq!(
        auth_headers.lock().await.as_slice(),
        &[Some(format!("Bearer {old_server_key}"))]
    );
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn redactor_uses_same_snapshot_as_outbound_runtime_after_auth_race() {
    let old_server_key = "old-server-provider-redaction-key";
    let upstream_body = serde_json::json!({
        "error": { "message": format!("upstream echoed {old_server_key}") }
    })
    .to_string();
    let (mock_base, auth_headers, server) =
        spawn_openai_auth_recording_mock(StatusCode::UNAUTHORIZED, upstream_body).await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "primary",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            Some(crate::config::SecretSourceConfig {
                inline: Some(old_server_key.to_string()),
                env: None,
            }),
        )],
        data_auth::DataAccess::ProxyKey {
            key: "old-proxy-key".to_string(),
        },
    )
    .await;
    let old_runtime = state.runtime.read().await.clone();
    let auth_context = request_auth_context_for_runtime(
        old_runtime,
        data_auth::DataAccess::ProxyKey {
            key: "old-proxy-key".to_string(),
        },
        data_auth::RequestAuthorization::ProxyKey,
    );
    replace_runtime_and_data_auth(
        &state,
        snapshot_race_config(
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        ),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;

    let (response, logs) = capture_error_logs(handle_request_core_with_auth_context(
        state,
        TestRequestCoreRequest {
            namespace: DEFAULT_NAMESPACE.to_string(),
            headers: HeaderMap::new(),
            path: "/openai/v1/chat/completions".to_string(),
            body: serde_json::json!({
                "model": "primary:gpt-4o-mini",
                "messages": [{ "role": "user", "content": "Hi" }],
                "stream": false
            }),
            requested_model: "primary:gpt-4o-mini".to_string(),
            client_format: crate::formats::UpstreamFormat::OpenAiChatCompletions,
            forced_stream: None,
            auth_context,
        },
    ))
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body_text = response_text(response).await;
    assert_no_secret_leak(&body_text, &[old_server_key], "snapshot redaction response");
    assert_no_secret_leak(&logs, &[old_server_key], "snapshot redaction logs");
    assert!(
        body_text.contains("[REDACTED]"),
        "response should show redacted placeholder: {body_text}"
    );
    assert_eq!(
        auth_headers.lock().await.as_slice(),
        &[Some(format!("Bearer {old_server_key}"))]
    );
    server.abort();
}

async fn spawn_anthropic_messages_stream_mock(
    response_events: Vec<Value>,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    use bytes::Bytes;
    use futures_util::stream;

    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_events: Vec<Value>,
    }

    async fn handle_messages(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Response<Body> {
        state.requests.lock().await.push(body);

        let pieces = state
            .response_events
            .iter()
            .flat_map(|event| {
                let event_type = event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message_delta")
                    .to_string();
                let event_bytes =
                    serde_json::to_vec(event).expect("serialize anthropic streaming payload");
                vec![
                    Ok::<Bytes, std::io::Error>(Bytes::from(format!("event: {event_type}\n"))),
                    Ok(Bytes::from_static(b"data: ")),
                    Ok(Bytes::from(event_bytes)),
                    Ok(Bytes::from_static(b"\n\n")),
                ]
            })
            .collect::<Vec<_>>();
        let body_stream = stream::iter(pieces);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(body_stream))
            .expect("streaming response")
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/messages", post(handle_messages))
        .route("/messages", post(handle_messages))
        .with_state(MockState {
            requests: requests.clone(),
            response_events,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind anthropic stream mock upstream");
    let addr = listener
        .local_addr()
        .expect("anthropic stream mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("anthropic stream mock server");
    });

    (format!("http://{addr}"), requests, server)
}

fn anthropic_commentary_then_tool_use_events() -> Vec<Value> {
    vec![
        serde_json::json!({
            "type": "message_start",
            "message": {
                "id": "msg_commentary",
                "type": "message",
                "role": "assistant",
                "model": "claude-3-7-sonnet",
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": { "input_tokens": 0, "output_tokens": 0 }
            }
        }),
        serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        }),
        serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "Preamble line\\n" }
        }),
        serde_json::json!({
            "type": "content_block_stop",
            "index": 0
        }),
        serde_json::json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "tool_use",
                "id": "call_1",
                "name": "exec_command",
                "input": {}
            }
        }),
        serde_json::json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "{\"cmd\":\"pwd\"}" }
        }),
        serde_json::json!({
            "type": "content_block_stop",
            "index": 1
        }),
        serde_json::json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" },
            "usage": { "input_tokens": 12, "output_tokens": 4 }
        }),
        serde_json::json!({
            "type": "message_stop"
        }),
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn non_stream_upstream_error_redacts_provider_and_proxy_secret_sources_from_response_and_logs(
) {
    let _env_guard = SECRET_REDACTION_ENV_LOCK.lock().await;
    let _provider_env =
        ScopedEnvVar::set(PROVIDER_ENV_REDACTION_ENV, PROVIDER_ENV_REDACTION_SECRET);
    let _provider_structured_env = ScopedEnvVar::set(
        PROVIDER_STRUCTURED_ENV_REDACTION_ENV,
        PROVIDER_STRUCTURED_ENV_REDACTION_SECRET,
    );
    let _proxy_env = ScopedEnvVar::set(PROXY_ENV_REDACTION_ENV, PROXY_ENV_REDACTION_SECRET);
    let _auth_mode = ScopedEnvVar::set(data_auth::AUTH_MODE_ENV, "proxy_key");
    let _default_proxy_env =
        ScopedEnvVar::set(data_auth::PROXY_KEY_ENV, PROXY_DEFAULT_ENV_REDACTION_SECRET);

    let cases = vec![
        (
            "inline proxy key",
            data_auth::DataAccess::ProxyKey {
                key: PROXY_INLINE_REDACTION_SECRET.to_string(),
            },
            PROXY_INLINE_REDACTION_SECRET,
        ),
        (
            "env proxy key",
            data_access_from_static_env_proxy_key(),
            PROXY_ENV_REDACTION_SECRET,
        ),
        (
            "default env proxy key",
            data_access_from_default_env_proxy_key(),
            PROXY_DEFAULT_ENV_REDACTION_SECRET,
        ),
    ];

    for (label, data_access, proxy_secret) in cases {
        let upstream_body = upstream_error_body_with_secrets(proxy_secret, None);
        let (mock_base, requests, server) =
            spawn_openai_raw_mock(StatusCode::UNAUTHORIZED, upstream_body).await;
        let state = app_state_with_all_provider_redaction_sources(
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            data_access,
        )
        .await;

        let (response, logs) = capture_error_logs(handle_request_core(
            state,
            DEFAULT_NAMESPACE.to_string(),
            HeaderMap::new(),
            "/openai/v1/chat/completions".to_string(),
            serde_json::json!({
                "model": "inline:gpt-4o-mini",
                "messages": [{ "role": "user", "content": "Hi" }],
                "stream": false
            }),
            "inline:gpt-4o-mini".to_string(),
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        ))
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
        let body_text = response_text(response).await;
        let secrets = [
            PROVIDER_INLINE_REDACTION_SECRET,
            PROVIDER_ENV_REDACTION_SECRET,
            PROVIDER_STRUCTURED_ENV_REDACTION_SECRET,
            proxy_secret,
        ];
        assert_no_secret_leak(&body_text, &secrets, label);
        assert!(
            body_text.contains("[REDACTED]"),
            "{label} response should show redacted placeholder: {body_text}"
        );
        assert_no_secret_leak(&logs, &secrets, label);
        assert!(
            logs.contains("[REDACTED]"),
            "{label} logs should show redacted placeholder: {logs}"
        );
        assert_eq!(requests.lock().await.len(), 1, "{label}");
        server.abort();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn streaming_upstream_error_redacts_runtime_secrets_from_sse_and_logs() {
    let _env_guard = SECRET_REDACTION_ENV_LOCK.lock().await;
    let _provider_env =
        ScopedEnvVar::set(PROVIDER_ENV_REDACTION_ENV, PROVIDER_ENV_REDACTION_SECRET);
    let _provider_structured_env = ScopedEnvVar::set(
        PROVIDER_STRUCTURED_ENV_REDACTION_ENV,
        PROVIDER_STRUCTURED_ENV_REDACTION_SECRET,
    );
    let proxy_secret = PROXY_INLINE_REDACTION_SECRET;
    let upstream_body = upstream_error_body_with_secrets(proxy_secret, None);
    let (mock_base, requests, server) =
        spawn_openai_raw_mock(StatusCode::FORBIDDEN, upstream_body).await;
    let state = app_state_with_all_provider_redaction_sources(
        &mock_base,
        crate::formats::UpstreamFormat::OpenAiResponses,
        data_auth::DataAccess::ProxyKey {
            key: proxy_secret.to_string(),
        },
    )
    .await;

    let (response, logs) = capture_error_logs(handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "inline:gpt-4o-mini",
            "input": "Hi",
            "stream": true
        }),
        "inline:gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    ))
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_text(response).await;
    let secrets = [
        PROVIDER_INLINE_REDACTION_SECRET,
        PROVIDER_ENV_REDACTION_SECRET,
        PROVIDER_STRUCTURED_ENV_REDACTION_SECRET,
        proxy_secret,
    ];
    assert!(body_text.contains("response.failed"), "body = {body_text}");
    assert_no_secret_leak(&body_text, &secrets, "streaming SSE");
    assert!(
        body_text.contains("[REDACTED]"),
        "streaming SSE should show redacted placeholder: {body_text}"
    );
    assert_no_secret_leak(&logs, &secrets, "streaming logs");
    assert!(
        logs.contains("[REDACTED]"),
        "streaming logs should show redacted placeholder: {logs}"
    );
    assert_eq!(requests.lock().await.len(), 1);
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn client_provider_key_upstream_error_redaction_is_request_scoped_and_not_in_admin_state() {
    let client_secret = CLIENT_PROVIDER_REDACTION_SECRET;
    let upstream_body = serde_json::json!({
        "error": { "message": format!("upstream echoed client provider key {client_secret}") }
    })
    .to_string();
    let (mock_base, requests, server) =
        spawn_openai_raw_mock(StatusCode::UNAUTHORIZED, upstream_body).await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "client",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        )],
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {client_secret}")).expect("client credential"),
    );

    let (response, logs) = capture_error_logs(handle_request_core(
        state.clone(),
        DEFAULT_NAMESPACE.to_string(),
        headers,
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "client:gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        "client:gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    ))
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body_text = response_text(response).await;
    assert_no_secret_leak(&body_text, &[client_secret], "client-provider response");
    assert!(
        body_text.contains("[REDACTED]"),
        "client-provider response should show redacted placeholder: {body_text}"
    );
    assert_no_secret_leak(&logs, &[client_secret], "client-provider logs");
    assert!(
        logs.contains("[REDACTED]"),
        "client-provider logs should show redacted placeholder: {logs}"
    );

    let admin_response = crate::server::admin::handle_admin_namespace_state(
        State(state),
        Path(DEFAULT_NAMESPACE.to_string()),
    )
    .await
    .into_response();
    assert_eq!(admin_response.status(), StatusCode::OK);
    let admin_body = response_text(admin_response).await;
    assert_no_secret_leak(&admin_body, &[client_secret], "admin namespace state");

    assert_eq!(requests.lock().await.len(), 1);
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn non_stream_success_redacts_known_credentials_from_response_and_debug_trace() {
    let server_provider_secret = "sv3";
    let proxy_secret = "px7";
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_success_redaction",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": format!("server echoed provider {server_provider_secret} and proxy {proxy_secret}")
            },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, _requests, server) = spawn_openai_completion_mock(server_response_body).await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "primary",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            Some(crate::config::SecretSourceConfig {
                inline: Some(server_provider_secret.to_string()),
                env: None,
            }),
        )],
        data_auth::DataAccess::ProxyKey {
            key: proxy_secret.to_string(),
        },
    )
    .await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "primary:gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        "primary:gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_text(response).await;
    let server_secrets = [server_provider_secret, proxy_secret];
    assert_no_secret_leak(&body_text, &server_secrets, "success JSON response");
    assert!(
        body_text.contains("[REDACTED]"),
        "success JSON response should show redaction placeholder: {body_text}"
    );
    let trace = wait_for_debug_trace_response(&trace_path).await;
    assert_no_secret_leak(&trace, &server_secrets, "success debug trace");
    assert!(
        trace.contains("[REDACTED]"),
        "success debug trace should show redaction placeholder: {trace}"
    );
    server.abort();

    let client_secret = "ck7";
    let client_response_body = serde_json::json!({
        "id": "chatcmpl_client_success_redaction",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": format!("client credential echoed {client_secret}")
            },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, _requests, server) = spawn_openai_completion_mock(client_response_body).await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "client",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        )],
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {client_secret}")).expect("client credential"),
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        headers,
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "client:gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        "client:gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_text(response).await;
    assert_no_secret_leak(&body_text, &[client_secret], "client success JSON response");
    assert!(
        body_text.contains("[REDACTED]"),
        "client success JSON response should show redaction placeholder: {body_text}"
    );
    let trace = wait_for_debug_trace_response(&trace_path).await;
    assert_no_secret_leak(&trace, &[client_secret], "client success debug trace");
    assert!(
        trace.contains("[REDACTED]"),
        "client success debug trace should show redaction placeholder: {trace}"
    );
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn streaming_success_redacts_known_credentials_from_sse_response() {
    let server_provider_secret = "sse-sv3";
    let proxy_secret = "sse-px7";
    let response_events = vec![serde_json::json!({
        "id": "chatcmpl_success_stream_redaction",
        "object": "chat.completion.chunk",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "delta": {
                "content": format!("server stream echoed {server_provider_secret} and {proxy_secret}")
            },
            "finish_reason": null
        }]
    })];
    let (mock_base, _requests, server) =
        spawn_openai_completion_stream_mock_with_events(response_events).await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "primary",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            Some(crate::config::SecretSourceConfig {
                inline: Some(server_provider_secret.to_string()),
                env: None,
            }),
        )],
        data_auth::DataAccess::ProxyKey {
            key: proxy_secret.to_string(),
        },
    )
    .await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "primary:gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }),
        "primary:gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_text(response).await;
    let server_secrets = [server_provider_secret, proxy_secret];
    assert_no_secret_leak(&body_text, &server_secrets, "success SSE response");
    assert!(
        body_text.contains("[REDACTED]"),
        "success SSE response should show redaction placeholder: {body_text}"
    );
    server.abort();

    let client_secret = "sck";
    let response_events = vec![serde_json::json!({
        "id": "chatcmpl_client_success_stream_redaction",
        "object": "chat.completion.chunk",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "delta": {
                "content": format!("client stream echoed {client_secret}")
            },
            "finish_reason": null
        }]
    })];
    let (mock_base, _requests, server) =
        spawn_openai_completion_stream_mock_with_events(response_events).await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "client",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        )],
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {client_secret}")).expect("client credential"),
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        headers,
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "client:gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }),
        "client:gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_text(response).await;
    assert_no_secret_leak(&body_text, &[client_secret], "client success SSE response");
    assert!(
        body_text.contains("[REDACTED]"),
        "client success SSE response should show redaction placeholder: {body_text}"
    );
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn request_metadata_redacts_client_provider_key_in_hook_debug_logs_and_metrics() {
    let client_secret = CLIENT_PROVIDER_REDACTION_SECRET;
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_client_metadata_redaction",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, _requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            "client",
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        )],
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {client_secret}")).expect("client credential"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("openai-organization"),
        HeaderValue::from_str(&format!("org-{client_secret}")).expect("metadata header"),
    );
    let path = format!("/openai/v1/chat/completions/{client_secret}");
    let requested_model = format!("model-{client_secret}");

    let (response, logs) = capture_debug_logs(handle_request_core(
        state.clone(),
        DEFAULT_NAMESPACE.to_string(),
        headers,
        path,
        serde_json::json!({
            "model": requested_model,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        format!("model-{client_secret}"),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    ))
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_text(response).await;
    assert_no_secret_leak(&body_text, &[client_secret], "client metadata response");

    let hook_payload = wait_for_hook_payload(&hook_payloads).await;
    let hook_text = serde_json::to_string(&hook_payload).expect("hook payload json");
    assert_no_secret_leak(&hook_text, &[client_secret], "client metadata hook");
    assert!(hook_text.contains("[REDACTED]"), "hook = {hook_text}");

    let trace = wait_for_debug_trace_response(&trace_path).await;
    assert_no_secret_leak(&trace, &[client_secret], "client metadata debug trace");
    assert!(trace.contains("[REDACTED]"), "trace = {trace}");

    let config = state
        .runtime
        .read()
        .await
        .namespaces
        .get(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .config
        .clone();
    let metrics_text = format!("{:?}", state.metrics.snapshot(&config));
    assert_no_secret_leak(&metrics_text, &[client_secret], "client metadata metrics");
    assert!(
        metrics_text.contains("[REDACTED]"),
        "metrics = {metrics_text}"
    );
    assert_no_secret_leak(&logs, &[client_secret], "client metadata logs");
    assert!(logs.contains("[REDACTED]"), "logs = {logs}");

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn request_processing_observability_is_emitted_to_trace_hooks_and_metrics() {
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_request_processing",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, _requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let (hook_base, exchange_payloads, usage_payloads, hook_server) =
        spawn_exchange_usage_hook_capture_mock().await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    enable_exchange_usage_hooks_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let response = handle_request_core(
        state.clone(),
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "prompt_cache_key": "stable-prefix",
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let _ = response_text(response).await;

    let trace = wait_for_debug_trace_response(&trace_path).await;
    assert!(
        !trace.contains("llmup:v1:"),
        "debug trace must not contain full synthesized prompt_cache_key: {trace}"
    );
    let request_entry = debug_trace_request_entry(&trace);
    assert_debug_trace_omits_stable_prefix(&trace, &request_entry);
    for payload in [
        request_entry,
        wait_for_hook_payload(&exchange_payloads).await,
        wait_for_hook_payload(&usage_payloads).await,
    ] {
        assert_llmup_external_observability(&payload, "client_body_preserved", "preserved_native");
    }

    let config = state
        .runtime
        .read()
        .await
        .namespaces
        .get(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .config
        .clone();
    let snapshot = state.metrics.snapshot(&config);
    let recent = snapshot
        .recent_requests
        .first()
        .expect("recent request should be recorded");
    assert_eq!(
        recent.llmup.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationNotRequired
    );
    assert!(recent.llmup.zero_transform_forwarding_active);
    assert_eq!(
        recent.llmup.state_bridge,
        crate::request_processing::StateBridgeModifier::Off
    );
    assert_eq!(
        recent.llmup.provider_native_prompt_cache,
        crate::request_processing::PromptCacheRequestControl::Preserved
    );

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn debug_trace_preserved_prompt_cache_does_not_backfill_missing_target_fields() {
    let path = std::env::temp_dir().join(format!(
        "llmup-preserved-prompt-cache-trace-{}.jsonl",
        uuid::Uuid::new_v4()
    ));
    let recorder = crate::debug_trace::DebugTraceRecorder::new(&crate::config::DebugTraceConfig {
        path: Some(path.to_string_lossy().to_string()),
        max_text_chars: 4096,
    })
    .expect("debug trace recorder");
    let ctx = crate::debug_trace::DebugTraceContext {
        request_id: uuid::Uuid::new_v4().to_string(),
        timestamp_ms: 1,
        path: "/openai/v1/chat/completions".to_string(),
        stream: false,
        client_model: "gpt-4o-mini".to_string(),
        upstream_name: "primary".to_string(),
        upstream_model: "gpt-4o-mini".to_string(),
        client_format: crate::formats::UpstreamFormat::OpenAiChatCompletions,
        upstream_format: crate::formats::UpstreamFormat::OpenAiResponses,
        llmup: crate::request_processing::RequestProcessingInfo {
            request_processing:
                crate::request_processing::RequestProcessing::RequestTransformationRequired,
            zero_transform_forwarding_active: false,
            state_bridge: crate::request_processing::StateBridgeModifier::Off,
            provider_native_prompt_cache:
                crate::request_processing::PromptCacheRequestControl::Preserved,
        },
    };
    let original_body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": "Hi" }],
        "prompt_cache_key": "stable-prefix"
    });
    let upstream_body = serde_json::json!({
        "model": "gpt-4o-mini",
        "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hi" }] }]
    });

    recorder.record_request_with_upstream(&ctx, &original_body, &upstream_body);

    let request_entries = wait_for_debug_trace_request_count(&path, 1).await;
    let trace = std::fs::read_to_string(&path).expect("debug trace should be readable");
    assert_debug_trace_omits_stable_prefix(&trace, &request_entries[0]);
    let prompt_cache_detail = &request_entries[0]["request"]["prompt_cache_request_control"];
    assert_eq!(
        prompt_cache_detail["disposition"],
        serde_json::json!("preserved_native"),
        "request_entry = {:?}",
        request_entries[0]
    );
    assert_eq!(
        prompt_cache_detail["source_fields"],
        serde_json::json!(["prompt_cache_key"]),
        "request_entry = {:?}",
        request_entries[0]
    );
    assert_eq!(
        prompt_cache_detail["target_fields"],
        serde_json::json!([]),
        "preserved trace detail must reflect the actual upstream body instead of backfilling source fields: {:?}",
        request_entries[0]
    );

    let _ = std::fs::remove_file(path);
}

#[tokio::test(flavor = "current_thread")]
async fn request_processing_observability_marks_local_state_capture_and_expansion() {
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_local_state",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, _requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let first = handle_request_core(
        state.clone(),
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": "First",
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_body: Value = serde_json::from_str(&response_text(first).await).unwrap();
    let local_id = first_body["id"]
        .as_str()
        .expect("local response id")
        .to_string();
    assert!(local_id.starts_with("resp_llmup_"), "body = {first_body:?}");

    let second = handle_request_core(
        state.clone(),
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "previous_response_id": local_id,
            "input": "Second",
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    let _ = response_text(second).await;

    let request_entries = wait_for_debug_trace_request_count(&trace_path, 2).await;
    assert_eq!(
        request_entries[0]["llmup"]["local_state_handling"],
        "capture_candidate"
    );
    assert!(request_entries[0]["llmup"].get("state_bridge").is_none());
    let capture_detail = &request_entries[0]["request"]["conversation_state_bridge"];
    assert_eq!(capture_detail["operation"], "capture");
    assert_eq!(capture_detail["capture_result"], "candidate");
    assert_eq!(capture_detail["request_item_count"], 1);
    assert_eq!(capture_detail["max_bytes"], 256 * 1024 * 1024);

    assert_eq!(
        request_entries[1]["llmup"]["local_state_handling"],
        "expanded"
    );
    assert!(request_entries[1]["llmup"].get("state_bridge").is_none());
    let replay_detail = &request_entries[1]["request"]["conversation_state_bridge"];
    assert_eq!(replay_detail["operation"], "replay");
    assert_eq!(replay_detail["lookup_result"], "hit");
    assert_eq!(replay_detail["stored_item_count"], 2);
    assert_eq!(replay_detail["current_item_count"], 1);
    assert_eq!(replay_detail["expanded_item_count"], 3);

    let hook_payloads = wait_for_hook_payload_count(&hook_payloads, 2).await;
    let mut hook_local_state_handlings = hook_payloads
        .iter()
        .map(|payload| {
            assert!(payload["llmup"].get("state_bridge").is_none());
            assert!(
                !json_contains_key(payload, "conversation_state_bridge"),
                "hook payload must not contain debug trace state bridge detail: {payload:?}"
            );
            payload["llmup"]["local_state_handling"]
                .as_str()
                .expect("hook payload local_state_handling")
        })
        .collect::<Vec<_>>();
    hook_local_state_handlings.sort_unstable();
    assert_eq!(
        hook_local_state_handlings,
        vec!["capture_candidate", "expanded"]
    );

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn debug_trace_records_prompt_cache_disposition_for_explicit_anthropic_mapping() {
    let server_response_body = serde_json::json!({
        "id": "msg_explicit_cache_mapping",
        "type": "message",
        "role": "assistant",
        "model": "claude-3",
        "content": [{ "type": "text", "text": "ok" }],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_anthropic_messages_mock(server_response_body).await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "extra_body": {
                "anthropic": {
                    "cache_control": { "type": "ephemeral", "ttl": "5m" }
                }
            },
            "stream": false
        }),
        "claude-3".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let _ = response_text(response).await;

    let trace = wait_for_debug_trace_response(&trace_path).await;
    let request_entry = debug_trace_request_entry(&trace);
    assert_llmup_external_observability(&request_entry, "constructed", "explicit_extension_mapped");
    assert_eq!(
        request_entry["request"]["prompt_cache_request_control"],
        serde_json::json!({
            "target_provider": "anthropic",
            "disposition": "explicit_extension_mapped",
            "source_fields": ["extra_body.anthropic.cache_control"],
            "target_fields": ["cache_control"],
            "ttl_or_retention_source": "extra_body.anthropic.cache_control.ttl"
        }),
        "request_entry = {request_entry:?}"
    );
    let hook_payload = wait_for_hook_payload(&hook_payloads).await;
    assert_llmup_external_observability(&hook_payload, "constructed", "explicit_extension_mapped");
    assert_prompt_cache_request_control_detail_absent_from_hook_payload(&hook_payload);

    let requests = requests.lock().await;
    assert_eq!(
        requests[0]["cache_control"],
        serde_json::json!({ "type": "ephemeral", "ttl": "5m" })
    );
    assert!(
        requests[0].get("extra_body").is_none(),
        "upstream body = {:?}",
        requests[0]
    );

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn debug_trace_records_prompt_cache_components_for_openai_to_anthropic_mixed_mapping_and_drop(
) {
    let server_response_body = serde_json::json!({
        "id": "msg_mixed_cache_mapping",
        "type": "message",
        "role": "assistant",
        "model": "claude-3",
        "content": [{ "type": "text", "text": "ok" }],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_anthropic_messages_mock(server_response_body).await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "prompt_cache_key": "stable-prefix",
            "prompt_cache_retention": "24h",
            "extra_body": {
                "anthropic": {
                    "cache_control": { "type": "ephemeral", "ttl": "5m" }
                }
            },
            "stream": false
        }),
        "claude-3".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-llmup-portability-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("prompt_cache_key")),
        "warnings = {warnings:?}"
    );
    let _ = response_text(response).await;

    let trace = wait_for_debug_trace_response(&trace_path).await;
    let request_entry = debug_trace_request_entry(&trace);
    assert_llmup_external_observability(&request_entry, "constructed", "explicit_extension_mapped");
    assert_debug_trace_omits_stable_prefix(&trace, &request_entry);
    let prompt_cache_detail = &request_entry["request"]["prompt_cache_request_control"];
    assert_eq!(
        prompt_cache_detail,
        &serde_json::json!({
            "target_provider": "anthropic",
            "disposition": "explicit_extension_mapped",
            "source_fields": ["extra_body.anthropic.cache_control"],
            "target_fields": ["cache_control"],
            "ttl_or_retention_source": "extra_body.anthropic.cache_control.ttl",
            "components": [
                {
                    "disposition": "explicit_extension_mapped",
                    "source_fields": ["extra_body.anthropic.cache_control"],
                    "target_fields": ["cache_control"],
                    "ttl_or_retention_source": "extra_body.anthropic.cache_control.ttl"
                },
                {
                    "disposition": "dropped",
                    "source_fields": ["prompt_cache_key", "prompt_cache_retention"],
                    "target_fields": [],
                    "ttl_or_retention_source": "prompt_cache_retention",
                    "omitted_reason": "openai_prompt_cache_key_not_anthropic_cache_control_or_breakpoint"
                }
            ]
        }),
        "request_entry = {request_entry:?}"
    );
    assert!(
        !serde_json::to_string(prompt_cache_detail)
            .expect("prompt-cache trace detail serializes")
            .contains("stable-prefix"),
        "prompt-cache trace detail must not contain key values: {prompt_cache_detail:?}"
    );
    let hook_payload = wait_for_hook_payload(&hook_payloads).await;
    assert_llmup_external_observability(&hook_payload, "constructed", "explicit_extension_mapped");
    assert_prompt_cache_request_control_detail_absent_from_hook_payload(&hook_payload);

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["cache_control"],
        serde_json::json!({ "type": "ephemeral", "ttl": "5m" })
    );
    assert!(
        requests[0].get("prompt_cache_key").is_none(),
        "upstream body = {:?}",
        requests[0]
    );
    assert!(
        requests[0].get("prompt_cache_retention").is_none(),
        "upstream body = {:?}",
        requests[0]
    );
    assert!(
        requests[0].get("extra_body").is_none(),
        "upstream body = {:?}",
        requests[0]
    );

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn debug_trace_records_prompt_cache_disposition_for_explicit_openai_mapping() {
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_explicit_openai_cache_mapping",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/anthropic/v1/messages".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "max_tokens": 32,
            "messages": [{
                "role": "user",
                "content": [{ "type": "text", "text": "Hi" }]
            }],
            "extra_body": {
                "openai": {
                    "prompt_cache_key": "stable-prefix",
                    "prompt_cache_retention": "24h"
                }
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::Anthropic,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let _ = response_text(response).await;

    let trace = wait_for_debug_trace_response(&trace_path).await;
    let request_entry = debug_trace_request_entry(&trace);
    assert_llmup_external_observability(&request_entry, "constructed", "explicit_extension_mapped");
    assert_debug_trace_omits_stable_prefix(&trace, &request_entry);
    let prompt_cache_detail = &request_entry["request"]["prompt_cache_request_control"];
    assert_eq!(
        prompt_cache_detail,
        &serde_json::json!({
            "target_provider": "openai_family",
            "disposition": "explicit_extension_mapped",
            "source_fields": [
                "extra_body.openai.prompt_cache_key",
                "extra_body.openai.prompt_cache_retention"
            ],
            "target_fields": ["prompt_cache_key", "prompt_cache_retention"],
            "ttl_or_retention_source": "extra_body.openai.prompt_cache_retention"
        }),
        "request_entry = {request_entry:?}"
    );
    assert!(
        !serde_json::to_string(prompt_cache_detail)
            .expect("prompt-cache trace detail serializes")
            .contains("stable-prefix"),
        "prompt-cache trace detail must not contain key values: {prompt_cache_detail:?}"
    );
    let hook_payload = wait_for_hook_payload(&hook_payloads).await;
    assert_llmup_external_observability(&hook_payload, "constructed", "explicit_extension_mapped");
    assert_prompt_cache_request_control_detail_absent_from_hook_payload(&hook_payload);

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["prompt_cache_key"], "stable-prefix");
    assert_eq!(requests[0]["prompt_cache_retention"], "24h");
    assert!(
        requests[0].get("extra_body").is_none(),
        "upstream body = {:?}",
        requests[0]
    );
    assert!(
        requests[0].get("cache_control").is_none(),
        "upstream body = {:?}",
        requests[0]
    );
    assert!(
        !json_contains_key(&requests[0], "cache_control"),
        "upstream body = {:?}",
        requests[0]
    );

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn debug_trace_records_prompt_cache_components_for_anthropic_to_openai_mixed_mapping_and_drop(
) {
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_mixed_openai_cache_mapping",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/anthropic/v1/messages".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "max_tokens": 32,
            "system": [
                { "type": "text", "text": "System", "cache_control": { "type": "ephemeral", "ttl": "5m" } }
            ],
            "messages": [{
                "role": "user",
                "content": [{ "type": "text", "text": "Hi" }]
            }],
            "extra_body": {
                "openai": {
                    "prompt_cache_key": "stable-prefix",
                    "prompt_cache_retention": "24h"
                }
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::Anthropic,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-llmup-portability-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("cache_control")),
        "warnings = {warnings:?}"
    );
    let _ = response_text(response).await;

    let trace = wait_for_debug_trace_response(&trace_path).await;
    let request_entry = debug_trace_request_entry(&trace);
    assert_llmup_external_observability(&request_entry, "constructed", "explicit_extension_mapped");
    assert_debug_trace_omits_stable_prefix(&trace, &request_entry);
    let prompt_cache_detail = &request_entry["request"]["prompt_cache_request_control"];
    assert_eq!(
        prompt_cache_detail,
        &serde_json::json!({
            "target_provider": "openai_family",
            "disposition": "explicit_extension_mapped",
            "source_fields": [
                "extra_body.openai.prompt_cache_key",
                "extra_body.openai.prompt_cache_retention"
            ],
            "target_fields": ["prompt_cache_key", "prompt_cache_retention"],
            "ttl_or_retention_source": "extra_body.openai.prompt_cache_retention",
            "components": [
                {
                    "disposition": "explicit_extension_mapped",
                    "source_fields": [
                        "extra_body.openai.prompt_cache_key",
                        "extra_body.openai.prompt_cache_retention"
                    ],
                    "target_fields": ["prompt_cache_key", "prompt_cache_retention"],
                    "ttl_or_retention_source": "extra_body.openai.prompt_cache_retention"
                },
                {
                    "disposition": "dropped",
                    "source_fields": ["system[].cache_control"],
                    "target_fields": [],
                    "ttl_or_retention_source": "system[].cache_control.ttl",
                    "omitted_reason": "anthropic_cache_control_not_openai_prompt_cache_key"
                }
            ]
        }),
        "request_entry = {request_entry:?}"
    );
    assert!(
        !serde_json::to_string(prompt_cache_detail)
            .expect("prompt-cache trace detail serializes")
            .contains("stable-prefix"),
        "prompt-cache trace detail must not contain key values: {prompt_cache_detail:?}"
    );
    let hook_payload = wait_for_hook_payload(&hook_payloads).await;
    assert_llmup_external_observability(&hook_payload, "constructed", "explicit_extension_mapped");
    assert_prompt_cache_request_control_detail_absent_from_hook_payload(&hook_payload);

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["prompt_cache_key"], "stable-prefix");
    assert_eq!(requests[0]["prompt_cache_retention"], "24h");
    assert!(
        requests[0].get("extra_body").is_none(),
        "upstream body = {:?}",
        requests[0]
    );
    assert!(
        !json_contains_key(&requests[0], "cache_control"),
        "upstream body = {:?}",
        requests[0]
    );

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_to_openai_chat_synthesizes_prompt_cache_key_from_final_static_prefix() {
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_synthesized_cache_key",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    async fn send(
        state: Arc<AppState>,
        user_text: &str,
        system_text: &str,
        tool_schema: Value,
    ) -> Response<Body> {
        handle_request_core(
            state,
            DEFAULT_NAMESPACE.to_string(),
            HeaderMap::new(),
            "/anthropic/v1/messages".to_string(),
            serde_json::json!({
                "model": "gpt-4o-mini",
                "max_tokens": 32,
                "system": [{ "type": "text", "text": system_text }],
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": user_text }]
                }],
                "tools": [{
                    "name": "lookup",
                    "description": "Look up static facts.",
                    "input_schema": tool_schema
                }],
                "stream": false
            }),
            "gpt-4o-mini".to_string(),
            crate::formats::UpstreamFormat::Anthropic,
            None,
        )
        .await
    }

    let base_schema = serde_json::json!({
        "type": "object",
        "properties": { "query": { "type": "string" } },
        "required": ["query"]
    });
    let changed_schema = serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" },
            "locale": { "type": "string" }
        },
        "required": ["query"]
    });

    for response in [
        send(
            state.clone(),
            "dynamic tail one",
            "Be terse and precise.",
            base_schema.clone(),
        )
        .await,
        send(
            state.clone(),
            "dynamic tail two",
            "Be terse and precise.",
            base_schema.clone(),
        )
        .await,
        send(
            state.clone(),
            "dynamic tail two",
            "Be expansive and precise.",
            base_schema.clone(),
        )
        .await,
        send(
            state,
            "dynamic tail two",
            "Be terse and precise.",
            changed_schema,
        )
        .await,
    ] {
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response_text(response).await;
    }

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 4);
    let keys = requests
        .iter()
        .map(|request| {
            let key = request["prompt_cache_key"]
                .as_str()
                .expect("synthesized prompt_cache_key")
                .to_string();
            assert!(
                key.starts_with("llmup:v1:"),
                "synthesized key should use llmup v1 fingerprint format: {key}"
            );
            assert!(
                !key.contains("dynamic tail") && !key.contains("Be terse"),
                "synthesized key must not contain prompt text: {key}"
            );
            assert!(
                request.get("prompt_cache_retention").is_none(),
                "synthesis must not set retention: {request:?}"
            );
            key
        })
        .collect::<Vec<_>>();

    assert_eq!(
        keys[0], keys[1],
        "different final user messages must share the same static-prefix key"
    );
    assert_ne!(keys[0], keys[2], "system changes must change the key");
    assert_ne!(keys[0], keys[3], "tool schema changes must change the key");

    upstream_server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_to_openai_responses_synthesizes_prompt_cache_key_without_retention() {
    let server_response_body = serde_json::json!({
        "id": "resp_synthesized_cache_key",
        "object": "response",
        "created_at": 123,
        "status": "completed",
        "model": "gpt-4o-mini",
        "output": []
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_responses_mock(server_response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/anthropic/v1/messages".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "max_tokens": 32,
            "system": [{ "type": "text", "text": "Be terse." }],
            "messages": [{
                "role": "user",
                "content": [{ "type": "text", "text": "Hi" }]
            }],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::Anthropic,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let _ = response_text(response).await;
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    let key = requests[0]["prompt_cache_key"]
        .as_str()
        .expect("synthesized prompt_cache_key");
    assert!(key.starts_with("llmup:v1:"), "key = {key}");
    assert!(
        requests[0].get("prompt_cache_retention").is_none(),
        "synthesis must not set retention: {:?}",
        requests[0]
    );

    upstream_server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_cache_control_changes_do_not_drive_synthesized_openai_key() {
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_cache_control_independent_key",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    for ttl in ["5m", "1h"] {
        let response = handle_request_core(
            state.clone(),
            DEFAULT_NAMESPACE.to_string(),
            HeaderMap::new(),
            "/anthropic/v1/messages".to_string(),
            serde_json::json!({
                "model": "gpt-4o-mini",
                "max_tokens": 32,
                "system": [{
                    "type": "text",
                    "text": "Stable system text.",
                    "cache_control": { "type": "ephemeral", "ttl": ttl }
                }],
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": "Hi" }]
                }],
                "stream": false
            }),
            "gpt-4o-mini".to_string(),
            crate::formats::UpstreamFormat::Anthropic,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response_text(response).await;
    }

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["prompt_cache_key"], requests[1]["prompt_cache_key"],
        "Anthropic cache_control TTL must not directly determine the OpenAI prompt_cache_key"
    );
    for request in requests.iter() {
        assert!(
            !json_contains_key(request, "cache_control"),
            "upstream body must not hard-map Anthropic cache_control: {request:?}"
        );
    }

    upstream_server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_claude_code_second_round_thinking_context_management_reaches_openai_chat() {
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_claude_code_second_round",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "preset-openai-compatible".to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: "gpt-4o-mini".to_string(),
                limits: None,
                surface: None,
            },
        );
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/anthropic/v1/messages".to_string(),
        serde_json::json!({
            "model": "preset-openai-compatible",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 1024 },
            "context_management": { "type": "auto" },
            "messages": [
                {
                    "role": "user",
                    "content": [{ "type": "text", "text": "hi" }]
                },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "I should answer briefly and keep visible context.",
                            "signature": "claude-code-prior-visible-thinking-signature"
                        },
                        { "type": "text", "text": "Hi there." },
                        {
                            "type": "tool_use",
                            "id": "toolu_lookup_context_1",
                            "name": "lookup_context",
                            "input": { "query": "current task status" }
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_lookup_context_1",
                            "content": [
                                { "type": "text", "text": "tool says ready" },
                                { "type": "json", "json": { "status": "ready", "attempt": 2 } }
                            ]
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "Please answer the current request."
                    }]
                }
            ],
            "stream": false
        }),
        "preset-openai-compatible".to_string(),
        crate::formats::UpstreamFormat::Anthropic,
        None,
    )
    .await;

    let status = response.status();
    assert_portability_warning_headers_contain(
        &response,
        &["thinking", "context_management", "signature"],
    );
    let body_text = response_text(response).await;
    assert_ne!(status, StatusCode::BAD_REQUEST, "body = {body_text}");
    assert_eq!(status, StatusCode::OK, "body = {body_text}");

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    assert!(
        upstream_body.get("thinking").is_none(),
        "OpenAI Chat body must not receive top-level Anthropic thinking: {upstream_body:?}"
    );
    assert!(
        upstream_body.get("context_management").is_none(),
        "OpenAI Chat body must not receive Anthropic context_management: {upstream_body:?}"
    );
    assert!(
        !json_contains_key(upstream_body, "signature"),
        "OpenAI Chat body must not receive Anthropic signed-thinking carrier fields: {upstream_body:?}"
    );
    let messages = upstream_body["messages"]
        .as_array()
        .expect("OpenAI Chat messages array");
    assert!(
        messages.iter().any(|message| {
            message.get("role").and_then(Value::as_str) == Some("user")
                && message
                    .to_string()
                    .contains("Please answer the current request.")
        }),
        "OpenAI Chat body must retain the current user request: {upstream_body:?}"
    );
    let assistant_tool_call_message = messages
        .iter()
        .find(|message| {
            message.get("role").and_then(Value::as_str) == Some("assistant")
                && message.get("tool_calls").is_some()
        })
        .expect("OpenAI Chat assistant tool-call message");
    assert_eq!(
        assistant_tool_call_message["reasoning_content"],
        "I should answer briefly and keep visible context."
    );
    assert_eq!(assistant_tool_call_message["content"], "Hi there.");
    let tool_calls = assistant_tool_call_message["tool_calls"]
        .as_array()
        .expect("OpenAI Chat tool_calls");
    assert_eq!(tool_calls.len(), 1, "tool_calls = {tool_calls:?}");
    assert_eq!(tool_calls[0]["id"], "toolu_lookup_context_1");
    assert_eq!(tool_calls[0]["type"], "function");
    assert_eq!(tool_calls[0]["function"]["name"], "lookup_context");
    let tool_arguments: Value = serde_json::from_str(
        tool_calls[0]["function"]["arguments"]
            .as_str()
            .unwrap_or(""),
    )
    .expect("OpenAI Chat function arguments JSON");
    assert_eq!(
        tool_arguments,
        serde_json::json!({ "query": "current task status" })
    );
    let tool_result_message = messages
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
        .expect("OpenAI Chat tool result message");
    assert_eq!(
        tool_result_message["tool_call_id"],
        "toolu_lookup_context_1"
    );
    assert_eq!(
        tool_result_message["content"],
        serde_json::json!([
            { "type": "text", "text": "tool says ready" },
            { "type": "json", "json": { "status": "ready", "attempt": 2 } }
        ])
    );

    upstream_server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_claude_code_second_round_thinking_context_management_reaches_openai_responses() {
    let server_response_body = serde_json::json!({
        "id": "resp_claude_code_second_round",
        "object": "response",
        "created_at": 123,
        "status": "completed",
        "model": "gpt-4o-mini",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "ok" }]
        }],
        "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_responses_mock(server_response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "preset-openai-compatible".to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: "gpt-4o-mini".to_string(),
                limits: None,
                surface: None,
            },
        );
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/anthropic/v1/messages".to_string(),
        serde_json::json!({
            "model": "preset-openai-compatible",
            "max_tokens": 64,
            "thinking": { "type": "enabled", "budget_tokens": 1024 },
            "context_management": { "type": "auto" },
            "messages": [
                {
                    "role": "user",
                    "content": [{ "type": "text", "text": "hi" }]
                },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "I should answer briefly and keep visible context.",
                            "signature": "claude-code-prior-visible-thinking-signature"
                        },
                        { "type": "text", "text": "Hi there." },
                        {
                            "type": "tool_use",
                            "id": "toolu_lookup_context_1",
                            "name": "lookup_context",
                            "input": { "query": "current task status" }
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_lookup_context_1",
                        "content": "tool says ready"
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "Please answer the current request."
                    }]
                }
            ],
            "stream": false
        }),
        "preset-openai-compatible".to_string(),
        crate::formats::UpstreamFormat::Anthropic,
        None,
    )
    .await;

    let status = response.status();
    assert_portability_warning_headers_contain(
        &response,
        &["thinking", "context_management", "signature"],
    );
    let body_text = response_text(response).await;
    assert_eq!(status, StatusCode::OK, "body = {body_text}");

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    assert!(
        upstream_body.get("thinking").is_none(),
        "OpenAI Responses body must not receive top-level Anthropic thinking: {upstream_body:?}"
    );
    assert!(
        upstream_body.get("context_management").is_none(),
        "OpenAI Responses body must not receive Anthropic context_management: {upstream_body:?}"
    );
    assert!(
        !json_contains_key(upstream_body, "signature"),
        "OpenAI Responses body must not receive Anthropic signed-thinking carrier fields: {upstream_body:?}"
    );
    let input = upstream_body["input"]
        .as_array()
        .expect("OpenAI Responses input array");
    assert!(
        input.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("user")
                && item
                    .to_string()
                    .contains("Please answer the current request.")
        }),
        "OpenAI Responses body must retain the current user request: {upstream_body:?}"
    );
    let reasoning_item = input
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .expect("OpenAI Responses reasoning item");
    assert_eq!(
        reasoning_item["summary"],
        serde_json::json!([{
            "type": "summary_text",
            "text": "I should answer briefly and keep visible context."
        }])
    );
    assert!(
        reasoning_item.get("encrypted_content").is_none(),
        "request-side signed thinking should degrade to unsigned Responses reasoning summary: {upstream_body:?}"
    );
    let function_call = input
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
        .expect("OpenAI Responses function_call item");
    assert_eq!(function_call["call_id"], "toolu_lookup_context_1");
    assert_eq!(function_call["name"], "lookup_context");
    let tool_arguments: Value =
        serde_json::from_str(function_call["arguments"].as_str().unwrap_or(""))
            .expect("OpenAI Responses function_call arguments JSON");
    assert_eq!(
        tool_arguments,
        serde_json::json!({ "query": "current task status" })
    );
    let function_call_output = input
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
        .expect("OpenAI Responses function_call_output item");
    assert_eq!(function_call_output["call_id"], "toolu_lookup_context_1");
    assert_eq!(function_call_output["output"], "tool says ready");

    upstream_server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn debug_trace_records_prompt_cache_disposition_for_synthesized_openai_key_and_dropped_anthropic_cache_control(
) {
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_synthesized_cache_control",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/anthropic/v1/messages".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "max_tokens": 32,
            "cache_control": { "type": "ephemeral" },
            "system": [
                { "type": "text", "text": "System", "cache_control": { "type": "ephemeral" } }
            ],
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Hi", "cache_control": { "type": "ephemeral" } }
                ]
            }],
            "tools": [{
                "name": "lookup",
                "input_schema": { "type": "object", "properties": {} },
                "cache_control": { "type": "ephemeral" }
            }],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::Anthropic,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-llmup-portability-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(response.headers().get("x-proxy-compat-warning").is_none());
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("cache_control")),
        "warnings = {warnings:?}"
    );
    let _ = response_text(response).await;

    let trace = wait_for_debug_trace_response(&trace_path).await;
    let request_entry = debug_trace_request_entry(&trace);
    assert_llmup_external_observability(&request_entry, "constructed", "synthesized");
    let prompt_cache_detail = &request_entry["request"]["prompt_cache_request_control"];
    assert_eq!(prompt_cache_detail["target_provider"], "openai_family");
    assert_eq!(prompt_cache_detail["disposition"], "synthesized");
    assert_eq!(
        prompt_cache_detail["synthesis_reason"],
        "canonical_static_prefix_digest"
    );
    assert_eq!(
        prompt_cache_detail["target_fields"],
        serde_json::json!(["prompt_cache_key"])
    );
    let key_fingerprint = prompt_cache_detail["key_fingerprint"]
        .as_str()
        .expect("synthesized key fingerprint");
    assert_eq!(key_fingerprint.len(), 12);
    assert!(prompt_cache_detail.get("prompt_cache_key").is_none());
    assert!(
        !serde_json::to_string(prompt_cache_detail)
            .expect("prompt-cache trace detail serializes")
            .contains("llmup:v1:"),
        "prompt-cache trace detail must not contain full synthesized key: {prompt_cache_detail:?}"
    );
    assert_eq!(
        prompt_cache_detail["components"],
        serde_json::json!([{
            "disposition": "dropped",
            "source_fields": [
                "cache_control",
                "system[].cache_control",
                "messages[].content[].cache_control",
                "tools[].cache_control"
            ],
            "target_fields": [],
            "omitted_reason": "anthropic_cache_control_not_openai_prompt_cache_key"
        }]),
        "request_entry = {request_entry:?}"
    );
    let hook_payload = wait_for_hook_payload(&hook_payloads).await;
    assert_llmup_external_observability(&hook_payload, "constructed", "synthesized");
    assert_prompt_cache_request_control_detail_absent_from_hook_payload(&hook_payload);
    let hook_text = serde_json::to_string(&hook_payload).expect("hook payload json");
    assert!(
        !hook_text.contains("llmup:v1:"),
        "hook payload must not contain full synthesized prompt_cache_key: {hook_text}"
    );

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    let synthesized_key = requests[0]["prompt_cache_key"]
        .as_str()
        .expect("synthesized prompt_cache_key");
    assert!(synthesized_key.starts_with("llmup:v1:"));
    assert!(
        requests[0].get("prompt_cache_retention").is_none(),
        "upstream body = {:?}",
        requests[0]
    );

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn debug_trace_records_prompt_cache_detail_for_dropped_openai_key_to_anthropic_without_cache_control(
) {
    let server_response_body = serde_json::json!({
        "id": "msg_dropped_openai_prompt_cache_key",
        "type": "message",
        "role": "assistant",
        "model": "claude-3",
        "content": [{ "type": "text", "text": "ok" }],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_anthropic_messages_mock(server_response_body).await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "prompt_cache_key": "stable-prefix",
            "prompt_cache_retention": "24h",
            "stream": false
        }),
        "claude-3".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-llmup-portability-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("prompt_cache_key")),
        "warnings = {warnings:?}"
    );
    let _ = response_text(response).await;

    let trace = wait_for_debug_trace_response(&trace_path).await;
    let request_entry = debug_trace_request_entry(&trace);
    assert_llmup_external_observability(&request_entry, "constructed", "dropped");
    assert_debug_trace_omits_stable_prefix(&trace, &request_entry);
    let prompt_cache_detail = &request_entry["request"]["prompt_cache_request_control"];
    assert_eq!(
        prompt_cache_detail,
        &serde_json::json!({
            "target_provider": "anthropic",
            "disposition": "dropped",
            "source_fields": ["prompt_cache_key", "prompt_cache_retention"],
            "target_fields": [],
            "ttl_or_retention_source": "prompt_cache_retention",
            "omitted_reason": "openai_prompt_cache_key_not_anthropic_cache_control_or_breakpoint"
        }),
        "request_entry = {request_entry:?}"
    );
    assert!(
        !serde_json::to_string(prompt_cache_detail)
            .expect("prompt-cache trace detail serializes")
            .contains("stable-prefix"),
        "prompt-cache trace detail must not contain key values: {prompt_cache_detail:?}"
    );
    let hook_payload = wait_for_hook_payload(&hook_payloads).await;
    assert_llmup_external_observability(&hook_payload, "constructed", "dropped");
    assert_prompt_cache_request_control_detail_absent_from_hook_payload(&hook_payload);

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].get("prompt_cache_key").is_none(),
        "upstream body = {:?}",
        requests[0]
    );
    assert!(
        requests[0].get("prompt_cache_retention").is_none(),
        "upstream body = {:?}",
        requests[0]
    );
    assert!(
        !json_contains_key(&requests[0], "cache_control"),
        "upstream body = {:?}",
        requests[0]
    );

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn constructed_non_stream_upstream_error_preserves_portability_warning_headers() {
    let upstream_body = serde_json::json!({
        "error": { "message": "upstream rejected" }
    })
    .to_string();
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_raw_mock(StatusCode::BAD_REQUEST, upstream_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/anthropic/v1/messages".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "max_tokens": 32,
            "cache_control": { "type": "ephemeral" },
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Hi", "cache_control": { "type": "ephemeral" } }
                ]
            }],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::Anthropic,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_cache_control_portability_warning_headers(&response);
    let _ = response_text(response).await;
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(
        !json_contains_key(&requests[0], "cache_control"),
        "upstream body = {:?}",
        requests[0]
    );
    upstream_server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_cache_synthesized_key_echoed_by_upstream_error_is_redacted_everywhere() {
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_prompt_cache_echo_error_mock().await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let (response, logs) = capture_error_logs(handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/anthropic/v1/messages".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "max_tokens": 32,
            "system": [{ "type": "text", "text": "Stable system prefix." }],
            "messages": [{
                "role": "user",
                "content": [{ "type": "text", "text": "Hi" }]
            }],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::Anthropic,
        None,
    ))
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_text = response_text(response).await;
    let request_entries = wait_for_debug_trace_request_count(&trace_path, 1).await;
    let trace = std::fs::read_to_string(&trace_path).expect("debug trace should be readable");
    let hook_text = serde_json::to_string(&hook_payloads.snapshot().await).expect("hook json");

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    let synthesized_key = requests[0]["prompt_cache_key"]
        .as_str()
        .expect("synthesized prompt_cache_key");
    assert!(synthesized_key.starts_with("llmup:v1:"));
    assert_eq!(request_entries.len(), 1);

    for (surface, text) in [
        ("public error response", body_text.as_str()),
        ("error logs", logs.as_str()),
        ("debug trace", trace.as_str()),
        ("hook payload", hook_text.as_str()),
    ] {
        assert!(
            !text.contains(synthesized_key),
            "{surface} must not leak full synthesized prompt_cache_key: {text}"
        );
    }

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_cache_synthesized_key_echoed_by_upstream_success_header_is_redacted() {
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_prompt_cache_header_success",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_prompt_cache_echo_header_mock(StatusCode::OK, server_response_body)
            .await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [
                { "role": "system", "content": "Stable static prefix." },
                { "role": "user", "content": "Hi" }
            ],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let header_value = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("forwarded x-request-id")
        .to_string();
    assert!(
        header_value.contains("[REDACTED]"),
        "header = {header_value}"
    );
    let _ = response_text(response).await;

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    let synthesized_key = requests[0]["prompt_cache_key"]
        .as_str()
        .expect("synthesized prompt_cache_key");
    assert!(
        !header_value.contains(synthesized_key),
        "forwarded success header must not leak full synthesized prompt_cache_key: {header_value}"
    );

    upstream_server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_cache_synthesized_key_echoed_by_upstream_error_header_is_redacted() {
    let server_response_body = serde_json::json!({
        "error": { "message": "upstream rejected" }
    });
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_prompt_cache_echo_header_mock(
            StatusCode::BAD_REQUEST,
            server_response_body,
        )
        .await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [
                { "role": "system", "content": "Stable static prefix." },
                { "role": "user", "content": "Hi" }
            ],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let header_value = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("forwarded x-request-id")
        .to_string();
    assert!(
        header_value.contains("[REDACTED]"),
        "header = {header_value}"
    );
    let _ = response_text(response).await;

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    let synthesized_key = requests[0]["prompt_cache_key"]
        .as_str()
        .expect("synthesized prompt_cache_key");
    assert!(
        !header_value.contains(synthesized_key),
        "forwarded error header must not leak full synthesized prompt_cache_key: {header_value}"
    );

    upstream_server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn constructed_streaming_upstream_error_preserves_portability_warning_headers() {
    let upstream_body = serde_json::json!({
        "error": { "message": "upstream unavailable" }
    })
    .to_string();
    let (mock_base, requests, upstream_server) =
        spawn_openai_completion_raw_mock(StatusCode::SERVICE_UNAVAILABLE, upstream_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/anthropic/v1/messages".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "max_tokens": 32,
            "cache_control": { "type": "ephemeral" },
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Hi", "cache_control": { "type": "ephemeral" } }
                ]
            }],
            "stream": true
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::Anthropic,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_cache_control_portability_warning_headers(&response);
    let _ = response_text(response).await;
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(
        !json_contains_key(&requests[0], "cache_control"),
        "upstream body = {:?}",
        requests[0]
    );
    upstream_server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn request_metadata_redacts_proxy_and_provider_keys_in_hook_debug_logs_and_metrics() {
    let provider_secret = PROVIDER_INLINE_REDACTION_SECRET;
    let proxy_secret = PROXY_INLINE_REDACTION_SECRET;
    let upstream_name = format!("primary-{provider_secret}");
    let server_response_body = serde_json::json!({
        "id": "chatcmpl_proxy_metadata_redaction",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, _requests, upstream_server) =
        spawn_openai_completion_mock(server_response_body).await;
    let (hook_base, hook_payloads, hook_server) = spawn_hook_capture_mock().await;
    let state = app_state_for_redaction_upstreams(
        vec![redaction_upstream_config(
            &upstream_name,
            &mock_base,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            Some(crate::config::SecretSourceConfig {
                inline: Some(provider_secret.to_string()),
                env: None,
            }),
        )],
        data_auth::DataAccess::ProxyKey {
            key: proxy_secret.to_string(),
        },
    )
    .await;
    enable_exchange_hook_for_default_namespace(&state, &hook_base).await;
    let trace_path = enable_debug_trace_for_default_namespace(&state).await;

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::HeaderName::from_static("openai-organization"),
        HeaderValue::from_str(&format!("org-{provider_secret}-{proxy_secret}"))
            .expect("metadata header"),
    );
    let path = format!("/openai/v1/chat/completions/{provider_secret}/{proxy_secret}");
    let requested_model = format!("{upstream_name}:model-{provider_secret}-{proxy_secret}");

    let (response, logs) = capture_debug_logs(handle_request_core(
        state.clone(),
        DEFAULT_NAMESPACE.to_string(),
        headers,
        path,
        serde_json::json!({
            "model": requested_model,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        format!("{upstream_name}:model-{provider_secret}-{proxy_secret}"),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    ))
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response_text(response).await;
    let secrets = [provider_secret, proxy_secret];
    assert_no_secret_leak(&body_text, &secrets, "proxy metadata response");

    let hook_payload = wait_for_hook_payload(&hook_payloads).await;
    let hook_text = serde_json::to_string(&hook_payload).expect("hook payload json");
    assert_no_secret_leak(&hook_text, &secrets, "proxy metadata hook");
    assert!(hook_text.contains("[REDACTED]"), "hook = {hook_text}");

    let trace = wait_for_debug_trace_response(&trace_path).await;
    assert_no_secret_leak(&trace, &secrets, "proxy metadata debug trace");
    assert!(trace.contains("[REDACTED]"), "trace = {trace}");

    let config = state
        .runtime
        .read()
        .await
        .namespaces
        .get(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .config
        .clone();
    let metrics_text = format!("{:?}", state.metrics.snapshot(&config));
    assert_no_secret_leak(&metrics_text, &secrets, "proxy metadata metrics");
    assert!(
        metrics_text.contains("[REDACTED]"),
        "metrics = {metrics_text}"
    );
    assert_no_secret_leak(&logs, &secrets, "proxy metadata logs");
    assert!(logs.contains("[REDACTED]"), "logs = {logs}");

    upstream_server.abort();
    hook_server.abort();
    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test(flavor = "current_thread")]
async fn request_metadata_redacts_proxy_and_provider_keys_from_public_model_errors() {
    let provider_secret = PROVIDER_INLINE_REDACTION_SECRET;
    let proxy_secret = PROXY_INLINE_REDACTION_SECRET;
    let left_upstream = format!("left-{provider_secret}");
    let state = app_state_for_redaction_upstreams(
        vec![
            redaction_upstream_config(
                &left_upstream,
                "http://127.0.0.1:9/v1",
                crate::formats::UpstreamFormat::OpenAiChatCompletions,
                Some(crate::config::SecretSourceConfig {
                    inline: Some(provider_secret.to_string()),
                    env: None,
                }),
            ),
            redaction_upstream_config(
                "right",
                "http://127.0.0.1:9/v1",
                crate::formats::UpstreamFormat::OpenAiChatCompletions,
                Some(crate::config::SecretSourceConfig {
                    inline: Some("right-provider-key".to_string()),
                    env: None,
                }),
            ),
        ],
        data_auth::DataAccess::ProxyKey {
            key: proxy_secret.to_string(),
        },
    )
    .await;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::HeaderName::from_static("openai-organization"),
        HeaderValue::from_str(&format!("org-{provider_secret}-{proxy_secret}"))
            .expect("metadata header"),
    );
    let path = format!("/openai/v1/chat/completions/{provider_secret}/{proxy_secret}");
    let requested_model = format!("ambiguous-{provider_secret}-{proxy_secret}");

    let (response, logs) = capture_debug_logs(handle_request_core(
        state.clone(),
        DEFAULT_NAMESPACE.to_string(),
        headers,
        path,
        serde_json::json!({
            "model": requested_model,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        format!("ambiguous-{provider_secret}-{proxy_secret}"),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    ))
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_text = response_text(response).await;
    let secrets = [provider_secret, proxy_secret];
    assert_no_secret_leak(&body_text, &secrets, "public model error");
    assert!(body_text.contains("[REDACTED]"), "body = {body_text}");
    assert_no_secret_leak(&logs, &secrets, "public model error logs");
    assert!(logs.contains("[REDACTED]"), "logs = {logs}");

    let config = state
        .runtime
        .read()
        .await
        .namespaces
        .get(DEFAULT_NAMESPACE)
        .expect("default namespace")
        .config
        .clone();
    let metrics_text = format!("{:?}", state.metrics.snapshot(&config));
    assert_no_secret_leak(&metrics_text, &secrets, "public model error metrics");
    assert!(
        metrics_text.contains("[REDACTED]"),
        "metrics = {metrics_text}"
    );
}

#[tokio::test]
async fn openai_chat_request_body_limit_rejects_before_upstream() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    set_resource_limits(
        &state,
        crate::config::ResourceLimits {
            max_request_body_bytes: 96,
            ..Default::default()
        },
    )
    .await;

    let oversized_secret = "REQUEST_BODY_SENTINEL_SHOULD_NOT_LEAK";
    let request_body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": format!("{oversized_secret}{}", "x".repeat(512)) }],
        "stream": false
    })
    .to_string();
    let mut request = axum::http::Request::builder()
        .header("Content-Type", "application/json")
        .header("openai-api-key", "test-client-provider-key")
        .body(Body::from(request_body))
        .expect("request");
    insert_request_auth_context_from_headers(&state, &mut request).await;

    let response =
        crate::server::proxy::handle_openai_chat_completions(State(state), None, request).await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(
        body_text.contains("request body exceeded"),
        "body = {body_text}"
    );
    assert!(!body_text.contains(oversized_secret), "body = {body_text}");
    assert!(requests.lock().await.is_empty());
    server.abort();
}

#[tokio::test]
async fn non_stream_upstream_response_body_limit_fails_closed_without_payload_leak() {
    let upstream_sentinel = "NON_STREAM_RESPONSE_SENTINEL_SHOULD_NOT_LEAK";
    let raw_body = serde_json::json!({
        "id": "chatcmpl_large",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": format!("{upstream_sentinel}{}", "x".repeat(1024)) },
            "finish_reason": "stop"
        }]
    })
    .to_string();
    let (mock_base, requests, server) =
        spawn_openai_completion_raw_mock(StatusCode::OK, raw_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    set_resource_limits(
        &state,
        crate::config::ResourceLimits {
            max_non_stream_response_bytes: 256,
            ..Default::default()
        },
    )
    .await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(
        body_text.contains("upstream response body exceeded"),
        "body = {body_text}"
    );
    assert!(!body_text.contains(upstream_sentinel), "body = {body_text}");
    assert_eq!(requests.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn upstream_error_body_limit_fails_closed_without_error_payload_leak() {
    let upstream_sentinel = "UPSTREAM_ERROR_BODY_SENTINEL_SHOULD_NOT_LEAK";
    let raw_body = format!("{upstream_sentinel}{}", "x".repeat(1024));
    let (mock_base, requests, server) =
        spawn_openai_completion_raw_mock(StatusCode::BAD_REQUEST, raw_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    set_resource_limits(
        &state,
        crate::config::ResourceLimits {
            max_upstream_error_body_bytes: 64,
            ..Default::default()
        },
    )
    .await;

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(
        body_text.contains("upstream error body exceeded"),
        "body = {body_text}"
    );
    assert!(!body_text.contains(upstream_sentinel), "body = {body_text}");
    assert_eq!(requests.lock().await.len(), 1);
    server.abort();
}

#[test]
fn classify_request_boundary_rejects_translated_stateful_responses_controls() {
    let decision = classify_request_boundary(
        crate::formats::UpstreamFormat::OpenAiResponses,
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "conversation": { "id": "conv_1" },
            "background": true,
            "store": true
        }),
    );

    let RequestBoundaryDecision::Reject(message) = decision else {
        panic!("expected rejection, got {decision:?}");
    };
    assert!(message.contains("conversation"));
    assert!(message.contains("background"));
    assert!(message.contains("store"));
    assert!(message.contains("native OpenAI Responses"));
}

#[test]
fn classify_request_boundary_rejects_translated_store_enabled_by_itself() {
    for (label, store) in [
        ("true", serde_json::json!(true)),
        ("non_bool", serde_json::json!("enabled")),
    ] {
        let decision = classify_request_boundary(
            crate::formats::UpstreamFormat::OpenAiResponses,
            crate::formats::UpstreamFormat::Anthropic,
            &serde_json::json!({
                "store": store
            }),
        );

        let RequestBoundaryDecision::Reject(message) = decision else {
            panic!("expected {label} store rejection, got {decision:?}");
        };
        assert!(message.contains("store"), "message = {message}");
        assert!(
            message.contains("native OpenAI Responses"),
            "message = {message}"
        );
    }
}

#[test]
fn classify_request_boundary_keeps_portability_warning_path() {
    let decision = classify_request_boundary(
        crate::formats::UpstreamFormat::OpenAiResponses,
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "tools": [{ "type": "web_search" }]
        }),
    );

    let RequestBoundaryDecision::AllowWithWarnings(warnings) = decision else {
        panic!("expected warning path, got {decision:?}");
    };
    assert!(warnings
        .iter()
        .any(|warning| warning.contains("non-function Responses tools")));
}

fn request_processing_input<'a>(
    client_format: crate::formats::UpstreamFormat,
    upstream_format: crate::formats::UpstreamFormat,
    body: &'a Value,
) -> crate::request_processing::RequestProcessingInput<'a> {
    let model = body.get("model").and_then(Value::as_str).unwrap_or("model");
    crate::request_processing::RequestProcessingInput {
        client_format,
        upstream_format,
        body,
        requested_model: model,
        upstream_model: model,
        stream: false,
        forced_stream: false,
        route_policy_requires_body_mutation: false,
        state_bridge: crate::request_processing::StateBridgeModifier::Off,
    }
}

fn assert_llmup_external_observability(
    payload: &Value,
    request_body_handling: &str,
    provider_prompt_cache_request_control: &str,
) {
    assert_eq!(
        payload["llmup"]["request_body_handling"], request_body_handling,
        "payload = {payload:?}"
    );
    assert_eq!(
        payload["llmup"]["provider_prompt_cache_request_control"],
        provider_prompt_cache_request_control,
        "payload = {payload:?}"
    );
    let llmup = payload["llmup"]
        .as_object()
        .expect("llmup observability object");
    assert!(
        !llmup.contains_key("request_processing"),
        "payload = {payload:?}"
    );
    assert!(
        !llmup.contains_key("zero_transform_forwarding_active"),
        "payload = {payload:?}"
    );
    assert!(
        !llmup.contains_key("provider_native_prompt_cache"),
        "payload = {payload:?}"
    );
    assert!(!llmup.contains_key("state_bridge"), "payload = {payload:?}");
    assert!(
        !llmup.contains_key("local_state_handling"),
        "payload = {payload:?}"
    );
    assert_eq!(llmup.len(), 2, "payload = {payload:?}");
}

fn assert_prompt_cache_request_control_detail_absent_from_hook_payload(hook_payload: &Value) {
    assert!(
        !json_contains_key(hook_payload, "prompt_cache_request_control"),
        "fine-grained prompt-cache trace detail must not enter hook payload: {hook_payload:?}"
    );
    assert!(
        hook_payload.get("prompt_cache_request_control").is_none(),
        "hook payload top-level object must not contain fine-grained prompt-cache trace detail: {hook_payload:?}"
    );
    assert!(
        hook_payload["llmup"]
            .get("prompt_cache_request_control")
            .is_none(),
        "hook llmup object must keep only coarse provider_prompt_cache_request_control: {hook_payload:?}"
    );
    assert!(
        !json_contains_key(
            &hook_payload["request"]["body"],
            "prompt_cache_request_control"
        ),
        "hook request body must not contain debug trace prompt-cache detail: {hook_payload:?}"
    );
}

#[test]
fn request_processing_provider_prompt_cache_request_control_serializes_only_supported_contract_states(
) {
    use crate::request_processing::PromptCacheRequestControl as Control;

    let states = [
        Control::None,
        Control::Preserved,
        Control::ExplicitExtensionMapped,
        Control::Synthesized,
        Control::Dropped,
    ];

    for state in states {
        match state {
            Control::None
            | Control::Preserved
            | Control::ExplicitExtensionMapped
            | Control::Synthesized
            | Control::Dropped => {}
        }
    }

    let serialized = states
        .into_iter()
        .map(|state| serde_json::to_value(state).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        serialized,
        vec![
            serde_json::json!("none"),
            serde_json::json!("preserved_native"),
            serde_json::json!("explicit_extension_mapped"),
            serde_json::json!("synthesized"),
            serde_json::json!("dropped"),
        ]
    );
    assert!(!serialized.contains(&serde_json::json!("preserved")));
}

#[test]
fn request_processing_serializes_local_state_handling_only_when_active() {
    use crate::request_processing::StateBridgeModifier;

    let inactive = crate::request_processing::RequestProcessingInfo {
        state_bridge: StateBridgeModifier::Off,
        ..crate::request_processing::RequestProcessingInfo::default()
    };
    let inactive = serde_json::to_value(inactive).unwrap();
    assert_eq!(inactive["request_body_handling"], "constructed");
    assert_eq!(inactive["provider_prompt_cache_request_control"], "none");
    assert!(
        inactive.get("state_bridge").is_none(),
        "inactive llmup = {inactive:?}"
    );
    assert!(
        inactive.get("local_state_handling").is_none(),
        "inactive llmup = {inactive:?}"
    );
    assert!(
        inactive.get("conversation_state_bridge").is_none(),
        "inactive llmup = {inactive:?}"
    );

    for (state_bridge, expected) in [
        (StateBridgeModifier::CaptureCandidate, "capture_candidate"),
        (StateBridgeModifier::Expanded, "expanded"),
    ] {
        let active = crate::request_processing::RequestProcessingInfo {
            state_bridge,
            ..crate::request_processing::RequestProcessingInfo::default()
        };
        let active = serde_json::to_value(active).unwrap();
        assert_eq!(active["local_state_handling"], expected);
        assert!(
            active.get("state_bridge").is_none(),
            "active llmup = {active:?}"
        );
        assert!(
            active.get("conversation_state_bridge").is_none(),
            "active llmup = {active:?}"
        );
    }
}

#[test]
fn classify_request_processing_marks_same_protocol_without_fields_as_transformation_not_required() {
    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": "Hi" }]
    });

    let info = crate::request_processing::classify_request_processing(request_processing_input(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &body,
    ));

    assert_eq!(
        info.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationNotRequired
    );
    assert!(
        !info.zero_transform_forwarding_active,
        "classification only reports eligibility; the data-plane flips this when it actually forwards raw bytes"
    );
    assert_eq!(
        info.state_bridge,
        crate::request_processing::StateBridgeModifier::Off
    );
    assert_eq!(
        info.provider_native_prompt_cache,
        crate::request_processing::PromptCacheRequestControl::None
    );
}

#[test]
fn classify_request_processing_marks_translation_and_route_mutation_as_transformation_required() {
    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": "Hi" }]
    });
    let cross_format =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            crate::formats::UpstreamFormat::Anthropic,
            &body,
        ));
    assert_eq!(
        cross_format.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationRequired
    );

    let mut alias_rewrite = request_processing_input(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &body,
    );
    alias_rewrite.requested_model = "alias-model";
    alias_rewrite.upstream_model = "gpt-4o-mini";
    let alias_rewrite = crate::request_processing::classify_request_processing(alias_rewrite);
    assert_eq!(
        alias_rewrite.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationRequired
    );

    let mut surface_policy = request_processing_input(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &body,
    );
    surface_policy.route_policy_requires_body_mutation = true;
    let surface_policy = crate::request_processing::classify_request_processing(surface_policy);
    assert_eq!(
        surface_policy.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationRequired
    );
}

#[test]
fn classify_request_processing_marks_forced_stream_as_transformation_required() {
    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": "Hi" }]
    });
    let mut input = request_processing_input(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &body,
    );
    input.forced_stream = true;

    let info = crate::request_processing::classify_request_processing(input);

    assert_eq!(
        info.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationRequired
    );
}

#[test]
fn classify_request_processing_marks_same_format_openai_chat_coalescing_as_transformation_required()
{
    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [
            { "role": "user", "content": "Part one" },
            { "role": "user", "content": "Part two" }
        ]
    });

    let info = crate::request_processing::classify_request_processing(request_processing_input(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &body,
    ));

    assert_eq!(
        info.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationRequired
    );
}

#[test]
fn classify_request_processing_marks_model_insertion_without_hitting_native_responses_stateful() {
    let chat_without_model = serde_json::json!({
        "messages": [{ "role": "user", "content": "Hi" }]
    });
    let chat = crate::request_processing::classify_request_processing(request_processing_input(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &chat_without_model,
    ));
    assert_eq!(
        chat.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationRequired
    );

    let responses_stateful_without_model = serde_json::json!({
        "previous_response_id": "resp_provider_123",
        "input": "Continue"
    });
    let responses = crate::request_processing::classify_request_processing(
        crate::request_processing::RequestProcessingInput {
            client_format: crate::formats::UpstreamFormat::OpenAiResponses,
            upstream_format: crate::formats::UpstreamFormat::OpenAiResponses,
            body: &responses_stateful_without_model,
            requested_model: "",
            upstream_model: "",
            stream: false,
            forced_stream: false,
            route_policy_requires_body_mutation: false,
            state_bridge: crate::request_processing::StateBridgeModifier::Off,
        },
    );
    assert_eq!(
        responses.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationNotRequired
    );

    let responses_stateful_with_empty_model = serde_json::json!({
        "model": "",
        "previous_response_id": "resp_provider_123",
        "input": "Continue"
    });
    let responses = crate::request_processing::classify_request_processing(
        crate::request_processing::RequestProcessingInput {
            client_format: crate::formats::UpstreamFormat::OpenAiResponses,
            upstream_format: crate::formats::UpstreamFormat::OpenAiResponses,
            body: &responses_stateful_with_empty_model,
            requested_model: "",
            upstream_model: "",
            stream: false,
            forced_stream: false,
            route_policy_requires_body_mutation: false,
            state_bridge: crate::request_processing::StateBridgeModifier::Off,
        },
    );
    assert_eq!(
        responses.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationRequired
    );

    let responses_disabled_stateful_without_model = serde_json::json!({
        "background": false,
        "store": null,
        "input": "Continue"
    });
    let responses = crate::request_processing::classify_request_processing(
        crate::request_processing::RequestProcessingInput {
            client_format: crate::formats::UpstreamFormat::OpenAiResponses,
            upstream_format: crate::formats::UpstreamFormat::OpenAiResponses,
            body: &responses_disabled_stateful_without_model,
            requested_model: "",
            upstream_model: "",
            stream: false,
            forced_stream: false,
            route_policy_requires_body_mutation: false,
            state_bridge: crate::request_processing::StateBridgeModifier::Off,
        },
    );
    assert_eq!(
        responses.request_processing,
        crate::request_processing::RequestProcessing::RequestTransformationRequired
    );
}

#[test]
fn classify_request_processing_exposes_state_bridge_fields_as_transformation_required() {
    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "input": "Hi"
    });
    for state_bridge in [
        crate::request_processing::StateBridgeModifier::CaptureCandidate,
        crate::request_processing::StateBridgeModifier::Expanded,
    ] {
        let mut input = request_processing_input(
            crate::formats::UpstreamFormat::OpenAiResponses,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            &body,
        );
        input.state_bridge = state_bridge;

        let info = crate::request_processing::classify_request_processing(input);

        assert_eq!(
            info.request_processing,
            crate::request_processing::RequestProcessing::RequestTransformationRequired
        );
        assert_eq!(info.state_bridge, state_bridge);
    }
}

#[test]
fn classify_request_processing_preserves_native_prompt_cache_field_only_on_same_protocol() {
    let openai_body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": "Hi" }],
        "prompt_cache_key": "stable-prefix",
        "prompt_cache_retention": "24h"
    });
    let openai_same =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            &openai_body,
        ));
    assert_eq!(
        openai_same.provider_native_prompt_cache,
        crate::request_processing::PromptCacheRequestControl::Preserved
    );

    let openai_cross =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            crate::formats::UpstreamFormat::Anthropic,
            &openai_body,
        ));
    assert_eq!(
        openai_cross.provider_native_prompt_cache,
        crate::request_processing::PromptCacheRequestControl::Dropped
    );

    let anthropic_body = serde_json::json!({
        "model": "claude-3-7-sonnet",
        "system": [{ "type": "text", "text": "System", "cache_control": { "type": "ephemeral" } }],
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": "Hi" }]
        }],
        "tools": [{ "name": "lookup", "input_schema": { "type": "object" }, "cache_control": { "type": "ephemeral" } }]
    });
    let anthropic_same =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::Anthropic,
            crate::formats::UpstreamFormat::Anthropic,
            &anthropic_body,
        ));
    assert_eq!(
        anthropic_same.provider_native_prompt_cache,
        crate::request_processing::PromptCacheRequestControl::Preserved
    );
}

#[test]
fn classify_request_processing_marks_anthropic_cache_control_to_openai_as_dropped() {
    let cases = [
        (
            "top_level_only",
            serde_json::json!({
                "model": "claude-3-7-sonnet",
                "cache_control": { "type": "ephemeral" },
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": "Hi" }]
                }]
            }),
        ),
        (
            "system_only",
            serde_json::json!({
                "model": "claude-3-7-sonnet",
                "system": [{
                    "type": "text",
                    "text": "System",
                    "cache_control": { "type": "ephemeral" }
                }],
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": "Hi" }]
                }]
            }),
        ),
        (
            "message_content_only",
            serde_json::json!({
                "model": "claude-3-7-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Hi", "cache_control": { "type": "ephemeral" } }
                    ]
                }]
            }),
        ),
        (
            "tools_only",
            serde_json::json!({
                "model": "claude-3-7-sonnet",
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": "Hi" }]
                }],
                "tools": [{
                    "name": "lookup",
                    "input_schema": { "type": "object" },
                    "cache_control": { "type": "ephemeral" }
                }]
            }),
        ),
    ];

    for (case_name, body) in cases {
        for upstream_format in [
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            crate::formats::UpstreamFormat::OpenAiResponses,
        ] {
            let info =
                crate::request_processing::classify_request_processing(request_processing_input(
                    crate::formats::UpstreamFormat::Anthropic,
                    upstream_format,
                    &body,
                ));
            assert_eq!(
                serde_json::to_value(info.provider_native_prompt_cache).unwrap(),
                serde_json::json!("dropped"),
                "case_name = {case_name}, upstream_format = {upstream_format}"
            );
        }
    }
}

#[test]
fn classify_request_processing_marks_openai_prompt_cache_key_to_anthropic_as_dropped() {
    let chat_body = serde_json::json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "prompt_cache_key": "stable-prefix",
        "prompt_cache_retention": "24h"
    });
    let chat_to_anthropic =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            crate::formats::UpstreamFormat::Anthropic,
            &chat_body,
        ));
    assert_eq!(
        serde_json::to_value(chat_to_anthropic.provider_native_prompt_cache).unwrap(),
        serde_json::json!("dropped")
    );

    let responses_body = serde_json::json!({
        "model": "claude-3",
        "input": "Hi",
        "prompt_cache_key": "stable-prefix",
        "prompt_cache_retention": "in_memory"
    });
    let responses_to_anthropic =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::OpenAiResponses,
            crate::formats::UpstreamFormat::Anthropic,
            &responses_body,
        ));
    assert_eq!(
        serde_json::to_value(responses_to_anthropic.provider_native_prompt_cache).unwrap(),
        serde_json::json!("dropped")
    );
}

#[test]
fn classify_request_processing_preserves_openai_family_prompt_cache_across_translation() {
    let chat_body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": "Hi" }],
        "prompt_cache_key": "stable-prefix"
    });
    let chat_to_responses =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            crate::formats::UpstreamFormat::OpenAiResponses,
            &chat_body,
        ));
    assert_eq!(
        chat_to_responses.provider_native_prompt_cache,
        crate::request_processing::PromptCacheRequestControl::Preserved
    );

    let responses_body = serde_json::json!({
        "model": "gpt-4o-mini",
        "input": "Hi",
        "prompt_cache_retention": "24h"
    });
    let responses_to_chat =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::OpenAiResponses,
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            &responses_body,
        ));
    assert_eq!(
        responses_to_chat.provider_native_prompt_cache,
        crate::request_processing::PromptCacheRequestControl::Preserved
    );
}

#[test]
fn classify_request_processing_marks_openai_to_anthropic_explicit_cache_control_extension_as_mapped(
) {
    let chat_body = serde_json::json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "extra_body": {
            "anthropic": {
                "cache_control": { "type": "ephemeral", "ttl": "5m" }
            }
        }
    });
    let chat_to_anthropic =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            crate::formats::UpstreamFormat::Anthropic,
            &chat_body,
        ));
    assert_eq!(
        serde_json::to_value(chat_to_anthropic.provider_native_prompt_cache).unwrap(),
        serde_json::json!("explicit_extension_mapped")
    );

    let responses_body = serde_json::json!({
        "model": "claude-3",
        "input": "Hi",
        "extra_body": {
            "anthropic": {
                "cache_control": { "type": "ephemeral", "ttl": "1h" }
            }
        }
    });
    let responses_to_anthropic =
        crate::request_processing::classify_request_processing(request_processing_input(
            crate::formats::UpstreamFormat::OpenAiResponses,
            crate::formats::UpstreamFormat::Anthropic,
            &responses_body,
        ));
    assert_eq!(
        serde_json::to_value(responses_to_anthropic.provider_native_prompt_cache).unwrap(),
        serde_json::json!("explicit_extension_mapped")
    );
}

#[test]
fn classify_request_processing_marks_anthropic_to_openai_explicit_prompt_cache_extension_as_mapped()
{
    let anthropic_body = serde_json::json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "extra_body": {
            "openai": {
                "prompt_cache_key": "stable-prefix",
                "prompt_cache_retention": "24h"
            }
        }
    });

    for upstream_format in [
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        crate::formats::UpstreamFormat::OpenAiResponses,
    ] {
        let info =
            crate::request_processing::classify_request_processing(request_processing_input(
                crate::formats::UpstreamFormat::Anthropic,
                upstream_format,
                &anthropic_body,
            ));
        assert_eq!(
            serde_json::to_value(info.provider_native_prompt_cache).unwrap(),
            serde_json::json!("explicit_extension_mapped"),
            "upstream_format = {upstream_format}"
        );
    }
}

#[test]
fn classify_request_processing_does_not_mark_anthropic_to_openai_prompt_cache_retention_extension_as_mapped(
) {
    let anthropic_body = serde_json::json!({
        "model": "claude-3",
        "messages": [{ "role": "user", "content": "Hi" }],
        "extra_body": {
            "openai": {
                "prompt_cache_retention": "24h"
            }
        }
    });

    for upstream_format in [
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        crate::formats::UpstreamFormat::OpenAiResponses,
    ] {
        let info =
            crate::request_processing::classify_request_processing(request_processing_input(
                crate::formats::UpstreamFormat::Anthropic,
                upstream_format,
                &anthropic_body,
            ));
        assert_eq!(
            serde_json::to_value(info.provider_native_prompt_cache).unwrap(),
            serde_json::json!("none"),
            "upstream_format = {upstream_format}"
        );
    }
}

#[test]
fn request_translation_policy_requires_body_mutation_only_for_injecting_policy_hooks() {
    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": "Hi" }]
    });
    let gate_only = crate::translate::RequestTranslationPolicy {
        surface: crate::config::ModelSurface {
            limits: Some(crate::config::ModelLimits {
                context_window: Some(128_000),
                max_output_tokens: None,
            }),
            modalities: Some(crate::config::ModelModalities {
                input: Some(vec![crate::config::ModelModality::Text]),
                output: None,
            }),
            tools: Some(crate::config::ModelToolSurface {
                supports_search: Some(true),
                supports_view_image: None,
                apply_patch_transport: None,
                supports_parallel_calls: Some(true),
            }),
        },
    };
    assert!(!request_translation_policy_requires_body_mutation(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &body,
        &gate_only
    ));

    let output_limit = crate::translate::RequestTranslationPolicy {
        surface: crate::config::ModelSurface {
            limits: Some(crate::config::ModelLimits {
                context_window: None,
                max_output_tokens: Some(4096),
            }),
            modalities: None,
            tools: None,
        },
    };
    assert!(request_translation_policy_requires_body_mutation(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &body,
        &output_limit
    ));

    let parallel_gate = crate::translate::RequestTranslationPolicy {
        surface: crate::config::ModelSurface {
            limits: None,
            modalities: None,
            tools: Some(crate::config::ModelToolSurface {
                supports_search: None,
                supports_view_image: None,
                apply_patch_transport: None,
                supports_parallel_calls: Some(false),
            }),
        },
    };
    assert!(!request_translation_policy_requires_body_mutation(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &body,
        &parallel_gate
    ));

    let body_with_tools = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": "Hi" }],
        "tools": [{
            "type": "function",
            "function": { "name": "lookup", "parameters": { "type": "object" } }
        }]
    });
    assert!(request_translation_policy_requires_body_mutation(
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        &body_with_tools,
        &parallel_gate
    ));
}

#[tokio::test]
async fn same_format_openai_streaming_transformation_required_rejects_reserved_tool_name() {
    let response_events = vec![serde_json::json!({
        "id": "chatcmpl-reserved",
        "object": "chat.completion.chunk",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_reserved",
                    "type": "function",
                    "function": {
                        "name": "__llmup_custom__apply_patch",
                        "arguments": "{}"
                    }
                }]
            },
            "finish_reason": null
        }]
    })];
    let (mock_base, requests, server) =
        spawn_openai_completion_stream_mock_with_events(response_events).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "alias-chat".to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: "gpt-4o-mini".to_string(),
                limits: None,
                surface: None,
            },
        );
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "alias-chat",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }),
        "alias-chat".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("stream body utf8");
    assert!(
        body_text.contains("\"code\":\"reserved_openai_custom_bridge_prefix\""),
        "body = {body_text}"
    );
    assert!(
        !body_text.contains("\"name\":\"__llmup_custom__apply_patch\""),
        "transformation-required request construction leaked reserved tool name: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(recorded[0]["model"], "gpt-4o-mini");
    server.abort();
}

#[tokio::test]
async fn live_openai_same_format_empty_policy_rejects_reserved_legacy_function_name() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "functions": [{
                "name": "__llmup_custom__legacy_exec",
                "parameters": { "type": "object", "properties": {} }
            }],
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body: Value = serde_json::from_slice(&body).expect("json body");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message string");
    assert_eq!(
        message,
        crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE
    );
    assert!(!message.contains("__llmup_custom__"), "message = {message}");
    assert!(!message.contains("legacy_exec"), "message = {message}");

    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");
    server.abort();
}

#[tokio::test]
async fn live_responses_store_true_fails_closed_before_upstream() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Hi" }]
            }],
            "store": true,
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let warnings = response
        .headers()
        .get_all("x-llmup-portability-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(warnings.is_empty(), "warnings = {warnings:?}");
    assert!(response.headers().get("x-proxy-compat-warning").is_none());
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body: Value = serde_json::from_slice(&body).expect("json body");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message string");
    assert!(message.contains("store"), "message = {message}");
    assert!(
        message.contains("native OpenAI Responses"),
        "message = {message}"
    );

    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_openai_chat_store_true_to_responses_fails_closed_before_upstream() {
    let response_body = serde_json::json!({
        "id": "resp_1",
        "object": "response",
        "created_at": 123,
        "status": "completed",
        "model": "gpt-4o-mini",
        "output": []
    });
    let (mock_base, requests, server) = spawn_openai_responses_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "store": true,
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let warnings = response
        .headers()
        .get_all("x-llmup-portability-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(warnings.is_empty(), "warnings = {warnings:?}");
    assert!(response.headers().get("x-proxy-compat-warning").is_none());
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body: Value = serde_json::from_slice(&body).expect("json body");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message string");
    assert!(message.contains("store"), "message = {message}");
    assert!(message.contains("OpenAI Responses"), "message = {message}");

    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_responses_plain_text_custom_tool_bridge_to_openai_keeps_visible_tool_names_stable() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code",
                "format": { "type": "text" }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "code_exec"
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    let tools = upstream_body["tools"].as_array().expect("upstream tools");
    assert_eq!(tools[0]["function"]["name"], "code_exec");
    assert_eq!(
        upstream_body["tool_choice"]["function"]["name"],
        "code_exec"
    );
    let serialized = upstream_body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__code_exec"),
        "translated live request leaked prefixed tool name: {upstream_body:?}"
    );
    assert!(
        upstream_body.get("_llmup_tool_bridge_context").is_none(),
        "internal bridge context must not be sent upstream: {upstream_body:?}"
    );

    server.abort();
}

#[tokio::test]
async fn live_responses_custom_tool_bridge_to_openai_restores_non_stream_response_custom_tool_call()
{
    let patch_input = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_apply_patch",
                    "type": "function",
                    "function": {
                        "name": "apply_patch",
                        "arguments": serde_json::to_string(
                            &serde_json::json!({ "input": patch_input })
                        )
                        .expect("bridge args")
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": { "type": "text" }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body: Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(
        body["output"][0],
        serde_json::json!({
            "type": "custom_tool_call",
            "call_id": "call_apply_patch",
            "name": "apply_patch",
            "input": patch_input
        })
    );
    let serialized = body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__"),
        "live response leaked reserved bridge prefix: {body:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    assert_eq!(upstream_body["tools"][0]["function"]["name"], "apply_patch");
    assert!(
        upstream_body
            .get(INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD)
            .is_none(),
        "internal bridge context must not be sent upstream: {upstream_body:?}"
    );

    server.abort();
}

#[tokio::test]
async fn live_openai_responses_transformation_required_rejects_upstream_bridge_context_leak() {
    let response_body = serde_json::json!({
        "_llmup_tool_bridge_context": {
            "version": 2,
            "purpose": "openai_responses_custom_tool_bridge",
            "entries": {
                "code_exec": {
                    "stable_name": "code_exec",
                    "source_kind": "custom_text",
                    "transport_kind": "function_object_wrapper",
                    "wrapper_field": "input",
                    "expected_canonical_shape": "single_required_string"
                }
            }
        },
        "id": "resp_leaky_context",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": []
    });
    let (mock_base, requests, server) = spawn_openai_responses_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "responses-alias".to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: "gpt-4o-mini".to_string(),
                limits: None,
                surface: None,
            },
        );
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "responses-alias",
            "input": "Hi",
            "stream": false
        }),
        "responses-alias".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "public egress leaked internal bridge context: {body_text}"
    );
    assert!(
        !body_text.contains("__llmup_custom__"),
        "public egress leaked internal custom prefix: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(recorded[0]["model"], "gpt-4o-mini");
    server.abort();
}

#[tokio::test]
async fn live_openai_responses_transformation_required_rejects_reserved_tool_identity_without_leak()
{
    let response_body = serde_json::json!({
        "id": "resp_reserved_identity",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [{
            "type": "custom_tool_call",
            "call_id": "call_reserved",
            "name": "__llmup_custom__apply_patch",
            "input": "*** Begin Patch\n*** End Patch\n"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_responses_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "responses-alias".to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: "gpt-4o-mini".to_string(),
                limits: None,
                surface: None,
            },
        );
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "responses-alias",
            "input": "Hi",
            "stream": false
        }),
        "responses-alias".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    assert!(
        !body_text.contains("__llmup_custom__"),
        "same-format non-stream rejection leaked reserved prefix: {body_text}"
    );
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "same-format non-stream rejection leaked bridge context field: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(recorded[0]["model"], "gpt-4o-mini");
    server.abort();
}

#[tokio::test]
async fn live_openai_responses_same_format_success_preserves_regular_text_and_schema_descriptions()
{
    let public_text = "plain success text mentions __llmup_custom__apply_patch";
    let schema_description =
        "schema docs may mention __llmup_custom__apply_patch as literal user text";
    let response_body = serde_json::json!({
        "id": "resp_plain_reserved_text",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "output": [{
            "type": "message",
            "id": "msg_plain_reserved_text",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": public_text,
                "annotations": []
            }]
        }],
        "tools": [{
            "type": "function",
            "name": "describe_patch_token",
            "description": schema_description,
            "parameters": {
                "type": "object",
                "properties": {
                    "literal": {
                        "type": "string",
                        "description": schema_description
                    }
                }
            }
        }]
    });
    let (mock_base, requests, server) = spawn_openai_responses_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": "Hi",
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("json body");
    assert_eq!(body["output"][0]["content"][0]["text"], public_text);
    assert_eq!(body["tools"][0]["description"], schema_description);
    assert_eq!(
        body["tools"][0]["parameters"]["properties"]["literal"]["description"],
        schema_description
    );
    assert!(body_text.contains("__llmup_custom__apply_patch"));

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    server.abort();
}

#[tokio::test]
async fn same_format_openai_streaming_passthrough_preserves_regular_delta_content() {
    let public_text = "delta content mentions __llmup_custom__apply_patch as text";
    let response_events = vec![serde_json::json!({
        "id": "chatcmpl-plain-reserved-text",
        "object": "chat.completion.chunk",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "delta": {
                "content": public_text
            },
            "finish_reason": null
        }]
    })];
    let (mock_base, requests, server) =
        spawn_openai_completion_stream_mock_with_events(response_events).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("stream body utf8");
    assert!(body_text.contains(public_text), "body = {body_text}");
    assert!(
        !body_text.contains("reserved_openai_custom_bridge_prefix"),
        "plain delta content should not be treated as a reserved tool identity: {body_text}"
    );
    assert!(
        !body_text.contains("response.failed"),
        "plain delta content should not fail the stream: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    server.abort();
}

#[tokio::test]
async fn live_responses_rejects_external_tool_bridge_context_ingress() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "_llmup_tool_bridge_context": {
                "version": 2,
                "purpose": "openai_responses_custom_tool_bridge",
                "entries": {
                    "code_exec": {
                        "stable_name": "code_exec",
                        "source_kind": "custom_text",
                        "transport_kind": "function_object_wrapper",
                        "wrapper_field": "input",
                        "expected_canonical_shape": "single_required_string"
                    }
                }
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body: Value = serde_json::from_slice(&body).expect("json body");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message string");
    assert_eq!(
        message,
        crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE
    );
    assert!(
        !message.contains(INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD),
        "message = {message}"
    );

    let recorded = requests.lock().await;
    assert!(recorded.is_empty(), "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_responses_custom_tool_bridge_uses_default_bridge() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Run this script" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code",
                "format": { "type": "text" }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "code_exec"
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert!(
        recorded[0]
            .get(INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD)
            .is_none(),
        "internal bridge context must not be sent upstream: {:?}",
        recorded[0]
    );

    server.abort();
}

#[tokio::test]
async fn live_responses_grammar_custom_tool_bridge_to_openai_uses_default_bridge() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_responses_grammar_custom_tool_bridge_to_openai_default_allows_with_warning_and_stable_names(
) {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-llmup-portability-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(response.headers().get("x-proxy-compat-warning").is_none());
    assert!(
        warnings.iter().any(|warning| {
            warning.contains("apply_patch") && warning.contains("OpenAI Chat Completions")
        }),
        "warnings = {warnings:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    let tools = upstream_body["tools"].as_array().expect("upstream tools");
    assert_eq!(tools[0]["function"]["name"], "apply_patch");
    let description = tools[0]["function"]["description"]
        .as_str()
        .expect("bridged OpenAI tool description");
    assert!(
        description.contains("OpenAI Chat Completions receives this tool"),
        "description = {description}"
    );
    assert!(
        description.contains("OpenAI Chat Completions will not enforce it structurally"),
        "description = {description}"
    );
    assert!(
        description.contains("syntax: lark"),
        "description = {description}"
    );
    assert_eq!(
        upstream_body["tool_choice"]["function"]["name"],
        "apply_patch"
    );
    let serialized = upstream_body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__"),
        "translated live request leaked prefixed tool name: {upstream_body:?}"
    );
    assert!(
        upstream_body.get("_llmup_tool_bridge_context").is_none(),
        "internal bridge context must not be sent upstream: {upstream_body:?}"
    );

    server.abort();
}

#[tokio::test]
async fn live_responses_custom_tool_bridge_to_anthropic_uses_default_for_plain_text_custom_tools() {
    let response_body = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "Hi" }],
        "model": "claude-3-7-sonnet",
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, server) = spawn_anthropic_messages_mock(response_body).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "claude-3-7-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Run this script" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "code_exec",
                "description": "Executes code",
                "format": { "type": "text" }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "code_exec"
            },
            "stream": false
        }),
        "claude-3-7-sonnet".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert!(
        recorded[0]
            .get(INTERNAL_TOOL_BRIDGE_CONTEXT_FIELD)
            .is_none(),
        "internal bridge context must not be sent upstream: {:?}",
        recorded[0]
    );

    server.abort();
}

#[tokio::test]
async fn live_responses_grammar_custom_tool_bridge_to_anthropic_uses_default_bridge() {
    let response_body = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "Hi" }],
        "model": "claude-3-7-sonnet",
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, server) = spawn_anthropic_messages_mock(response_body).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "claude-3-7-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
            "stream": false
        }),
        "claude-3-7-sonnet".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");

    server.abort();
}

#[tokio::test]
async fn live_responses_grammar_custom_tool_bridge_to_anthropic_default_allows_with_warning_and_stable_names(
) {
    let response_body = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "Hi" }],
        "model": "claude-3-7-sonnet",
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, server) = spawn_anthropic_messages_mock(response_body).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "claude-3-7-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Create hello.txt" }]
            }],
            "tools": [{
                "type": "custom",
                "name": "apply_patch",
                "description": "Apply a patch",
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": "start: /.+/"
                }
            }],
            "tool_choice": {
                "type": "custom",
                "name": "apply_patch"
            },
            "stream": false
        }),
        "claude-3-7-sonnet".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-llmup-portability-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(response.headers().get("x-proxy-compat-warning").is_none());
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("apply_patch") && warning.contains("Anthropic")),
        "warnings = {warnings:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    let upstream_body = &recorded[0];
    assert_eq!(upstream_body["tools"][0]["name"], "apply_patch");
    assert_eq!(upstream_body["tool_choice"]["name"], "apply_patch");
    let serialized = upstream_body.to_string();
    assert!(
        !serialized.contains("__llmup_custom__"),
        "translated live request leaked prefixed tool name: {upstream_body:?}"
    );
    assert!(
        upstream_body.get("_llmup_tool_bridge_context").is_none(),
        "internal bridge context must not be sent upstream: {upstream_body:?}"
    );

    server.abort();
}

#[tokio::test]
async fn live_responses_anthropic_stream_emits_commentary_message_done_before_tool_item() {
    let (mock_base, requests, server) =
        spawn_anthropic_messages_stream_mock(anthropic_commentary_then_tool_use_events()).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "claude-3-7-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Run pwd" }]
            }],
            "tools": [{
                "type": "function",
                "name": "exec_command",
                "description": "Run a shell command",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cmd": { "type": "string" }
                    },
                    "required": ["cmd"],
                    "additionalProperties": false
                }
            }],
            "tool_choice": {
                "type": "function",
                "name": "exec_command"
            },
            "stream": true
        }),
        "claude-3-7-sonnet".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("stream body utf8");
    let events = parse_sse_events(&body);

    let commentary_done_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "message"
        })
        .expect("completed assistant message item");
    let commentary_done = &events[commentary_done_idx];
    assert_eq!(
        commentary_done["item"]["phase"], "commentary",
        "body = {body_text}"
    );
    assert_eq!(commentary_done["item"]["status"], "completed");
    assert!(
        commentary_done["item"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|part| {
                part.get("type").and_then(Value::as_str) == Some("output_text")
                    && part
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .contains("Preamble line")
            }),
        "body = {body_text}"
    );

    let tool_added_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
                && event["item"]["type"] == "function_call"
                && event["item"]["name"] == "exec_command"
        })
        .expect("function-call item added");
    assert!(
        commentary_done_idx < tool_added_idx,
        "commentary message should complete before tool work begins: {body_text}"
    );

    let tool_args_delta_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str)
                == Some("response.function_call_arguments.delta")
                && event["name"] == "exec_command"
        })
        .expect("function-call arguments delta");
    let tool_done_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "function_call"
                && event["item"]["name"] == "exec_command"
        })
        .expect("function-call item done");
    assert!(
        tool_added_idx < tool_args_delta_idx && tool_args_delta_idx < tool_done_idx,
        "tool item lifecycle should stay intact: {body_text}"
    );
    assert_eq!(
        events[tool_done_idx]["item"]["arguments"],
        "{\"cmd\":\"pwd\"}"
    );
    assert!(
        !body_text.contains("__llmup_custom__"),
        "translated stream must not leak internal bridge artifacts: {body_text}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(recorded[0]["tools"][0]["name"], "exec_command");
    assert_eq!(recorded[0]["stream"], true);

    server.abort();
}

#[tokio::test]
async fn live_responses_anthropic_stream_keeps_tool_item_lifecycle_after_commentary_preamble() {
    let (mock_base, _requests, server) =
        spawn_anthropic_messages_stream_mock(anthropic_commentary_then_tool_use_events()).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "claude-3-7-sonnet",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Run pwd" }]
            }],
            "tools": [{
                "type": "function",
                "name": "exec_command",
                "description": "Run a shell command",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cmd": { "type": "string" }
                    },
                    "required": ["cmd"],
                    "additionalProperties": false
                }
            }],
            "tool_choice": {
                "type": "function",
                "name": "exec_command"
            },
            "stream": true
        }),
        "claude-3-7-sonnet".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("stream body utf8");
    let events = parse_sse_events(&body);

    let tool_added_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.added")
                && event["item"]["type"] == "function_call"
                && event["item"]["name"] == "exec_command"
        })
        .expect("function-call item added");
    let tool_args_delta_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str)
                == Some("response.function_call_arguments.delta")
                && event["name"] == "exec_command"
        })
        .expect("function-call arguments delta");
    let tool_args_done_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str)
                == Some("response.function_call_arguments.done")
                && event["name"] == "exec_command"
        })
        .expect("function-call arguments done");
    let tool_done_idx = events
        .iter()
        .position(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_item.done")
                && event["item"]["type"] == "function_call"
                && event["item"]["name"] == "exec_command"
        })
        .expect("function-call item done");

    assert!(
        tool_added_idx < tool_args_delta_idx
            && tool_args_delta_idx < tool_args_done_idx
            && tool_args_done_idx < tool_done_idx,
        "tool item lifecycle should stay intact: {body_text}"
    );
    assert_eq!(
        events[tool_done_idx]["item"]["arguments"],
        "{\"cmd\":\"pwd\"}"
    );

    server.abort();
}

#[tokio::test]
async fn live_openai_request_uses_configured_default_output_limit_for_anthropic_upstream() {
    let response_body = serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "Hi" }],
        "model": "claude-3-7-sonnet",
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let (mock_base, requests, server) = spawn_anthropic_messages_mock(response_body).await;
    let state = app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::Anthropic);
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "minimax-openai".to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: "claude-3-7-sonnet".to_string(),
                limits: Some(crate::config::ModelLimits {
                    context_window: None,
                    max_output_tokens: Some(128_000),
                }),
                surface: None,
            },
        );
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "minimax-openai",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }),
        "minimax-openai".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(recorded[0]["model"], "claude-3-7-sonnet");
    assert_eq!(
        recorded[0]["max_tokens"], 128_000,
        "configured default output limit should propagate to real Anthropic upstream body when the client omits it: {:?}",
        recorded[0]
    );

    server.abort();
}

#[tokio::test]
async fn live_openai_same_format_request_applies_surface_parallel_tool_gate() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );
    {
        let mut runtime = state.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "serial-openai".to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: "gpt-4o-mini".to_string(),
                limits: None,
                surface: Some(crate::config::ModelSurfacePatch {
                    modalities: None,
                    tools: Some(crate::config::ModelToolSurface {
                        supports_search: None,
                        supports_view_image: None,
                        apply_patch_transport: None,
                        supports_parallel_calls: Some(false),
                    }),
                }),
            },
        );
    }

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "serial-openai",
            "messages": [{ "role": "user", "content": "Hi" }],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup_weather",
                    "parameters": { "type": "object", "properties": {} }
                }
            }],
            "stream": false
        }),
        "serial-openai".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert_eq!(recorded[0]["model"], "gpt-4o-mini");
    assert_eq!(
        recorded[0]["parallel_tool_calls"], false,
        "same-format request should still inherit ModelSurface parallel-call policy before hitting the upstream: {:?}",
        recorded[0]
    );

    server.abort();
}

#[test]
fn resolve_requested_model_or_error_requires_model_for_multi_upstream_namespace() {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![
            crate::config::UpstreamConfig {
                name: "a".to_string(),
                api_root: "https://example.com/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                provider_key_env: None,
                provider_key: None,
                upstream_headers: Vec::new(),
                proxy: None,
                limits: None,
                surface_defaults: None,
            },
            crate::config::UpstreamConfig {
                name: "b".to_string(),
                api_root: "https://example.org/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                provider_key_env: None,
                provider_key: None,
                upstream_headers: Vec::new(),
                proxy: None,
                limits: None,
                surface_defaults: None,
            },
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    };

    let error = resolve_requested_model_or_error(
        &config,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({}),
    )
    .expect_err("missing model should fail");

    assert!(error.contains("request must include a routable `model`"));
}

#[test]
fn resolve_requested_model_or_error_explains_previous_response_boundary() {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![
            crate::config::UpstreamConfig {
                name: "a".to_string(),
                api_root: "https://example.com/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                provider_key_env: None,
                provider_key: None,
                upstream_headers: Vec::new(),
                proxy: None,
                limits: None,
                surface_defaults: None,
            },
            crate::config::UpstreamConfig {
                name: "b".to_string(),
                api_root: "https://example.org/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                provider_key_env: None,
                provider_key: None,
                upstream_headers: Vec::new(),
                proxy: None,
                limits: None,
                surface_defaults: None,
            },
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    };

    let error = resolve_requested_model_or_error(
        &config,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "previous_response_id": "resp_1" }),
    )
    .expect_err("missing model should fail");

    assert!(error.contains("previous_response_id"));
    assert!(error.contains("does not reconstruct response-to-upstream state"));
}

#[tokio::test]
async fn openai_responses_non_stream_transport_error_uses_json_error_shape() {
    let state = app_state_for_single_upstream_with_timeout(
        "http://127.0.0.1:9/v1".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        std::time::Duration::from_millis(50),
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": "Hi",
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body: Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(body["error"]["type"], "server_error");
}

#[tokio::test]
async fn openai_chat_streaming_first_response_timeout_fails_closed_without_body_leak() {
    const SENTINEL: &str = "STREAM_HEADER_TIMEOUT_SENTINEL_SHOULD_NOT_LEAK";
    let (mock_base, server) = spawn_header_delayed_openai_completion_stream_mock(
        std::time::Duration::from_secs(2),
        SENTINEL,
    )
    .await;
    let state = app_state_for_single_upstream_with_timeout(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        std::time::Duration::from_millis(50),
    );

    let started = std::time::Instant::now();
    let response = match tokio::time::timeout(
        std::time::Duration::from_millis(500),
        handle_request_core(
            state,
            DEFAULT_NAMESPACE.to_string(),
            HeaderMap::new(),
            "/openai/v1/chat/completions".to_string(),
            serde_json::json!({
                "model": "gpt-4o-mini",
                "messages": [{ "role": "user", "content": "Hi" }],
                "stream": true
            }),
            "gpt-4o-mini".to_string(),
            crate::formats::UpstreamFormat::OpenAiChatCompletions,
            None,
        ),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => {
            server.abort();
            panic!("streaming request did not fail within the first-response timeout budget");
        }
    };

    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "streaming first-response timeout should fire before the mock returns headers"
    );
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("error body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("error body utf8");
    assert!(body_text.contains("timed out"), "body = {body_text}");
    assert!(!body_text.contains(SENTINEL), "body = {body_text}");

    server.abort();
}

#[tokio::test]
async fn same_format_openai_chat_streaming_fails_closed_on_non_sse_success() {
    const SENTINEL: &str = "NON_SSE_STREAM_SENTINEL_SHOULD_NOT_LEAK";
    let (mock_base, _requests, server) = spawn_openai_completion_raw_mock(
        StatusCode::OK,
        format!(r#"{{"id":"chatcmpl_json","object":"chat.completion","sentinel":"{SENTINEL}"}}"#),
    )
    .await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("error body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("error body utf8");
    assert!(
        body_text.contains("upstream returned non-SSE response for streaming request"),
        "body = {body_text}"
    );
    assert!(!body_text.contains(SENTINEL), "body = {body_text}");

    server.abort();
}

#[tokio::test]
async fn translated_responses_streaming_fails_closed_on_non_sse_success() {
    const SENTINEL: &str = "TRANSLATED_NON_SSE_STREAM_SENTINEL_SHOULD_NOT_LEAK";
    let (mock_base, _requests, server) = spawn_openai_completion_raw_mock(
        StatusCode::OK,
        format!(r#"{{"id":"chatcmpl_json","object":"chat.completion","sentinel":"{SENTINEL}"}}"#),
    )
    .await;
    let state = app_state_for_single_upstream(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": "Hi",
            "stream": true
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream error body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("stream error body utf8");
    assert!(body_text.contains("response.failed"), "body = {body_text}");
    assert!(
        body_text.contains("upstream returned non-SSE response for streaming request"),
        "body = {body_text}"
    );
    assert!(!body_text.contains(SENTINEL), "body = {body_text}");

    server.abort();
}

#[tokio::test]
async fn streaming_requests_are_not_cut_off_by_unary_upstream_timeout() {
    let (mock_base, server) =
        spawn_delayed_openai_completion_stream_mock(std::time::Duration::from_millis(150)).await;
    let state = app_state_for_single_upstream_with_timeout(
        mock_base,
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        std::time::Duration::from_millis(50),
    );

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/chat/completions".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiChatCompletions,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("stream body bytes");
    let body = String::from_utf8(body.to_vec()).expect("utf8 stream body");
    assert!(body.contains("\"content\":\"Hi\""), "body = {body}");
    assert!(body.contains("data: [DONE]"), "body = {body}");

    server.abort();
}
