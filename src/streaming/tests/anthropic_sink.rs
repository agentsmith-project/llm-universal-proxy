use super::*;

fn parse_sse_events(bytes: Vec<Vec<u8>>) -> Vec<Value> {
    bytes
        .into_iter()
        .map(|bytes| parse_sse_json(&bytes))
        .collect()
}

#[test]
fn openai_chunk_to_claude_sse_emits_message_start_then_content_block() {
    let chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": null }]
    });
    let mut state = StreamState::default();
    let out = openai_chunk_to_claude_sse(&chunk, &mut state);
    assert!(!out.is_empty());
    assert!(state.message_start_sent);
    let chunk2 = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
    });
    let out2 = openai_chunk_to_claude_sse(&chunk2, &mut state);
    assert!(!out2.is_empty());
    let has_content_block = out2
        .iter()
        .any(|b| String::from_utf8_lossy(b).contains("content_block"));
    assert!(has_content_block);
}

#[test]
fn openai_chunk_to_claude_sse_maps_context_window_finish_reason() {
    let chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "context_length_exceeded" }]
    });
    let mut state = StreamState::default();
    let out = openai_chunk_to_claude_sse(&chunk, &mut state);
    let joined = out
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"stop_reason\":\"model_context_window_exceeded\""));
}

#[test]
fn openai_chunk_to_claude_sse_emits_error_event_for_error_finish() {
    let chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "error" }]
    });
    let mut state = StreamState::default();
    let out = openai_chunk_to_claude_sse(&chunk, &mut state);
    let joined = out
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"type\":\"error\""));
    assert!(joined.contains("\"api_error\""));
    assert!(!joined.contains("\"stop_reason\":\"end_turn\""));
    assert!(!joined.contains("\"type\":\"message_stop\""));
}

#[test]
fn openai_chunk_to_claude_sse_emits_error_event_for_tool_error_finish() {
    let chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_error" }]
    });
    let mut state = StreamState::default();
    let out = openai_chunk_to_claude_sse(&chunk, &mut state);
    let joined = out
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("\"type\":\"error\""));
    assert!(joined.contains("\"invalid_request_error\""));
    assert!(!joined.contains("\"stop_reason\":\"end_turn\""));
    assert!(!joined.contains("\"type\":\"message_stop\""));
}

#[test]
fn openai_chunk_to_claude_sse_emits_unsigned_thinking_for_reasoning_content() {
    let reasoning_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "role": "assistant", "reasoning_content": "think" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });
    let mut state = StreamState::default();
    let out1 = openai_chunk_to_claude_sse(&reasoning_chunk, &mut state);
    let out2 = openai_chunk_to_claude_sse(&finish_chunk, &mut state);
    let events = parse_sse_events(out1.into_iter().chain(out2).collect());
    assert_eq!(events[0]["type"], "message_start");
    assert_eq!(events[1]["type"], "content_block_start");
    assert_eq!(events[1]["content_block"]["type"], "thinking");
    assert_eq!(events[1]["content_block"]["thinking"], "");
    assert_eq!(events[2]["type"], "content_block_delta");
    assert_eq!(events[2]["delta"]["type"], "thinking_delta");
    assert_eq!(events[2]["delta"]["thinking"], "think");
    assert_eq!(events[3]["type"], "content_block_stop");
    assert_eq!(events[4]["type"], "message_delta");
    assert_eq!(events[4]["delta"]["stop_reason"], "end_turn");
    assert_eq!(events[5]["type"], "message_stop");
    assert!(events.iter().all(|event| event["type"] != "error"));
}

