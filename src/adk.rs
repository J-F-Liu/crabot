use std::sync::Arc;

use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, Tool, ToolCall, ToolResponse};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use tokio::runtime::Runtime;

use crate::chat::DisplayMessage;
use crate::tools::DevTool;

/// Max agent loop iterations to prevent infinite tool-calling cycles.
const MAX_ITERATIONS: usize = 25;

/// Configuration for a send request to the LLM.
pub struct SendConfig {
    pub base_url: String,
    pub api_type: String,
    pub api_key: String,
    pub model_id: String,
    pub workspace: std::path::PathBuf,
    pub system_prompt: String,
    pub user_prompt: String,
    pub tools: Vec<Tool>,
}

/// Result of one LLM turn — both genai messages (for next-turn history)
/// and app-level messages (for UI display).
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// GenAI messages from this turn — append to session history.
    pub genai_messages: Vec<ChatMessage>,
    /// Application messages for chat display.
    pub app_messages: Vec<DisplayMessage>,
}

/// Send a request to the LLM, with tool execution loop.
///
/// `history` is the raw genai message history from previous turns
/// (stored in `Session::history`). It is used directly — no
/// reconstruction from app messages.
///
/// Returns both the new genai messages (for next-turn context) and
/// app-level messages (for UI display).
pub fn send(config: SendConfig, history: Vec<ChatMessage>) -> Result<TurnResult, String> {
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

    rt.block_on(async {
        let client = build_client(&base_url, &api_key, &api_type);

        let mut app_messages: Vec<DisplayMessage> = Vec::new();
        let mut genai_messages: Vec<ChatMessage> = Vec::new();

        // Build ChatRequest from genai history directly.
        let mut chat_req = ChatRequest::default().with_system(system_prompt);
        for msg in &history {
            chat_req = chat_req.append_message(msg.clone());
        }

        // Add the new user message.
        let user_msg = ChatMessage::user(&user_prompt);
        chat_req = chat_req.append_message(user_msg.clone());
        genai_messages.push(user_msg);

        // Chat options: normalize reasoning content for Anthropic/DeepSeek style thinking.
        let chat_options = ChatOptions::default().with_normalize_reasoning_content(true);

        // Agent loop: keep calling the LLM until it responds without tool calls.
        for _ in 0..MAX_ITERATIONS {
            let req = chat_req.clone().with_tools(tools.clone());
            let chat_res = client
                .exec_chat(&model_id, req, Some(&chat_options))
                .await
                .map_err(|e| format!("exec_chat: {e}"))?;

            let text = chat_res.content.first_text().unwrap_or("").to_string();
            let reasoning = chat_res.reasoning_content.clone();

            app_messages.push(DisplayMessage::assistant(text.clone(), reasoning.clone()));

            // Build the full genai assistant message from the raw response content.
            // This preserves text, tool calls, and reasoning — the exact message
            // the LLM sent (needed for correct multi-turn context).
            let mut assistant_msg = ChatMessage::assistant(chat_res.content.clone());
            if let Some(ref rc) = reasoning {
                assistant_msg = assistant_msg.with_reasoning_content(Some(rc.clone()));
            }

            // Extract tool calls from response.
            let tool_calls: Vec<ToolCall> =
                chat_res.content.tool_calls().into_iter().cloned().collect();

            // Always push the assistant message — even the final response
            // must be in history for correct multi-turn context.
            chat_req = chat_req.append_message(assistant_msg.clone());
            genai_messages.push(assistant_msg);

            if tool_calls.is_empty() {
                // Final assistant response — no more tool calls.
                break;
            }

            // Execute each tool call and record results.
            let mut tool_responses: Vec<ToolResponse> = Vec::new();
            for tc in &tool_calls {
                let tool = DevTool::from_name(&tc.fn_name)
                    .ok_or_else(|| format!("Unknown tool: {}", tc.fn_name))?;
                let exec_result = tool.execute(&tc.fn_arguments, &workspace);

                match exec_result {
                    Ok(output) => {
                        let resp = ToolResponse::from_tool_call(tc, output.clone());
                        tool_responses.push(resp);
                        app_messages.push(DisplayMessage::tool(
                            &tc.fn_name,
                            &tc.fn_arguments,
                            Some(tc.call_id.clone()),
                            output,
                        ));
                    }
                    Err(err) => {
                        let resp = ToolResponse::from_tool_call(tc, err.clone());
                        tool_responses.push(resp);
                        app_messages.push(DisplayMessage::tool(
                            &tc.fn_name,
                            &tc.fn_arguments,
                            Some(tc.call_id.clone()),
                            err,
                        ));
                    }
                }
            }

            // Append tool responses to the request as one genai message.
            let tool_resp_msg: ChatMessage = tool_responses.clone().into();
            chat_req = chat_req.append_message(tool_responses);
            genai_messages.push(tool_resp_msg);
        }

        Ok(TurnResult {
            genai_messages,
            app_messages,
        })
    })
}

/// Build a genai `Client` with custom auth, endpoint, and adapter kind.
fn build_client(base_url: &str, api_key: &str, api_type: &str) -> Client {
    let adapter_kind = AdapterKind::from_lower_str(api_type).unwrap_or(AdapterKind::OpenAI);
    let has_custom_endpoint = !base_url.is_empty();
    let has_custom_key = !api_key.is_empty();

    if !has_custom_endpoint && !has_custom_key {
        return Client::default();
    }

    let mut base_url = base_url.to_string();
    // Ensure trailing slash so genai's URL join appends rather than replaces
    // the last path segment (e.g. "/v1/" + "chat/completions" → "/v1/chat/completions").
    if !base_url.ends_with('/') {
        base_url.push('/');
    }
    let api_key = api_key.to_string();

    let target_resolver = ServiceTargetResolver::from_resolver_fn(
        move |target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
            let ServiceTarget {
                endpoint: default_endpoint,
                auth: default_auth,
                model,
            } = target;

            let endpoint = if has_custom_endpoint {
                Endpoint::from_owned(Arc::from(base_url.as_str()))
            } else {
                default_endpoint
            };

            let auth = if has_custom_key {
                AuthData::from_single(api_key.as_str())
            } else {
                default_auth
            };
            Ok(ServiceTarget {
                endpoint,
                auth,
                model: ModelIden::new(adapter_kind, model.model_name),
            })
        },
    );

    Client::builder()
        .with_service_target_resolver(target_resolver)
        .build()
}
