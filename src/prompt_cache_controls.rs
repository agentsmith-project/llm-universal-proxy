use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::formats::UpstreamFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheRequestControl {
    None,
    #[serde(rename = "preserved_native")]
    Preserved,
    ExplicitExtensionMapped,
    Synthesized,
    Dropped,
}

const PROMPT_CACHE_SYNTHESIS_VERSION: &str = "v1";
const PROMPT_CACHE_SYNTHESIS_SCHEMA: &str = "llmup.openai_family_prompt_cache_static_prefix.v1";
const PROMPT_CACHE_KEY_PREFIX: &str = "llmup:v1:";

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct OpenAiFamilyPromptCacheKeySynthesis {
    key: String,
    key_fingerprint: String,
}

impl std::fmt::Debug for OpenAiFamilyPromptCacheKeySynthesis {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiFamilyPromptCacheKeySynthesis")
            .field("key_fingerprint", &self.key_fingerprint)
            .finish()
    }
}

impl OpenAiFamilyPromptCacheKeySynthesis {
    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    pub(crate) fn key_fingerprint(&self) -> &str {
        &self.key_fingerprint
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct OpenAiFamilyPromptCacheKeySynthesisContext<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) upstream_name: &'a str,
    pub(crate) upstream_model: &'a str,
    pub(crate) upstream_format: UpstreamFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderPromptCacheRequestControlAnalysis {
    target_provider: PromptCacheTargetProvider,
    components: Vec<ProviderPromptCacheRequestControlComponent>,
}

impl ProviderPromptCacheRequestControlAnalysis {
    pub(crate) fn target_provider(&self) -> &'static str {
        self.target_provider.as_str()
    }

    pub(crate) fn components(&self) -> &[ProviderPromptCacheRequestControlComponent] {
        &self.components
    }

    pub(crate) fn coarse_control(&self) -> PromptCacheRequestControl {
        project_prompt_cache_request_control(
            || {
                self.components.iter().any(|component| {
                    component.disposition == PromptCacheRequestControl::ExplicitExtensionMapped
                })
            },
            || {
                self.components
                    .iter()
                    .any(|component| component.disposition == PromptCacheRequestControl::Dropped)
            },
            || {
                self.components
                    .iter()
                    .any(|component| component.disposition == PromptCacheRequestControl::Preserved)
            },
        )
    }
}

fn project_prompt_cache_request_control<Mapped, Dropped, Preserved>(
    mapped_present: Mapped,
    dropped_present: Dropped,
    preserved_present: Preserved,
) -> PromptCacheRequestControl
where
    Mapped: FnOnce() -> bool,
    Dropped: FnOnce() -> bool,
    Preserved: FnOnce() -> bool,
{
    if mapped_present() {
        return PromptCacheRequestControl::ExplicitExtensionMapped;
    }
    if dropped_present() {
        return PromptCacheRequestControl::Dropped;
    }
    if preserved_present() {
        return PromptCacheRequestControl::Preserved;
    }
    PromptCacheRequestControl::None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderPromptCacheRequestControlComponent {
    disposition: PromptCacheRequestControl,
    source_fields: Vec<&'static str>,
    target_fields: PromptCacheTargetFields,
    ttl_or_retention_source: Option<&'static str>,
    omitted_reason: Option<&'static str>,
}

impl ProviderPromptCacheRequestControlComponent {
    fn new(
        disposition: PromptCacheRequestControl,
        source_fields: Vec<&'static str>,
        target_fields: PromptCacheTargetFields,
        ttl_or_retention_source: Option<&'static str>,
        omitted_reason: Option<&'static str>,
    ) -> Self {
        Self {
            disposition,
            source_fields,
            target_fields,
            ttl_or_retention_source,
            omitted_reason,
        }
    }

    pub(crate) fn disposition(&self) -> PromptCacheRequestControl {
        self.disposition
    }

    pub(crate) fn source_fields(&self) -> &[&'static str] {
        &self.source_fields
    }

    pub(crate) fn target_fields(&self, upstream_body: &Value) -> Vec<&'static str> {
        self.target_fields.fields(upstream_body)
    }

    pub(crate) fn ttl_or_retention_source(&self) -> Option<&'static str> {
        self.ttl_or_retention_source
    }

    pub(crate) fn omitted_reason(&self) -> Option<&'static str> {
        self.omitted_reason
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptCacheTargetProvider {
    Anthropic,
    OpenAiFamily,
}

impl PromptCacheTargetProvider {
    fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAiFamily => "openai_family",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptCacheTargetFields {
    None,
    AnthropicTopLevelCacheControl,
    AnthropicProtocolCacheControl,
    OpenAiFamilyTopLevelPromptCache,
}

impl PromptCacheTargetFields {
    fn fields(self, upstream_body: &Value) -> Vec<&'static str> {
        match self {
            Self::None => Vec::new(),
            Self::AnthropicTopLevelCacheControl => {
                anthropic_top_level_cache_control_field(upstream_body)
            }
            Self::AnthropicProtocolCacheControl => {
                anthropic_protocol_cache_control_fields(upstream_body)
            }
            Self::OpenAiFamilyTopLevelPromptCache => {
                openai_family_top_level_prompt_cache_fields(upstream_body)
            }
        }
    }
}

