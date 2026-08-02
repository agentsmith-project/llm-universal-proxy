use std::collections::BTreeSet;

use serde_json::Value;

use crate::config::{DialectBlock, ModelModality, ModelSurface};
use crate::formats::UpstreamFormat;
use crate::prompt_cache_controls::{
    anthropic_extra_body_openai_prompt_cache_controls_present,
    anthropic_protocol_cache_control_present, openai_extra_body_anthropic_cache_control_present,
};
use crate::provider_state_controls::{
    provider_state_control_enabled, responses_stateful_request_controls,
};

use super::dialect_emit::dialect_maps_reasoning;
use super::media::{openai_file_part_mime_conflict_message, openai_file_part_resolved_mime_type};
use super::messages::{
    custom_tool_format_portability_warning_message, custom_tool_format_reject_message,
    custom_tools_not_portable_message, openai_assistant_audio_history_not_portable_message,
    openai_request_audio_not_portable_message, responses_reasoning_continuity_not_portable_message,
    single_candidate_choice_contract_message, translation_target_label,
};
use super::models::{
    NormalizedLogprobsControls, NormalizedOpenAiAudioContract, NormalizedOpenAiFamilyToolDef,
    SemanticToolKind, SharedControlProfile, TranslationAssessment,
};
use super::openai_family::{
    openai_extra_body_google_cached_content, validated_openai_extra_body_anthropic_cache_control,
};
use super::openai_responses::{
    responses_agent_message_text, responses_compaction_summary_text,
    responses_input_item_is_compaction, responses_input_item_is_message, responses_input_item_type,
    responses_reasoning_summary_text,
};
use super::tools::{
    normalized_responses_tool_definition, openai_custom_tool_format_is_plain_text,
    openai_custom_tool_format_supports_anthropic_bridge, openai_tool_arguments_to_structured_value,
    responses_tool_call_item_to_openai_tool_call, responses_tool_call_to_structured_value,
    semantic_tool_kind_from_value, tool_call_is_marked_non_replayable,
};
use super::{
    anthropic_nonportable_content_block_message,
    anthropic_request_nonportable_tool_definition_message,
    anthropic_request_tool_result_order_message, validate_anthropic_extra_body_openai_controls,
};

fn openai_family_format(format: UpstreamFormat) -> bool {
    matches!(
        format,
        UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::OpenAiResponses
    )
}

fn assess_prompt_cache_extension_target_mismatch(
    assessment: &mut TranslationAssessment,
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) {
    if openai_family_format(client_format)
        && upstream_format != UpstreamFormat::Anthropic
        && openai_extra_body_anthropic_cache_control_present(body)
    {
        assessment.reject(format!(
            "extra_body.anthropic.cache_control is an explicit Anthropic prompt-cache control and cannot be forwarded to {}; target Anthropic is required",
            translation_target_label(upstream_format)
        ));
    }

    if client_format == UpstreamFormat::Anthropic
        && !openai_family_format(upstream_format)
        && anthropic_extra_body_openai_prompt_cache_controls_present(body)
    {
        assessment.reject(format!(
            "extra_body.openai prompt-cache controls are explicit OpenAI prompt-cache controls and cannot be forwarded to {}; target OpenAI is required",
            translation_target_label(upstream_format)
        ));
    }
}

fn assess_openai_family_prompt_cache_extensions(
    assessment: &mut TranslationAssessment,
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) {
    if !openai_family_format(client_format) || client_format == upstream_format {
        return;
    }

    if openai_extra_body_google_cached_content(body).is_some() {
        assessment.reject(format!(
            "extra_body.google.cached_content requires Gemini cache scope and cannot be translated to {}; the proxy does not map Gemini cached content",
            translation_target_label(upstream_format)
        ));
    }

    if upstream_format == UpstreamFormat::Anthropic
        && openai_extra_body_anthropic_cache_control_present(body)
    {
        if let Err(message) = validated_openai_extra_body_anthropic_cache_control(body) {
            assessment.reject(message);
        }
    }

    if upstream_format == UpstreamFormat::Anthropic {
        let paths = openai_known_cache_control_paths(client_format, body);
        if !paths.is_empty() {
            let quoted = paths
                .iter()
                .map(|path| format!("`{path}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.reject(format!(
                "OpenAI block-level/provider `cache_control` is not supported on translated Anthropic paths; use `extra_body.anthropic.cache_control` for top-level Anthropic cache control instead of {quoted}"
            ));
        }
    }
}

fn assess_anthropic_prompt_cache_extensions(
    assessment: &mut TranslationAssessment,
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) {
    if client_format != UpstreamFormat::Anthropic || !openai_family_format(upstream_format) {
        return;
    }

    if let Err(message) = validate_anthropic_extra_body_openai_controls(body) {
        assessment.reject(message);
    }
}

fn openai_known_cache_control_paths(client_format: UpstreamFormat, body: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    match client_format {
        UpstreamFormat::OpenAiChatCompletions => {
            collect_top_level_tool_cache_control_paths(body, &mut paths);
            if let Some(messages) = body.get("messages").and_then(Value::as_array) {
                for (message_index, message) in messages.iter().enumerate() {
                    if let Some(content) = message.get("content") {
                        collect_content_position_cache_control_paths(
                            content,
                            format!("messages[{message_index}].content"),
                            &mut paths,
                        );
                    }
                    if message.get("role").and_then(Value::as_str) == Some("assistant") {
                        if let Some(tool_calls) =
                            message.get("tool_calls").and_then(Value::as_array)
                        {
                            for (tool_call_index, tool_call) in tool_calls.iter().enumerate() {
                                if tool_call.get("cache_control").is_some() {
                                    paths.push(format!(
                                        "messages[{message_index}].tool_calls[{tool_call_index}].cache_control"
                                    ));
                                }
                            }
                        }
                    }
                    if message.get("role").and_then(Value::as_str) == Some("tool")
                        && message.get("cache_control").is_some()
                    {
                        paths.push(format!("messages[{message_index}].cache_control"));
                    }
                }
            }
        }
        UpstreamFormat::OpenAiResponses => {
            collect_top_level_tool_cache_control_paths(body, &mut paths);
            if let Some(input) = body.get("input").and_then(Value::as_array) {
                for (item_index, item) in input.iter().enumerate() {
                    if responses_input_item_is_message(item) {
                        if let Some(content) = item.get("content") {
                            collect_content_position_cache_control_paths(
                                content,
                                format!("input[{item_index}].content"),
                                &mut paths,
                            );
                        }
                    }
                    if matches!(
                        responses_input_item_type(item),
                        Some("function_call" | "function_call_output")
                    ) && item.get("cache_control").is_some()
                    {
                        paths.push(format!("input[{item_index}].cache_control"));
                    }
                }
            }
        }
        UpstreamFormat::Anthropic => {}
    }
    paths
}

fn collect_top_level_tool_cache_control_paths(body: &Value, paths: &mut Vec<String>) {
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        for (tool_index, tool) in tools.iter().enumerate() {
            if tool.get("cache_control").is_some() {
                paths.push(format!("tools[{tool_index}].cache_control"));
            }
        }
    }
}

fn collect_content_position_cache_control_paths(
    content: &Value,
    base_path: String,
    paths: &mut Vec<String>,
) {
    match content {
        Value::Array(parts) => {
            for (part_index, part) in parts.iter().enumerate() {
                if part.get("cache_control").is_some() {
                    paths.push(format!("{base_path}[{part_index}].cache_control"));
                }
            }
        }
        Value::Object(_) => {
            if content.get("cache_control").is_some() {
                paths.push(format!("{base_path}.cache_control"));
            }
        }
        _ => {}
    }
}

pub(super) fn cross_protocol_store_reject_message(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
) -> String {
    format!(
        "{} request field `store` is an enabled provider persistence/storage request and cannot be translated to {}; same-wire provider-native handling is required",
        translation_target_label(client_format),
        translation_target_label(upstream_format)
    )
}

pub(super) fn shared_control_profile_for_target(
    target_format: UpstreamFormat,
) -> SharedControlProfile {
    match target_format {
        UpstreamFormat::OpenAiChatCompletions => SharedControlProfile {
            metadata: true,
            user: true,
            service_tier: true,
            stream_include_obfuscation: true,
            verbosity: true,
            reasoning_effort: true,
            prompt_cache_key: true,
            prompt_cache_retention: true,
            safety_identifier: true,
            top_logprobs: true,
            parallel_tool_calls: true,
            logit_bias: true,
        },
        UpstreamFormat::OpenAiResponses => SharedControlProfile {
            metadata: true,
            user: true,
            service_tier: true,
            stream_include_obfuscation: true,
            verbosity: true,
            reasoning_effort: true,
            prompt_cache_key: true,
            prompt_cache_retention: true,
            safety_identifier: true,
            top_logprobs: true,
            parallel_tool_calls: true,
            logit_bias: false,
        },
        UpstreamFormat::Anthropic => SharedControlProfile {
            metadata: true,
            parallel_tool_calls: true,
            ..SharedControlProfile::default()
        },
    }
}

pub(super) fn request_stream_include_obfuscation(body: &Value) -> Option<Value> {
    body.get("stream_options")
        .and_then(Value::as_object)
        .and_then(|stream_options| stream_options.get("include_obfuscation"))
        .cloned()
}

pub(super) fn openai_normalized_logprobs_controls(
    body: &Value,
) -> Option<NormalizedLogprobsControls> {
    let enabled = body.get("logprobs").and_then(Value::as_bool) == Some(true);
    let top_logprobs = body.get("top_logprobs").cloned();
    (enabled || top_logprobs.is_some()).then_some(NormalizedLogprobsControls {
        enabled,
        top_logprobs,
    })
}

pub(super) fn responses_normalized_logprobs_controls(
    body: &Value,
) -> Option<NormalizedLogprobsControls> {
    let enabled = responses_include_requests_output_text_logprobs(body);
    let top_logprobs = body.get("top_logprobs").cloned();
    (enabled || top_logprobs.is_some()).then_some(NormalizedLogprobsControls {
        enabled,
        top_logprobs,
    })
}

