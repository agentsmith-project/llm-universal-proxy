use super::*;

#[test]
fn extract_forwardable_headers_keeps_only_protocol_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", HeaderValue::from_static("Bearer test"));
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert("OpenAI-Organization", HeaderValue::from_static("org_123"));
    headers.insert("OpenAI-Project", HeaderValue::from_static("proj_123"));
    headers.insert("Idempotency-Key", HeaderValue::from_static("idem_123"));
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("accept-language", HeaderValue::from_static("*"));
    headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
    headers.insert("connection", HeaderValue::from_static("keep-alive"));
    headers.insert(
        data_auth::LEGACY_DATA_TOKEN_HEADER,
        HeaderValue::from_static("data-token"),
    );

    let forwarded = extract_forwardable_headers(&headers);
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "authorization" && v == "Bearer test"));
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"));
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "openai-organization" && v == "org_123"));
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "openai-project" && v == "proj_123"));
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "idempotency-key" && v == "idem_123"));
    assert!(!forwarded.iter().any(|(k, _)| k == "content-type"));
    assert!(!forwarded.iter().any(|(k, _)| k == "accept-language"));
    assert!(!forwarded.iter().any(|(k, _)| k == "sec-fetch-mode"));
    assert!(!forwarded.iter().any(|(k, _)| k == "connection"));
    assert!(!forwarded
        .iter()
        .any(|(k, _)| k == data_auth::LEGACY_DATA_TOKEN_HEADER));
}

#[test]
fn upstream_response_headers_drop_sensitive_vendor_headers_and_keep_safe_protocol_headers() {
    let mut upstream_headers = reqwest::header::HeaderMap::new();
    for name in [
        "openai-api-key",
        "anthropic-api-key",
        "openai-session-token",
        "anthropic-credential",
        "authorization",
        "set-cookie",
    ] {
        upstream_headers.insert(
            reqwest::header::HeaderName::from_static(name),
            reqwest::header::HeaderValue::from_static("sensitive"),
        );
    }
    for (name, value) in [
        ("request-id", "req_123"),
        ("x-request-id", "xreq_123"),
        ("retry-after", "2"),
        ("ratelimit-limit", "100"),
        ("x-ratelimit-remaining", "99"),
        ("rate-limit", "100"),
        ("openai-processing-ms", "42"),
        ("anthropic-ratelimit-requests-limit", "99"),
    ] {
        upstream_headers.insert(
            reqwest::header::HeaderName::from_static(name),
            reqwest::header::HeaderValue::from_static(value),
        );
    }

    let mut response = Response::builder()
        .body(Body::empty())
        .expect("test response");
    crate::server::headers::append_upstream_protocol_response_headers(
        &mut response,
        &upstream_headers,
        &crate::server::secret_redaction::SecretRedactor::default(),
    );

    for name in [
        "openai-api-key",
        "anthropic-api-key",
        "openai-session-token",
        "anthropic-credential",
        "authorization",
        "set-cookie",
    ] {
        assert!(
            !response.headers().contains_key(name),
            "{name} must not be forwarded"
        );
    }
    for (name, value) in [
        ("request-id", "req_123"),
        ("x-request-id", "xreq_123"),
        ("retry-after", "2"),
        ("ratelimit-limit", "100"),
        ("x-ratelimit-remaining", "99"),
        ("rate-limit", "100"),
        ("openai-processing-ms", "42"),
        ("anthropic-ratelimit-requests-limit", "99"),
    ] {
        assert_eq!(
            response.headers().get(name).and_then(|v| v.to_str().ok()),
            Some(value),
            "{name} should be forwarded"
        );
    }
}

#[test]
fn upstream_response_headers_redact_allowed_header_values_and_keep_safe_values() {
    let secret = "secret-value";
    let mut upstream_headers = reqwest::header::HeaderMap::new();
    upstream_headers.insert(
        reqwest::header::HeaderName::from_static("request-id"),
        reqwest::header::HeaderValue::from_str(&format!("req-{secret}"))
            .expect("secret request id"),
    );
    upstream_headers.insert(
        reqwest::header::HeaderName::from_static("retry-after"),
        reqwest::header::HeaderValue::from_static("2"),
    );

    let mut response = Response::builder()
        .body(Body::empty())
        .expect("test response");
    crate::server::headers::append_upstream_protocol_response_headers(
        &mut response,
        &upstream_headers,
        &crate::server::secret_redaction::SecretRedactor::new([secret]),
    );

    assert_eq!(
        response
            .headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok()),
        Some("req-[REDACTED]")
    );
    assert_eq!(
        response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("2")
    );
}

#[test]
fn raw_upstream_response_headers_keep_encoding_matching_length_and_redact_protocol_values() {
    let secret = "secret-value";
    let encoded_body = b"\x1f\x8b\x08\x00encoded-json";
    let mut upstream_headers = reqwest::header::HeaderMap::new();
    upstream_headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    upstream_headers.insert(
        reqwest::header::CONTENT_ENCODING,
        reqwest::header::HeaderValue::from_static("gzip"),
    );
    upstream_headers.insert(
        reqwest::header::CONTENT_LENGTH,
        reqwest::header::HeaderValue::from_str(&encoded_body.len().to_string())
            .expect("content length"),
    );
    upstream_headers.insert(
        reqwest::header::TRANSFER_ENCODING,
        reqwest::header::HeaderValue::from_static("chunked"),
    );
    upstream_headers.insert(
        reqwest::header::CONNECTION,
        reqwest::header::HeaderValue::from_static("close"),
    );
    upstream_headers.insert(
        reqwest::header::HeaderName::from_static("request-id"),
        reqwest::header::HeaderValue::from_str(&format!("req-{secret}"))
            .expect("secret request id"),
    );
    upstream_headers.insert(
        reqwest::header::HeaderName::from_static("set-cookie"),
        reqwest::header::HeaderValue::from_static("session=must-not-forward"),
    );

    let mut response = Response::builder()
        .body(Body::empty())
        .expect("test response");
    crate::server::headers::append_raw_upstream_response_headers(
        &mut response,
        &upstream_headers,
        encoded_body.len(),
        &crate::server::secret_redaction::SecretRedactor::new([secret]),
    );

    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("gzip")
    );
    let expected_content_length = encoded_body.len().to_string();
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some(expected_content_length.as_str())
    );
    assert_eq!(
        response
            .headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok()),
        Some("req-[REDACTED]")
    );
    for name in [
        reqwest::header::TRANSFER_ENCODING.as_str(),
        reqwest::header::CONNECTION.as_str(),
        "set-cookie",
    ] {
        assert!(
            !response.headers().contains_key(name),
            "{name} must not be forwarded"
        );
    }
}

#[test]
fn raw_upstream_response_headers_drop_mismatched_content_length() {
    let mut upstream_headers = reqwest::header::HeaderMap::new();
    upstream_headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    upstream_headers.insert(
        reqwest::header::CONTENT_LENGTH,
        reqwest::header::HeaderValue::from_static("999"),
    );

    let mut response = Response::builder()
        .body(Body::empty())
        .expect("test response");
    crate::server::headers::append_raw_upstream_response_headers(
        &mut response,
        &upstream_headers,
        b"{}".len(),
        &crate::server::secret_redaction::SecretRedactor::default(),
    );

    assert!(!response
        .headers()
        .contains_key(reqwest::header::CONTENT_LENGTH));
}
