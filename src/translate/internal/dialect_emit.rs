//! Dialect-switched reasoning-effort emit + level capping (Step 3 of the reasoning/dialect plan).
//!
//! This module is the single choke-point for *emit*: given a resolved upstream [`DialectBlock`],
//! the client's normalized reasoning level, and the final upstream request body, it mutates the
//! body into the dialect's native wire shape and returns a portability warning when the level is
//! capped or dropped. It is a pure `&mut Value` transform with no I/O and no access to the
//! upstream config, which keeps it trivially unit-testable and free of the proxy's warning channel.
//!
//! Wiring lives in the proxy layer (`src/server/proxy.rs`): the proxy resolves the dialect from
//! `UpstreamConfig.dialect`, extracts the client level from the *original* (pre-translation) body,
//! calls [`apply_dialect_reasoning_emit`] on the final upstream body, and pushes any returned
//! warning into the existing `portability_warnings` channel. When no dialect is configured, the
//! proxy never calls into this module and behavior is byte-identical to today.

use serde_json::Value;

use crate::config::{DialectBlock, ReasoningLevel, ReasoningMechanism};
use crate::formats::UpstreamFormat;

/// Whether the dialect's mechanism translates a client reasoning-effort value into the upstream's
/// native shape (rather than dropping it). The `auto-only` and `none` mechanisms do not map.
pub(crate) fn dialect_maps_reasoning(dialect: Option<&DialectBlock>) -> bool {
    dialect.is_some_and(|block| block.reasoning.maps_reasoning())
}

impl ReasoningMechanism {
    /// Whether this mechanism emits a translated effort (vs. dropping it).
    pub(crate) fn maps_reasoning(&self) -> bool {
        matches!(
            self,
            ReasoningMechanism::OpenAiEffort
                | ReasoningMechanism::AnthropicEffort
                | ReasoningMechanism::AnthropicThinking
        )
    }
}

/// Extract the client's reasoning effort from a pre-translation client body, normalized to the
/// union vocabulary.
///
/// Recognizes the per-protocol inbound shapes: OpenAI Chat `reasoning_effort`, OpenAI Responses
/// `reasoning.effort`, and Anthropic `output_config.effort`. Returns `None` when the client sent
/// no effort or the value is not a recognized union level.
pub(crate) fn parse_client_reasoning_effort(
    body: &Value,
    client_format: UpstreamFormat,
) -> Option<ReasoningLevel> {
    let raw = match client_format {
        UpstreamFormat::OpenAiChatCompletions => body
            .get("reasoning_effort")
            .and_then(Value::as_str),
        UpstreamFormat::OpenAiResponses => body
            .get("reasoning")
            .and_then(Value::as_object)
            .and_then(|reasoning| reasoning.get("effort"))
            .and_then(Value::as_str),
        UpstreamFormat::Anthropic => body
            .get("output_config")
            .and_then(Value::as_object)
            .and_then(|config| config.get("effort"))
            .and_then(Value::as_str),
    }?;
    ReasoningLevel::parse(raw).ok()
}

/// Apply the dialect-aware reasoning-effort emit to the final upstream request body.
///
/// `effort` is the client's normalized level. The body is mutated into the shape selected by
/// `dialect.reasoning`; the level is capped to the dialect's declared ceiling. Returns a
/// portability warning string when the level was capped or dropped, and `None` when it was
/// emitted unchanged.
pub(crate) fn apply_dialect_reasoning_emit(
    body: &mut Value,
    upstream_format: UpstreamFormat,
    dialect: &DialectBlock,
    effort: ReasoningLevel,
) -> Option<String> {
    match dialect.reasoning {
        ReasoningMechanism::OpenAiEffort => {
            let (level, capped) = cap_to_ceiling(effort, dialect);
            emit_openai_effort(body, upstream_format, level);
            capped.then(|| cap_warning(effort, level))
        }
        ReasoningMechanism::AnthropicEffort => {
            let (level, capped) = cap_to_ceiling(effort, dialect);
            emit_anthropic_effort(body, level);
            capped.then(|| cap_warning(effort, level))
        }
        ReasoningMechanism::AnthropicThinking => {
            let (level, capped) = cap_to_ceiling(effort, dialect);
            emit_anthropic_thinking(body, level);
            capped.then(|| cap_warning(effort, level))
        }
        ReasoningMechanism::AutoOnly | ReasoningMechanism::None => {
            // Provider auto-reasons or has no reasoning knob; the client's effort is not
            // translatable. Strip any reasoning field the translator may have forwarded so the
            // upstream never receives a knob its dialect declared unsupported.
            strip_reasoning_fields(body, upstream_format);
            Some(format!(
                "reasoning effort `{effort}` is not translatable to this upstream's dialect mechanism (`{}`) and was dropped",
                dialect.reasoning
            ))
        }
    }
}

