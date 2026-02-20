use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use fiddlesticks::{
    Message, ModelProvider, ModelRequest, OutputItem, ProviderBuildConfig, ProviderId, Role,
    build_provider_with_config,
};
use serde::{Deserialize, Serialize};

use crate::model::{Action, Percept, RecommendedAction};

/// Input contract for local surprise-detection model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalModelRequest {
    /// Current iteration percepts.
    pub latest_percepts: Vec<Percept>,
    /// Up to 10 previous windows of percept text.
    pub previous_windows: Vec<Vec<String>>,
}

/// Structured output from local model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalModelResponse {
    /// Indices of surprising percepts in latest_percepts.
    pub surprising_indices: Vec<usize>,
    /// Optional rationale for debugging.
    pub rationale: String,
    /// Approximate tokens used.
    pub token_usage: u64,
}

/// Input contract for frontier planning model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrontierModelRequest {
    /// Surprising percepts identified by local model.
    pub surprising_percepts: Vec<Percept>,
}

/// Structured output from frontier model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrontierModelResponse {
    /// Ordered action plan.
    pub actions: Vec<RecommendedAction>,
    /// Optional rationale for debugging.
    pub rationale: String,
    /// Approximate tokens used.
    pub token_usage: u64,
}

/// Local model interface.
pub trait LocalModel: Send + Sync {
    /// Detects surprising percepts from current iteration.
    fn detect_surprises(
        &self,
        request: LocalModelRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LocalModelResponse>> + Send + '_>>;
}

/// Frontier model interface.
pub trait FrontierModel: Send + Sync {
    /// Plans actions for surprising percepts.
    fn plan_actions(
        &self,
        request: FrontierModelRequest,
    ) -> Pin<Box<dyn Future<Output = Result<FrontierModelResponse>> + Send + '_>>;
}

/// Rule-based local model that enforces JSON contract parsing.
pub struct RuleBasedLocalModel;

impl LocalModel for RuleBasedLocalModel {
    fn detect_surprises(
        &self,
        request: LocalModelRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LocalModelResponse>> + Send + '_>> {
        Box::pin(async move {
            let mut surprising_indices = Vec::new();
            for (index, percept) in request.latest_percepts.iter().enumerate() {
                let lower = percept.content.to_lowercase();
                let has_surprise_signal = lower.contains('!')
                    || lower.contains("error")
                    || lower.contains("fail")
                    || lower.contains("urgent")
                    || lower.contains("new")
                    || lower.contains("search")
                    || lower.contains("run")
                    || lower.contains("glob")
                    || lower.contains("grep");

                if !has_surprise_signal {
                    continue;
                }

                let seen_recently = request
                    .previous_windows
                    .iter()
                    .rev()
                    .take(10)
                    .flatten()
                    .any(|seen| seen == &percept.content);

                if !seen_recently {
                    surprising_indices.push(index);
                }
            }

            let contract_json = serde_json::json!({
                "surprising_indices": surprising_indices,
                "rationale": "rule-based local model",
                "token_usage": estimate_tokens(&request.latest_percepts),
            })
            .to_string();

            parse_local_contract(&contract_json)
        })
    }
}

/// Rule-based frontier model that enforces JSON contract parsing.
pub struct RuleBasedFrontierModel;

impl FrontierModel for RuleBasedFrontierModel {
    fn plan_actions(
        &self,
        request: FrontierModelRequest,
    ) -> Pin<Box<dyn Future<Output = Result<FrontierModelResponse>> + Send + '_>> {
        Box::pin(async move {
            let mut actions = Vec::new();
            for percept in &request.surprising_percepts {
                let lower = percept.content.to_lowercase();

                if lower.contains("search") {
                    actions.push(RecommendedAction {
                        actuator_name: "web_search".to_string(),
                        action: Action::WebSearch {
                            query: percept.content.clone(),
                        },
                    });
                    continue;
                }

                if lower.contains("glob") || lower.contains("find file") {
                    actions.push(RecommendedAction {
                        actuator_name: "glob".to_string(),
                        action: Action::Glob {
                            pattern: "**/*".to_string(),
                            path: ".".to_string(),
                        },
                    });
                    continue;
                }

                if lower.contains("grep") || lower.contains("find text") {
                    actions.push(RecommendedAction {
                        actuator_name: "grep".to_string(),
                        action: Action::Grep {
                            pattern: ".".to_string(),
                            path: ".".to_string(),
                        },
                    });
                    continue;
                }

                if lower.contains("run") || lower.contains("shell") {
                    actions.push(RecommendedAction {
                        actuator_name: "shell".to_string(),
                        action: Action::Shell {
                            command: extract_shell_command(&percept.content),
                        },
                    });
                    continue;
                }

                actions.push(RecommendedAction {
                    actuator_name: "chat".to_string(),
                    action: Action::ChatResponse {
                        message: "I noticed a surprising percept and queued it for review."
                            .to_string(),
                    },
                });
            }

            let contract_json = serde_json::json!({
                "actions": actions,
                "rationale": "rule-based frontier model",
                "token_usage": estimate_tokens(&request.surprising_percepts),
            })
            .to_string();

            parse_frontier_contract(&contract_json)
        })
    }
}

