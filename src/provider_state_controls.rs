use serde_json::Value;

pub(crate) fn provider_state_control_enabled(value: Option<&Value>) -> bool {
    !matches!(value, None | Some(Value::Null) | Some(Value::Bool(false)))
}