pub(crate) fn analyze_provider_prompt_cache_request_control(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> ProviderPromptCacheRequestControlAnalysis {
    let mut components = Vec::new();

    if openai_family_format(client_format) && upstream_format == UpstreamFormat::Anthropic {
        if openai_extra_body_anthropic_cache_control_present(body) {
            components.push(ProviderPromptCacheRequestControlComponent::new(
                PromptCacheRequestControl::ExplicitExtensionMapped,
                vec!["extra_body.anthropic.cache_control"],
                PromptCacheTargetFields::AnthropicTopLevelCacheControl,
                openai_extra_body_anthropic_cache_control_ttl_source(body),
                None,
            ));
        }
        if openai_family_prompt_cache_top_level_fields_present(body) {
            components.push(ProviderPromptCacheRequestControlComponent::new(
                PromptCacheRequestControl::Dropped,
                openai_family_top_level_prompt_cache_fields(body),
                PromptCacheTargetFields::None,
                openai_family_prompt_cache_retention_source(body),
                Some("openai_prompt_cache_key_not_anthropic_cache_control_or_breakpoint"),
            ));
        }
    } else if client_format == UpstreamFormat::Anthropic && openai_family_format(upstream_format) {
        if anthropic_extra_body_openai_prompt_cache_key_present(body) {
            components.push(ProviderPromptCacheRequestControlComponent::new(
                PromptCacheRequestControl::ExplicitExtensionMapped,
                anthropic_extra_body_openai_prompt_cache_fields(body),
                PromptCacheTargetFields::OpenAiFamilyTopLevelPromptCache,
                anthropic_extra_body_openai_retention_source(body),
                None,
            ));
        }
        if anthropic_protocol_cache_control_present(body) {
            components.push(ProviderPromptCacheRequestControlComponent::new(
                PromptCacheRequestControl::Dropped,
                anthropic_protocol_cache_control_fields(body),
                PromptCacheTargetFields::None,
                anthropic_protocol_cache_control_ttl_source(body),
                Some("anthropic_cache_control_not_openai_prompt_cache_key"),
            ));
        }
    } else if openai_family_format(client_format) && openai_family_format(upstream_format) {
        if openai_family_prompt_cache_top_level_fields_present(body) {
            components.push(ProviderPromptCacheRequestControlComponent::new(
                PromptCacheRequestControl::Preserved,
                openai_family_top_level_prompt_cache_fields(body),
                PromptCacheTargetFields::OpenAiFamilyTopLevelPromptCache,
                openai_family_prompt_cache_retention_source(body),
                None,
            ));
        }
    } else if client_format == upstream_format
        && client_format == UpstreamFormat::Anthropic
        && anthropic_protocol_cache_control_present(body)
    {
        components.push(ProviderPromptCacheRequestControlComponent::new(
            PromptCacheRequestControl::Preserved,
            anthropic_protocol_cache_control_fields(body),
            PromptCacheTargetFields::AnthropicProtocolCacheControl,
            anthropic_protocol_cache_control_ttl_source(body),
            None,
        ));
    }

    ProviderPromptCacheRequestControlAnalysis {
        target_provider: prompt_cache_target_provider(upstream_format),
        components,
    }
}

pub(crate) fn classify_provider_prompt_cache_request_control(
    client_format: UpstreamFormat,
    upstream_format: UpstreamFormat,
    body: &Value,
) -> PromptCacheRequestControl {
    project_prompt_cache_request_control(
        || {
            (openai_family_format(client_format)
                && upstream_format == UpstreamFormat::Anthropic
                && openai_extra_body_anthropic_cache_control_present(body))
                || (client_format == UpstreamFormat::Anthropic
                    && openai_family_format(upstream_format)
                    && anthropic_extra_body_openai_prompt_cache_key_present(body))
        },
        || {
            (openai_family_format(client_format)
                && upstream_format == UpstreamFormat::Anthropic
                && openai_family_prompt_cache_top_level_fields_present(body))
                || (client_format == UpstreamFormat::Anthropic
                    && openai_family_format(upstream_format)
                    && anthropic_protocol_cache_control_present(body))
        },
        || {
            (openai_family_format(client_format)
                && openai_family_format(upstream_format)
                && openai_family_prompt_cache_top_level_fields_present(body))
                || (client_format == upstream_format
                    && client_format == UpstreamFormat::Anthropic
                    && anthropic_protocol_cache_control_present(body))
        },
    )
}

#[cfg(test)]
pub(crate) fn synthesize_openai_family_prompt_cache_key(
    context: OpenAiFamilyPromptCacheKeySynthesisContext<'_>,
    upstream_body: &mut Value,
) -> Option<OpenAiFamilyPromptCacheKeySynthesis> {
    synthesize_openai_family_prompt_cache_key_with_instruction_source(
        context,
        context.upstream_format,
        None,
        upstream_body,
    )
}

pub(crate) fn synthesize_openai_family_prompt_cache_key_from_source(
    context: OpenAiFamilyPromptCacheKeySynthesisContext<'_>,
    source_format: UpstreamFormat,
    source_body: &Value,
    upstream_body: &mut Value,
) -> Option<OpenAiFamilyPromptCacheKeySynthesis> {
    synthesize_openai_family_prompt_cache_key_with_instruction_source(
        context,
        source_format,
        Some(source_body),
        upstream_body,
    )
}

fn synthesize_openai_family_prompt_cache_key_with_instruction_source(
    context: OpenAiFamilyPromptCacheKeySynthesisContext<'_>,
    source_format: UpstreamFormat,
    source_body: Option<&Value>,
    upstream_body: &mut Value,
) -> Option<OpenAiFamilyPromptCacheKeySynthesis> {
    if !openai_family_format(context.upstream_format) {
        return None;
    }
    if upstream_body.get("prompt_cache_key").is_some() {
        return None;
    }
    if openai_family_provider_state_control_present(upstream_body) {
        return None;
    }

    let instruction_body = source_body.unwrap_or(upstream_body);
    let static_prefix =
        openai_family_static_prefix(&context, source_format, instruction_body, upstream_body)?;
    let canonical_static_prefix = canonical_json(&static_prefix);
    let namespace_fp = scoped_fingerprint("namespace", context.namespace, 16);
    let upstream_fp = scoped_fingerprint("upstream", context.upstream_name, 16);
    let model_fp = scoped_fingerprint("model", context.upstream_model, 16);
    let static_prefix_fp = scoped_fingerprint("static_prefix", &canonical_static_prefix, 32);
    let key = format!(
        "llmup:{PROMPT_CACHE_SYNTHESIS_VERSION}:{namespace_fp}:{upstream_fp}:{model_fp}:{static_prefix_fp}"
    );

    upstream_body
        .as_object_mut()?
        .insert("prompt_cache_key".to_string(), Value::String(key.clone()));
    let key_fingerprint = synthesized_prompt_cache_key_debug_fingerprint(&key);
    Some(OpenAiFamilyPromptCacheKeySynthesis {
        key,
        key_fingerprint,
    })
}

pub(crate) fn synthesized_prompt_cache_key_debug_fingerprint(key: &str) -> String {
    scoped_fingerprint("synthesized_key_debug", key, 12)
}

pub(crate) fn synthesized_prompt_cache_key_present(body: &Value) -> bool {
    body.get("prompt_cache_key")
        .and_then(Value::as_str)
        .is_some_and(|key| key.starts_with(PROMPT_CACHE_KEY_PREFIX))
}

fn openai_family_provider_state_control_present(body: &Value) -> bool {
    ["previous_response_id", "conversation", "prompt"]
        .into_iter()
        .any(|field| body.get(field).is_some())
}

fn openai_family_static_prefix(
    context: &OpenAiFamilyPromptCacheKeySynthesisContext<'_>,
    instruction_format: UpstreamFormat,
    instruction_body: &Value,
    upstream_body: &Value,
) -> Option<Value> {
    upstream_body.as_object()?;
    let protocol = match context.upstream_format {
        UpstreamFormat::OpenAiCompletion => "openai_chat_completions",
        UpstreamFormat::OpenAiResponses => "openai_responses",
        UpstreamFormat::Anthropic => return None,
    };

    Some(serde_json::json!({
        "schema": PROMPT_CACHE_SYNTHESIS_SCHEMA,
        "namespace": context.namespace,
        "upstream": context.upstream_name,
        "model": context.upstream_model,
        "protocol": protocol,
        "version": PROMPT_CACHE_SYNTHESIS_VERSION,
        "instructions": static_prefix_instructions(instruction_format, instruction_body),
        "tools": upstream_body.get("tools").cloned().unwrap_or(Value::Array(Vec::new())),
        "static_config": openai_family_static_config(context.upstream_format, upstream_body),
    }))
}

fn static_prefix_instructions(instruction_format: UpstreamFormat, body: &Value) -> Value {
    match instruction_format {
        UpstreamFormat::OpenAiCompletion => openai_chat_static_instructions(body),
        UpstreamFormat::OpenAiResponses => {
            let mut instructions = serde_json::Map::new();
            if let Some(value) = body.get("instructions") {
                instructions.insert("instructions".to_string(), value.clone());
            }
            if let Some(items) = body.get("input").and_then(Value::as_array) {
                let static_items = items
                    .iter()
                    .take_while(|item| openai_responses_static_instruction_item(item))
                    .cloned()
                    .collect::<Vec<_>>();
                if !static_items.is_empty() {
                    instructions.insert(
                        "input_static_messages".to_string(),
                        Value::Array(static_items),
                    );
                }
            }
            Value::Object(instructions)
        }
        UpstreamFormat::Anthropic => anthropic_static_instructions(body),
    }
}

fn anthropic_static_instructions(body: &Value) -> Value {
    let mut instructions = serde_json::Map::new();
    if let Some(system) = body.get("system") {
        instructions.insert(
            "system".to_string(),
            value_without_anthropic_cache_control(system),
        );
    }
    Value::Object(instructions)
}

fn value_without_anthropic_cache_control(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(value_without_anthropic_cache_control)
                .collect(),
        ),
        Value::Object(object) => {
            let mut cleaned = serde_json::Map::new();
            for (key, value) in object {
                if key == "cache_control" {
                    continue;
                }
                cleaned.insert(key.clone(), value_without_anthropic_cache_control(value));
            }
            Value::Object(cleaned)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
    }
}

