//! Upstream HTTP client: build request URLs and call upstream resources.

use std::error::Error;
use std::fmt;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;
use reqwest::{header::CONTENT_TYPE, Client, Proxy};
use serde_json::Value;

use crate::config::{Config, ProxyConfig, UpstreamConfig};
use crate::downstream::DownstreamCancellation;
use crate::formats::UpstreamFormat;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedProxySource {
    Upstream,
    Namespace,
    Environment,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedProxyTarget {
    Inherited,
    Direct,
    Proxy { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedProxyMetadata {
    pub source: ResolvedProxySource,
    pub target: ResolvedProxyTarget,
}

pub(crate) fn resolve_upstream_proxy(
    upstream_proxy: Option<&ProxyConfig>,
    namespace_proxy: Option<&ProxyConfig>,
) -> ResolvedProxyMetadata {
    if let Some(proxy) = upstream_proxy {
        return ResolvedProxyMetadata {
            source: ResolvedProxySource::Upstream,
            target: resolved_proxy_target(proxy),
        };
    }
    if let Some(proxy) = namespace_proxy {
        return ResolvedProxyMetadata {
            source: ResolvedProxySource::Namespace,
            target: resolved_proxy_target(proxy),
        };
    }
    if has_environment_proxy_configuration() {
        return ResolvedProxyMetadata {
            source: ResolvedProxySource::Environment,
            target: ResolvedProxyTarget::Inherited,
        };
    }
    ResolvedProxyMetadata {
        source: ResolvedProxySource::None,
        target: ResolvedProxyTarget::Inherited,
    }
}

fn resolved_proxy_target(proxy: &ProxyConfig) -> ResolvedProxyTarget {
    match proxy {
        ProxyConfig::Direct => ResolvedProxyTarget::Direct,
        ProxyConfig::Proxy { url } => ResolvedProxyTarget::Proxy { url: url.clone() },
    }
}

fn has_environment_proxy_configuration() -> bool {
    const CANDIDATES: [&str; 6] = [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ];
    CANDIDATES.into_iter().any(|key| {
        std::env::var(key)
            .ok()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    })
}

fn build_client_with_proxy(
    timeout: Duration,
    resolved_proxy: &ResolvedProxyMetadata,
    streaming: bool,
    auto_decompression: bool,
) -> Result<Client, String> {
    let mut builder = Client::builder();
    builder = if streaming {
        builder.connect_timeout(timeout)
    } else {
        builder.timeout(timeout)
    };
    if !auto_decompression {
        builder = builder.no_gzip().no_brotli().no_zstd().no_deflate();
    }
    match resolved_proxy.target {
        ResolvedProxyTarget::Inherited => {}
        ResolvedProxyTarget::Direct => {
            builder = builder.no_proxy();
        }
        ResolvedProxyTarget::Proxy { ref url } => {
            builder = builder.no_proxy();
            let proxy = Proxy::all(url).map_err(|error| {
                format!(
                    "invalid explicit upstream proxy `{}`: {error}",
                    crate::config::sanitize_url_for_admin(url)
                )
            })?;
            builder = builder.proxy(proxy);
        }
    }
    builder
        .build()
        .map_err(|error| format!("failed to build upstream HTTP client: {error}"))
}

pub(crate) fn build_upstream_clients(
    config: &Config,
    upstream_proxy: Option<&ProxyConfig>,
    namespace_proxy: Option<&ProxyConfig>,
) -> Result<(Client, Client, ResolvedProxyMetadata), String> {
    let resolved_proxy = resolve_upstream_proxy(upstream_proxy, namespace_proxy);
    let client = build_client_with_proxy(config.upstream_timeout, &resolved_proxy, false, true)?;
    let streaming_client =
        build_client_with_proxy(config.upstream_timeout, &resolved_proxy, true, true)?;
    Ok((client, streaming_client, resolved_proxy))
}

pub(crate) fn build_no_auto_decompression_client(
    timeout: Duration,
    resolved_proxy: &ResolvedProxyMetadata,
) -> Result<Client, String> {
    build_client_with_proxy(timeout, resolved_proxy, false, false)
}

pub(crate) fn build_no_auto_decompression_streaming_client(
    timeout: Duration,
    resolved_proxy: &ResolvedProxyMetadata,
) -> Result<Client, String> {
    build_client_with_proxy(timeout, resolved_proxy, true, false)
}

/// Build a reqwest client with timeout from config.
pub fn build_client(config: &Config) -> Client {
    build_upstream_clients(config, None, config.proxy.as_ref())
        .map(|(client, _, _)| client)
        .unwrap_or_else(|_| Client::new())
}

/// Build a reqwest client for streaming requests.
///
/// Keep the connect/setup timeout, but avoid a total request timeout so long-lived
/// SSE streams are not cut off mid-body by the unary timeout budget.
pub fn build_streaming_client(config: &Config) -> Client {
    build_upstream_clients(config, None, config.proxy.as_ref())
        .map(|(_, client, _)| client)
        .unwrap_or_else(|_| Client::new())
}

/// Call upstream with JSON body; for non-streaming, read full body and return (status, body bytes).
/// For streaming, returns the response so caller can forward the stream.
pub async fn call_upstream(
    client: &Client,
    url: &str,
    body: &Value,
    stream: bool,
    headers: &[(String, String)],
) -> Result<reqwest::Response, reqwest::Error> {
    let mut req = client.post(url).json(body);
    if stream {
        req = req.header("Accept", "text/event-stream");
    }
    // Forward auth headers
    for (name, value) in headers {
        req = req.header(name, value);
    }
    req.send().await
}

pub(crate) enum UpstreamRequestBody<'a> {
    Json(&'a Value),
    RawJson(&'a Bytes),
}

fn post_request_with_body(
    client: &Client,
    url: &str,
    body: UpstreamRequestBody<'_>,
) -> reqwest::RequestBuilder {
    match body {
        UpstreamRequestBody::Json(body) => client.post(url).json(body),
        UpstreamRequestBody::RawJson(raw_body) => client
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .body(raw_body.clone()),
    }
}

#[derive(Debug)]
pub(crate) enum DownstreamAwareError<E> {
    Inner(E),
    DownstreamCancelled,
}

#[derive(Debug)]
pub(crate) enum UpstreamSendError {
    Transport(reqwest::Error),
    FirstResponseTimeout { timeout: Duration },
}

impl fmt::Display for UpstreamSendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "{error}"),
            Self::FirstResponseTimeout { timeout } => write!(
                f,
                "upstream streaming response headers timed out after {timeout:?}"
            ),
        }
    }
}