pub(super) fn normalized_openai_audio_contract(
    body: &Value,
) -> Result<Option<NormalizedOpenAiAudioContract>, String> {
    let modalities = body
        .get("modalities")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim().to_ascii_lowercase())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let requests_audio =
        modalities.iter().any(|item| item == "audio") || body.get("audio").is_some();
    if !requests_audio {
        return Ok(None);
    }

    let audio = body.get("audio").and_then(Value::as_object).ok_or(
        "OpenAI Chat audio output requests require a top-level `audio` object.".to_string(),
    )?;
    if body.get("audio").is_some()
        && !modalities.is_empty()
        && !modalities.iter().any(|item| item == "audio")
    {
        return Err(
            "OpenAI Chat audio output requests require `modalities` to include `audio`."
                .to_string(),
        );
    }
    if let Some(format) = audio.get("format").and_then(Value::as_str) {
        return Err(format!(
            "OpenAI Chat audio field `audio.format` value `{format}` is outside the portable cross-protocol audio subset."
        ));
    }
    let voice_name = match audio.get("voice") {
        Some(Value::String(voice)) if !voice.trim().is_empty() => Some(voice.clone()),
        Some(Value::Object(voice)) => {
            let id = voice.get("id").and_then(Value::as_str).unwrap_or("");
            return Err(format!(
                "OpenAI Chat audio voice `{id}` is outside the portable cross-protocol audio subset."
            ));
        }
        Some(_) => {
            return Err(
                "OpenAI Chat audio voice must be a non-empty string for portable cross-protocol audio translation."
                    .to_string(),
            )
        }
        None => None,
    };

    let normalized_modalities = if modalities.is_empty() {
        vec!["audio".to_string()]
    } else {
        modalities
            .iter()
            .filter(|item| item.as_str() == "text" || item.as_str() == "audio")
            .cloned()
            .collect::<Vec<_>>()
    };
    if normalized_modalities.is_empty() {
        return Err(
            "OpenAI Chat audio output requests require `modalities` to include `audio`."
                .to_string(),
        );
    }

    Ok(Some(NormalizedOpenAiAudioContract {
        response_modalities: normalized_modalities,
        voice_name,
    }))
}

pub(super) fn openai_assistant_history_audio_present(body: &Value) -> bool {
    body.get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages.iter().any(|message| {
                message.get("role").and_then(Value::as_str) == Some("assistant")
                    && message
                        .get("audio")
                        .filter(|audio| !audio.is_null())
                        .is_some()
            })
        })
        .unwrap_or(false)
}

pub(super) fn responses_include_items(body: &Value) -> Vec<&str> {
    body.get("include")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

pub(super) fn responses_include_requests_output_text_logprobs(body: &Value) -> bool {
    responses_include_items(body).contains(&"message.output_text.logprobs")
}

pub(super) fn responses_include_has_nonportable_items(
    body: &Value,
    target_format: UpstreamFormat,
) -> bool {
    let include_items = responses_include_items(body);
    if include_items.is_empty() {
        return body.get("include").is_some();
    }

    include_items.iter().any(|item| {
        if *item == "reasoning.encrypted_content" {
            return false;
        }
        !matches!(
            (target_format, *item),
            (
                UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::OpenAiResponses,
                "message.output_text.logprobs"
            )
        )
    })
}

pub(super) fn responses_text_verbosity(body: &Value) -> Option<Value> {
    body.get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("verbosity"))
        .cloned()
}

pub(super) fn responses_reasoning_effort(body: &Value) -> Option<Value> {
    body.get("reasoning")
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .cloned()
}

pub(super) fn object_has_only_keys(
    object: &serde_json::Map<String, Value>,
    allowed_keys: &[&str],
) -> bool {
    object
        .keys()
        .all(|key| allowed_keys.contains(&key.as_str()))
}

pub(super) fn responses_text_has_nonportable_fields(
    body: &Value,
    profile: SharedControlProfile,
) -> bool {
    let Some(text) = body.get("text").and_then(Value::as_object) else {
        return false;
    };
    let mut allowed_keys = vec!["format"];
    if profile.verbosity {
        allowed_keys.push("verbosity");
    }
    !object_has_only_keys(text, &allowed_keys)
}

pub(super) fn responses_reasoning_has_nonportable_fields(
    body: &Value,
    profile: SharedControlProfile,
) -> bool {
    let Some(reasoning) = body.get("reasoning").and_then(Value::as_object) else {
        return false;
    };
    let mut allowed_keys = Vec::new();
    if profile.reasoning_effort {
        allowed_keys.push("effort");
    }
    !object_has_only_keys(reasoning, &allowed_keys)
}

pub(super) fn openai_to_responses_dropped_control_names(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in [
        "stop",
        "seed",
        "presence_penalty",
        "frequency_penalty",
        "logit_bias",
        "prediction",
        "web_search_options",
    ] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    controls
}

pub(super) fn openai_to_anthropic_dropped_control_names(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in ["seed", "presence_penalty", "frequency_penalty"] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    controls
}

pub(super) fn openai_warning_only_request_controls_for_translate(
    body: &Value,
    target_format: UpstreamFormat,
    reasoning_mapped: bool,
) -> Vec<String> {
    let profile = shared_control_profile_for_target(target_format);
    let mut controls = Vec::new();
    if body.get("metadata").is_some() && !profile.metadata {
        controls.push("metadata".to_string());
    }
    if body.get("user").is_some() && !profile.user {
        controls.push("user".to_string());
    }
    if body.get("service_tier").is_some() && !profile.service_tier {
        controls.push("service_tier".to_string());
    }
    if request_stream_include_obfuscation(body).is_some() && !profile.stream_include_obfuscation {
        controls.push("stream_options.include_obfuscation".to_string());
    }
    if body.get("verbosity").is_some() && !profile.verbosity {
        controls.push("verbosity".to_string());
    }
    if body.get("reasoning_effort").is_some() && !profile.reasoning_effort && !reasoning_mapped {
        controls.push("reasoning_effort".to_string());
    }
    if body.get("prompt_cache_key").is_some() && !profile.prompt_cache_key {
        controls.push("prompt_cache_key".to_string());
    }
    if body.get("prompt_cache_retention").is_some() && !profile.prompt_cache_retention {
        controls.push("prompt_cache_retention".to_string());
    }
    if body.get("safety_identifier").is_some() && !profile.safety_identifier {
        controls.push("safety_identifier".to_string());
    }
    if body.get("logprobs").and_then(Value::as_bool) == Some(true) && !profile.top_logprobs {
        controls.push("logprobs".to_string());
    }
    if body.get("top_logprobs").is_some() && !profile.top_logprobs {
        controls.push("top_logprobs".to_string());
    }
    if body.get("logit_bias").is_some() && !profile.logit_bias {
        controls.push("logit_bias".to_string());
    }
    if body.get("prediction").is_some() {
        controls.push("prediction".to_string());
    }
    if body.get("web_search_options").is_some() {
        controls.push("web_search_options".to_string());
    }
    controls
}

pub(super) fn responses_warning_only_request_controls_for_translate(
    body: &Value,
    target_format: UpstreamFormat,
    reasoning_mapped: bool,
) -> Vec<String> {
    let profile = shared_control_profile_for_target(target_format);
    let mut controls = Vec::new();
    for field in [
        "stop",
        "seed",
        "presence_penalty",
        "frequency_penalty",
        "max_tool_calls",
        "truncation",
    ] {
        if body.get(field).is_some() {
            controls.push(field.to_string());
        }
    }
    if responses_include_has_nonportable_items(body, target_format) {
        controls.push("include".to_string());
    }
    if responses_include_items(body).contains(&"reasoning.encrypted_content") {
        controls.push("reasoning.encrypted_content".to_string());
    }
    if responses_input_reasoning_encrypted_content_present(body) {
        controls.push("input[].reasoning.encrypted_content".to_string());
    }
    if responses_input_compaction_carrier_present(body) {
        controls.push("input[].compaction".to_string());
    }

    if body.get("reasoning").is_some()
        && ((!profile.reasoning_effort && !reasoning_mapped)
            || responses_reasoning_has_nonportable_fields(body, profile))
    {
        controls.push("reasoning".to_string());
    }
    if body.get("text").is_some() && responses_text_has_nonportable_fields(body, profile) {
        controls.push("text".to_string());
    }
    if body.get("metadata").is_some() && !profile.metadata {
        controls.push("metadata".to_string());
    }
    if body.get("user").is_some() && !profile.user {
        controls.push("user".to_string());
    }
    if body.get("service_tier").is_some() && !profile.service_tier {
        controls.push("service_tier".to_string());
    }
    if body.get("prompt_cache_key").is_some() && !profile.prompt_cache_key {
        controls.push("prompt_cache_key".to_string());
    }
    if body.get("prompt_cache_retention").is_some() && !profile.prompt_cache_retention {
        controls.push("prompt_cache_retention".to_string());
    }
    if body.get("safety_identifier").is_some() && !profile.safety_identifier {
        controls.push("safety_identifier".to_string());
    }
    if responses_include_requests_output_text_logprobs(body)
        && !profile.top_logprobs
        && !controls.iter().any(|control| control == "include")
    {
        controls.push("include".to_string());
    }
    if body.get("top_logprobs").is_some() && !profile.top_logprobs {
        controls.push("top_logprobs".to_string());
    }
    if request_stream_include_obfuscation(body).is_some() && !profile.stream_include_obfuscation {
        controls.push("stream_options.include_obfuscation".to_string());
    }
    if responses_text_verbosity(body).is_some() && !profile.verbosity {
        controls.push("text.verbosity".to_string());
    }
    if responses_reasoning_effort(body).is_some() && !profile.reasoning_effort && !reasoning_mapped
    {
        controls.push("reasoning.effort".to_string());
    }
    if body.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false)
        && !profile.parallel_tool_calls
    {
        controls.push("parallel_tool_calls".to_string());
    }
    controls
}

