use std::sync::Arc;

use adk_rust::futures::StreamExt;
use adk_rust::{
    Content, Llm, LlmRequest,
    model::{
        GeminiModel,
        anthropic::{AnthropicClient, AnthropicConfig},
        deepseek::{DeepSeekClient, DeepSeekConfig},
        groq::{GroqClient, GroqConfig},
        ollama::{OllamaConfig, OllamaModel},
        openai::{OpenAIClient, OpenAIConfig},
        openrouter::{OpenRouterClient, OpenRouterConfig},
    },
};
use tokio::runtime::Runtime;

use crate::user::ChatMessage;

/// Send a request to the LLM and return a ChatMessage with role, content, and reasoning.
pub fn send(
    base_url: String,
    api_type: String,
    api_key: String,
    model_id: String,
    system_prompt: String,
    user_input: String,
) -> Result<ChatMessage, String> {
    let rt = Runtime::new().map_err(|e| format!("tokio runtime: {e}"))?;
    rt.block_on(async {
        let model: Arc<dyn Llm> = build_model(&base_url, &api_type, &api_key, &model_id)?;

        let contents = vec![
            Content::new("system").with_text(&system_prompt),
            Content::new("user").with_text(&user_input),
        ];
        let request = LlmRequest::new(&model_id, contents);

        let mut stream = model
            .generate_content(request, true)
            .await
            .map_err(|e| format!("generate_content: {e}"))?;

        let mut result = String::new();
        let mut reasoning = String::new();
        let mut role: Option<String> = None;
        while let Some(item) = stream.next().await {
            let item = item.map_err(|e| format!("stream: {e}"))?;
            if let Some(content) = item.content {
                if role.is_none() && !content.role.is_empty() {
                    role = Some(content.role);
                }
                for part in &content.parts {
                    if let Some(text) = part.text() {
                        result.push_str(text);
                    }
                    if let Some(think) = part.thinking_text() {
                        reasoning.push_str(think);
                    }
                }
            }
        }

        let reasoning = (!reasoning.is_empty()).then_some(reasoning);
        Ok(ChatMessage {
            role: role.unwrap_or_else(|| "Assistant".into()),
            content: result,
            reasoning,
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        })
    })
}

fn build_model(
    base_url: &str,
    api_type: &str,
    api_key: &str,
    model_id: &str,
) -> Result<Arc<dyn Llm>, String> {
    let base_url = (!base_url.is_empty()).then(|| base_url.to_owned());
    match api_type {
        "gemini" => Ok(Arc::new(
            GeminiModel::new(api_key, model_id).map_err(|e| format!("gemini: {e}"))?,
        )),
        "anthropic" => {
            let config = AnthropicConfig {
                base_url: base_url.clone(),
                ..AnthropicConfig::new(api_key, model_id)
            };
            Ok(Arc::new(
                AnthropicClient::new(config).map_err(|e| format!("anthropic: {e}"))?,
            ))
        }
        "openrouter" => {
            let mut config = OpenRouterConfig::new(api_key, model_id);
            if let Some(ref url) = base_url {
                config.base_url.clone_from(url);
            }
            Ok(Arc::new(
                OpenRouterClient::new(config).map_err(|e| format!("openrouter: {e}"))?,
            ))
        }
        "deepseek" => {
            let mut config = DeepSeekConfig::new(api_key, model_id);
            if let Some(ref url) = base_url {
                config = config.with_base_url(url);
            }
            Ok(Arc::new(
                DeepSeekClient::new(config).map_err(|e| format!("deepseek: {e}"))?,
            ))
        }
        "groq" => {
            let config = GroqConfig {
                base_url: base_url.clone(),
                ..GroqConfig::new(api_key, model_id)
            };
            Ok(Arc::new(
                GroqClient::new(config).map_err(|e| format!("groq: {e}"))?,
            ))
        }
        "ollama" => {
            let host = base_url.as_deref().unwrap_or("http://localhost:11434");
            let config = OllamaConfig::with_host(host, model_id);
            Ok(Arc::new(
                OllamaModel::new(config).map_err(|e| format!("ollama: {e}"))?,
            ))
        }
        _ => {
            let config = OpenAIConfig {
                base_url,
                api_key: api_key.to_owned(),
                model: model_id.to_owned(),
                ..Default::default()
            };
            Ok(Arc::new(
                OpenAIClient::new(config).map_err(|e| format!("openai: {e}"))?,
            ))
        }
    }
}