impl Error for UpstreamSendError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::FirstResponseTimeout { .. } => None,
        }
    }
}

type BoxError = Box<dyn Error + Send + Sync>;

pub(crate) struct UpstreamResourceTarget {
    reqwest_url: String,
    raw_uri: hyper::Uri,
    requires_raw_path_fidelity: bool,
}

pub(crate) struct UpstreamResourceRequest<'a> {
    pub(crate) method: reqwest::Method,
    pub(crate) target: &'a UpstreamResourceTarget,
    pub(crate) body: Option<&'a Value>,
    pub(crate) headers: &'a [(String, String)],
    pub(crate) accept_event_stream: bool,
    pub(crate) resolved_proxy: &'a ResolvedProxyMetadata,
}

pub(crate) enum UpstreamResourceResponse {
    Reqwest(reqwest::Response),
    Hyper(hyper::Response<hyper::body::Incoming>),
}

impl UpstreamResourceResponse {
    pub(crate) fn status(&self) -> reqwest::StatusCode {
        match self {
            Self::Reqwest(response) => response.status(),
            Self::Hyper(response) => response.status(),
        }
    }

    pub(crate) fn headers(&self) -> &reqwest::header::HeaderMap {
        match self {
            Self::Reqwest(response) => response.headers(),
            Self::Hyper(response) => response.headers(),
        }
    }

    pub(crate) fn into_bytes_stream(
        self,
    ) -> Pin<Box<dyn Stream<Item = Result<Bytes, BoxError>> + Send>> {
        match self {
            Self::Reqwest(response) => Box::pin(
                response
                    .bytes_stream()
                    .map(|result| result.map_err(|error| Box::new(error) as BoxError)),
            ),
            Self::Hyper(response) => Box::pin(
                response
                    .into_body()
                    .into_data_stream()
                    .map(|result| result.map_err(|error| Box::new(error) as BoxError)),
            ),
        }
    }
}

