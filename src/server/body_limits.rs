use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{HeaderMap, Response, StatusCode},
};
use bytes::Bytes;
use serde_json::Value;

use crate::formats::UpstreamFormat;

use super::data_auth::RequestAuthContext;
use super::errors::error_response;
use super::state::AppState;

#[derive(Debug, Clone)]
pub(super) struct JsonRequestBody {
    raw_bytes: Bytes,
    parsed: Value,
}

impl JsonRequestBody {
    pub(super) fn parsed(&self) -> &Value {
        &self.parsed
    }

    pub(super) fn into_parts(self) -> (Bytes, Value) {
        (self.raw_bytes, self.parsed)
    }

    #[cfg(test)]
    pub(super) fn from_parsed_value(parsed: Value) -> Self {
        let raw_bytes = serde_json::to_vec(&parsed)
            .map(Bytes::from)
            .expect("serde_json::Value should serialize");
        Self { raw_bytes, parsed }
    }
}

pub(super) async fn read_limited_json_request(
    _state: &Arc<AppState>,
    namespace: &str,
    client_format: UpstreamFormat,
    auth_context: &RequestAuthContext,
    request: Request,
) -> Result<(HeaderMap, JsonRequestBody), Response<Body>> {
    let max_request_body_bytes = request_body_limit_for_namespace(namespace, auth_context);
    let headers = request.headers().clone();
    let raw_bytes = match to_bytes(request.into_body(), max_request_body_bytes).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Err(error_response(
                client_format,
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeded resource limit",
            ));
        }
    };
    let parsed = serde_json::from_slice(&raw_bytes).map_err(|_| {
        error_response(
            client_format,
            StatusCode::BAD_REQUEST,
            "invalid JSON request body",
        )
    })?;
    let body = JsonRequestBody { raw_bytes, parsed };
    Ok((headers, body))
}

pub(super) async fn read_limited_json_value_request(
    state: &Arc<AppState>,
    namespace: &str,
    client_format: UpstreamFormat,
    auth_context: &RequestAuthContext,
    request: Request,
) -> Result<(HeaderMap, Value), Response<Body>> {
    read_limited_json_request(state, namespace, client_format, auth_context, request)
        .await
        .map(|(headers, body)| {
            let (_, parsed) = body.into_parts();
            (headers, parsed)
        })
}

fn request_body_limit_for_namespace(namespace: &str, auth_context: &RequestAuthContext) -> usize {
    auth_context
        .runtime()
        .namespaces
        .get(namespace)
        .map(|namespace_state| {
            namespace_state
                .config
                .resource_limits
                .max_request_body_bytes
        })
        .unwrap_or_else(|| crate::config::ResourceLimits::default().max_request_body_bytes)
}
