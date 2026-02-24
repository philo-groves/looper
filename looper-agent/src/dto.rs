use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::model::{
    Actuator, McpDetails, PluginActuatorDetails, SafetyPolicy, Sensor, SensorIngressConfig,
    SensorRestFormat, WorkflowDetails,
};

/// Sensor ingress options accepted by REST API.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SensorIngressCreateRequest {
    /// Sensor receives percepts via watched directory files.
    Directory { path: String },
    /// Sensor receives percepts via REST API.
    RestApi { format: SensorRestFormat },
}

/// API request body for creating a sensor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SensorCreateRequest {
    /// Sensor name.
    pub name: String,
    /// Description of the percept stream.
    pub description: String,
    /// Optional sensitivity score from 0 to 100.
    #[serde(default)]
    pub sensitivity_score: Option<u8>,
    /// Optional ingress configuration for non-internal sensors.
    #[serde(default)]
    pub ingress: Option<SensorIngressCreateRequest>,
}

impl SensorCreateRequest {
    /// Converts the request into a runtime sensor.
    pub fn into_sensor(self) -> Sensor {
        let mut sensor = Sensor::with_sensitivity_score(
            self.name,
            self.description,
            self.sensitivity_score.unwrap_or(50),
        );
        sensor.ingress = match self.ingress {
            Some(SensorIngressCreateRequest::Directory { path }) => {
                SensorIngressConfig::Directory { path }
            }
            Some(SensorIngressCreateRequest::RestApi { format }) => {
                SensorIngressConfig::RestApi { format }
            }
            None => SensorIngressConfig::RestApi {
                format: SensorRestFormat::Text,
            },
        };
        sensor
    }
}

/// Supported external actuator types accepted by REST API.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActuatorRegistrationType {
    /// MCP actuator registration.
    Mcp,
    /// Agentic workflow registration.
    Workflow,
    /// Plugin actuator registration.
    Plugin,
}

/// Top-level API request body for creating an actuator.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActuatorCreateRequest {
    /// Actuator name.
    pub name: String,
    /// Description of actuator behavior.
    pub description: String,
    /// Actuator type.
    #[serde(rename = "type")]
    pub actuator_type: ActuatorRegistrationType,
    /// Type-specific details.
    pub details: serde_json::Value,
    /// Optional safety policy.
    #[serde(default)]
    pub policy: SafetyPolicy,
}

impl ActuatorCreateRequest {
    /// Converts the request into a runtime actuator.
    pub fn try_into_actuator(self) -> Result<Actuator> {
        self.policy.validate()?;

        match self.actuator_type {
            ActuatorRegistrationType::Mcp => {
                let details: McpDetails = serde_json::from_value(self.details)?;
                Actuator::mcp(self.name, self.description, details, self.policy)
            }
            ActuatorRegistrationType::Workflow => {
                let details: WorkflowDetails = serde_json::from_value(self.details)?;
                Actuator::workflow(self.name, self.description, details, self.policy)
            }
            ActuatorRegistrationType::Plugin => {
                let details: PluginActuatorDetails = serde_json::from_value(self.details)?;
                Actuator::plugin(self.name, self.description, details, self.policy)
            }
        }
    }
}

/// API request body for importing a plugin package.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginImportRequest {
    /// Directory containing `looper-plugin.json`.
    pub path: String,
}

#[cfg(test)]
mod tests {
    use crate::model::ActuatorType;

    use super::*;

    #[test]
    fn actuator_rest_payload_parses() {
        let json = r#"
        {
            "name": "bundle deploy",
            "description": "Deploys our databricks bundle",
            "type": "workflow",
            "details": {
                "name": "Bundle Validation & Deployment",
                "cells": ["step one", "%shell cargo test"]
            },
            "policy": {
                "allowlist": ["shell"],
                "require_hitl": true,
                "sandboxed": true
            }
        }
        "#;

        let request: ActuatorCreateRequest = serde_json::from_str(json).expect("json should parse");
        let actuator = request
            .try_into_actuator()
            .expect("request should convert to actuator");

        assert_eq!(actuator.name, "bundle deploy");
        assert!(matches!(actuator.kind, ActuatorType::Workflow(_)));
    }
}