pub(super) fn responses_tool_choice_allowed_tools_array(
    choice: &serde_json::Map<String, Value>,
) -> Option<&Vec<Value>> {
    choice.get("tools").and_then(Value::as_array).or_else(|| {
        choice
            .get("allowed_tools")
            .and_then(Value::as_object)
            .and_then(|allowed_tools| allowed_tools.get("tools"))
            .and_then(Value::as_array)
    })
}

pub(super) fn openai_named_tool_choice_name<'a>(
    value: &'a Value,
    tool_type: &str,
) -> Option<&'a str> {
    let object = value.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some(tool_type) {
        return None;
    }
    object
        .get(tool_type)
        .and_then(Value::as_object)
        .and_then(|named| named.get("name"))
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

pub(super) fn openai_tool_choice_contains_custom(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    match object.get("type").and_then(Value::as_str) {
        Some("custom") => openai_named_tool_choice_name(value, "custom").is_some(),
        Some("allowed_tools") => {
            let tools = object
                .get("allowed_tools")
                .and_then(Value::as_object)
                .and_then(|allowed_tools| allowed_tools.get("tools"))
                .or_else(|| object.get("tools"))
                .and_then(Value::as_array);
            tools
                .map(|tools| {
                    tools.iter().any(|tool| {
                        tool.get("type").and_then(Value::as_str) == Some("custom")
                            && openai_named_tool_choice_name(tool, "custom").is_some()
                    })
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

pub(super) fn responses_nonportable_tool_choice_message(
    body: &Value,
    target_format: UpstreamFormat,
) -> Option<String> {
    let target_label = translation_target_label(target_format);
    let tool_choice = body.get("tool_choice").filter(|value| !value.is_null())?;
    if tool_choice.is_string() {
        return None;
    }
    let tool_choice = tool_choice.as_object()?;
    let choice_type = tool_choice.get("type").and_then(Value::as_str)?;
    match choice_type {
        "function" => None,
        "custom" => match target_format {
            UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::Anthropic => None,
            _ => Some(format!(
                "OpenAI Responses tool_choice.type `custom` cannot be faithfully translated to {target_label}"
            )),
        },
        // A standalone `{type:"namespace", name:<ns>}` selector is bridged when
        // the namespace has all-function children (expanded to flat function
        // selectors) and otherwise warn-and-omit, mirroring namespace tool
        // definitions and allowed_tools namespace entries.
        "namespace" => None,
        "allowed_tools" => responses_tool_choice_allowed_tools_array(tool_choice).and_then(
            |tools| {
                tools.iter().find_map(|tool| match tool.get("type").and_then(Value::as_str) {
                    Some("function") => None,
                    Some("custom")
                        if matches!(
                            target_format,
                            UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::Anthropic
                        ) =>
                    {
                        None
                    }
                    Some("custom") => Some(format!(
                        "OpenAI Responses tool_choice.allowed_tools selected custom tool `{}` and cannot be faithfully translated to {target_label}",
                        tool.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                    )),
                    // Namespace selections (Codex `multi_agent_v1`, `mcp__*`) are
                    // warn-and-omit, mirroring namespace tool definitions: do not reject.
                    // The entry is passed through/dropped by the tool_choice translation.
                    Some("namespace") => None,
                    Some(other) => Some(format!(
                        "OpenAI Responses tool_choice.allowed_tools selected hosted/built-in tool `{other}` and cannot be faithfully translated to {target_label}"
                    )),
                    None => Some(format!(
                        "OpenAI Responses tool_choice.allowed_tools selected an unnamed tool that cannot be faithfully translated to {target_label}"
                    )),
                })
            },
        ),
        other => Some(format!(
            "OpenAI Responses tool_choice.type `{other}` cannot be faithfully translated to {target_label}"
        )),
    }
}

pub(super) fn responses_nonportable_tool_definition_message(body: &Value) -> Option<String> {
    let tools = body.get("tools").and_then(Value::as_array)?;
    tools
        .iter()
        .find_map(|tool| normalized_responses_tool_definition(tool).err())
}

pub(super) fn responses_has_warning_only_nonportable_tool_definitions(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools.iter().any(|tool| {
                normalized_responses_tool_definition(tool)
                    .ok()
                    .flatten()
                    .is_none()
            })
        })
        .unwrap_or(false)
}

/// Whether an `agent_message` input item carries an `encrypted_content` part.
/// The opaque blob is provider-owned (OpenAI Responses service round-trip only)
/// and cannot be decrypted by a non-OpenAI upstream. The translator keeps the
/// visible input_text envelope header and drops the blob, so the recipient model
/// sees only routing metadata with an empty payload body. This is best-effort:
/// the request stays portable (not rejected) but the warning surfaces the loss.
pub(super) fn responses_has_warning_only_encrypted_agent_message(body: &Value) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                responses_input_item_type(item) == Some("agent_message")
                    && item
                        .get("content")
                        .and_then(Value::as_array)
                        .is_some_and(|parts| {
                            parts.iter().any(|part| {
                                part.get("type").and_then(Value::as_str)
                                    == Some("encrypted_content")
                            })
                        })
            })
        })
        .unwrap_or(false)
}

pub(super) fn responses_custom_tool_format_reject_message(
    body: &Value,
    target_format: UpstreamFormat,
) -> Option<String> {
    if !matches!(
        target_format,
        UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::Anthropic
    ) {
        return None;
    }

    let tools = body.get("tools").and_then(Value::as_array)?;
    tools
        .iter()
        .find_map(|tool| match normalized_responses_tool_definition(tool) {
            Ok(Some(NormalizedOpenAiFamilyToolDef::Custom(custom)))
                if !openai_custom_tool_format_supports_anthropic_bridge(custom.format.as_ref()) =>
            {
                Some(custom_tool_format_reject_message(
                    "OpenAI Responses",
                    &custom.name,
                    translation_target_label(target_format),
                ))
            }
            _ => None,
        })
}

fn responses_custom_tool_bridge_warning_messages(
    body: &Value,
    target_format: UpstreamFormat,
) -> Vec<String> {
    if !matches!(
        target_format,
        UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::Anthropic
    ) {
        return Vec::new();
    }

    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| match normalized_responses_tool_definition(tool) {
                    Ok(Some(NormalizedOpenAiFamilyToolDef::Custom(custom)))
                        if openai_custom_tool_format_supports_anthropic_bridge(
                            custom.format.as_ref(),
                        ) && !openai_custom_tool_format_is_plain_text(
                            custom.format.as_ref(),
                        ) =>
                    {
                        Some(custom_tool_format_portability_warning_message(
                            "OpenAI Responses",
                            &custom.name,
                            translation_target_label(target_format),
                        ))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn responses_hosted_input_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "file_search_call"
            | "web_search_call"
            | "code_interpreter_call"
            | "mcp_call"
            | "image_generation_call"
            | "computer_call"
            | "computer_call_output"
    )
}

/// Whether a namespace name is bridged: defined in the request's `tools` as a
/// `type: "namespace"` group whose children are ALL `{type: "function"}`.
fn responses_namespace_is_bridged(body: &Value, namespace: &str) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools.iter().any(|tool| {
                normalized_responses_tool_definition(tool)
                    .ok()
                    .flatten()
                    .is_some_and(|t| match t {
                        NormalizedOpenAiFamilyToolDef::Namespace { name, .. } => name == namespace,
                        _ => false,
                    })
            })
        })
        .unwrap_or(false)
}

pub(super) fn responses_portable_input_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "message"
            | "function_call"
            | "custom_tool_call"
            | "function_call_output"
            | "custom_tool_call_output"
            | "reasoning"
            | "agent_message"
    )
}

fn responses_input_compaction_carrier_present(body: &Value) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| items.iter().any(responses_input_item_is_compaction))
        .unwrap_or(false)
}

fn responses_request_has_visible_portable_context(body: &Value) -> bool {
    match body.get("input") {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(items)) => items
            .iter()
            .any(responses_input_item_has_visible_portable_context),
        _ => false,
    }
}

fn responses_request_has_non_compaction_visible_portable_context(body: &Value) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                !responses_input_item_is_compaction(item)
                    && responses_input_item_has_visible_portable_context(item)
            })
        })
        .unwrap_or(false)
}

fn responses_compaction_item_can_drop_opaque_state(
    item: &Value,
    request_has_non_compaction_visible_context: bool,
) -> bool {
    !responses_compaction_summary_text(item).trim().is_empty()
        || request_has_non_compaction_visible_context
}

fn responses_reasoning_item_can_drop_opaque_state(
    item: &Value,
    request_has_visible_context: bool,
) -> bool {
    !responses_reasoning_summary_text(item).trim().is_empty() || request_has_visible_context
}

fn responses_input_item_has_visible_portable_context(item: &Value) -> bool {
    match responses_input_item_type(item) {
        Some("message") => responses_content_has_visible_portable_context(item.get("content")),
        Some("reasoning") => !responses_reasoning_summary_text(item).trim().is_empty(),
        Some("compaction" | "compaction_summary") => {
            !responses_compaction_summary_text(item).trim().is_empty()
        }
        // A cross-provider `agent_message` carries a visible text envelope
        // (routing metadata + payload). Counting it as visible portable context
        // prevents a multi-turn child's own accumulated reasoning.encrypted_content
        // from triggering a native-continuity rejection when a new agent_message
        // arrives in the same request. Fork-inherited reasoning is stripped
        // before reaching the child, so this arm only covers that co-occurrence.
        Some("agent_message") => responses_agent_message_text(item).is_some(),
        _ => false,
    }
}

fn responses_content_has_visible_portable_context(content: Option<&Value>) -> bool {
    match content {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(parts)) => parts
            .iter()
            .any(responses_content_part_has_visible_portable_context),
        Some(Value::Object(_)) => {
            content.is_some_and(responses_content_part_has_visible_portable_context)
        }
        _ => false,
    }
}

