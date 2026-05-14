use std::sync::Arc;
use shared::CliConfig;
use crate::state::store::Store;
use crate::tui::scroll_area::Message;
use crate::conversation::system_prompt::SystemPrompt;

#[derive(Clone)]
pub struct ConversationEngine {
    pub store: Arc<Store>,
    pub config: CliConfig,
}

impl ConversationEngine {
    pub fn new(store: Arc<Store>, config: CliConfig) -> Self {
        Self { store, config }
    }

    pub async fn process_prompt(
        &self,
        user_input: String,
        system_prompt: &SystemPrompt,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // Set streaming state
        self.store.set_state(|s| {
            s.is_streaming = true;
        });

        // Build the OpenRouter-compatible request body
        let system_content = system_prompt.render();

        // Convert conversation history into OpenAI chat message format
        // Note: OpenRouter uses the same format as the OpenAI Chat Completions API.
        let mut messages: Vec<serde_json::Value> = Vec::new();

        // System prompt as the first message
        messages.push(serde_json::json!({
            "role": "system",
            "content": system_content
        }));

        // Add conversation history from the store
        let history = self.store.get_state().messages;
        for msg in &history {
            match msg {
                Message::User(s) => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": s
                    }));
                }
                Message::Assistant(s) => {
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": s
                    }));
                }
                Message::ToolCall { name, input, result } => {
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": format!("Tool [{name}]: input={input}"),
                        "tool_call": { "name": name, "input": input }
                    }));
                    if let Some(r) = result {
                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_name": name,
                            "content": r
                        }));
                    }
                }
                Message::System(s) => {
                    messages.push(serde_json::json!({
                        "role": "system",
                        "content": s
                    }));
                }
                Message::Thinking => {}
            }
        }

        // Append the current user input
        messages.push(serde_json::json!({
            "role": "user",
            "content": user_input
        }));

        // Add thinking indicator to scroll area
        self.store.set_state(|s| {
            s.messages.push(Message::Thinking);
        });

        // Build the request payload
        let api_key = self.config.openrouter_api_key.as_ref()
            .ok_or("no OpenRouter API key configured")?;

        let model = self.config.model.clone();

        let request_body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": 4096
        });

        // Send request to OpenRouter's OpenAI-compatible chat completions endpoint
        let client = reqwest::Client::new();
        let response = client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let status = response.status();
        let body = response.text().await
            .map_err(|e| format!("Failed to read response body: {e}"))?;

        if !status.is_success() {
            return Err(format!("OpenRouter API error ({}): {}", status, body).into());
        }

        // Parse the response JSON
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        let response_text = parsed["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| "unexpected response format from OpenRouter".to_string())?
            .to_string();

        // Update state with the response
        let response_text_clone = response_text.clone();
        self.store.set_state(|s| {
            // Remove thinking indicator
            s.messages.retain(|m| !matches!(m, Message::Thinking));
            // Add user message (if not already added) and assistant response
            s.messages.push(Message::Assistant(response_text_clone));
            s.is_streaming = false;

            // Check if compaction is needed
            if s.messages.len() > 100 {
                s.should_compact = true;
            }
        });

        // Run compaction if needed
        if self.store.get_state().should_compact {
            let msgs = self.store.get_state().messages;
            let compacted = crate::conversation::compaction::compact_messages(&msgs, 80_000);
            self.store.set_state(|s| {
                s.messages = compacted;
                s.should_compact = false;
            });
        }

        Ok(response_text)
    }
}