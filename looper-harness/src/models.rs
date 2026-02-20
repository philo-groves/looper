use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use fiddlesticks::{
    Message, ModelProvider, ModelRequest, OutputItem, ProviderBuildConfig, ProviderId, Role,
    build_provider_with_config,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::model::{Action, ModelProviderKind, Percept, RecommendedAction};

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
    /// Builds a local model adapter backed by a configured provider.
    pub fn from_provider(
        provider_kind: ModelProviderKind,
        model: impl Into<String>,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let provider = build_provider(provider_kind, api_key, Duration::from_secs(120))?;

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
    /// Builds a frontier model adapter backed by a configured provider.
    pub fn from_provider(
        provider_kind: ModelProviderKind,
        model: impl Into<String>,
        api_key: Option<&str>,
    ) -> Result<Self> {
        let provider = build_provider(provider_kind, api_key, Duration::from_secs(180))?;

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
            match parse_frontier_contract(&response) {
                Ok(parsed) => Ok(parsed),
                Err(_) => Ok(frontier_fallback_from_plain_text(&response)),
            }
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
    parse_json_contract(raw, "local")
}

fn parse_frontier_contract(raw: &str) -> Result<FrontierModelResponse> {
    if let Ok(parsed) = parse_json_contract(raw, "frontier") {
        return Ok(parsed);
    }

    parse_frontier_contract_lenient(raw)
}

fn parse_frontier_contract_lenient(raw: &str) -> Result<FrontierModelResponse> {
    let trimmed = raw.trim();
    let parsed_value = if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        value
    } else if let Some(payload) = parse_json_from_embedded_payload::<serde_json::Value>(trimmed) {
        payload
    } else {
        return Err(anyhow!(
            "failed to parse frontier model contract: expected valid JSON object in model output"
        ));
    };

    let object = parsed_value
        .as_object()
        .ok_or_else(|| anyhow!("failed to parse frontier model contract: expected JSON object"))?;

    let actions = object
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut normalized_actions = Vec::new();
    for item in actions {
        let Some(action_obj) = item.as_object() else {
            continue;
        };

        let actuator_name = action_obj
            .get("actuator_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("chat")
            .to_string();

        let action = action_obj
            .get("action")
            .and_then(parse_lenient_action)
            .unwrap_or(Action::ChatResponse {
                message: "I noticed a surprising percept and queued it for review.".to_string(),
            });

        normalized_actions.push(RecommendedAction {
            actuator_name,
            action,
        });
    }

    let rationale = object
        .get("rationale")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("frontier model lenient contract parse")
        .to_string();

    let token_usage = object
        .get("token_usage")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    Ok(FrontierModelResponse {
        actions: normalized_actions,
        rationale,
        token_usage,
    })
}

fn parse_lenient_action(value: &serde_json::Value) -> Option<Action> {
    let obj = value.as_object()?;

    if let Some(kind) = obj.get("type").and_then(serde_json::Value::as_str) {
        return match kind {
            "ChatResponse" => {
                let message = obj
                    .get("message")
                    .or_else(|| obj.get("content"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Some(Action::ChatResponse { message })
            }
            "Grep" => Some(Action::Grep {
                pattern: obj
                    .get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(".")
                    .to_string(),
                path: obj
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(".")
                    .to_string(),
            }),
            "Glob" => Some(Action::Glob {
                pattern: obj
                    .get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("**/*")
                    .to_string(),
                path: obj
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(".")
                    .to_string(),
            }),
            "Shell" => Some(Action::Shell {
                command: obj
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            "WebSearch" => Some(Action::WebSearch {
                query: obj
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            _ => None,
        };
    }

    if let Some(chat_value) = obj.get("ChatResponse") {
        let message = chat_value
            .get("message")
            .or_else(|| chat_value.get("content"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Some(Action::ChatResponse { message });
    }

    None
}

fn parse_json_contract<T>(raw: &str, contract_name: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let trimmed = raw.trim();
    if let Ok(parsed) = serde_json::from_str::<T>(trimmed) {
        return Ok(parsed);
    }

    if let Some(parsed) = parse_json_from_embedded_payload::<T>(trimmed) {
        return Ok(parsed);
    }

    Err(anyhow!(
        "failed to parse {contract_name} model contract: expected valid JSON object in model output"
    ))
}

fn parse_json_from_embedded_payload<T>(raw: &str) -> Option<T>
where
    T: DeserializeOwned,
{
    for (start, ch) in raw.char_indices() {
        if ch != '{' && ch != '[' {
            continue;
        }

        let Some(candidate) = extract_json_value(raw, start) else {
            continue;
        };
        if let Ok(parsed) = serde_json::from_str::<T>(candidate) {
            return Some(parsed);
        }
    }

    None
}

fn extract_json_value(input: &str, start: usize) -> Option<&str> {
    let mut in_string = false;
    let mut escaping = false;
    let mut stack = Vec::new();

    for (offset, ch) in input[start..].char_indices() {
        if in_string {
            if escaping {
                escaping = false;
                continue;
            }

            if ch == '\\' {
                escaping = true;
                continue;
            }

            if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                if stack.pop() != Some(ch) {
                    return None;
                }

                if stack.is_empty() {
                    let end = start + offset + ch.len_utf8();
                    return Some(&input[start..end]);
                }
            }
            _ => {}
        }
    }

    None
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

fn frontier_fallback_from_plain_text(raw: &str) -> FrontierModelResponse {
    let message = raw.trim();
    let content = if message.is_empty() {
        "I received your message and am ready to help.".to_string()
    } else {
        message.to_string()
    };

    FrontierModelResponse {
        actions: vec![RecommendedAction {
            actuator_name: "chat".to_string(),
            action: Action::ChatResponse { message: content },
        }],
        rationale: "frontier model returned non-JSON output; used chat fallback".to_string(),
        token_usage: (raw.split_whitespace().count() as u64).saturating_add(4),
    }
}

fn build_provider(
    provider_kind: ModelProviderKind,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<Arc<dyn ModelProvider>> {
    let provider_id = match provider_kind {
        ModelProviderKind::Ollama => ProviderId::Ollama,
        ModelProviderKind::OpenAi => ProviderId::OpenAi,
        ModelProviderKind::OpenCodeZen => ProviderId::OpenCodeZen,
    };

    let key = api_key.unwrap_or_default();
    build_provider_with_config(ProviderBuildConfig::new(provider_id, key).with_timeout(timeout))
        .map_err(|error| anyhow!("failed to build {provider_kind:?} provider: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_local_contract_accepts_markdown_fence() {
        let raw =
            "```json\n{\"surprising_indices\":[0],\"rationale\":\"ok\",\"token_usage\":12}\n```";
        let parsed = parse_local_contract(raw).expect("fenced payload should parse");
        assert_eq!(parsed.surprising_indices, vec![0]);
        assert_eq!(parsed.rationale, "ok");
        assert_eq!(parsed.token_usage, 12);
    }

    #[test]
    fn parse_frontier_contract_accepts_prefixed_text() {
        let raw = "Sure, here is the JSON response:\n{\"actions\":[],\"rationale\":\"none\",\"token_usage\":3}";
        let parsed = parse_frontier_contract(raw).expect("prefixed json should parse");
        assert!(parsed.actions.is_empty());
        assert_eq!(parsed.rationale, "none");
        assert_eq!(parsed.token_usage, 3);
    }

    #[test]
    fn frontier_fallback_wraps_plain_text_as_chat_action() {
        let raw = "Sure thing - I can help with that.";
        let parsed = frontier_fallback_from_plain_text(raw);
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(
            parsed.rationale,
            "frontier model returned non-JSON output; used chat fallback"
        );
        match &parsed.actions[0].action {
            Action::ChatResponse { message } => assert_eq!(message, raw),
            _ => panic!("expected chat response fallback action"),
        }
    }

    #[test]
    fn parse_frontier_contract_accepts_type_content_action_shape() {
        let raw = r#"{
            "actions": [
                {
                    "actuator_name": "chat",
                    "action": {
                        "type": "ChatResponse",
                        "content": "Hello. What would you like to do?"
                    }
                }
            ],
            "rationale": "ok",
            "token_usage": 11
        }"#;

        let parsed = parse_frontier_contract(raw).expect("lenient action shape should parse");
        assert_eq!(parsed.actions.len(), 1);
        match &parsed.actions[0].action {
            Action::ChatResponse { message } => {
                assert_eq!(message, "Hello. What would you like to do?")
            }
            _ => panic!("expected chat response action"),
        }
    }
}