#[test]
fn openai_chunk_to_claude_sse_preserves_reasoning_text_and_tool_block_order() {
    let reasoning_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "role": "assistant", "reasoning_content": "need tool" }, "finish_reason": null }]
    });
    let text_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "content": "Calling tool." }, "finish_reason": null }]
    });
    let tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_weather",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "usage": { "prompt_tokens": 1, "completion_tokens": 4, "total_tokens": 5 },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
    });
    let mut state = StreamState::default();
    let events = parse_sse_events(
        openai_chunk_to_claude_sse(&reasoning_chunk, &mut state)
            .into_iter()
            .chain(openai_chunk_to_claude_sse(&text_chunk, &mut state))
            .chain(openai_chunk_to_claude_sse(&tool_chunk, &mut state))
            .chain(openai_chunk_to_claude_sse(&finish_chunk, &mut state))
            .collect(),
    );

    assert_eq!(events[0]["type"], "message_start");
    assert_eq!(events[1]["type"], "content_block_start");
    assert_eq!(events[1]["content_block"]["type"], "thinking");
    assert_eq!(events[2]["type"], "content_block_delta");
    assert_eq!(events[2]["delta"]["type"], "thinking_delta");
    assert_eq!(events[2]["delta"]["thinking"], "need tool");
    assert_eq!(events[3]["type"], "content_block_stop");
    assert_eq!(events[3]["index"], 0);
    assert_eq!(events[4]["type"], "content_block_start");
    assert_eq!(events[4]["content_block"]["type"], "text");
    assert_eq!(events[5]["type"], "content_block_delta");
    assert_eq!(events[5]["delta"]["type"], "text_delta");
    assert_eq!(events[5]["delta"]["text"], "Calling tool.");
    assert_eq!(events[6]["type"], "content_block_stop");
    assert_eq!(events[6]["index"], 1);
    assert_eq!(events[7]["type"], "content_block_start");
    assert_eq!(events[7]["content_block"]["type"], "tool_use");
    assert_eq!(events[7]["content_block"]["id"], "call_weather");
    assert_eq!(events[7]["content_block"]["name"], "get_weather");
    assert_eq!(events[8]["type"], "content_block_delta");
    assert_eq!(events[8]["delta"]["type"], "input_json_delta");
    assert_eq!(events[8]["delta"]["partial_json"], "{\"city\":\"Tokyo\"}");
    assert_eq!(events[9]["type"], "content_block_stop");
    assert_eq!(events[9]["index"], 2);
    assert_eq!(events[10]["type"], "message_delta");
    assert_eq!(events[10]["delta"]["stop_reason"], "tool_use");
    assert_eq!(events[11]["type"], "message_stop");
    assert!(events.iter().all(|event| event["type"] != "error"));
}

#[test]
fn openai_chunk_to_claude_sse_continues_after_reasoning_into_text_and_finish() {
    let reasoning_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "reasoning_content": "think" }, "finish_reason": null }]
    });
    let content_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });
    let mut state = StreamState::default();
    let events = parse_sse_events(
        openai_chunk_to_claude_sse(&reasoning_chunk, &mut state)
            .into_iter()
            .chain(openai_chunk_to_claude_sse(&content_chunk, &mut state))
            .chain(openai_chunk_to_claude_sse(&finish_chunk, &mut state))
            .collect(),
    );

    assert_eq!(events[0]["type"], "message_start");
    assert_eq!(events[1]["type"], "content_block_start");
    assert_eq!(events[1]["content_block"]["type"], "thinking");
    assert_eq!(events[2]["type"], "content_block_delta");
    assert_eq!(events[2]["delta"]["thinking"], "think");
    assert_eq!(events[3]["type"], "content_block_stop");
    assert_eq!(events[4]["type"], "content_block_start");
    assert_eq!(events[4]["content_block"]["type"], "text");
    assert_eq!(events[5]["type"], "content_block_delta");
    assert_eq!(events[5]["delta"]["text"], "Hi");
    assert_eq!(events[6]["type"], "content_block_stop");
    assert_eq!(events[7]["type"], "message_delta");
    assert_eq!(events[8]["type"], "message_stop");
    assert!(events.iter().all(|event| event["type"] != "error"));
}