fn openai_chat_static_instructions(body: &Value) -> Value {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return Value::Array(Vec::new());
    };
    let instruction_messages = messages
        .iter()
        .take_while(|message| openai_chat_explicit_static_instruction_message(message))
        .cloned()
        .collect::<Vec<_>>();
    if !instruction_messages.is_empty() {
        return Value::Array(instruction_messages);
    }

    let compatibility_instructions = messages
        .iter()
        .map_while(openai_chat_compatibility_instruction)
        .collect::<Vec<_>>();
    Value::Array(compatibility_instructions)
}

fn openai_responses_static_instruction_item(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("message")
        && matches!(
            item.get("role").and_then(Value::as_str),
            Some("system" | "developer")
        )
}

fn openai_chat_explicit_static_instruction_message(message: &Value) -> bool {
    matches!(
        message.get("role").and_then(Value::as_str),
        Some("system" | "developer")
    )
}

fn openai_chat_compatibility_instruction(message: &Value) -> Option<Value> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let content = message.get("content").and_then(Value::as_str)?;
    let label = ["System instructions:\n", "Developer instructions:\n"]
        .into_iter()
        .find(|label| content.starts_with(label))?;
    Some(serde_json::json!({
        "role": if label.starts_with("Developer") { "developer" } else { "system" },
        "content": content.strip_prefix(label).unwrap_or(content)
    }))
}