fn responses_content_part_has_visible_portable_context(part: &Value) -> bool {
    match part.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text" | "refusal") => part
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty()),
        Some("input_image" | "image_url") => part.get("image_url").is_some(),
        Some("input_audio") => part.get("input_audio").is_some(),
        Some("input_file" | "file") => [
            "file_id",
            "file_data",
            "file_url",
            "filename",
            "mime_type",
            "mimeType",
        ]
        .iter()
        .any(|field| part.get(*field).is_some()),
        _ => false,
    }
}

pub(super) fn responses_nonportable_input_item_message(
    body: &Value,
    target_format: UpstreamFormat,
) -> Option<String> {
    let target_label = translation_target_label(target_format);
    let items = body.get("input").and_then(Value::as_array)?;
    let has_non_compaction_visible_portable_context =
        responses_request_has_non_compaction_visible_portable_context(body);
    items.iter().find_map(|item| {
        let item_type = responses_input_item_type(item)?;
        if matches!(item_type, "function_call" | "custom_tool_call")
            && item.get("namespace").is_some()
        {
            let ns = item.get("namespace").and_then(Value::as_str).unwrap_or("");
            // Allow when the namespace was bridged (defined in `tools` with
            // all-function children); the translator flattens the call to
            // `<ns>__<name>`. Otherwise reject as before.
            if !responses_namespace_is_bridged(body, ns) {
                return Some(format!(
                    "OpenAI Responses namespaced tool call `{}` cannot be faithfully translated to {target_label}",
                    item.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                ));
            }
            return None;
        }
        if responses_input_item_is_compaction(item) {
            if responses_compaction_item_can_drop_opaque_state(
                item,
                has_non_compaction_visible_portable_context,
            )
            {
                return None;
            }
            return Some(format!(
                "OpenAI Responses compaction input item contains provider-owned opaque state and cannot be safely dropped when translating to {target_label} because no visible portable transcript or summary remains"
            ));
        }
        if responses_portable_input_item_type(item_type) {
            return None;
        }
        if responses_hosted_input_item_type(item_type) {
            return Some(format!(
                "OpenAI Responses input item `{item_type}` cannot be faithfully translated to {target_label}"
            ));
        }
        Some(format!(
            "OpenAI Responses input item type `{item_type}` is outside the portable cross-protocol subset and cannot be faithfully translated to {target_label}"
        ))
    })
}

fn responses_input_reasoning_encrypted_content_present(body: &Value) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                responses_input_item_type(item) == Some("reasoning")
                    && item.get("encrypted_content").is_some()
            })
        })
        .unwrap_or(false)
}

fn responses_input_reasoning_encrypted_content_requires_native_continuity(body: &Value) -> bool {
    let has_visible_portable_context = responses_request_has_visible_portable_context(body);
    body.get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                responses_input_item_type(item) == Some("reasoning")
                    && item.get("encrypted_content").is_some()
                    && !responses_reasoning_item_can_drop_opaque_state(
                        item,
                        has_visible_portable_context,
                    )
            })
        })
        .unwrap_or(false)
}

pub(super) fn responses_reasoning_continuity_request_message(
    body: &Value,
    target_format: UpstreamFormat,
) -> Option<String> {
    if responses_input_reasoning_encrypted_content_requires_native_continuity(body) {
        let target_label = translation_target_label(target_format);
        return Some(responses_reasoning_continuity_not_portable_message(
            "input[].reasoning.encrypted_content",
            target_label,
        ));
    }
    None
}

pub(super) fn cross_protocol_requested_choice_count(
    client_format: UpstreamFormat,
    body: &Value,
) -> Option<(&'static str, u64)> {
    match client_format {
        UpstreamFormat::OpenAiChatCompletions => {
            body.get("n").and_then(Value::as_u64).map(|n| ("n", n))
        }
        _ => None,
    }
}

pub(super) fn cross_protocol_requested_choice_count_message(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> Option<String> {
    let (field_name, count) = cross_protocol_requested_choice_count(client_format, body)?;
    if count <= 1 {
        return None;
    }
    Some(single_candidate_choice_contract_message(
        translation_target_label(client_format),
        translation_target_label(upstream_format),
        field_name,
        count as usize,
    ))
}

pub(super) fn request_has_custom_tools(client_format: UpstreamFormat, body: &Value) -> bool {
    match client_format {
        UpstreamFormat::OpenAiChatCompletions => {
            body.get("tools")
                .and_then(Value::as_array)
                .map(|tools| {
                    tools.iter().any(|tool| {
                        semantic_tool_kind_from_value(tool) == SemanticToolKind::OpenAiCustom
                    })
                })
                .unwrap_or(false)
                || body
                    .get("messages")
                    .and_then(Value::as_array)
                    .map(|messages| {
                        messages.iter().any(|message| {
                            message
                                .get("tool_calls")
                                .and_then(Value::as_array)
                                .map(|tool_calls| {
                                    tool_calls.iter().any(|tool_call| {
                                        semantic_tool_kind_from_value(tool_call)
                                            == SemanticToolKind::OpenAiCustom
                                    })
                                })
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
                || body
                    .get("tool_choice")
                    .map(openai_tool_choice_contains_custom)
                    .unwrap_or(false)
        }
        UpstreamFormat::OpenAiResponses => {
            body.get("tools")
                .and_then(Value::as_array)
                .map(|tools| {
                    tools.iter().any(|tool| {
                        semantic_tool_kind_from_value(tool) == SemanticToolKind::OpenAiCustom
                    })
                })
                .unwrap_or(false)
                || body
                    .get("input")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items.iter().any(|item| {
                            responses_tool_call_item_to_openai_tool_call(item)
                                .map(|tool_call| {
                                    semantic_tool_kind_from_value(&tool_call)
                                        == SemanticToolKind::OpenAiCustom
                                })
                                .unwrap_or_else(|| {
                                    semantic_tool_kind_from_value(item)
                                        == SemanticToolKind::OpenAiCustom
                                })
                        })
                    })
                    .unwrap_or(false)
                || body
                    .get("tool_choice")
                    .map(openai_tool_choice_contains_custom)
                    .unwrap_or(false)
        }
        _ => false,
    }
}

pub(super) fn request_invalid_structured_tool_arguments_message(
    client_format: UpstreamFormat,
    body: &Value,
    target_label: &str,
) -> Option<String> {
    match client_format {
        UpstreamFormat::OpenAiChatCompletions => body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages.iter().find_map(|message| {
                    message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .and_then(|tool_calls| {
                            tool_calls.iter().find_map(|tool_call| {
                                (semantic_tool_kind_from_value(tool_call)
                                    != SemanticToolKind::OpenAiCustom
                                    && !tool_call_is_marked_non_replayable(tool_call))
                                .then(|| {
                                    openai_tool_arguments_to_structured_value(
                                        tool_call,
                                        target_label,
                                    )
                                    .err()
                                })
                                .flatten()
                            })
                        })
                })
            }),
        UpstreamFormat::OpenAiResponses => {
            body.get("input")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items.iter().find_map(|item| {
                        matches!(
                            item.get("type").and_then(Value::as_str),
                            Some("function_call") | Some("custom_tool_call")
                        )
                        .then(|| {
                            (semantic_tool_kind_from_value(item) != SemanticToolKind::OpenAiCustom
                                && !tool_call_is_marked_non_replayable(item))
                            .then(|| {
                                responses_tool_call_to_structured_value(item, target_label).err()
                            })
                            .flatten()
                        })
                        .flatten()
                    })
                })
        }
        _ => None,
    }
}

fn assess_surface_request_policy(
    assessment: &mut TranslationAssessment,
    client_format: UpstreamFormat,
    body: &Value,
    surface: &ModelSurface,
) {
    if surface
        .tools
        .as_ref()
        .and_then(|tools| tools.supports_parallel_calls)
        == Some(false)
        && request_has_surface_tooling(client_format, body)
        && request_explicitly_enables_parallel_tool_calls(client_format, body)
    {
        assessment.reject(
            "request explicitly enables parallel tool execution (`parallel_tool_calls=true` or `tool_choice.disable_parallel_tool_use=false`), but model surface `tools.supports_parallel_calls=false`"
                .to_string(),
        );
    }

    for modality in request_input_modalities(client_format, body) {
        if !surface_allows_modality(
            surface
                .modalities
                .as_ref()
                .and_then(|modalities| modalities.input.as_ref()),
            modality,
        ) {
            assessment.reject(format!(
                "request uses {} input, but model surface `modalities.input` does not include `{}`",
                modality_label(modality),
                modality_label(modality),
            ));
        }
    }

    for modality in request_output_modalities(client_format, body) {
        if !surface_allows_modality(
            surface
                .modalities
                .as_ref()
                .and_then(|modalities| modalities.output.as_ref()),
            modality,
        ) {
            assessment.reject(format!(
                "request asks for {} output, but model surface `modalities.output` does not include `{}`",
                modality_label(modality),
                modality_label(modality),
            ));
        }
    }
}

fn surface_allows_modality(allowed: Option<&Vec<ModelModality>>, modality: ModelModality) -> bool {
    allowed
        .map(|allowed| {
            allowed.contains(&modality)
                || (modality == ModelModality::Pdf && allowed.contains(&ModelModality::File))
        })
        .unwrap_or(true)
}

fn modality_label(modality: ModelModality) -> &'static str {
    match modality {
        ModelModality::Text => "text",
        ModelModality::Image => "image",
        ModelModality::Audio => "audio",
        ModelModality::Pdf => "pdf",
        ModelModality::File => "file",
        ModelModality::Video => "video",
    }
}

fn request_has_surface_tooling(client_format: UpstreamFormat, body: &Value) -> bool {
    match client_format {
        UpstreamFormat::OpenAiChatCompletions
        | UpstreamFormat::OpenAiResponses
        | UpstreamFormat::Anthropic => body
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty()),
    }
}

