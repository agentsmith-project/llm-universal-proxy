use serde::{ser::SerializeStruct, Serialize, Serializer};
use serde_json::Value;

use crate::formats::UpstreamFormat;
use crate::prompt_cache_controls::{
    anthropic_extra_body_openai_prompt_cache_key_present, anthropic_protocol_cache_control_present,
    openai_extra_body_anthropic_cache_control_present,
    openai_family_prompt_cache_top_level_fields_present,
};
use crate::provider_state_controls::responses_stateful_request_controls_present;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestProcessing {
    RequestTransformationNotRequired,
    RequestTransformationRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateBridgeModifier {
    Off,
    CaptureCandidate,
    Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheRequestControl {
    None,
    #[serde(rename = "preserved_native")]
    Preserved,
    ExplicitExtensionMapped,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestProcessingInfo {
    pub request_processing: RequestProcessing,
    pub zero_transform_forwarding_active: bool,
    pub state_bridge: StateBridgeModifier,
    pub provider_native_prompt_cache: PromptCacheRequestControl,
}

impl Default for RequestProcessingInfo {
    fn default() -> Self {
        Self {
            request_processing: RequestProcessing::RequestTransformationRequired,
            zero_transform_forwarding_active: false,
            state_bridge: StateBridgeModifier::Off,
            provider_native_prompt_cache: PromptCacheRequestControl::None,
        }
    }
}

impl RequestProcessingInfo {
    fn request_body_handling(self) -> &'static str {
        if self.zero_transform_forwarding_active {
            "client_body_preserved"
        } else {
            "constructed"
        }
    }
}

impl Serialize for RequestProcessingInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.state_bridge == StateBridgeModifier::Off {
            2
        } else {
            3
        };
        let mut state = serializer.serialize_struct("RequestProcessingInfo", field_count)?;
        state.serialize_field("request_body_handling", self.request_body_handling())?;
        if self.state_bridge != StateBridgeModifier::Off {
            state.serialize_field("local_state_handling", &self.state_bridge)?;
        }
        state.serialize_field(
            "provider_prompt_cache_request_control",
            &self.provider_native_prompt_cache,
        )?;
        state.end()
    }
}

pub struct RequestProcessingInput<'a> {
    pub client_format: UpstreamFormat,
    pub upstream_format: UpstreamFormat,
    pub body: &'a Value,
    pub requested_model: &'a str,
    pub upstream_model: &'a str,
    pub stream: bool,
    pub forced_stream: bool,
    pub route_policy_requires_body_mutation: bool,
    pub state_bridge: StateBridgeModifier,
}

pub fn classify_request_processing(input: RequestProcessingInput<'_>) -> RequestProcessingInfo {
    let provider_native_prompt_cache = classify_provider_native_prompt_cache(
        input.client_format,
        input.upstream_format,
        input.body,
    );
    let request_processing = if request_transformation_required(&input) {
        RequestProcessing::RequestTransformationRequired
    } else {
        RequestProcessing::RequestTransformationNotRequired
    };

    RequestProcessingInfo {
        request_processing,
        zero_transform_forwarding_active: false,
        state_bridge: input.state_bridge,
        provider_native_prompt_cache,
    }
}

fn request_transformation_required(input: &RequestProcessingInput<'_>) -> bool {
    input.client_format != input.upstream_format
        || input.state_bridge != StateBridgeModifier::Off
        || input.forced_stream
        || model_body_mutation_required(input)
        || input.route_policy_requires_body_mutation
        || same_protocol_request_mutation_required(input)
}

fn model_body_mutation_required(input: &RequestProcessingInput<'_>) -> bool {
    if native_responses_model_less_stateful_passthrough_without_model_insertion(input) {
        return false;
    }
    if native_responses_model_less_stateful_model_removal(input) {
        return true;
    }
    if input.requested_model.trim() != input.upstream_model.trim() {
        return true;
    }
    let Some(object) = input.body.as_object() else {
        return false;
    };
    object.get("model").and_then(Value::as_str) != Some(input.upstream_model)
}