async fn await_with_downstream_cancellation<F, T, E>(
    future: F,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<T, DownstreamAwareError<E>>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    tokio::select! {
        result = future => result.map_err(DownstreamAwareError::Inner),
        _ = downstream_cancellation.cancelled() => Err(DownstreamAwareError::DownstreamCancelled),
    }
}

async fn send_with_optional_first_response_timeout(
    request: reqwest::RequestBuilder,
    first_response_timeout: Option<Duration>,
) -> Result<reqwest::Response, UpstreamSendError> {
    let send = request.send();
    match first_response_timeout {
        Some(timeout) => tokio::time::timeout(timeout, send)
            .await
            .map_err(|_| UpstreamSendError::FirstResponseTimeout { timeout })?
            .map_err(UpstreamSendError::Transport),
        None => send.await.map_err(UpstreamSendError::Transport),
    }
}

pub(crate) async fn call_upstream_with_cancellation(
    client: &Client,
    url: &str,
    body: UpstreamRequestBody<'_>,
    stream: bool,
    headers: &[(String, String)],
    first_response_timeout: Option<Duration>,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<reqwest::Response, DownstreamAwareError<UpstreamSendError>> {
    let mut req = post_request_with_body(client, url, body);
    if stream {
        req = req.header("Accept", "text/event-stream");
    }
    for (name, value) in headers {
        req = req.header(name, value);
    }
    await_with_downstream_cancellation(
        send_with_optional_first_response_timeout(req, first_response_timeout),
        downstream_cancellation,
    )
    .await
}

pub(crate) fn build_upstream_resource_target(
    api_root: &str,
    resource_path: &str,
    query: Option<&str>,
) -> Result<UpstreamResourceTarget, String> {
    let mut reqwest_url = crate::config::build_upstream_resource_url(api_root, resource_path);
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        reqwest_url.push('?');
        reqwest_url.push_str(query);
    }

    let parsed = url::Url::parse(api_root)
        .map_err(|error| format!("upstream api_root is not a valid URL: {error}"))?;
    let origin = parsed.origin().ascii_serialization();
    let base_path = parsed.path().trim_end_matches('/');
    let resource_path = resource_path.trim_start_matches('/');
    let mut path_and_query = if base_path.is_empty() || base_path == "/" {
        format!("/{resource_path}")
    } else {
        format!("{base_path}/{resource_path}")
    };
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        path_and_query.push('?');
        path_and_query.push_str(query);
    }
    let raw_uri = format!("{origin}{path_and_query}")
        .parse::<hyper::Uri>()
        .map_err(|error| format!("upstream resource URI is invalid: {error}"))?;

    Ok(UpstreamResourceTarget {
        reqwest_url,
        raw_uri,
        requires_raw_path_fidelity: resource_path_requires_raw_path_fidelity(resource_path),
    })
}

fn resource_path_requires_raw_path_fidelity(resource_path: &str) -> bool {
    resource_path
        .trim_matches('/')
        .split('/')
        .any(url_stack_normalizes_dot_segment)
}

fn url_stack_normalizes_dot_segment(segment: &str) -> bool {
    let normalized = segment.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "." | ".." | "%2e" | "%2e%2e" | "%2e." | ".%2e"
    )
}

/// Call an arbitrary upstream HTTP resource.
pub async fn call_upstream_resource(
    client: &Client,
    method: reqwest::Method,
    url: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
) -> Result<reqwest::Response, reqwest::Error> {
    send_upstream_resource_request(client, method, url, body, headers, false).await
}

async fn send_upstream_resource_request(
    client: &Client,
    method: reqwest::Method,
    url: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
    accept_event_stream: bool,
) -> Result<reqwest::Response, reqwest::Error> {
    let mut req = client.request(method, url);
    if accept_event_stream {
        req = req.header("Accept", "text/event-stream");
    }
    if let Some(body) = body {
        req = req.json(body);
    }
    for (name, value) in headers {
        req = req.header(name, value);
    }
    req.send().await
}