fn request_explicitly_enables_parallel_tool_calls(
    client_format: UpstreamFormat,
    body: &Value,
) -> bool {
    match client_format {
        UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::OpenAiResponses => {
            body.get("parallel_tool_calls").and_then(Value::as_bool) == Some(true)
        }
        UpstreamFormat::Anthropic => {
            body.get("tool_choice")
                .and_then(Value::as_object)
                .and_then(|tool_choice| tool_choice.get("disable_parallel_tool_use"))
                .and_then(Value::as_bool)
                == Some(false)
        }
    }
}

fn openai_request_file_mime_conflict_message(
    client_format: UpstreamFormat,
    body: &Value,
) -> Option<String> {
    match client_format {
        UpstreamFormat::OpenAiChatCompletions => openai_completion_file_mime_conflict_message(body),
        UpstreamFormat::OpenAiResponses => openai_responses_file_mime_conflict_message(body),
        UpstreamFormat::Anthropic => None,
    }
}

fn openai_completion_file_mime_conflict_message(body: &Value) -> Option<String> {
    body.get("messages")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|message| openai_content_file_mime_conflict_message(message.get("content")))
}

fn openai_responses_file_mime_conflict_message(body: &Value) -> Option<String> {
    body.get("input")
        .and_then(Value::as_array)?
        .iter()
        .find_map(openai_responses_input_item_file_mime_conflict_message)
}

fn openai_responses_input_item_file_mime_conflict_message(item: &Value) -> Option<String> {
    openai_input_part_file_mime_conflict_message(item).or_else(|| {
        responses_input_item_is_message(item)
            .then(|| openai_content_file_mime_conflict_message(item.get("content")))
            .flatten()
    })
}

fn openai_content_file_mime_conflict_message(content: Option<&Value>) -> Option<String> {
    match content {
        Some(Value::Array(parts)) => parts
            .iter()
            .find_map(openai_input_part_file_mime_conflict_message),
        Some(Value::Object(_)) => content.and_then(openai_input_part_file_mime_conflict_message),
        _ => None,
    }
}

fn openai_input_part_file_mime_conflict_message(part: &Value) -> Option<String> {
    matches!(
        part.get("type").and_then(Value::as_str),
        Some("file") | Some("input_file")
    )
    .then(|| openai_file_part_mime_conflict_message(part))
    .flatten()
}

fn request_input_modalities(
    client_format: UpstreamFormat,
    body: &Value,
) -> BTreeSet<ModelModality> {
    let mut modalities = BTreeSet::new();
    match client_format {
        UpstreamFormat::OpenAiChatCompletions => {
            openai_collect_completion_input_modalities(body, &mut modalities);
        }
        UpstreamFormat::OpenAiResponses => {
            openai_collect_responses_input_modalities(body, &mut modalities);
        }
        UpstreamFormat::Anthropic => {
            anthropic_collect_request_input_modalities(body, &mut modalities);
        }
    }
    modalities
}

fn insert_input_modality(modalities: &mut BTreeSet<ModelModality>, modality: ModelModality) {
    if modality != ModelModality::Text {
        modalities.insert(modality);
    }
}

fn anthropic_collect_request_input_modalities(
    body: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    anthropic_collect_content_input_modalities(body.get("system"), modalities);
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            anthropic_collect_content_input_modalities(message.get("content"), modalities);
        }
    }
}

fn anthropic_collect_content_input_modalities(
    content: Option<&Value>,
    modalities: &mut BTreeSet<ModelModality>,
) {
    match content {
        Some(Value::Array(blocks)) => {
            for block in blocks {
                anthropic_collect_block_input_modalities(block, modalities);
            }
        }
        Some(Value::Object(_)) => {
            if let Some(block) = content {
                anthropic_collect_block_input_modalities(block, modalities);
            }
        }
        _ => {}
    }
}

fn anthropic_collect_block_input_modalities(
    block: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    match block.get("type").and_then(Value::as_str) {
        Some("image") => insert_input_modality(modalities, ModelModality::Image),
        Some("audio") => insert_input_modality(modalities, ModelModality::Audio),
        Some("document") => {
            let modality = block
                .get("source")
                .and_then(|source| source.get("media_type"))
                .and_then(Value::as_str)
                .and_then(mime_type_to_input_modality)
                .filter(|modality| *modality == ModelModality::Pdf)
                .unwrap_or(ModelModality::File);
            insert_input_modality(modalities, modality);
        }
        _ => {
            if let Some(modality) = block
                .get("source")
                .and_then(|source| source.get("media_type"))
                .and_then(Value::as_str)
                .and_then(mime_type_to_input_modality)
            {
                insert_input_modality(modalities, modality);
            }
        }
    }
}

fn openai_collect_completion_input_modalities(
    body: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    if openai_assistant_history_audio_present(body) {
        insert_input_modality(modalities, ModelModality::Audio);
    }

    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            openai_collect_completion_message_input_modalities(message, modalities);
        }
    }
}

fn openai_collect_completion_message_input_modalities(
    message: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    if message
        .get("audio")
        .filter(|audio| !audio.is_null())
        .is_some()
    {
        insert_input_modality(modalities, ModelModality::Audio);
    }

    if let Some(parts) = message.get("content").and_then(Value::as_array) {
        for part in parts {
            openai_collect_input_part_modalities(part, modalities);
        }
    }
}

fn openai_collect_responses_input_modalities(
    body: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    if let Some(items) = body.get("input").and_then(Value::as_array) {
        for item in items {
            openai_collect_responses_input_item_modalities(item, modalities);
        }
    }
}

fn openai_collect_responses_input_item_modalities(
    item: &Value,
    modalities: &mut BTreeSet<ModelModality>,
) {
    openai_collect_input_part_modalities(item, modalities);

    match responses_input_item_type(item) {
        Some("message") => {
            if let Some(parts) = item.get("content").and_then(Value::as_array) {
                for part in parts {
                    openai_collect_input_part_modalities(part, modalities);
                }
            }
        }
        Some("output_audio") => insert_input_modality(modalities, ModelModality::Audio),
        _ => {}
    }
}

fn openai_collect_input_part_modalities(part: &Value, modalities: &mut BTreeSet<ModelModality>) {
    match part.get("type").and_then(Value::as_str) {
        Some("image_url") | Some("input_image") => {
            insert_input_modality(modalities, ModelModality::Image);
        }
        Some("input_audio") => {
            insert_input_modality(modalities, ModelModality::Audio);
        }
        Some("file") | Some("input_file") => {
            insert_input_modality(modalities, openai_file_part_modality(part));
        }
        _ => {}
    }
}

fn openai_file_part_modality(part: &Value) -> ModelModality {
    openai_file_part_resolved_mime_type(part)
        .ok()
        .flatten()
        .and_then(|mime_type| mime_type_to_input_modality(&mime_type))
        .unwrap_or(ModelModality::File)
}

fn mime_type_to_input_modality(mime_type: &str) -> Option<ModelModality> {
    let mime_type = mime_type
        .split_once(';')
        .map_or(mime_type, |(mime_type, _)| mime_type)
        .trim()
        .to_ascii_lowercase();
    if mime_type.is_empty() {
        return None;
    }
    if mime_type.starts_with("text/") {
        Some(ModelModality::Text)
    } else if mime_type.starts_with("image/") {
        Some(ModelModality::Image)
    } else if mime_type.starts_with("audio/") {
        Some(ModelModality::Audio)
    } else if mime_type.starts_with("video/") {
        Some(ModelModality::Video)
    } else if mime_type == "application/pdf" {
        Some(ModelModality::Pdf)
    } else {
        Some(ModelModality::File)
    }
}

fn request_output_modalities(
    client_format: UpstreamFormat,
    body: &Value,
) -> BTreeSet<ModelModality> {
    let mut modalities = BTreeSet::new();
    match client_format {
        UpstreamFormat::OpenAiChatCompletions | UpstreamFormat::OpenAiResponses => {
            openai_collect_output_modalities(body, &mut modalities);
        }
        UpstreamFormat::Anthropic => {}
    }
    modalities
}

fn openai_collect_output_modalities(body: &Value, modalities: &mut BTreeSet<ModelModality>) {
    if body.get("audio").is_some()
        || body
            .get("modalities")
            .and_then(Value::as_array)
            .is_some_and(|modalities| {
                modalities.iter().any(|requested_modality| {
                    requested_modality
                        .as_str()
                        .is_some_and(|requested_modality| {
                            requested_modality.eq_ignore_ascii_case("audio")
                        })
                })
            })
    {
        modalities.insert(ModelModality::Audio);
    }
}

pub(crate) fn assess_request_translation_with_surface(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
    surface: &ModelSurface,
    resolved_upstream_model: &str,
    dialect: Option<&DialectBlock>,
) -> TranslationAssessment {
    let mut assessment = TranslationAssessment::default();
    assess_surface_request_policy(&mut assessment, client_format, body, surface);
    assessment.issues.extend(
        assess_request_translation_with_dialect(client_format, upstream_format, body, dialect)
            .issues,
    );
    assess_anthropic_nondefault_sampling_withhold(
        &mut assessment,
        client_format,
        upstream_format,
        body,
        resolved_upstream_model,
    );
    assessment
}

/// Records a portability warning for each non-default OpenAI sampling control
/// (`temperature`/`top_p`, default `1.0`) that the resolved Anthropic upstream
/// model rejects. This is the single decision point that the cross-protocol
/// OpenAI -> Anthropic body-building path (`openai_to_claude`) honors, so the
/// emitted warning can never diverge from what is actually withheld.
fn assess_anthropic_nondefault_sampling_withhold(
    assessment: &mut TranslationAssessment,
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
    resolved_upstream_model: &str,
) {
    if !openai_family_format(client_format) || upstream_format != UpstreamFormat::Anthropic {
        return;
    }
    let client_label = translation_target_label(client_format);
    for field in anthropic_nondefault_sampling_withhold_fields(resolved_upstream_model, body) {
        assessment.warning(format!(
            "{client_label} sampling control `{field}` is not accepted by the resolved Anthropic upstream model and will be withheld to avoid a 400 response"
        ));
    }
}

