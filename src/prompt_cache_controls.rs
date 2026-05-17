use serde_json::Value;

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
}
