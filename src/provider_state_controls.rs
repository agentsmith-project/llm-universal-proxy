use serde_json::Value;

pub(crate) fn provider_state_control_enabled(value: Option<&Value>) -> bool {
    !matches!(value, None | Some(Value::Null) | Some(Value::Bool(false)))
}

pub(crate) fn responses_stateful_request_controls(body: &Value) -> Vec<&'static str> {
    let mut controls = Vec::new();
    if body.get("previous_response_id").is_some() {
        controls.push("previous_response_id");
    }
    if body.get("conversation").is_some() {
        controls.push("conversation");
    }
    if provider_state_control_enabled(body.get("background")) {
        controls.push("background");
    }
    if provider_state_control_enabled(body.get("store")) {
        controls.push("store");
    }
    if body.get("prompt").is_some() {
        controls.push("prompt");
    }
    if body.get("context_management").is_some() {
        controls.push("context_management");
    }
    controls
}

pub(crate) fn responses_stateful_request_controls_present(body: &Value) -> bool {
    !responses_stateful_request_controls(body).is_empty()
}