/// Returns the subset of `["temperature", "top_p"]` that is present, non-default
/// (not equal to the OpenAI default of `1.0`), and rejected by the resolved
/// Anthropic upstream model. Shared by the assessment warning above and the
/// `openai_to_claude` forwarding decision so both stay in lockstep.
pub(super) fn anthropic_nondefault_sampling_withhold_fields(
    resolved_upstream_model: &str,
    body: &Value,
) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if anthropic_upstream_rejects_nondefault_sampling(resolved_upstream_model) {
        for field in ["temperature", "top_p"] {
            if openai_sampling_field_is_nondefault(body.get(field)) {
                fields.push(field);
            }
        }
    }
    fields
}

/// Whether the given OpenAI sampling control value is present and non-default.
/// The OpenAI default for both `temperature` and `top_p` is `1.0`; absent or
/// non-numeric values are treated as default and never withheld.
fn openai_sampling_field_is_nondefault(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 1.0),
        _ => false,
    }
}

/// Prefix/range predicate (not an enumeration) over the resolved Anthropic
/// upstream model id. Matches the refreshed Anthropic baseline of models that
/// reject non-default sampling params with HTTP 400: Opus 4.7+
/// (`claude-opus-4-{n}` with `n >= 7`), Sonnet 5 (`claude-sonnet-5*`),
/// Fable 5 (`claude-fable-5*`), and Opus 5 (`claude-opus-5*`).
pub(super) fn anthropic_upstream_rejects_nondefault_sampling(
    resolved_upstream_model: &str,
) -> bool {
    let model = resolved_upstream_model.trim();
    if let Some(rest) = model.strip_prefix("claude-opus-4-") {
        let leading_digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        return leading_digits.parse::<u32>().is_ok_and(|minor| minor >= 7);
    }
    model.starts_with("claude-sonnet-5")
        || model.starts_with("claude-fable-5")
        || model.starts_with("claude-opus-5")
}

pub(super) fn anthropic_warning_only_request_controls_for_translate(
    body: &Value,
) -> Vec<&'static str> {
    let mut controls = Vec::new();
    for field in ["top_k", "service_tier"] {
        if body.get(field).is_some() {
            controls.push(field);
        }
    }
    if anthropic_protocol_cache_control_present(body) {
        controls.push("cache_control");
    }
    controls
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnthropicRequestControlDisposition {
    WarnDrop,
    FailClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnthropicRequestControlAssessment {
    field: &'static str,
    disposition: AnthropicRequestControlDisposition,
    reason: String,
}

fn anthropic_top_level_request_control_assessments(
    body: &Value,
    target_label: &str,
) -> Vec<AnthropicRequestControlAssessment> {
    let has_complete_visible_history = anthropic_request_has_complete_visible_history(body);
    let mut assessments = Vec::new();

    if body.get("thinking").is_some() {
        assessments.push(if has_complete_visible_history {
            AnthropicRequestControlAssessment {
                field: "thinking",
                disposition: AnthropicRequestControlDisposition::WarnDrop,
                reason:
                    "top-level Anthropic thinking is a provider-specific capability hint"
                        .to_string(),
            }
        } else {
            AnthropicRequestControlAssessment {
                field: "thinking",
                disposition: AnthropicRequestControlDisposition::FailClosed,
                reason: format!(
                    "top-level Anthropic thinking requires complete visible history before translation to {target_label}"
                ),
            }
        });
    }

    if let Some(context_management) = body.get("context_management").filter(|context_management| {
        !matches!(context_management, Value::Null | Value::Bool(false))
    }) {
        assessments.push(if let Some(reason) =
            anthropic_context_management_fail_closed_reason(context_management, target_label)
        {
            AnthropicRequestControlAssessment {
                field: "context_management",
                disposition: AnthropicRequestControlDisposition::FailClosed,
                reason,
            }
        } else if has_complete_visible_history {
            AnthropicRequestControlAssessment {
                field: "context_management",
                disposition: AnthropicRequestControlDisposition::WarnDrop,
                reason:
                    "Anthropic context_management is a request-local context editing hint"
                        .to_string(),
            }
        } else {
            AnthropicRequestControlAssessment {
                field: "context_management",
                disposition: AnthropicRequestControlDisposition::FailClosed,
                reason: format!(
                    "Anthropic context_management requires complete visible history before translation to {target_label}"
                ),
            }
        });
    }

    if let Some(container) = body.get("container") {
        assessments.push(if anthropic_container_requires_provider_runtime(container) {
            AnthropicRequestControlAssessment {
                field: "container",
                disposition: AnthropicRequestControlDisposition::FailClosed,
                reason: format!(
                    "Anthropic container depends on provider-owned runtime/state/resource semantics and cannot be translated to {target_label}"
                ),
            }
        } else if has_complete_visible_history {
            AnthropicRequestControlAssessment {
                field: "container",
                disposition: AnthropicRequestControlDisposition::WarnDrop,
                reason: "Anthropic container is a request-local hint without portable OpenAI-family semantics"
                    .to_string(),
            }
        } else {
            AnthropicRequestControlAssessment {
                field: "container",
                disposition: AnthropicRequestControlDisposition::FailClosed,
                reason: format!(
                    "Anthropic container hints require complete visible history before translation to {target_label}"
                ),
            }
        });
    }

    assessments
}

fn anthropic_request_has_complete_visible_history(body: &Value) -> bool {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return false;
    };
    if messages.is_empty() {
        return false;
    }

    if anthropic_content_has_provider_owned_state_hint(body.get("system")) {
        return false;
    }

    let mut has_visible_user_context =
        anthropic_content_has_visible_user_context(body.get("system"));
    let mut pending_tool_use_ids = BTreeSet::new();

    for message in messages {
        let role = message.get("role").and_then(Value::as_str);
        let content = message.get("content");

        if anthropic_content_has_provider_owned_state_hint(content) {
            return false;
        }

        if role == Some("user") && anthropic_content_has_visible_user_context(content) {
            has_visible_user_context = true;
        }

        let mut tool_history_is_structural = true;
        anthropic_visit_content_blocks(content, |block| {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use" | "server_tool_use") if role == Some("assistant") => {
                    let Some(tool_use_id) = block
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|tool_use_id| !tool_use_id.is_empty())
                    else {
                        tool_history_is_structural = false;
                        return;
                    };
                    if !pending_tool_use_ids.insert(tool_use_id.to_string()) {
                        tool_history_is_structural = false;
                    }
                }
                Some("tool_result") => {
                    let Some(tool_use_id) = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|tool_use_id| !tool_use_id.is_empty())
                    else {
                        tool_history_is_structural = false;
                        return;
                    };
                    if !pending_tool_use_ids.remove(tool_use_id) {
                        tool_history_is_structural = false;
                    }
                }
                _ => {}
            }
        });
        if !tool_history_is_structural {
            return false;
        }
    }

    has_visible_user_context && pending_tool_use_ids.is_empty()
}

fn anthropic_content_has_visible_user_context(content: Option<&Value>) -> bool {
    match content {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(blocks)) => blocks.iter().any(anthropic_block_has_visible_user_context),
        Some(Value::Object(_)) => content.is_some_and(anthropic_block_has_visible_user_context),
        _ => false,
    }
}

fn anthropic_block_has_visible_user_context(block: &Value) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty()),
        Some("image") => block.get("source").is_some(),
        _ => false,
    }
}

fn anthropic_content_has_provider_owned_state_hint(content: Option<&Value>) -> bool {
    let mut has_provider_state_hint = false;
    anthropic_visit_content_blocks(content, |block| {
        if anthropic_block_has_provider_owned_state_hint(block) {
            has_provider_state_hint = true;
        }
    });
    has_provider_state_hint
}

fn anthropic_block_has_provider_owned_state_hint(block: &Value) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("redacted_thinking" | "encrypted_thinking") => true,
        Some("thinking") => {
            if !anthropic_block_has_visible_thinking_text(block) {
                return true;
            }
            block.get("data").is_some()
                || block.get("redacted_content").is_some()
                || block.get("encrypted_content").is_some()
        }
        _ => false,
    }
}

fn anthropic_block_has_visible_thinking_text(block: &Value) -> bool {
    block
        .get("thinking")
        .and_then(Value::as_str)
        .is_some_and(|thinking| !thinking.trim().is_empty())
}

fn anthropic_visit_content_blocks(content: Option<&Value>, mut visit: impl FnMut(&Value)) {
    match content {
        Some(Value::Array(blocks)) => {
            for block in blocks {
                visit(block);
            }
        }
        Some(Value::Object(_)) => {
            if let Some(block) = content {
                visit(block);
            }
        }
        _ => {}
    }
}

fn anthropic_container_requires_provider_runtime(container: &Value) -> bool {
    match container {
        Value::Object(object) => object.iter().any(|(key, value)| {
            if value.is_null() || value.as_bool() == Some(false) {
                return false;
            }
            anthropic_container_key_requires_provider_runtime(key)
                || anthropic_container_requires_provider_runtime(value)
        }),
        Value::Array(items) => items
            .iter()
            .any(anthropic_container_requires_provider_runtime),
        Value::String(value) => anthropic_container_string_requires_provider_runtime(value),
        _ => false,
    }
}

fn anthropic_container_key_requires_provider_runtime(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace('-', "_");
    normalized == "id"
        || normalized == "handle"
        || normalized.contains("container")
        || normalized.contains("skill")
        || normalized.contains("code_execution")
        || normalized.contains("mcp")
        || normalized.contains("server_tool")
        || normalized.contains("resource")
        || normalized.contains("runtime")
        || normalized.contains("opaque")
        || normalized.contains("encrypted")
        || normalized.contains("state")
}

fn anthropic_container_string_requires_provider_runtime(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase().replace('-', "_");
    normalized.contains("container")
        || normalized.contains("skill")
        || normalized.contains("code_execution")
        || normalized.contains("mcp")
        || normalized.contains("server_tool")
        || normalized.contains("resource")
        || normalized.contains("runtime")
        || normalized.contains("opaque")
        || normalized.contains("encrypted")
        || normalized.contains("state")
}

