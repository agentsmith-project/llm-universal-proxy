use super::openai_sink::*;
use super::state::*;
use super::*;
use crate::formats::UpstreamFormat;
use crate::translate::translate_response;

fn parse_sse_json(bytes: &[u8]) -> Value {
    let mut buf = bytes.to_vec();
    take_one_sse_event(&mut buf).expect("parse sse event")
}

fn typed_tool_bridge_context(stable_name: &str, source_kind: &str) -> Value {
    let mut entries = serde_json::Map::new();
    entries.insert(
        stable_name.to_string(),
        serde_json::json!({
            "stable_name": stable_name,
            "source_kind": source_kind,
            "transport_kind": "function_object_wrapper",
            "wrapper_field": "input",
            "expected_canonical_shape": "single_required_string"
        }),
    );
    serde_json::json!({
        "version": 3,
        "purpose": "openai_responses_custom_tool_bridge",
        "entries": entries,
        "namespace_entries": {}
    })
}

fn typed_namespace_bridge_context(namespace: &str, child: &str) -> Value {
    let flat = format!("{namespace}__{child}");
    let mut ns_entries = serde_json::Map::new();
    ns_entries.insert(
        flat,
        serde_json::json!({
            "namespace": namespace,
            "child": child
        }),
    );
    serde_json::json!({
        "version": 3,
        "purpose": "openai_responses_custom_tool_bridge",
        "entries": {},
        "namespace_entries": ns_entries
    })
}

mod anthropic_sink;
mod anthropic_source;
mod responses_sink;
mod responses_source;
mod stream;
mod wire;