/// Resolve the effective ceiling for a dialect: the maximum of `reasoning_levels` when declared,
/// otherwise the mechanism default from the plan's §3.2 table.
fn ceiling_for(dialect: &DialectBlock) -> ReasoningLevel {
    if let Some(levels) = dialect.reasoning_levels.as_ref() {
        // Validation guarantees strictly-increasing union order, so the last entry is the max.
        return *levels.last().expect("validated non-empty");
    }
    match dialect.reasoning {
        // §3.2 defaults: openai-effort and anthropic-effort cap at `max`; anthropic-thinking's
        // budget table covers the whole union, so its effective ceiling is `ultra` (no cap).
        ReasoningMechanism::OpenAiEffort | ReasoningMechanism::AnthropicEffort => {
            ReasoningLevel::Max
        }
        ReasoningMechanism::AnthropicThinking => ReasoningLevel::Ultra,
        ReasoningMechanism::AutoOnly | ReasoningMechanism::None => ReasoningLevel::Ultra,
    }
}

/// Cap `effort` to the dialect ceiling, returning `(level, capped)`.
fn cap_to_ceiling(effort: ReasoningLevel, dialect: &DialectBlock) -> (ReasoningLevel, bool) {
    let ceiling = ceiling_for(dialect);
    if effort > ceiling {
        (ceiling, true)
    } else {
        (effort, false)
    }
}

fn cap_warning(original: ReasoningLevel, capped: ReasoningLevel) -> String {
    format!(
        "reasoning effort `{original}` exceeds this upstream dialect's ceiling `{capped}` and was capped"
    )
}

fn emit_openai_effort(body: &mut Value, upstream_format: UpstreamFormat, level: ReasoningLevel) {
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    match upstream_format {
        UpstreamFormat::OpenAiChatCompletions => {
            obj.insert(
                "reasoning_effort".to_string(),
                Value::String(level.to_string()),
            );
        }
        UpstreamFormat::OpenAiResponses => {
            // Responses carries effort under `reasoning.effort`; drop any stale chat-shaped field.
            obj.remove("reasoning_effort");
            let reasoning = obj
                .entry("reasoning".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(reasoning) = reasoning.as_object_mut() {
                reasoning.insert("effort".to_string(), Value::String(level.to_string()));
            }
        }
        UpstreamFormat::Anthropic => {
            // Mechanism/format mismatch (a preset would never pair openai-effort with anthropic).
            // Fall back to the chat-shaped field rather than silently dropping.
            obj.insert(
                "reasoning_effort".to_string(),
                Value::String(level.to_string()),
            );
        }
    }
}

fn emit_anthropic_effort(body: &mut Value, level: ReasoningLevel) {
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    let output_config = obj
        .entry("output_config".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(config) = output_config.as_object_mut() {
        config.insert("effort".to_string(), Value::String(level.to_string()));
    }
}

fn emit_anthropic_thinking(body: &mut Value, level: ReasoningLevel) {
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    let thinking = match level {
        ReasoningLevel::None => serde_json::json!({ "type": "disabled" }),
        other => serde_json::json!({
            "type": "enabled",
            "budget_tokens": budget_tokens_for(other),
        }),
    };
    obj.insert("thinking".to_string(), thinking);
}

/// §3.3 effort → `budget_tokens` table (Haiku 4.5 / legacy `anthropic-thinking` only).
fn budget_tokens_for(level: ReasoningLevel) -> u32 {
    match level {
        ReasoningLevel::Minimal => 1024,
        ReasoningLevel::Low => 2048,
        ReasoningLevel::Medium => 8000,
        ReasoningLevel::High => 16000,
        ReasoningLevel::Xhigh => 24000,
        ReasoningLevel::Max | ReasoningLevel::Ultra => 32000,
        ReasoningLevel::None => 1024, // unreachable: `none` takes the disabled branch
    }
}

fn strip_reasoning_fields(body: &mut Value, upstream_format: UpstreamFormat) {
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    match upstream_format {
        UpstreamFormat::OpenAiChatCompletions => {
            obj.remove("reasoning_effort");
        }
        UpstreamFormat::OpenAiResponses => {
            obj.remove("reasoning_effort");
            if let Some(reasoning) = obj.get_mut("reasoning").and_then(Value::as_object_mut) {
                reasoning.remove("effort");
            }
        }
        UpstreamFormat::Anthropic => {
            if let Some(config) = obj.get_mut("output_config").and_then(Value::as_object_mut) {
                config.remove("effort");
            }
            obj.remove("thinking");
        }
    }
}