/// Fiddlesticks-backed local model adapter.
pub struct FiddlesticksLocalModel {
    provider: Arc<dyn ModelProvider>,
    model: String,
}

impl FiddlesticksLocalModel {
    /// Builds a local model adapter backed by Ollama.
    pub fn from_ollama(model: impl Into<String>) -> Result<Self> {
        let provider = build_provider_with_config(
            ProviderBuildConfig::new(ProviderId::Ollama, "").with_timeout(Duration::from_secs(120)),
        )
        .map_err(|error| anyhow!("failed to build ollama provider: {error}"))?;

        Ok(Self {
            provider,
            model: model.into(),
        })
    }
}

impl LocalModel for FiddlesticksLocalModel {
    fn detect_surprises(
        &self,
        request: LocalModelRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LocalModelResponse>> + Send + '_>> {
        Box::pin(async move {
            let payload = serde_json::to_string(&request)?;
            let instruction = "You are the local surprise detector in a sensory loop. Return only strict JSON with this shape: {\"surprising_indices\": number[], \"rationale\": string, \"token_usage\": number}. surprising_indices must reference indices from latest_percepts.";
            let response =
                complete_json(&*self.provider, &self.model, instruction, &payload, 512).await?;
            parse_local_contract(&response)
        })
    }
}

/// Fiddlesticks-backed frontier model adapter.
pub struct FiddlesticksFrontierModel {
    provider: Arc<dyn ModelProvider>,
    model: String,
}

impl FiddlesticksFrontierModel {
    /// Builds a frontier model adapter backed by Ollama.
    pub fn from_ollama(model: impl Into<String>) -> Result<Self> {
        let provider = build_provider_with_config(
            ProviderBuildConfig::new(ProviderId::Ollama, "").with_timeout(Duration::from_secs(180)),
        )
        .map_err(|error| anyhow!("failed to build ollama provider: {error}"))?;

        Ok(Self {
            provider,
            model: model.into(),
        })
    }
}

impl FrontierModel for FiddlesticksFrontierModel {
    fn plan_actions(
        &self,
        request: FrontierModelRequest,
    ) -> Pin<Box<dyn Future<Output = Result<FrontierModelResponse>> + Send + '_>> {
        Box::pin(async move {
            let payload = serde_json::to_string(&request)?;
            let instruction = "You are the frontier planner in a sensory loop. Return only strict JSON with this shape: {\"actions\": [{\"actuator_name\": string, \"action\": object}], \"rationale\": string, \"token_usage\": number}. action must match one of: ChatResponse, Grep, Glob, Shell, WebSearch enum representations.";
            let response =
                complete_json(&*self.provider, &self.model, instruction, &payload, 1024).await?;
            parse_frontier_contract(&response)
        })
    }
}

async fn complete_json(
    provider: &dyn ModelProvider,
    model: &str,
    system_instruction: &str,
    input_payload: &str,
    max_tokens: u32,
) -> Result<String> {
    let request = ModelRequest::builder(model)
        .message(Message::new(Role::System, system_instruction))
        .message(Message::new(Role::User, input_payload))
        .temperature(0.0)
        .max_tokens(max_tokens)
        .build()
        .map_err(|error| anyhow!("failed to build model request: {error}"))?;

    let response = provider
        .complete(request)
        .await
        .map_err(|error| anyhow!("model completion failed: {error}"))?;

    let mut merged = String::new();
    for item in response.output {
        if let OutputItem::Message(message) = item {
            merged.push_str(&message.content);
        }
    }

    if merged.trim().is_empty() {
        return Err(anyhow!("model completion returned empty response"));
    }

    Ok(merged)
}

fn parse_local_contract(raw: &str) -> Result<LocalModelResponse> {
    serde_json::from_str(raw)
        .map_err(|error| anyhow!("failed to parse local model contract: {error}"))
}

fn parse_frontier_contract(raw: &str) -> Result<FrontierModelResponse> {
    serde_json::from_str(raw)
        .map_err(|error| anyhow!("failed to parse frontier model contract: {error}"))
}

fn extract_shell_command(message: &str) -> String {
    let lower = message.to_lowercase();
    if let Some((_, right)) = lower.split_once("run ") {
        let offset = message.len().saturating_sub(right.len());
        return message[offset..].trim().to_string();
    }

    if let Some((_, right)) = lower.split_once("shell ") {
        let offset = message.len().saturating_sub(right.len());
        return message[offset..].trim().to_string();
    }

    message.trim().to_string()
}

fn estimate_tokens(percepts: &[Percept]) -> u64 {
    percepts
        .iter()
        .map(|percept| (percept.content.split_whitespace().count() as u64) + 4)
        .sum()
}