#[test]
fn openai_chunk_to_claude_sse_translates_bridged_custom_tool_calls_without_rejection() {
    let custom_tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_custom",
                    "function": {
                        "name": "code_exec",
                        "arguments": "{\"input\":\"print('hi')\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
    });

    let mut state = StreamState::default();
    let joined = openai_chunk_to_claude_sse(&custom_tool_chunk, &mut state)
        .into_iter()
        .chain(openai_chunk_to_claude_sse(&finish_chunk, &mut state))
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"type\":\"message_start\""), "{joined}");
    assert!(joined.contains("\"type\":\"tool_use\""), "{joined}");
    assert!(joined.contains("\"name\":\"code_exec\""), "{joined}");
    assert!(joined.contains("input_json_delta"), "{joined}");
    assert!(
        joined.contains("{\\\"input\\\":\\\"print('hi')\\\"}"),
        "{joined}"
    );
    assert!(joined.contains("\"stop_reason\":\"tool_use\""), "{joined}");
    assert!(!joined.contains("event: error"), "{joined}");
}

#[test]
fn openai_chunk_to_claude_sse_still_rejects_unbridged_custom_tool_calls() {
    let custom_tool_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_custom",
                    "type": "custom",
                    "function": {
                        "name": "code_exec",
                        "arguments": "print('hi')"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let mut state = StreamState::default();
    let joined = openai_chunk_to_claude_sse(&custom_tool_chunk, &mut state)
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("event: error"), "{joined}");
    assert!(joined.contains("\"type\":\"error\""), "{joined}");
    assert!(joined.contains("custom tools"), "{joined}");
    assert!(!joined.contains("tool_use"), "{joined}");
    assert!(!joined.contains("input_json_delta"), "{joined}");
}

#[test]
fn openai_chunk_to_claude_sse_translates_usage_to_anthropic_shape() {
    let finish_chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 18
        },
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });
    let mut state = StreamState::default();
    let joined = openai_chunk_to_claude_sse(&finish_chunk, &mut state)
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"usage\":{\"input_tokens\":11,\"output_tokens\":7"));
    assert!(!joined.contains("\"prompt_tokens\""));
    assert!(!joined.contains("\"completion_tokens\""));
}

