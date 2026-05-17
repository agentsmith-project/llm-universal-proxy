use serde::Serialize;
use serde_json::Value;

use crate::formats::UpstreamFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCacheRequestControl {
    None,
    #[serde(rename = "preserved_native")]
    Preserved,
    ExplicitExtensionMapped,
    Dropped,
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
    fn provider_prompt_cache_analysis_orders_mapped_before_dropped_and_projects_mixed_to_explicit_mapping(
    ) {
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