fn openai_family_static_config(upstream_format: UpstreamFormat, body: &Value) -> Value {
    let fields: &[&str] = match upstream_format {
        UpstreamFormat::OpenAiCompletion => &[
            "response_format",
            "tool_choice",
            "parallel_tool_calls",
            "reasoning_effort",
        ],
        UpstreamFormat::OpenAiResponses => {
            &["text", "tool_choice", "parallel_tool_calls", "reasoning"]
        }
        UpstreamFormat::Anthropic => &[],
    };
    let mut object = serde_json::Map::new();
    for field in fields {
        if let Some(value) = body.get(*field) {
            object.insert((*field).to_string(), value.clone());
        }
    }
    Value::Object(object)
}

fn scoped_fingerprint(scope: &str, value: &str, hex_chars: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"llmup.provider_prompt_cache.");
    hasher.update(scope.as_bytes());
    hasher.update(b"\0");
    hasher.update(value.as_bytes());
    let digest = hex::encode(hasher.finalize());
    digest[..hex_chars.min(digest.len())].to_string()
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
        }
        Value::Array(items) => {
            let rendered = items.iter().map(canonical_json).collect::<Vec<_>>();
            format!("[{}]", rendered.join(","))
        }
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let rendered = keys
                .into_iter()
                .map(|key| {
                    let key_json =
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                    let value_json = canonical_json(&object[key]);
                    format!("{key_json}:{value_json}")
                })
                .collect::<Vec<_>>();
            format!("{{{}}}", rendered.join(","))
        }
    }
}

pub(crate) fn openai_family_prompt_cache_top_level_fields_present(body: &Value) -> bool {
    body.get("prompt_cache_key").is_some() || body.get("prompt_cache_retention").is_some()
}

pub(crate) fn openai_extra_body_anthropic_cache_control(body: &Value) -> Option<&Value> {
    body.get("extra_body")
        .and_then(|extra_body| extra_body.get("anthropic"))
        .and_then(|anthropic| anthropic.get("cache_control"))
}

pub(crate) fn openai_extra_body_anthropic_cache_control_present(body: &Value) -> bool {
    openai_extra_body_anthropic_cache_control(body).is_some()
}

pub(crate) fn anthropic_extra_body_openai_prompt_cache_controls(body: &Value) -> Option<&Value> {
    body.get("extra_body")
        .and_then(|extra_body| extra_body.get("openai"))
}

pub(crate) fn anthropic_extra_body_openai_prompt_cache_controls_present(body: &Value) -> bool {
    anthropic_extra_body_openai_prompt_cache_controls(body).is_some_and(|openai| {
        openai.get("prompt_cache_key").is_some() || openai.get("prompt_cache_retention").is_some()
    })
}

pub(crate) fn anthropic_extra_body_openai_prompt_cache_key_present(body: &Value) -> bool {
    anthropic_extra_body_openai_prompt_cache_controls(body)
        .is_some_and(|openai| openai.get("prompt_cache_key").is_some())
}

