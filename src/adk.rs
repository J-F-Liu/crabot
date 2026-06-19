use std::collections::HashMap;
use std::sync::Arc;

use adk_core::types::FunctionResponseData;
use adk_rust::futures::StreamExt;
use adk_rust::{
    Content, Llm, LlmRequest, Part,
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
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::chat::{ChatMessage, MessageContent, Role};
use crate::tools::DevTool;

/// Max agent loop iterations to prevent infinite tool-calling cycles.
const MAX_ITERATIONS: usize = 25;

/// Configuration for a send request to the LLM.
pub struct SendConfig {
    pub base_url: String,
    pub api_type: String,
    pub api_key: String,
    pub model_id: String,
    pub workspace: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub tools: HashMap<String, Value>,
}

/// Send a request to the LLM, with tool execution loop.
///
/// Returns all new `ChatMessage`s from this turn (user prompt, tool calls,
/// tool results, and final assistant response).
pub fn send(config: SendConfig, history: &[ChatMessage]) -> Result<Vec<ChatMessage>, String> {
    let SendConfig {
        base_url,
        api_type,
        api_key,
        model_id,
        workspace,
        system_prompt,
        user_prompt,
        tools,
    } = config;
    let rt = Runtime::new().map_err(|e| format!("tokio runtime: {e}"))?;
    let workspace_path = std::path::PathBuf::from(&workspace);

    rt.block_on(async {
        let model: Arc<dyn Llm> = build_model(&base_url, &api_type, &api_key, &model_id)?;
        let mut results: Vec<ChatMessage> = Vec::new();

        // Build initial contents: system prompt.
        let mut contents: Vec<Content> = Vec::new();
        contents.push(Content::new("system").with_text(&system_prompt));

        // Reconstruct conversation history, merging consecutive
        // assistant + tool messages back into the original structure:
        //   model(text + fc₁ + fc₂), function(resp₁), function(resp₂), …
        // This preserves the prefix the LLM originally saw, keeping
        // server-side prompt caches warm across turns.
        let mut i = 0;
        while i < history.len() {
            let msg = &history[i];
            match msg.role {
                Role::User => {
                    if let MessageContent::Text { content, .. } = &msg.content {
                        contents.push(Content::new("user").with_text(content));
                    }
                    i += 1;
                }
                Role::Assistant => {
                    if let MessageContent::Text { content, reasoning } = &msg.content {
                        let mut model_content = Content::new("model");
                        model_content = model_content.with_text(content);
                        if let Some(reasoning) = reasoning {
                            model_content = model_content.with_thinking(reasoning);
                        }
                        i += 1;

                        // Consume all consecutive tool messages that belong
                        // to this assistant turn.
                        let mut function_responses: Vec<Content> = Vec::new();
                        while i < history.len() {
                            if let MessageContent::Tool {
                                name,
                                call_id,
                                args,
                                result,
                            } = &history[i].content
                            {
                                model_content.parts.push(Part::FunctionCall {
                                    name: name.clone(),
                                    args: serde_json::from_str(args).unwrap_or_default(),
                                    id: call_id.clone(),
                                    thought_signature: None,
                                });
                                let func_resp = FunctionResponseData::new(
                                    name.clone(),
                                    serde_json::Value::String(result.clone()),
                                );
                                let mut resp_content = Content::new("function");
                                resp_content.parts.push(Part::FunctionResponse {
                                    function_response: func_resp,
                                    id: call_id.clone(),
                                });
                                function_responses.push(resp_content);
                                i += 1;
                            } else {
                                break;
                            }
                        }

                        contents.push(model_content);
                        contents.extend(function_responses);
                    } else {
                        i += 1;
                    }
                }
                Role::Tool => {
                    // Standalone tool message — shouldn't occur in normal
                    // flow, but handled defensively.
                    if let MessageContent::Tool {
                        name,
                        call_id,
                        args,
                        result,
                    } = &msg.content
                    {
                        let mut call_content = Content::new("model");
                        call_content.parts.push(Part::FunctionCall {
                            name: name.clone(),
                            args: serde_json::from_str(args).unwrap_or_default(),
                            id: call_id.clone(),
                            thought_signature: None,
                        });
                        contents.push(call_content);

                        let func_resp = FunctionResponseData::new(
                            name.clone(),
                            serde_json::Value::String(result.clone()),
                        );
                        let mut resp_content = Content::new("function");
                        resp_content.parts.push(Part::FunctionResponse {
                            function_response: func_resp,
                            id: call_id.clone(),
                        });
                        contents.push(resp_content);
                    }
                    i += 1;
                }
            }
        }

        // Add the new user message.
        contents.push(Content::new("user").with_text(&user_prompt));

        // Agent loop: keep calling the LLM until it responds without tool calls.
        for _ in 0..MAX_ITERATIONS {
            let mut request = LlmRequest::new(&model_id, contents.clone());
            request.tools = tools.clone();

            let stream = model
                .generate_content(request, true)
                .await
                .map_err(|e| format!("generate_content: {e}"))?;

            // Collect streaming response into a single Content.
            let response_content = collect_stream(stream).await?;

            let text = response_content
                .parts
                .iter()
                .filter_map(|p| p.text())
                .collect::<Vec<_>>()
                .join("");
            let reasoning_text: String = response_content
                .parts
                .iter()
                .filter_map(|p| p.thinking_text())
                .collect();
            let reasoning = (!reasoning_text.is_empty()).then_some(reasoning_text);
            results.push(ChatMessage::assistant(text, reasoning));

            let has_function_calls = response_content
                .parts
                .iter()
                .any(|p| matches!(p, Part::FunctionCall { .. }));

            if !has_function_calls {
                // Final assistant response
                break;
            }

            // Build the assistant content (includes both text and function calls).
            contents.push(response_content.clone());

            // Record tool calls and execute them.
            let mut function_response_contents: Vec<Content> = Vec::new();
            for part in &response_content.parts {
                if let Part::FunctionCall { name, args, id, .. } = part {
                    let tool =
                        DevTool::from_name(name).ok_or_else(|| format!("Unknown tool: {name}"))?;
                    let exec_result = tool.execute(args, &workspace_path);

                    match exec_result {
                        Ok(output) => {
                            let func_resp =
                                FunctionResponseData::new(name, Value::String(output.clone()));
                            let mut fc = Content::new("function");
                            fc.parts.push(Part::FunctionResponse {
                                function_response: func_resp,
                                id: id.clone(),
                            });
                            function_response_contents.push(fc);
                            results.push(ChatMessage::tool(name, args, id.clone(), output));
                        }
                        Err(err) => {
                            let func_resp =
                                FunctionResponseData::new(name, Value::String(err.clone()));
                            let mut fc = Content::new("function");
                            fc.parts.push(Part::FunctionResponse {
                                function_response: func_resp,
                                id: id.clone(),
                            });
                            function_response_contents.push(fc);
                            results.push(ChatMessage::tool(name, args, id.clone(), err));
                        }
                    }
                }
            }

            // Append tool results to conversation.
            contents.extend(function_response_contents);
        }

        Ok(results)
    })
}

/// Collect the stream into a single Content with thinking baked in.
async fn collect_stream(mut stream: adk_rust::LlmResponseStream) -> Result<Content, String> {
    let mut role: Option<String> = None;
    let mut parts: Vec<Part> = Vec::new();
    let mut reasoning = String::new();
    let mut text_buf = String::new();

    while let Some(item) = stream.next().await {
        let item = item.map_err(|e| format!("stream: {e}"))?;
        if let Some(content) = item.content {
            if role.is_none() && !content.role.is_empty() {
                role = Some(content.role);
            }
            for part in content.parts {
                match &part {
                    Part::Text { text } if !text.trim().is_empty() => text_buf.push_str(text),
                    Part::Thinking { thinking, .. } => reasoning.push_str(thinking),
                    _ => {
                        if !text_buf.is_empty() {
                            parts.push(Part::Text {
                                text: std::mem::take(&mut text_buf),
                            });
                        }
                        parts.push(part);
                    }
                }
            }
        }
        if item.turn_complete {
            break;
        }
    }

    if !text_buf.is_empty() {
        parts.push(Part::Text {
            text: std::mem::take(&mut text_buf),
        });
    }

    let mut content = Content {
        role: role.unwrap_or_else(|| "model".into()),
        parts,
    };
    if !reasoning.is_empty() {
        content = content.with_thinking(reasoning);
    }
    Ok(content)
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