#[allow(clippy::too_many_arguments)]
async fn send_upstream_resource_request_preserving_path(
    client: &Client,
    method: reqwest::Method,
    target: &UpstreamResourceTarget,
    body: Option<&Value>,
    headers: &[(String, String)],
    accept_event_stream: bool,
    resolved_proxy: &ResolvedProxyMetadata,
    first_response_timeout: Option<Duration>,
) -> Result<UpstreamResourceResponse, BoxError> {
    if target.requires_raw_path_fidelity {
        if !raw_path_fidelity_sender_can_use_direct_connection(resolved_proxy) {
            return Err("Responses resource path contains a dot segment that requires raw request-target fidelity, but the configured upstream proxy would route this request through the URL-normalizing client".into());
        }
        return send_raw_path_upstream_resource_request(
            method,
            target.raw_uri.clone(),
            body,
            headers,
            accept_event_stream,
            first_response_timeout,
        )
        .await;
    }

    send_upstream_resource_request(
        client,
        method,
        &target.reqwest_url,
        body,
        headers,
        accept_event_stream,
    )
    .await
    .map(UpstreamResourceResponse::Reqwest)
    .map_err(|error| Box::new(error) as BoxError)
}

fn raw_path_fidelity_sender_can_use_direct_connection(
    resolved_proxy: &ResolvedProxyMetadata,
) -> bool {
    matches!(
        (&resolved_proxy.source, &resolved_proxy.target),
        (ResolvedProxySource::None, ResolvedProxyTarget::Inherited)
            | (_, ResolvedProxyTarget::Direct)
    )
}

async fn send_raw_path_upstream_resource_request(
    method: reqwest::Method,
    uri: hyper::Uri,
    body: Option<&Value>,
    headers: &[(String, String)],
    accept_event_stream: bool,
    first_response_timeout: Option<Duration>,
) -> Result<UpstreamResourceResponse, BoxError> {
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let client: HyperClient<_, Full<Bytes>> =
        HyperClient::builder(TokioExecutor::new()).build(https);

    let body_bytes = match body {
        Some(body) => {
            Bytes::from(serde_json::to_vec(body).map_err(|error| Box::new(error) as BoxError)?)
        }
        None => Bytes::new(),
    };
    let mut request = hyper::Request::builder().method(method.as_str()).uri(uri);
    if accept_event_stream {
        request = request.header(reqwest::header::ACCEPT, "text/event-stream");
    }
    if body.is_some() {
        request = request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::CONTENT_LENGTH,
                body_bytes.len().to_string(),
            );
    }
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }

    let request = request
        .body(Full::new(body_bytes))
        .map_err(|error| Box::new(error) as BoxError)?;
    // The hyper client is built fresh with no built-in timeout, so bound the
    // wait for response headers with the same first-response timeout the
    // reqwest sibling (`call_upstream_with_cancellation`) already applies. On a
    // hung upstream that accepts the connection but never replies this prevents
    // an unbounded hang that downstream cancellation alone would not catch.
    let response = match first_response_timeout {
        Some(timeout) => match tokio::time::timeout(timeout, client.request(request)).await {
            Ok(result) => result.map_err(|error| Box::new(error) as BoxError)?,
            Err(_) => {
                return Err(format!(
                    "upstream raw-path response headers timed out after {timeout:?}"
                )
                .into());
            }
        },
        None => client
            .request(request)
            .await
            .map_err(|error| Box::new(error) as BoxError)?,
    };
    Ok(UpstreamResourceResponse::Hyper(response))
}

pub(crate) async fn call_upstream_resource_target_with_streaming_accept_and_cancellation(
    client: &Client,
    request: UpstreamResourceRequest<'_>,
    downstream_cancellation: &DownstreamCancellation,
    first_response_timeout: Option<Duration>,
) -> Result<UpstreamResourceResponse, DownstreamAwareError<BoxError>> {
    await_with_downstream_cancellation(
        send_upstream_resource_request_preserving_path(
            client,
            request.method,
            request.target,
            request.body,
            request.headers,
            request.accept_event_stream,
            request.resolved_proxy,
            first_response_timeout,
        ),
        downstream_cancellation,
    )
    .await
}

#[derive(Debug)]
pub(crate) enum ResponseBodyLimitError<E> {
    Inner(E),
    LimitExceeded { limit: usize },
}