pub(crate) fn anthropic_protocol_cache_control_present(body: &Value) -> bool {
    if body.get("cache_control").is_some() {
        return true;
    }
    if body
        .get("system")
        .is_some_and(anthropic_system_cache_control_present)
    {
        return true;
    }
    if body
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| messages.iter().any(anthropic_message_cache_control_present))
    {
        return true;
    }
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| tools.iter().any(anthropic_block_cache_control_present))
}

fn anthropic_system_cache_control_present(system: &Value) -> bool {
    match system {
        Value::Array(blocks) => blocks.iter().any(anthropic_block_cache_control_present),
        Value::Object(_) => anthropic_block_cache_control_present(system),
        _ => false,
    }
}

fn anthropic_message_cache_control_present(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| blocks.iter().any(anthropic_block_cache_control_present))
}

fn anthropic_block_cache_control_present(block: &Value) -> bool {
    block.get("cache_control").is_some()
}

fn prompt_cache_target_provider(format: UpstreamFormat) -> PromptCacheTargetProvider {
    match format {
        UpstreamFormat::Anthropic => PromptCacheTargetProvider::Anthropic,
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses => {
            PromptCacheTargetProvider::OpenAiFamily
        }
    }
}

fn openai_family_format(format: UpstreamFormat) -> bool {
    matches!(
        format,
        UpstreamFormat::OpenAiCompletion | UpstreamFormat::OpenAiResponses
    )
}

pub(crate) fn openai_family_top_level_prompt_cache_fields(body: &Value) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if body.get("prompt_cache_key").is_some() {
        fields.push("prompt_cache_key");
    }
    if body.get("prompt_cache_retention").is_some() {
        fields.push("prompt_cache_retention");
    }
    fields
}

fn openai_family_prompt_cache_retention_source(body: &Value) -> Option<&'static str> {
    body.get("prompt_cache_retention")
        .is_some()
        .then_some("prompt_cache_retention")
}

fn anthropic_top_level_cache_control_field(body: &Value) -> Vec<&'static str> {
    if body.get("cache_control").is_some() {
        vec!["cache_control"]
    } else {
        Vec::new()
    }
}

fn openai_extra_body_anthropic_cache_control_ttl_source(body: &Value) -> Option<&'static str> {
    openai_extra_body_anthropic_cache_control(body)?
        .get("ttl")
        .is_some()
        .then_some("extra_body.anthropic.cache_control.ttl")
}

pub(crate) fn anthropic_extra_body_openai_prompt_cache_fields(body: &Value) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if let Some(openai) = anthropic_extra_body_openai_prompt_cache_controls(body) {
        if openai.get("prompt_cache_key").is_some() {
            fields.push("extra_body.openai.prompt_cache_key");
        }
        if openai.get("prompt_cache_retention").is_some() {
            fields.push("extra_body.openai.prompt_cache_retention");
        }
    }
    fields
}

fn anthropic_extra_body_openai_retention_source(body: &Value) -> Option<&'static str> {
    anthropic_extra_body_openai_prompt_cache_controls(body)?
        .get("prompt_cache_retention")
        .is_some()
        .then_some("extra_body.openai.prompt_cache_retention")
}

pub(crate) fn anthropic_protocol_cache_control_fields(body: &Value) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if body.get("cache_control").is_some() {
        fields.push("cache_control");
    }
    if body
        .get("system")
        .is_some_and(anthropic_system_cache_control_present)
    {
        fields.push(anthropic_system_cache_control_field(body.get("system")));
    }
    if body
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| messages.iter().any(anthropic_message_cache_control_present))
    {
        fields.push("messages[].content[].cache_control");
    }
    if body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| tools.iter().any(anthropic_block_cache_control_present))
    {
        fields.push("tools[].cache_control");
    }
    fields
}

fn anthropic_system_cache_control_field(system: Option<&Value>) -> &'static str {
    match system {
        Some(Value::Object(_)) => "system.cache_control",
        _ => "system[].cache_control",
    }
}

fn anthropic_protocol_cache_control_ttl_source(body: &Value) -> Option<&'static str> {
    if body
        .get("cache_control")
        .and_then(|cache_control| cache_control.get("ttl"))
        .is_some()
    {
        return Some("cache_control.ttl");
    }
    if let Some(source) = anthropic_system_cache_control_ttl_source(body.get("system")) {
        return Some(source);
    }
    if body
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|blocks| {
                        blocks.iter().any(anthropic_block_cache_control_ttl_present)
                    })
            })
        })
    {
        return Some("messages[].content[].cache_control.ttl");
    }
    if body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| tools.iter().any(anthropic_block_cache_control_ttl_present))
    {
        return Some("tools[].cache_control.ttl");
    }
    None
}

fn anthropic_system_cache_control_ttl_source(system: Option<&Value>) -> Option<&'static str> {
    match system {
        Some(Value::Array(blocks))
            if blocks.iter().any(anthropic_block_cache_control_ttl_present) =>
        {
            Some("system[].cache_control.ttl")
        }
        Some(Value::Object(_)) if system.is_some_and(anthropic_block_cache_control_ttl_present) => {
            Some("system.cache_control.ttl")
        }
        _ => None,
    }
}