fn anthropic_context_management_fail_closed_reason(
    context_management: &Value,
    target_label: &str,
) -> Option<String> {
    anthropic_context_management_schema_error(context_management).map(|reason| {
        format!("Anthropic context_management {reason} and cannot be translated to {target_label}")
    })
}

fn anthropic_context_management_schema_error(context_management: &Value) -> Option<String> {
    match context_management {
        Value::Null => None,
        Value::Bool(false) => None,
        Value::Object(object) => {
            if let Some(provider_field) = object
                .keys()
                .find(|key| anthropic_context_management_provider_owned_field(key))
            {
                return Some(format!(
                    "contains provider-owned state field `{provider_field}`"
                ));
            }

            if let Some(context_type) = object.get("type") {
                let Some(context_type) = context_type.as_str() else {
                    return Some(
                        "field `type` must be the legacy string `auto` or omitted when using `edits`"
                            .to_string(),
                    );
                };
                if context_type == "auto" {
                    if object.len() == 1 {
                        return None;
                    }
                    return Some(
                        "legacy no-op `type:auto` does not support additional fields".to_string(),
                    );
                }
                return Some(format!("uses unsupported legacy type `{context_type}`"));
            }

            let Some(edits) = object.get("edits") else {
                if let Some(unknown_field) = object.keys().next() {
                    return Some(format!("contains unsupported field `{unknown_field}`"));
                }
                return Some("must use known legacy `type:auto` or an `edits` array".to_string());
            };
            if object.len() != 1 {
                let unknown_field = object
                    .keys()
                    .find(|key| key.as_str() != "edits")
                    .map(String::as_str)
                    .unwrap_or("unknown");
                return Some(format!(
                    "contains unsupported field `{unknown_field}` alongside `edits`"
                ));
            }
            let Some(edits) = edits.as_array() else {
                return Some("field `edits` must be an array".to_string());
            };
            validate_anthropic_context_management_edits(edits)
        }
        _ => Some("must be an object, null, or false".to_string()),
    }
}

fn anthropic_context_management_provider_owned_field(key: &str) -> bool {
    matches!(
        key,
        "applied_edits"
            | "original_input_tokens"
            | "cleared_input_tokens"
            | "cleared_tool_uses"
            | "cleared_thinking_turns"
            | "id"
            | "handle"
            | "state"
            | "opaque_state"
            | "encrypted_content"
    )
}

fn validate_anthropic_context_management_edits(edits: &[Value]) -> Option<String> {
    for (index, edit) in edits.iter().enumerate() {
        let Some(object) = edit.as_object() else {
            return Some(format!("edit at index {index} must be an object"));
        };
        let Some(edit_type) = object.get("type").and_then(Value::as_str) else {
            return Some(format!(
                "edit at index {index} requires a string `type` field"
            ));
        };
        match edit_type {
            "clear_thinking_20251015" => {
                if index != 0 {
                    return Some(
                        "`clear_thinking_20251015` must be the first context edit".to_string(),
                    );
                }
                if let Some(message) = validate_anthropic_clear_thinking_edit(object) {
                    return Some(message);
                }
            }
            "clear_tool_uses_20250919" => {
                if let Some(message) = validate_anthropic_clear_tool_uses_edit(object) {
                    return Some(message);
                }
            }
            _ => {
                return Some(format!("contains unsupported edit type `{edit_type}`"));
            }
        }
    }
    None
}

fn validate_anthropic_clear_thinking_edit(
    object: &serde_json::Map<String, Value>,
) -> Option<String> {
    if let Some(field) = first_unknown_field(object, &["type", "keep"]) {
        return Some(format!(
            "clear_thinking_20251015 contains unsupported field `{field}`"
        ));
    }
    let keep = object.get("keep")?;
    match keep {
        Value::String(value) if value == "all" => None,
        Value::Object(keep_object) => {
            if let Some(field) = first_unknown_field(keep_object, &["type", "value"]) {
                return Some(format!(
                    "clear_thinking_20251015.keep contains unsupported field `{field}`"
                ));
            }
            if keep_object.get("type").and_then(Value::as_str) != Some("thinking_turns") {
                return Some(
                    "clear_thinking_20251015.keep.type must be `thinking_turns`".to_string(),
                );
            }
            positive_integer_schema_error(
                keep_object.get("value"),
                "clear_thinking_20251015.keep.value",
            )
        }
        _ => Some(
            "clear_thinking_20251015.keep must be `all` or `{type:\"thinking_turns\", value:N}`"
                .to_string(),
        ),
    }
}

fn validate_anthropic_clear_tool_uses_edit(
    object: &serde_json::Map<String, Value>,
) -> Option<String> {
    if let Some(field) = first_unknown_field(
        object,
        &["type", "keep", "exclude_tools", "clear_tool_inputs"],
    ) {
        return Some(format!(
            "clear_tool_uses_20250919 contains unsupported field `{field}`"
        ));
    }

    if let Some(message) = validate_context_management_metric(
        object.get("keep"),
        "clear_tool_uses_20250919.keep",
        &["tool_uses"],
    ) {
        return Some(message);
    }
    if let Some(exclude_tools) = object.get("exclude_tools") {
        let Some(exclude_tools) = exclude_tools.as_array() else {
            return Some("clear_tool_uses_20250919.exclude_tools must be an array".to_string());
        };
        if exclude_tools.iter().any(|tool| {
            tool.as_str()
                .map(str::trim)
                .is_none_or(|tool| tool.is_empty())
        }) {
            return Some(
                "clear_tool_uses_20250919.exclude_tools entries must be non-empty strings"
                    .to_string(),
            );
        }
    }
    if object
        .get("clear_tool_inputs")
        .is_some_and(|value| !value.is_boolean())
    {
        return Some("clear_tool_uses_20250919.clear_tool_inputs must be a boolean".to_string());
    }
    None
}

fn validate_context_management_metric(
    value: Option<&Value>,
    path: &str,
    allowed_types: &[&str],
) -> Option<String> {
    let value = value?;
    let Some(object) = value.as_object() else {
        return Some(format!("{path} must be an object"));
    };
    if let Some(field) = first_unknown_field(object, &["type", "value"]) {
        return Some(format!("{path} contains unsupported field `{field}`"));
    }
    let Some(metric_type) = object.get("type").and_then(Value::as_str) else {
        return Some(format!("{path}.type must be a string"));
    };
    if !allowed_types.contains(&metric_type) {
        return Some(format!("{path}.type `{metric_type}` is not supported"));
    }
    positive_integer_schema_error(object.get("value"), &format!("{path}.value"))
}

fn positive_integer_schema_error(value: Option<&Value>, path: &str) -> Option<String> {
    if value.and_then(Value::as_u64).is_some_and(|value| value > 0) {
        None
    } else {
        Some(format!("{path} must be a positive integer"))
    }
}

fn first_unknown_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    allowed_fields: &[&str],
) -> Option<&'a str> {
    object
        .keys()
        .find(|key| !allowed_fields.contains(&key.as_str()))
        .map(String::as_str)
}

fn anthropic_request_visible_thinking_carrier_fields(body: &Value) -> Vec<&'static str> {
    let mut fields = Vec::new();
    anthropic_collect_thinking_carrier_fields(body.get("system"), &mut fields);
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            anthropic_collect_thinking_carrier_fields(message.get("content"), &mut fields);
        }
    }
    fields.sort_unstable();
    fields.dedup();
    fields
}

fn anthropic_collect_thinking_carrier_fields(
    content: Option<&Value>,
    fields: &mut Vec<&'static str>,
) {
    match content {
        Some(Value::Array(blocks)) => {
            for block in blocks {
                anthropic_collect_block_thinking_carrier_fields(block, fields);
            }
        }
        Some(Value::Object(_)) => {
            if let Some(block) = content {
                anthropic_collect_block_thinking_carrier_fields(block, fields);
            }
        }
        _ => {}
    }
}

fn anthropic_collect_block_thinking_carrier_fields(block: &Value, fields: &mut Vec<&'static str>) {
    if block.get("type").and_then(Value::as_str) != Some("thinking") {
        return;
    }
    if !anthropic_block_has_visible_thinking_text(block) {
        return;
    }
    if block.get("signature").is_some() {
        fields.push("signature");
    }
    if block.get("encrypted_content").is_some() {
        fields.push("encrypted_content");
    }
    if block.get("redacted_content").is_some() || block.get("data").is_some() {
        fields.push("redacted_content");
    }
}

fn anthropic_opaque_thinking_carrier_message(body: &Value, target_label: &str) -> Option<String> {
    anthropic_opaque_thinking_carrier_in_content(body.get("system"), target_label).or_else(|| {
        body.get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages.iter().find_map(|message| {
                    anthropic_opaque_thinking_carrier_in_content(
                        message.get("content"),
                        target_label,
                    )
                })
            })
    })
}

fn anthropic_opaque_thinking_carrier_in_content(
    content: Option<&Value>,
    target_label: &str,
) -> Option<String> {
    match content {
        Some(Value::Array(blocks)) => blocks
            .iter()
            .find_map(|block| anthropic_opaque_thinking_carrier_in_block(block, target_label)),
        Some(Value::Object(_)) => content
            .and_then(|block| anthropic_opaque_thinking_carrier_in_block(block, target_label)),
        _ => None,
    }
}

fn anthropic_opaque_thinking_carrier_in_block(block: &Value, target_label: &str) -> Option<String> {
    match block.get("type").and_then(Value::as_str) {
        Some("thinking") => {
            if anthropic_block_has_visible_thinking_text(block) {
                None
            } else {
                Some(format!(
                    "Anthropic opaque/redacted/encrypted thinking carriers without visible text require native Anthropic semantics and cannot be faithfully translated to {target_label}"
                ))
            }
        }
        Some("redacted_thinking" | "encrypted_thinking") => Some(format!(
            "Anthropic opaque/redacted/encrypted thinking carriers without visible text require native Anthropic semantics and cannot be faithfully translated to {target_label}"
        )),
        _ => None,
    }
}

