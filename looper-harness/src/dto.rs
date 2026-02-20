use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::model::{Actuator, McpDetails, SafetyPolicy, Sensor, WorkflowDetails};

/// API request body for creating a sensor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SensorCreateRequest {
    /// Sensor name.
    pub name: String,
    /// Description of the percept stream.
    pub description: String,
}

impl SensorCreateRequest {
    /// Converts the request into a runtime sensor.
    pub fn into_sensor(self) -> Sensor {
        Sensor::new(self.name, self.description)
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
        }
    }
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