fn anthropic_block_cache_control_ttl_present(block: &Value) -> bool {
    block
        .get("cache_control")
        .and_then(|cache_control| cache_control.get("ttl"))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prompt_cache_synthesis_context(
        upstream_format: UpstreamFormat,
    ) -> OpenAiFamilyPromptCacheKeySynthesisContext<'static> {
        OpenAiFamilyPromptCacheKeySynthesisContext {
            namespace: "default",
            upstream_name: "primary",
            upstream_model: "gpt-4o-mini",
            upstream_format,
        }
    }

    fn synthesized_prompt_cache_key(mut body: Value, upstream_format: UpstreamFormat) -> String {
        synthesize_openai_family_prompt_cache_key(
            prompt_cache_synthesis_context(upstream_format),
            &mut body,
        )
        .expect("prompt-cache key should be synthesized");
        body["prompt_cache_key"]
            .as_str()
            .expect("synthesized prompt-cache key")
            .to_string()
    }

    #[test]
    fn detects_openai_family_prompt_cache_top_level_fields() {
        assert!(openai_family_prompt_cache_top_level_fields_present(
            &json!({ "prompt_cache_key": "stable-prefix" })
        ));
        assert!(openai_family_prompt_cache_top_level_fields_present(
            &json!({ "prompt_cache_retention": "24h" })
        ));
        assert!(!openai_family_prompt_cache_top_level_fields_present(
            &json!({ "messages": [] })
        ));
    }

    #[test]
    fn detects_openai_extra_body_anthropic_cache_control() {
        assert!(openai_extra_body_anthropic_cache_control_present(&json!({
            "extra_body": {
                "anthropic": {
                    "cache_control": { "type": "ephemeral" }
                }
            }
        })));
        assert!(!openai_extra_body_anthropic_cache_control_present(
            &json!({ "extra_body": { "anthropic": {} } })
        ));
    }

    #[test]
    fn detects_anthropic_extra_body_openai_prompt_cache_controls() {
        assert!(anthropic_extra_body_openai_prompt_cache_controls_present(
            &json!({ "extra_body": { "openai": { "prompt_cache_key": "stable-prefix" } } })
        ));
        assert!(anthropic_extra_body_openai_prompt_cache_controls_present(
            &json!({ "extra_body": { "openai": { "prompt_cache_retention": "24h" } } })
        ));
        assert!(!anthropic_extra_body_openai_prompt_cache_controls_present(
            &json!({ "extra_body": { "openai": {} } })
        ));
    }

    #[test]
    fn detects_anthropic_extra_body_openai_prompt_cache_key() {
        assert!(anthropic_extra_body_openai_prompt_cache_key_present(
            &json!({ "extra_body": { "openai": { "prompt_cache_key": "stable-prefix" } } })
        ));
        assert!(!anthropic_extra_body_openai_prompt_cache_key_present(
            &json!({ "extra_body": { "openai": { "prompt_cache_retention": "24h" } } })
        ));
        assert!(!anthropic_extra_body_openai_prompt_cache_key_present(
            &json!({ "extra_body": { "openai": {} } })
        ));
    }

    #[test]
    fn detects_anthropic_protocol_cache_control_paths() {
        let cases = [
            json!({ "cache_control": { "type": "ephemeral" } }),
            json!({ "system": [{ "type": "text", "text": "System", "cache_control": { "type": "ephemeral" } }] }),
            json!({ "system": { "type": "text", "text": "System", "cache_control": { "type": "ephemeral" } } }),
            json!({ "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Hi", "cache_control": { "type": "ephemeral" } }] }] }),
            json!({ "tools": [{ "name": "lookup", "input_schema": { "type": "object" }, "cache_control": { "type": "ephemeral" } }] }),
        ];

        for body in cases {
            assert!(
                anthropic_protocol_cache_control_present(&body),
                "body = {body:?}"
            );
        }
        assert!(!anthropic_protocol_cache_control_present(
            &json!({ "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Hi" }] }] })
        ));
    }

    #[test]
    fn prompt_cache_chat_synthesis_ignores_dynamic_tail_user_instruction_labels() {
        let baseline = synthesized_prompt_cache_key(
            json!({
                "model": "gpt-4o-mini",
                "messages": [
                    { "role": "user", "content": "First user turn." },
                    { "role": "assistant", "content": "First answer." },
                    { "role": "user", "content": "Final dynamic user turn." }
                ]
            }),
            UpstreamFormat::OpenAiCompletion,
        );
        let injected_tail = synthesized_prompt_cache_key(
            json!({
                "model": "gpt-4o-mini",
                "messages": [
                    { "role": "user", "content": "First user turn." },
                    { "role": "assistant", "content": "First answer." },
                    {
                        "role": "user",
                        "content": "System instructions:\nDo not let this dynamic tail alter the cache key."
                    }
                ]
            }),
            UpstreamFormat::OpenAiCompletion,
        );

        assert_eq!(
            baseline, injected_tail,
            "dynamic tail user content must not affect the synthesized prompt-cache key"
        );
    }

    #[test]
    fn prompt_cache_chat_synthesis_does_not_collapse_multisegment_static_prefix() {
        let first = synthesized_prompt_cache_key(
            json!({
                "model": "gpt-4o-mini",
                "messages": [
                    {
                        "role": "user",
                        "content": "System instructions:\nShared first segment.\n\nUnique second segment A."
                    },
                    { "role": "user", "content": "Dynamic question." }
                ]
            }),
            UpstreamFormat::OpenAiCompletion,
        );
        let second = synthesized_prompt_cache_key(
            json!({
                "model": "gpt-4o-mini",
                "messages": [
                    {
                        "role": "user",
                        "content": "System instructions:\nShared first segment.\n\nUnique second segment B."
                    },
                    { "role": "user", "content": "Dynamic question." }
                ]
            }),
            UpstreamFormat::OpenAiCompletion,
        );

        assert_ne!(
            first, second,
            "multi-segment static instruction prefixes must use the full content"
        );
    }

    #[test]
    fn prompt_cache_responses_synthesis_ignores_non_leading_static_messages() {
        let first = synthesized_prompt_cache_key(
            json!({
                "model": "gpt-4o-mini",
                "input": [
                    {
                        "type": "message",
                        "role": "system",
                        "content": [{ "type": "input_text", "text": "Stable leading system." }]
                    },
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": "Dynamic user turn." }]
                    },
                    {
                        "type": "message",
                        "role": "developer",
                        "content": [{ "type": "input_text", "text": "Non-leading developer A." }]
                    }
                ]
            }),
            UpstreamFormat::OpenAiResponses,
        );
        let second = synthesized_prompt_cache_key(
            json!({
                "model": "gpt-4o-mini",
                "input": [
                    {
                        "type": "message",
                        "role": "system",
                        "content": [{ "type": "input_text", "text": "Stable leading system." }]
                    },
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": "Dynamic user turn." }]
                    },
                    {
                        "type": "message",
                        "role": "developer",
                        "content": [{ "type": "input_text", "text": "Non-leading developer B." }]
                    }
                ]
            }),
            UpstreamFormat::OpenAiResponses,
        );

        assert_eq!(
            first, second,
            "non-leading system/developer input items are dynamic tail and must not enter the digest"
        );
    }

    #[test]
    fn prompt_cache_synthesis_respects_explicit_key_and_provider_state_controls() {
        let mut explicit = json!({
            "model": "gpt-4o-mini",
            "messages": [{ "role": "system", "content": "Stable." }],
            "prompt_cache_key": "caller-explicit-key"
        });
        assert!(
            synthesize_openai_family_prompt_cache_key(
                prompt_cache_synthesis_context(UpstreamFormat::OpenAiCompletion),
                &mut explicit,
            )
            .is_none()
        );
        assert_eq!(explicit["prompt_cache_key"], "caller-explicit-key");

        for field in ["previous_response_id", "conversation", "prompt"] {
            let mut body = json!({
                "model": "gpt-4o-mini",
                "input": [
                    {
                        "type": "message",
                        "role": "system",
                        "content": [{ "type": "input_text", "text": "Stable." }]
                    },
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": "Hi" }]
                    }
                ]
            });
            body.as_object_mut()
                .expect("object")
                .insert(field.to_string(), json!("provider-state"));

            assert!(
                synthesize_openai_family_prompt_cache_key(
                    prompt_cache_synthesis_context(UpstreamFormat::OpenAiResponses),
                    &mut body,
                )
                .is_none(),
                "{field} must disable prompt-cache key synthesis"
            );
            assert!(
                body.get("prompt_cache_key").is_none(),
                "{field} request must not be rewritten with a synthesized key"
            );
        }
    }

    #[test]
    fn provider_prompt_cache_analysis_orders_mapped_before_dropped_and_projects_mixed_to_explicit_mapping()
     {
        let body = json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "prompt_cache_key": "stable-prefix",
            "prompt_cache_retention": "24h",
            "extra_body": {
                "anthropic": {
                    "cache_control": { "type": "ephemeral", "ttl": "5m" }
                }
            }
        });

        let analysis = analyze_provider_prompt_cache_request_control(
            crate::formats::UpstreamFormat::OpenAiCompletion,
            crate::formats::UpstreamFormat::Anthropic,
            &body,
        );

        assert_eq!(
            analysis.coarse_control(),
            crate::request_processing::PromptCacheRequestControl::ExplicitExtensionMapped
        );
        let components = analysis.components();
        assert_eq!(components.len(), 2);
        assert_eq!(
            components[0].disposition(),
            crate::request_processing::PromptCacheRequestControl::ExplicitExtensionMapped
        );
        assert_eq!(
            components[0].source_fields(),
            &["extra_body.anthropic.cache_control"]
        );
        assert_eq!(
            components[1].disposition(),
            crate::request_processing::PromptCacheRequestControl::Dropped
        );
        assert_eq!(
            components[1].source_fields(),
            &["prompt_cache_key", "prompt_cache_retention"]
        );
    }

    #[test]
    fn lightweight_prompt_cache_classifier_matches_full_analysis_projection() {
        let cases = [
            (
                "openai_to_anthropic_mixed",
                crate::formats::UpstreamFormat::OpenAiCompletion,
                crate::formats::UpstreamFormat::Anthropic,
                json!({
                    "messages": [{ "role": "user", "content": "Hi" }],
                    "prompt_cache_key": "stable-prefix",
                    "prompt_cache_retention": "24h",
                    "extra_body": {
                        "anthropic": {
                            "cache_control": { "type": "ephemeral", "ttl": "5m" }
                        }
                    }
                }),
                PromptCacheRequestControl::ExplicitExtensionMapped,
            ),
            (
                "openai_to_anthropic_mapped_only",
                crate::formats::UpstreamFormat::OpenAiResponses,
                crate::formats::UpstreamFormat::Anthropic,
                json!({
                    "input": "Hi",
                    "extra_body": {
                        "anthropic": {
                            "cache_control": { "type": "ephemeral" }
                        }
                    }
                }),
                PromptCacheRequestControl::ExplicitExtensionMapped,
            ),
            (
                "openai_to_anthropic_dropped_only",
                crate::formats::UpstreamFormat::OpenAiCompletion,
                crate::formats::UpstreamFormat::Anthropic,
                json!({
                    "messages": [{ "role": "user", "content": "Hi" }],
                    "prompt_cache_key": "stable-prefix",
                    "prompt_cache_retention": "24h"
                }),
                PromptCacheRequestControl::Dropped,
            ),
            (
                "anthropic_to_openai_mixed",
                crate::formats::UpstreamFormat::Anthropic,
                crate::formats::UpstreamFormat::OpenAiCompletion,
                json!({
                    "messages": [
                        {
                            "role": "user",
                            "content": [
                                {
                                    "type": "text",
                                    "text": "Hi",
                                    "cache_control": { "type": "ephemeral", "ttl": "5m" }
                                }
                            ]
                        }
                    ],
                    "extra_body": {
                        "openai": {
                            "prompt_cache_key": "stable-prefix",
                            "prompt_cache_retention": "24h"
                        }
                    }
                }),
                PromptCacheRequestControl::ExplicitExtensionMapped,
            ),
            (
                "anthropic_to_openai_mapped_only",
                crate::formats::UpstreamFormat::Anthropic,
                crate::formats::UpstreamFormat::OpenAiResponses,
                json!({
                    "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Hi" }] }],
                    "extra_body": {
                        "openai": {
                            "prompt_cache_key": "stable-prefix",
                            "prompt_cache_retention": "24h"
                        }
                    }
                }),
                PromptCacheRequestControl::ExplicitExtensionMapped,
            ),
            (
                "anthropic_to_openai_dropped_only",
                crate::formats::UpstreamFormat::Anthropic,
                crate::formats::UpstreamFormat::OpenAiCompletion,
                json!({
                    "system": [
                        {
                            "type": "text",
                            "text": "System",
                            "cache_control": { "type": "ephemeral" }
                        }
                    ],
                    "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Hi" }] }]
                }),
                PromptCacheRequestControl::Dropped,
            ),
            (
                "openai_same_family_preserved",
                crate::formats::UpstreamFormat::OpenAiCompletion,
                crate::formats::UpstreamFormat::OpenAiResponses,
                json!({
                    "messages": [{ "role": "user", "content": "Hi" }],
                    "prompt_cache_key": "stable-prefix"
                }),
                PromptCacheRequestControl::Preserved,
            ),
            (
                "anthropic_same_protocol_preserved",
                crate::formats::UpstreamFormat::Anthropic,
                crate::formats::UpstreamFormat::Anthropic,
                json!({
                    "cache_control": { "type": "ephemeral" },
                    "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Hi" }] }]
                }),
                PromptCacheRequestControl::Preserved,
            ),
            (
                "anthropic_to_openai_retention_without_key",
                crate::formats::UpstreamFormat::Anthropic,
                crate::formats::UpstreamFormat::OpenAiCompletion,
                json!({
                    "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Hi" }] }],
                    "extra_body": {
                        "openai": {
                            "prompt_cache_retention": "24h"
                        }
                    }
                }),
                PromptCacheRequestControl::None,
            ),
        ];

        for (name, client_format, upstream_format, body, expected) in cases {
            let full_projection = analyze_provider_prompt_cache_request_control(
                client_format,
                upstream_format,
                &body,
            )
            .coarse_control();
            let lightweight_projection = classify_provider_prompt_cache_request_control(
                client_format,
                upstream_format,
                &body,
            );

            assert_eq!(full_projection, expected, "full analysis case {name}");
            assert_eq!(
                lightweight_projection, full_projection,
                "lightweight classifier case {name}"
            );
        }
    }
}