fn anthropic_duplicate_assistant_tool_use_id_message(
    body: &Value,
    target_label: &str,
) -> Option<String> {
    let messages = body.get("messages").and_then(Value::as_array)?;
    let mut seen_tool_use_ids = BTreeSet::new();
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let mut duplicate_tool_use_id = None;
        anthropic_visit_content_blocks(message.get("content"), |block| {
            if duplicate_tool_use_id.is_some()
                || !matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("tool_use" | "server_tool_use")
                )
            {
                return;
            }
            let Some(tool_use_id) = block
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|tool_use_id| !tool_use_id.is_empty())
            else {
                return;
            };
            if !seen_tool_use_ids.insert(tool_use_id.to_string()) {
                duplicate_tool_use_id = Some(tool_use_id.to_string());
            }
        });
        if let Some(duplicate_tool_use_id) = duplicate_tool_use_id {
            return Some(format!(
                "Anthropic assistant tool_use id `{duplicate_tool_use_id}` is duplicated and cannot be translated to {target_label}; OpenAI-family tool_call ids must be unique"
            ));
        }
    }
    None
}

#[cfg(test)]
pub(crate) fn assess_request_translation(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> TranslationAssessment {
    assess_request_translation_with_dialect(client_format, upstream_format, body, None)
}

/// Dialect-aware variant of [`assess_request_translation`]. When a resolved [`DialectBlock`] is
/// supplied and its mechanism maps reasoning effort, the reasoning-effort drop warnings are
/// suppressed (the effort is emitted in the upstream's native shape by the proxy emit pass, not
/// dropped). Passing `None` is identical to the historical no-dialect behavior.
pub(crate) fn assess_request_translation_with_dialect(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
    dialect: Option<&DialectBlock>,
) -> TranslationAssessment {
    let reasoning_mapped = dialect_maps_reasoning(dialect);
    let mut assessment = TranslationAssessment::default();

    if let Some(message) = openai_request_file_mime_conflict_message(client_format, body) {
        assessment.reject(message);
    }

    assess_prompt_cache_extension_target_mismatch(
        &mut assessment,
        client_format,
        upstream_format,
        body,
    );

    if client_format == upstream_format {
        return assessment;
    }

    assess_openai_family_prompt_cache_extensions(
        &mut assessment,
        client_format,
        upstream_format,
        body,
    );
    assess_anthropic_prompt_cache_extensions(&mut assessment, client_format, upstream_format, body);

    if let Some(message) =
        cross_protocol_requested_choice_count_message(client_format, upstream_format, body)
    {
        assessment.reject(message);
    }

    if client_format == UpstreamFormat::OpenAiResponses
        && upstream_format != UpstreamFormat::OpenAiResponses
    {
        let controls = responses_stateful_request_controls(body);
        if !controls.is_empty() {
            let quoted = controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.reject(format!(
                "Responses request controls {quoted} require a native OpenAI Responses upstream and cannot be translated to {upstream_format}; the proxy does not reconstruct provider state"
            ));
        }
        if let Some(message) = responses_reasoning_continuity_request_message(body, upstream_format)
        {
            assessment.reject(message);
        }
        let dropped_controls = responses_warning_only_request_controls_for_translate(
            body,
            upstream_format,
            reasoning_mapped,
        );
        if !dropped_controls.is_empty() {
            let quoted = dropped_controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "OpenAI Responses controls {quoted} are not portable on this translation path to {} and will be dropped",
                translation_target_label(upstream_format)
            ));
        }
        if let Some(message) = responses_nonportable_tool_choice_message(body, upstream_format) {
            assessment.reject(message);
        }
        if let Some(message) = responses_nonportable_input_item_message(body, upstream_format) {
            assessment.reject(message);
        }
        if let Some(message) = responses_nonportable_tool_definition_message(body) {
            assessment.reject(message);
        } else if responses_has_warning_only_nonportable_tool_definitions(body) {
            assessment.warning(format!(
                "non-function Responses tools are not portable to {upstream_format} and will be dropped"
            ));
        }
        if responses_has_warning_only_encrypted_agent_message(body) {
            assessment.warning(format!(
                "OpenAI Responses `agent_message` input carries an `encrypted_content` part whose payload is opaque to {upstream_format}; only the routing metadata envelope remains visible and the encrypted payload body is dropped"
            ));
        }
        if let Some(message) = responses_custom_tool_format_reject_message(body, upstream_format) {
            assessment.reject(message);
        }
        for warning in responses_custom_tool_bridge_warning_messages(body, upstream_format) {
            assessment.warning(warning);
        }
    }

    if provider_state_control_enabled(body.get("store")) {
        assessment.reject(cross_protocol_store_reject_message(
            client_format,
            upstream_format,
        ));
    }

    if client_format == UpstreamFormat::OpenAiChatCompletions
        && upstream_format == UpstreamFormat::OpenAiResponses
    {
        if let Some(message) = normalized_openai_audio_contract(body).err().or_else(|| {
            normalized_openai_audio_contract(body)
                .ok()
                .flatten()
                .map(|_| openai_request_audio_not_portable_message("OpenAI Responses"))
        }) {
            assessment.reject(message);
        }
        if openai_assistant_history_audio_present(body) {
            assessment.reject(openai_assistant_audio_history_not_portable_message(
                "OpenAI Responses",
            ));
        }
        let controls = openai_to_responses_dropped_control_names(body);
        if !controls.is_empty() {
            let quoted = controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "OpenAI Chat Completions controls {quoted} have no direct OpenAI Responses equivalent in this translator and will be dropped"
            ));
        }
    }

    if client_format == UpstreamFormat::OpenAiChatCompletions
        && upstream_format == UpstreamFormat::Anthropic
    {
        if let Some(message) = normalized_openai_audio_contract(body).err().or_else(|| {
            normalized_openai_audio_contract(body)
                .ok()
                .flatten()
                .map(|_| openai_request_audio_not_portable_message("Anthropic"))
        }) {
            assessment.reject(message);
        }
        if openai_assistant_history_audio_present(body) {
            assessment.reject(openai_assistant_audio_history_not_portable_message(
                "Anthropic",
            ));
        }
        let mut controls = openai_to_anthropic_dropped_control_names(body)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        controls.extend(openai_warning_only_request_controls_for_translate(
            body,
            upstream_format,
            reasoning_mapped,
        ));
        if !controls.is_empty() {
            let quoted = controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "OpenAI Chat Completions controls {quoted} are not portable to Anthropic and will be dropped"
            ));
        }
    }

    if client_format == UpstreamFormat::Anthropic && upstream_format != UpstreamFormat::Anthropic {
        let target_label = translation_target_label(upstream_format);
        let top_level_control_assessments =
            anthropic_top_level_request_control_assessments(body, target_label);
        let reject_controls = top_level_control_assessments
            .iter()
            .filter(|assessment| {
                assessment.disposition == AnthropicRequestControlDisposition::FailClosed
            })
            .collect::<Vec<_>>();
        if !reject_controls.is_empty() {
            let quoted = reject_controls
                .iter()
                .map(|assessment| format!("`{}`", assessment.field))
                .collect::<Vec<_>>()
                .join(", ");
            let reasons = reject_controls
                .iter()
                .map(|assessment| assessment.reason.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            assessment.reject(format!(
                "Anthropic request controls {quoted} cannot be faithfully translated to {target_label}: {reasons}"
            ));
        }
        let dropped_hint_controls = top_level_control_assessments
            .iter()
            .filter(|assessment| {
                assessment.disposition == AnthropicRequestControlDisposition::WarnDrop
            })
            .map(|assessment| assessment.field)
            .collect::<Vec<_>>();
        if !dropped_hint_controls.is_empty() {
            let quoted = dropped_hint_controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "Anthropic request controls {quoted} are not portable to {target_label} with complete visible history and will be dropped"
            ));
        }
        let warning_controls = anthropic_warning_only_request_controls_for_translate(body);
        if !warning_controls.is_empty() {
            let quoted = warning_controls
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "Anthropic request controls {quoted} are not portable to {target_label} and will be dropped"
            ));
        }
        if let Some(message) = anthropic_opaque_thinking_carrier_message(body, target_label) {
            assessment.reject(message);
        }
        let dropped_thinking_carriers = anthropic_request_visible_thinking_carrier_fields(body);
        if !dropped_thinking_carriers.is_empty() {
            let quoted = dropped_thinking_carriers
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            assessment.warning(format!(
                "Anthropic visible thinking carrier fields {quoted} cannot be translated to {target_label}; visible thinking text will be preserved and carrier fields dropped"
            ));
        }
        if let Some(message) =
            anthropic_request_nonportable_tool_definition_message(body, target_label)
        {
            assessment.reject(message);
        }
        if let Some(message) = anthropic_duplicate_assistant_tool_use_id_message(body, target_label)
        {
            assessment.reject(message);
        }
        if let Some(message) = anthropic_request_tool_result_order_message(body, target_label) {
            assessment.reject(message);
        }
        if let Some(message) = anthropic_nonportable_content_block_message(body, target_label) {
            assessment.reject(message);
        }
    }

    let responses_custom_bridge_supported = client_format == UpstreamFormat::OpenAiResponses
        && upstream_format == UpstreamFormat::Anthropic;
    if upstream_format == UpstreamFormat::Anthropic
        && request_has_custom_tools(client_format, body)
        && !responses_custom_bridge_supported
    {
        assessment.reject(custom_tools_not_portable_message(upstream_format));
    }

    if upstream_format == UpstreamFormat::Anthropic {
        if let Some(message) = request_invalid_structured_tool_arguments_message(
            client_format,
            body,
            translation_target_label(upstream_format),
        ) {
            assessment.reject(message);
        }
    }

    assessment
}