async fn read_response_bytes_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<bytes::Bytes, ResponseBodyLimitError<reqwest::Error>> {
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ResponseBodyLimitError::Inner)?;
        if out.len().saturating_add(chunk.len()) > limit {
            return Err(ResponseBodyLimitError::LimitExceeded { limit });
        }
        out.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(out))
}

pub(crate) async fn read_response_bytes_limited_with_cancellation(
    response: reqwest::Response,
    limit: usize,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<bytes::Bytes, DownstreamAwareError<ResponseBodyLimitError<reqwest::Error>>> {
    await_with_downstream_cancellation(
        read_response_bytes_limited(response, limit),
        downstream_cancellation,
    )
    .await
}

pub(crate) async fn read_response_text_limited_with_cancellation(
    response: reqwest::Response,
    limit: usize,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<String, DownstreamAwareError<ResponseBodyLimitError<reqwest::Error>>> {
    read_response_bytes_limited_with_cancellation(response, limit, downstream_cancellation)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
}

async fn read_resource_response_bytes_limited(
    response: UpstreamResourceResponse,
    limit: usize,
) -> Result<bytes::Bytes, ResponseBodyLimitError<BoxError>> {
    let mut stream = response.into_bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ResponseBodyLimitError::Inner)?;
        if out.len().saturating_add(chunk.len()) > limit {
            return Err(ResponseBodyLimitError::LimitExceeded { limit });
        }
        out.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(out))
}