fn native_responses_model_less_stateful_passthrough_without_model_insertion(
    input: &RequestProcessingInput<'_>,
) -> bool {
    input.client_format == UpstreamFormat::OpenAiResponses
        && input.upstream_format == UpstreamFormat::OpenAiResponses
        && input.requested_model.trim().is_empty()
        && input.upstream_model.trim().is_empty()
        && input
            .body
            .as_object()
            .is_some_and(|object| !object.contains_key("model"))
        && responses_stateful_request_controls_present(input.body)
}

fn native_responses_model_less_stateful_model_removal(input: &RequestProcessingInput<'_>) -> bool {
    input.client_format == UpstreamFormat::OpenAiResponses
        && input.upstream_format == UpstreamFormat::OpenAiResponses
        && input.requested_model.trim().is_empty()
        && input.upstream_model.trim().is_empty()
        && input
            .body
            .as_object()
            .is_some_and(|object| object.contains_key("model"))
        && responses_stateful_request_controls_present(input.body)
}

fn same_protocol_request_mutation_required(input: &RequestProcessingInput<'_>) -> bool {
    if input.client_format != input.upstream_format {
        return false;
    }
    match input.client_format {
        UpstreamFormat::OpenAiCompletion => {
            openai_chat_has_instruction_roles(input.body)
                || openai_chat_has_adjacent_coalescible_string_messages(input.body)
                || input.upstream_model.starts_with("MiniMax-")
        }
        UpstreamFormat::OpenAiResponses => {
            input.stream && responses_input_has_developer_role(input.body)
        }
        UpstreamFormat::Anthropic => false,
    }
}

fn openai_chat_has_instruction_roles(body: &Value) -> bool {
    body.get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                matches!(
                    message.get("role").and_then(Value::as_str),
                    Some("system" | "developer")
                )
            })
        })
}

fn openai_chat_has_adjacent_coalescible_string_messages(body: &Value) -> bool {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return false;
    };
    messages.windows(2).any(|pair| {
        let Some(left_role) = openai_string_message_role_if_coalescible(&pair[0]) else {
            return false;
        };
        openai_string_message_role_if_coalescible(&pair[1]) == Some(left_role)
    })
}

fn openai_string_message_role_if_coalescible(message: &Value) -> Option<&str> {
    let role = message.get("role").and_then(Value::as_str)?;
    message.get("content").and_then(Value::as_str)?;
    let object = message.as_object()?;
    object
        .keys()
        .all(|key| key == "role" || key == "content")
        .then_some(role)
}

fn responses_input_has_developer_role(body: &Value) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("message")
                    && item.get("role").and_then(Value::as_str) == Some("developer")
            })
        })
}

fn classify_provider_native_prompt_cache(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> PromptCacheRequestControl {
    if openai_family_format(client_format)
        && upstream_format == UpstreamFormat::Anthropic
        && openai_extra_body_anthropic_cache_control_present(body)
    {
        return PromptCacheRequestControl::ExplicitExtensionMapped;
    }

    if openai_family_format(client_format)
        && upstream_format == UpstreamFormat::Anthropic
        && openai_family_prompt_cache_top_level_fields_present(body)
    {
        return PromptCacheRequestControl::Dropped;
    }

    if client_format == UpstreamFormat::Anthropic
        && openai_family_format(upstream_format)
        && anthropic_extra_body_openai_prompt_cache_key_present(body)
    {
        return PromptCacheRequestControl::ExplicitExtensionMapped;
    }

    if client_format == UpstreamFormat::Anthropic
        && openai_family_format(upstream_format)
        && anthropic_protocol_cache_control_present(body)
    {
        return PromptCacheRequestControl::Dropped;
    }

    if openai_family_format(client_format)
        && openai_family_format(upstream_format)
        && openai_family_prompt_cache_top_level_fields_present(body)
    {
        return PromptCacheRequestControl::Preserved;
    }

    if client_format == upstream_format
        && client_format == UpstreamFormat::Anthropic
        && anthropic_protocol_cache_control_present(body)
    {
        PromptCacheRequestControl::Preserved
    } else {
        PromptCacheRequestControl::None
    }
}

fn openai_family_format(format: UpstreamFormat) -> bool {
    matches!(
        format,
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses
    )
}