#[test]
fn openai_chunk_to_claude_sse_restores_server_tool_use_from_proxied_tool_kind() {
    let mut state = StreamState::default();
    let mut chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "server_1",
                    "proxied_tool_kind": "anthropic_server_tool_use",
                    "function": {
                        "name": "web_search",
                        "arguments": "{\"query\":\"rust\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    // Only a proxy-attested marker (valid process-local keyed-MAC) restores
    // `server_tool_use`; a forged marker must degrade to `tool_use`.
    crate::translate::attest_proxied_tool_kind(&mut chunk["choices"][0]["delta"]["tool_calls"][0]);

    let out = openai_chunk_to_claude_sse(&chunk, &mut state);
    let content_block_start = out
        .iter()
        .map(|bytes| parse_sse_json(bytes))
        .find(|event| event.get("type").and_then(Value::as_str) == Some("content_block_start"))
        .expect("content_block_start event");

    assert_eq!(
        content_block_start["content_block"]["type"],
        "server_tool_use"
    );
}

#[test]
fn openai_chunk_to_claude_sse_preserves_standard_function_tool_use() {
    let mut state = StreamState::default();
    let chunk = serde_json::json!({
        "id": "chatcmpl-msg123",
        "created": 123,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": {
                        "name": "lookup_weather",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    let out = openai_chunk_to_claude_sse(&chunk, &mut state);
    let joined = out
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(joined.contains("\"type\":\"tool_use\""), "{joined}");
    assert!(joined.contains("input_json_delta"), "{joined}");
    assert!(!joined.contains("event: error"), "{joined}");
}

#[test]
fn openai_chunk_to_claude_sse_reopens_text_block_after_reasoning() {
    // PF-3: after text -> reasoning -> text (common for reasoning models), the
    // second text delta must open a NEW content_block at a fresh index. The bug
    // left `text_block_started=true` after closing, so the trailing text emitted
    // a content_block_delta against an already-closed block index.
    let text1 = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "content": "Hello" }, "finish_reason": null }]
    });
    let reasoning = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "reasoning_content": "think" }, "finish_reason": null }]
    });
    let text2 = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "content": "World" }, "finish_reason": null }]
    });
    let finish = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });

    let mut state = StreamState::default();
    let events = parse_sse_events(
        openai_chunk_to_claude_sse(&text1, &mut state)
            .into_iter()
            .chain(openai_chunk_to_claude_sse(&reasoning, &mut state))
            .chain(openai_chunk_to_claude_sse(&text2, &mut state))
            .chain(openai_chunk_to_claude_sse(&finish, &mut state))
            .collect(),
    );

    let text_block_starts: Vec<u64> = events
        .iter()
        .filter(|e| e["type"] == "content_block_start" && e["content_block"]["type"] == "text")
        .map(|e| e["index"].as_u64().unwrap())
        .collect();
    assert_eq!(
        text_block_starts.len(),
        2,
        "expected two text content_block_start events, got {text_block_starts:?} in {events:?}"
    );
    let (first_text_idx, second_text_idx) = (text_block_starts[0], text_block_starts[1]);
    assert!(
        second_text_idx > first_text_idx,
        "second text block should open at a new, higher index"
    );

    // The "World" delta must target the new text block, and a
    // content_block_start for that index must precede its first delta.
    let world_delta = events
        .iter()
        .find(|e| e["type"] == "content_block_delta" && e["delta"]["text"] == "World")
        .expect("text_delta for World");
    let world_idx = world_delta["index"].as_u64().unwrap();
    assert_eq!(
        world_idx, second_text_idx,
        "second text delta should target the new text block"
    );

    let start_position = events
        .iter()
        .position(|e| e["type"] == "content_block_start" && e["index"].as_u64() == Some(world_idx))
        .expect("content_block_start for second text block");
    let delta_position = events
        .iter()
        .position(|e| e["type"] == "content_block_delta" && e["delta"]["text"] == "World")
        .expect("content_block_delta for World");
    assert!(start_position < delta_position);

    // The first text block must have been closed before the second one opened.
    let first_stop_position = events
        .iter()
        .position(|e| {
            e["type"] == "content_block_stop" && e["index"].as_u64() == Some(first_text_idx)
        })
        .expect("content_block_stop for first text block");
    assert!(first_stop_position < start_position);
    assert!(events.iter().all(|e| e["type"] != "error"));
}

#[test]
fn openai_chunk_to_claude_sse_captures_usage_from_choices_empty_chunk() {
    // PF-6: a trailing usage-only chunk (`choices: []` + `usage`), as emitted by
    // MiniMax/vLLM-style gateways, must still populate state.usage so the
    // subsequent message_delta reports real token counts instead of zeros. The
    // bug early-returned on empty choices before capturing usage.
    let content = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }]
    });
    let usage_only = serde_json::json!({
        "id": "chatcmpl-msg123",
        "usage": { "prompt_tokens": 5, "completion_tokens": 9, "total_tokens": 14 },
        "choices": []
    });
    let finish = serde_json::json!({
        "id": "chatcmpl-msg123",
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });

    let mut state = StreamState::default();
    let events = parse_sse_events(
        openai_chunk_to_claude_sse(&content, &mut state)
            .into_iter()
            .chain(openai_chunk_to_claude_sse(&usage_only, &mut state))
            .chain(openai_chunk_to_claude_sse(&finish, &mut state))
            .collect(),
    );

    let message_delta = events
        .iter()
        .find(|e| e["type"] == "message_delta")
        .expect("message_delta event");
    assert_eq!(message_delta["usage"]["input_tokens"], 5);
    assert_eq!(message_delta["usage"]["output_tokens"], 9);
}