pub(crate) async fn read_resource_response_bytes_limited_with_cancellation(
    response: UpstreamResourceResponse,
    limit: usize,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<bytes::Bytes, DownstreamAwareError<ResponseBodyLimitError<BoxError>>> {
    await_with_downstream_cancellation(
        read_resource_response_bytes_limited(response, limit),
        downstream_cancellation,
    )
    .await
}

pub(crate) async fn read_resource_response_text_limited_with_cancellation(
    response: UpstreamResourceResponse,
    limit: usize,
    downstream_cancellation: &DownstreamCancellation,
) -> Result<String, DownstreamAwareError<ResponseBodyLimitError<BoxError>>> {
    read_resource_response_bytes_limited_with_cancellation(response, limit, downstream_cancellation)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
}

/// Resolve upstream URL for the given active wire format using config base URL.
pub fn upstream_url(
    config: &Config,
    upstream: &UpstreamConfig,
    format: UpstreamFormat,
    model: Option<&str>,
    stream: bool,
) -> String {
    config.upstream_url_for_format(upstream, format, model, stream)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        body::Body,
        extract::{Request, State},
        http::{HeaderMap, Method, StatusCode, Uri},
        response::Response,
        routing::any,
        Router,
    };
    use bytes::Bytes;
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Clone, Default)]
    struct CapturedProxyRequests {
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl CapturedProxyRequests {
        fn push(&self, uri: String) {
            self.requests.lock().expect("proxy request lock").push(uri);
        }

        fn snapshot(&self) -> Vec<String> {
            self.requests.lock().expect("proxy request lock").clone()
        }
    }

    #[derive(Clone)]
    struct ProxyState {
        captured: CapturedProxyRequests,
        client: Client,
    }

    async fn proxy_handler(
        State(state): State<ProxyState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        request: Request,
    ) -> Response {
        let body_bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
            .await
            .expect("read proxy body");
        let target_url = proxy_target_url(&uri, &headers).expect("proxy target URL");
        state.captured.push(target_url.clone());
        let mut upstream = state.client.request(method, &target_url).body(body_bytes);
        for (name, value) in &headers {
            if name.as_str().eq_ignore_ascii_case("host")
                || name.as_str().eq_ignore_ascii_case("proxy-connection")
            {
                continue;
            }
            upstream = upstream.header(name, value);
        }
        let response = upstream.send().await.expect("proxy upstream response");
        build_proxy_response(response).await
    }

    fn proxy_target_url(uri: &Uri, headers: &HeaderMap) -> Option<String> {
        if uri.scheme_str().is_some() && uri.authority().is_some() {
            return Some(uri.to_string());
        }
        let host = headers.get("host")?.to_str().ok()?;
        let path = uri
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/");
        Some(format!("http://{host}{path}"))
    }

    async fn build_proxy_response(response: reqwest::Response) -> Response {
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.unwrap_or_else(|error| {
            Bytes::from(format!("failed to read proxied response body: {error}"))
        });
        let mut builder = Response::builder().status(status);
        for (name, value) in &headers {
            builder = builder.header(name, value);
        }
        builder.body(Body::from(body)).expect("proxy response")
    }

    async fn spawn_forward_proxy() -> (String, CapturedProxyRequests, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
        let addr = listener.local_addr().expect("proxy addr");
        let captured = CapturedProxyRequests::default();
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("proxy client");
        let app = Router::new()
            .route("/", any(proxy_handler))
            .route("/*path", any(proxy_handler))
            .with_state(ProxyState {
                captured: captured.clone(),
                client,
            });
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("proxy server");
        });
        (format!("http://{addr}"), captured, handle)
    }

    async fn spawn_direct_upstream(
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        #[derive(Clone)]
        struct DirectState {
            requests: Arc<Mutex<Vec<String>>>,
        }

        async fn direct_handler(
            uri: Uri,
            State(state): State<DirectState>,
            request: Request,
        ) -> Response {
            let _body = axum::body::to_bytes(request.into_body(), usize::MAX)
                .await
                .expect("read direct body");
            state
                .requests
                .lock()
                .expect("direct request lock")
                .push(uri.path().to_string());
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from("ok"))
                .expect("direct response")
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind direct upstream");
        let addr = listener.local_addr().expect("direct upstream addr");
        let app = Router::new()
            .route("/", any(direct_handler))
            .route("/*path", any(direct_handler))
            .with_state(DirectState {
                requests: requests.clone(),
            });
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("direct upstream server");
        });
        (format!("http://{addr}"), requests, handle)
    }

    fn test_config(timeout: Duration) -> Config {
        Config {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout: timeout,
            proxy: Some(ProxyConfig::Direct),
            upstreams: Vec::new(),
            model_aliases: Default::default(),
            hooks: Default::default(),
            debug_trace: crate::config::DebugTraceConfig::default(),
            resource_limits: Default::default(),
            conversation_state_bridge: Default::default(),
            data_auth: None,
        }
    }

    #[test]
    fn resolve_upstream_proxy_prefers_upstream_then_namespace() {
        // Explicit proxy sources win purely by precedence, with no dependence on
        // the process environment: an upstream-level proxy beats a namespace
        // direct override, and the namespace direct override is itself resolved
        // when no upstream proxy is supplied.
        assert_eq!(
            resolve_upstream_proxy(
                Some(&ProxyConfig::Proxy {
                    url: "http://upstream-proxy.example:8080".to_string(),
                }),
                Some(&ProxyConfig::Direct),
            ),
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Upstream,
                target: ResolvedProxyTarget::Proxy {
                    url: "http://upstream-proxy.example:8080".to_string(),
                },
            }
        );
        assert_eq!(
            resolve_upstream_proxy(None, Some(&ProxyConfig::Direct)),
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Namespace,
                target: ResolvedProxyTarget::Direct,
            }
        );
    }

    #[test]
    fn resolve_upstream_proxy_without_explicit_sources_reflects_ambient_proxy_env() {
        // With no explicit proxy sources, resolution falls through to the process
        // environment. Rather than mutating process env (which races with other
        // tests that read env for config resolution), observe the ambient
        // environment directly and assert the resolver agrees with it.
        let ambient_proxy_present = has_environment_proxy_configuration();
        let expected_source = if ambient_proxy_present {
            ResolvedProxySource::Environment
        } else {
            ResolvedProxySource::None
        };
        assert_eq!(
            resolve_upstream_proxy(None, None),
            ResolvedProxyMetadata {
                source: expected_source,
                target: ResolvedProxyTarget::Inherited,
            }
        );
    }

    #[tokio::test]
    async fn build_upstream_clients_explicit_proxy_routes_request_through_it() {
        // An explicit upstream proxy is wired into the reqwest client via
        // `Proxy::all` together with `no_proxy()`, so the request is routed
        // through the configured proxy regardless of process environment.
        let (explicit_proxy_base, explicit_captured, explicit_proxy_server) =
            spawn_forward_proxy().await;
        let (target_base, direct_requests, direct_server) = spawn_direct_upstream().await;
        let config = test_config(Duration::from_secs(5));

        let (client, _, resolved_proxy) = build_upstream_clients(
            &config,
            Some(&ProxyConfig::Proxy {
                url: explicit_proxy_base.clone(),
            }),
            None,
        )
        .expect("explicit proxy client");

        let response = call_upstream_resource(
            &client,
            reqwest::Method::POST,
            &format!("{target_base}/resource"),
            None,
            &[],
        )
        .await
        .expect("proxied request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            resolved_proxy,
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Upstream,
                target: ResolvedProxyTarget::Proxy {
                    url: explicit_proxy_base.clone(),
                },
            }
        );
        assert_eq!(explicit_captured.snapshot().len(), 1);
        assert_eq!(
            direct_requests.lock().expect("direct request lock").len(),
            1
        );

        direct_server.abort();
        explicit_proxy_server.abort();
    }

    #[tokio::test]
    async fn build_upstream_clients_direct_proxy_routes_request_directly() {
        // An explicit direct override calls `no_proxy()`, so the request bypasses
        // any proxy and reaches the upstream directly, regardless of process
        // environment.
        let (target_base, direct_requests, direct_server) = spawn_direct_upstream().await;
        let config = test_config(Duration::from_secs(5));

        let (client, _, resolved_proxy) =
            build_upstream_clients(&config, Some(&ProxyConfig::Direct), None)
                .expect("direct upstream client");

        let response = call_upstream_resource(
            &client,
            reqwest::Method::GET,
            &format!("{target_base}/resource"),
            None,
            &[],
        )
        .await
        .expect("direct request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            resolved_proxy,
            ResolvedProxyMetadata {
                source: ResolvedProxySource::Upstream,
                target: ResolvedProxyTarget::Direct,
            }
        );
        assert_eq!(
            direct_requests.lock().expect("direct request lock").len(),
            1
        );

        direct_server.abort();
    }

    #[test]
    fn build_client_with_proxy_error_does_not_leak_proxy_credentials() {
        // A credentialed proxy URL whose port is invalid is rejected by
        // `Proxy::all`, exercising the error-message construction in
        // `build_client_with_proxy`. The resulting error must not echo the raw
        // (credentialed) URL; it must be sanitized like every other admin surface.
        let credentialed = "socks5h://leak-user:leak-pass@host.example:abc";
        let resolved = ResolvedProxyMetadata {
            source: ResolvedProxySource::Upstream,
            target: ResolvedProxyTarget::Proxy {
                url: credentialed.to_string(),
            },
        };
        let error = build_client_with_proxy(
            Duration::from_secs(5),
            &resolved,
            false,
            true,
        )
        .expect_err("invalid-port credentialed proxy must fail to build a client");
        let message = error.to_string();
        assert!(
            !message.contains("leak-user"),
            "proxy error must not leak username: {message}"
        );
        assert!(
            !message.contains("leak-pass"),
            "proxy error must not leak password: {message}"
        );
    }

    async fn spawn_silent_upstream() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind silent upstream");
        let addr = listener.local_addr().expect("silent upstream addr");
        let handle = tokio::spawn(async move {
            // Accept every inbound connection but hold it open without ever
            // reading or writing, so the peer hangs forever waiting for
            // response headers.
            let mut held: Vec<tokio::net::TcpStream> = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn send_raw_path_upstream_resource_request_is_bounded_by_first_response_timeout() {
        // Against an upstream that accepts the connection but never replies, the
        // raw-path hyper sender must be bounded by its first-response timeout
        // rather than hanging indefinitely.
        let (base, server_handle) = spawn_silent_upstream().await;
        let uri: hyper::Uri = format!("{base}/silent")
            .parse()
            .expect("silent upstream uri");
        let timeout = Duration::from_millis(300);

        let start = tokio::time::Instant::now();
        let result = send_raw_path_upstream_resource_request(
            reqwest::Method::GET,
            uri,
            None,
            &[],
            false,
            Some(timeout),
        )
        .await;
        let elapsed = start.elapsed();

        server_handle.abort();

        let error = match result {
            Ok(_) => panic!("raw-path call against a silent upstream must time out, not hang"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            message.contains("timed out"),
            "expected a timeout error against the silent upstream, got: {message}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "raw-path call must be bounded by the timeout, not hang; elapsed={elapsed:?}"
        );
    }
}
