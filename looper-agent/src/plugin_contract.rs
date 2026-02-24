use std::sync::OnceLock;

use serde::Deserialize;

/// Parsed deterministic route details from a plugin percept signal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeterministicPluginRoute {
    /// Fully-qualified actuator name to execute.
    pub actuator_name: String,
    /// Optional chat message passed into the action payload.
    pub action_message: String,
}

#[derive(Debug, Deserialize)]
struct RouteContractSchema {
    required: Vec<String>,
    properties: RouteContractProperties,
}

#[derive(Debug, Deserialize)]
struct RouteContractProperties {
    looper_signal: RouteContractSignalField,
}

#[derive(Debug, Deserialize)]
struct RouteContractSignalField {
    #[serde(rename = "const")]
    constant: String,
}

/// Parses a plugin percept into a deterministic route when it matches contract v1.
pub fn parse_plugin_route_signal(raw: &str) -> Option<DeterministicPluginRoute> {
    let schema = route_contract_schema();
    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let object = value.as_object()?;

    if !schema
        .required
        .iter()
        .all(|field| object.contains_key(field))
    {
        return None;
    }

    if object.get("looper_signal")?.as_str()? != schema.properties.looper_signal.constant {
        return None;
    }

    let actuator_name = object
        .get("route_to_actuator")?
        .as_str()?
        .trim()
        .to_string();
    if actuator_name.is_empty() {
        return None;
    }

    let action_message = object
        .get("action_message")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("plugin deterministic action")
        .to_string();

    Some(DeterministicPluginRoute {
        actuator_name,
        action_message,
    })
}

fn route_contract_schema() -> &'static RouteContractSchema {
    static SCHEMA: OnceLock<RouteContractSchema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../looper-plugins/contracts/plugin-route-v1.json"
        ))
        .expect("plugin route contract schema should be valid JSON")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_plugin_route_signal() {
        let raw = r#"{
            "looper_signal": "plugin_route_v1",
            "event": "new_risky_commit",
            "route_to_actuator": "git_commit_guard:desktop_notify_secrets",
            "action_message": "Notify desktop"
        }"#;

        let parsed = parse_plugin_route_signal(raw).expect("signal should parse");
        assert_eq!(
            parsed.actuator_name,
            "git_commit_guard:desktop_notify_secrets"
        );
        assert_eq!(parsed.action_message, "Notify desktop");
    }

    #[test]
    fn rejects_wrong_signal_literal() {
        let raw = r#"{
            "looper_signal": "wrong",
            "event": "new_risky_commit",
            "route_to_actuator": "git_commit_guard:desktop_notify_secrets"
        }"#;

        assert!(parse_plugin_route_signal(raw).is_none());
    }
}
