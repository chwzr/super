//! Google Generative AI (Gemini) adapter.

use super::{PartialBuilder, apply_provider_headers, thinking_budget};
use crate::event::EventSink;
use crate::model::Model;
use crate::sse::{SseEvent, SseParser};
use crate::stream::{StreamOptions, http_client};
use crate::types::{ContentBlock, Context, Message, StopReason, UserContent};
use futures::StreamExt;
use serde_json::{Value, json};

pub async fn stream(model: &Model, context: &Context, options: &StreamOptions, sink: EventSink) {
    let mut builder = PartialBuilder::new(model, sink);
    let Some(credential) = options.credential.as_ref() else {
        builder.fail(
            format!(
                "no API key for provider {} (set GEMINI_API_KEY)",
                model.provider
            ),
            false,
            model,
        );
        return;
    };
    let api_key = credential.value().to_string();
    let body = build_request(model, context, options);
    let url = match google_url(model) {
        Ok(url) => url,
        Err(error) => {
            builder.fail(error.to_string(), false, model);
            return;
        }
    };
    let mut request = http_client()
        .post(&url)
        .header("content-type", "application/json")
        .json(&body);
    if priority_header(model, options).is_some() {
        request = request.header("X-Vertex-AI-LLM-Shared-Request-Type", "priority");
    }
    if credential.is_bearer() {
        request = request.bearer_auth(&api_key);
    } else if model.api == "google-vertex" {
        if let Some(token) = api_key.strip_prefix("vertex-oauth:") {
            request = request.bearer_auth(token);
        } else {
            request = request.header("x-goog-api-key", &api_key);
        }
    } else {
        request = request.header("x-goog-api-key", &api_key);
    }
    for (k, v) in &model.headers {
        request = request.header(k, v);
    }
    request = apply_provider_headers(request, model, context, options);

    let response = tokio::select! {
        r = request.send() => r,
        _ = options.cancel.cancelled() => { builder.fail("Request aborted", true, model); return; }
    };
    let response = match response {
        Ok(r) => r,
        Err(e) => {
            builder.fail(format!("request failed: {e}"), false, model);
            return;
        }
    };
    if !response.status().is_success() {
        builder.fail(super::http_error(response).await, false, model);
        return;
    }

    let mut parser = SseParser::new();
    let mut events: Vec<SseEvent> = Vec::new();
    let mut body_stream = response.bytes_stream();
    let mut state = DecodeState::default();
    builder.start();

    loop {
        let chunk = tokio::select! {
            c = body_stream.next() => c,
            _ = options.cancel.cancelled() => { builder.fail("Request aborted", true, model); return; }
        };
        match chunk {
            Some(Ok(bytes)) => {
                parser.feed(&bytes, &mut events);
                for event in events.drain(..) {
                    if let Err(msg) = handle_chunk(&event.data, &mut builder, &mut state) {
                        builder.fail(msg, false, model);
                        return;
                    }
                }
            }
            Some(Err(e)) => {
                builder.fail(format!("stream error: {e}"), false, model);
                return;
            }
            None => break,
        }
    }

    if let Some(idx) = state.text_idx.take() {
        builder.end_text(idx);
    }
    if let Some(idx) = state.thinking_idx.take() {
        builder.end_thinking(idx);
    }
    builder.message.raw_stop_reason = state.finish_reason.clone();
    let stop = match state.finish_reason.as_deref() {
        Some("MAX_TOKENS") => StopReason::Length,
        Some("STOP") | None => StopReason::Stop,
        Some(other) => {
            builder.fail(format!("generation stopped: {other}"), false, model);
            return;
        }
    };
    builder.finish(stop, model);
}

fn priority_header(model: &Model, options: &StreamOptions) -> Option<&'static str> {
    (options.fast_mode && model.supports_fast_mode()).then_some("priority")
}

fn google_url(model: &Model) -> anyhow::Result<String> {
    if model.api != "google-vertex" {
        return Ok(format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            model.base_url.trim_end_matches('/'),
            model.id
        ));
    }
    let project = crate::auth::provider_env("google-vertex", "GOOGLE_CLOUD_PROJECT")
        .or_else(|| crate::auth::provider_env("google-vertex", "GCLOUD_PROJECT"))
        .ok_or_else(|| anyhow::anyhow!("Google Vertex needs GOOGLE_CLOUD_PROJECT"))?;
    let location = crate::auth::provider_env("google-vertex", "GOOGLE_CLOUD_LOCATION")
        .ok_or_else(|| anyhow::anyhow!("Google Vertex needs GOOGLE_CLOUD_LOCATION"))?;
    Ok(vertex_url(model, &project, &location))
}

fn vertex_url(model: &Model, project: &str, location: &str) -> String {
    let base_url = if model.base_url.is_empty() {
        format!("https://{location}-aiplatform.googleapis.com")
    } else {
        model.base_url.replace("{location}", location)
    };
    format!(
        "{}/v1/projects/{project}/locations/{location}/publishers/google/models/{}:streamGenerateContent?alt=sse",
        base_url.trim_end_matches('/'),
        model.id
    )
}

#[derive(Default)]
struct DecodeState {
    text_idx: Option<usize>,
    thinking_idx: Option<usize>,
    finish_reason: Option<String>,
}

fn handle_chunk(
    data: &str,
    builder: &mut PartialBuilder,
    state: &mut DecodeState,
) -> Result<(), String> {
    let chunk: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    if let Some(err) = chunk.get("error") {
        return Err(err["message"]
            .as_str()
            .unwrap_or("provider error")
            .to_string());
    }
    if let Some(usage) = chunk.get("usageMetadata") {
        let cached = usage["cachedContentTokenCount"].as_u64().unwrap_or(0);
        let prompt = usage["promptTokenCount"].as_u64().unwrap_or(0);
        builder.message.usage.input = prompt.saturating_sub(cached);
        builder.message.usage.cache_read = cached;
        builder.message.usage.cache_read_available = usage.get("cachedContentTokenCount").is_some();
        let thoughts = usage["thoughtsTokenCount"].as_u64().unwrap_or(0);
        builder.message.usage.output =
            usage["candidatesTokenCount"].as_u64().unwrap_or(0) + thoughts;
        if thoughts > 0 {
            builder.message.usage.reasoning = Some(thoughts);
        }
    }
    if let Some(id) = chunk["responseId"].as_str() {
        builder.message.response_id = Some(id.to_string());
    }
    let Some(candidate) = chunk["candidates"].as_array().and_then(|c| c.first()) else {
        return Ok(());
    };
    if let Some(reason) = candidate["finishReason"].as_str() {
        state.finish_reason = Some(reason.to_string());
    }
    if let Some(parts) = candidate["content"]["parts"].as_array() {
        for part in parts {
            if let Some(fc) = part.get("functionCall") {
                if let Some(t_idx) = state.thinking_idx.take() {
                    builder.end_thinking(t_idx);
                }
                if let Some(idx) = state.text_idx.take() {
                    builder.end_text(idx);
                }
                let name = fc["name"].as_str().unwrap_or_default().to_string();
                let id = format!("call_{}", uuid_ish());
                let idx = builder.begin_tool_call(id, name);
                if let Some(sig) = part["thoughtSignature"].as_str() {
                    builder.set_tool_thought_signature(idx, sig.to_string());
                }
                builder.end_tool_call(idx, Some(fc["args"].clone()));
            } else if let Some(text) = part["text"].as_str() {
                if part["thought"].as_bool() == Some(true) {
                    let idx = *state
                        .thinking_idx
                        .get_or_insert_with(|| builder.begin_thinking());
                    builder.append_thinking(idx, text);
                    if let Some(sig) = part["thoughtSignature"].as_str() {
                        builder.set_thinking_signature(idx, sig.to_string());
                    }
                } else {
                    if let Some(t_idx) = state.thinking_idx.take() {
                        builder.end_thinking(t_idx);
                    }
                    let idx = *state.text_idx.get_or_insert_with(|| builder.begin_text());
                    builder.append_text(idx, text);
                }
            }
        }
    }
    Ok(())
}

fn uuid_ish() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:x}{:08x}", chrono::Utc::now().timestamp_micros(), nanos)
}

fn build_request(model: &Model, context: &Context, options: &StreamOptions) -> Value {
    let mut contents: Vec<Value> = Vec::new();
    for message in &context.messages {
        match message {
            Message::User(user) => {
                let parts = match &user.content {
                    UserContent::Text(t) => json!([{ "text": t }]),
                    UserContent::Blocks(blocks) => Value::Array(
                        blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text, .. } => Some(json!({"text": text})),
                                ContentBlock::Image { data, mime_type } => Some(json!({
                                    "inlineData": {"mimeType": mime_type, "data": data},
                                })),
                                _ => None,
                            })
                            .collect(),
                    ),
                };
                contents.push(json!({"role": "user", "parts": parts}));
            }
            Message::Assistant(assistant) => {
                let mut parts: Vec<Value> = Vec::new();
                for block in &assistant.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            if !text.is_empty() {
                                parts.push(json!({"text": text}));
                            }
                        }
                        ContentBlock::ToolCall(tc) => {
                            let mut part = json!({
                                "functionCall": {"name": tc.name, "args": tc.arguments},
                            });
                            if let Some(sig) = &tc.thought_signature {
                                part["thoughtSignature"] = json!(sig);
                            }
                            parts.push(part);
                        }
                        // Thinking text is not replayed. Signatures ride on tool calls.
                        ContentBlock::Thinking { .. } | ContentBlock::Image { .. } => {}
                    }
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
                }
            }
            Message::ToolResult(result) => {
                let text: String = result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": result.tool_name,
                            "response": {"output": text},
                        },
                    }],
                }));
            }
        }
    }

    let mut generation_config = json!({
        "maxOutputTokens": options.max_tokens.unwrap_or(model.max_tokens),
    });
    if let Some(t) = options.temperature {
        generation_config["temperature"] = json!(t);
    }
    if model.reasoning {
        let level = model.map_thinking_level(options.reasoning);
        let id = model.id.to_ascii_lowercase();
        if (id.contains("gemini-3")
            || matches!(
                id.as_str(),
                "gemini-flash-latest" | "gemini-flash-lite-latest"
            )
            || id.contains("gemma-4")
            || id.contains("gemma4"))
            && level != crate::ThinkingLevel::Off
        {
            generation_config["thinkingConfig"] = json!({
                "thinkingLevel": level.as_str().to_ascii_uppercase(),
                "includeThoughts": true,
            });
        } else if let Some(budget) =
            thinking_budget(level, options.max_tokens.unwrap_or(model.max_tokens))
        {
            generation_config["thinkingConfig"] = json!({
                "thinkingBudget": budget,
                "includeThoughts": true,
            });
        }
    }

    let mut body = json!({
        "contents": contents,
        "generationConfig": generation_config,
    });
    if let Some(system) = &context.system_prompt {
        body["systemInstruction"] = json!({"parts": [{"text": system}]});
    }
    if !context.tools.is_empty() {
        body["tools"] = json!([{
            "functionDeclarations": context
                .tools
                .iter()
                .map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": sanitize_schema(&t.parameters),
                }))
                .collect::<Vec<Value>>(),
        }]);
    }
    body
}

/// Gemini's function schemas are a subset of JSON schema: some keywords are
/// rejected, and every schema needs a `type`. Strip the unsupported keywords
/// and give a schema without a type one, so that a request never fails on a
/// tool's schema. Its schema knows `minimum` and `maximum` but not their
/// exclusive forms, which the file tools' line counts use.
fn sanitize_schema(schema: &Value) -> Value {
    typed_schema(schema, "object")
}

/// Keywords Gemini's function schemas reject.
fn unsupported_keyword(key: &str) -> bool {
    matches!(
        key,
        "additionalProperties" | "$schema" | "default" | "exclusiveMinimum" | "exclusiveMaximum"
    )
}

/// `schema` without the unsupported keywords, at any depth, and with a
/// `type` wherever a schema stands: the parameters themselves, each
/// property, `items` and the alternatives of `anyOf`/`oneOf`/`allOf`. A
/// schema without one, such as `Workflow`'s `args`, which takes any value,
/// becomes an object when it has properties, an array when it has items,
/// and else `default_type`: a narrower type than any value, but one every
/// value the model then sends is still valid for.
fn typed_schema(schema: &Value, default_type: &str) -> Value {
    let Value::Object(map) = schema else {
        return schema.clone();
    };
    let mut out = serde_json::Map::new();
    for (key, value) in map {
        if unsupported_keyword(key) {
            continue;
        }
        let value = match (key.as_str(), value) {
            ("properties", Value::Object(properties)) => Value::Object(
                properties
                    .iter()
                    .map(|(name, property)| (name.clone(), typed_schema(property, "string")))
                    .collect(),
            ),
            ("items", _) => typed_schema(value, "string"),
            ("anyOf" | "oneOf" | "allOf", Value::Array(options)) => Value::Array(
                options
                    .iter()
                    .map(|option| typed_schema(option, "string"))
                    .collect(),
            ),
            _ => without_unsupported_keywords(value),
        };
        out.insert(key.clone(), value);
    }
    let alternatives = ["anyOf", "oneOf", "allOf"]
        .iter()
        .any(|key| out.contains_key(*key));
    if !out.contains_key("type") && !alternatives {
        let kind = if out.contains_key("properties") {
            "object"
        } else if out.contains_key("items") {
            "array"
        } else {
            default_type
        };
        out.insert("type".into(), json!(kind));
    }
    Value::Object(out)
}

/// Any other value inside a schema, without the unsupported keywords.
fn without_unsupported_keywords(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !unsupported_keyword(key))
                .map(|(key, value)| (key.clone(), without_unsupported_keywords(value)))
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.iter().map(without_unsupported_keywords).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod vertex_tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn tool_schemas_lose_the_keywords_gemini_rejects() {
        let schema = json!({
            "type": "object",
            "properties": {
                "offset": {"type": "integer", "minimum": 0, "maximum": 10},
                "limit": {"type": "integer", "exclusiveMinimum": 0, "exclusiveMaximum": 9},
                "replace_all": {"type": "boolean", "default": false},
            },
            "required": ["offset"],
            "additionalProperties": false,
        });
        assert_eq!(
            sanitize_schema(&schema),
            json!({
                "type": "object",
                "properties": {
                    "offset": {"type": "integer", "minimum": 0, "maximum": 10},
                    "limit": {"type": "integer"},
                    "replace_all": {"type": "boolean"},
                },
                "required": ["offset"],
            })
        );
    }

    /// Whether every schema in `schema` has a type or alternatives.
    fn every_schema_is_typed(schema: &Value) -> bool {
        let Some(map) = schema.as_object() else {
            return true;
        };
        let typed = map.contains_key("type")
            || ["anyOf", "oneOf", "allOf"]
                .iter()
                .any(|key| map.contains_key(*key));
        let properties = map
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(|properties| properties.values().all(every_schema_is_typed));
        let items = map.get("items").is_none_or(every_schema_is_typed);
        typed && properties && items
    }

    #[test]
    fn a_property_without_a_type_gets_one_so_the_request_cannot_fail() {
        // The shape of `Workflow`'s parameters: `args` takes any value and
        // has a description only.
        let workflow = json!({
            "type": "object",
            "properties": {
                "script": {"type": "string", "maxLength": 100, "description": "script"},
                "args": {"description": "args"},
                "resumeFromRunId": {"type": "string", "pattern": "^wf_[a-z0-9-]{6,}$"},
            },
            "additionalProperties": false,
        });
        let sent = sanitize_schema(&workflow);
        assert_eq!(
            sent["properties"]["args"],
            json!({"description": "args", "type": "string"})
        );
        assert_eq!(
            sent["properties"]["script"],
            workflow["properties"]["script"]
        );
        assert_eq!(
            sent["properties"]["resumeFromRunId"],
            workflow["properties"]["resumeFromRunId"]
        );
        assert!(every_schema_is_typed(&sent), "{sent}");

        // Deeper too, and a schema says what it is by its keywords.
        let nested = json!({
            "properties": {
                "list": {"items": {}},
                "shape": {"properties": {"default": {"type": "boolean", "default": true}}},
                "either": {"anyOf": [{"type": "string"}, {"description": "anything"}]},
                "choice": {"enum": ["a", "b"]},
            },
        });
        let sent = sanitize_schema(&nested);
        assert!(every_schema_is_typed(&sent), "{sent}");
        assert_eq!(sent["type"], "object");
        assert_eq!(
            sent["properties"]["list"],
            json!({"type": "array", "items": {"type": "string"}})
        );
        // A property named like a keyword stays; the keyword in it goes.
        assert_eq!(
            sent["properties"]["shape"],
            json!({"type": "object", "properties": {"default": {"type": "boolean"}}})
        );
        assert_eq!(
            sent["properties"]["either"],
            json!({"anyOf": [{"type": "string"}, {"description": "anything", "type": "string"}]})
        );
        assert_eq!(
            sent["properties"]["choice"],
            json!({"enum": ["a", "b"], "type": "string"})
        );
        // Parameters without anything are an object without properties.
        assert_eq!(sanitize_schema(&json!({})), json!({"type": "object"}));
    }

    #[test]
    fn builds_vertex_stream_url() {
        let model = Model {
            id: "gemini-test".into(),
            name: String::new(),
            api: "google-vertex".into(),
            provider: "google-vertex".into(),
            base_url: "https://{location}-aiplatform.googleapis.com".into(),
            reasoning: false,
            input: vec!["text".into()],
            cost: Default::default(),
            prompt_cache: None,
            context_window: 100,
            max_tokens: 10,
            compat: None,
            thinking_level_map: BTreeMap::new(),
            headers: BTreeMap::new(),
            sampling_params: Default::default(),
        };
        assert_eq!(
            vertex_url(&model, "project-one", "us-central1"),
            "https://us-central1-aiplatform.googleapis.com/v1/projects/project-one/locations/us-central1/publishers/google/models/gemini-test:streamGenerateContent?alt=sse"
        );
        assert!(priority_header(&model, &StreamOptions::default()).is_none());
        assert_eq!(
            priority_header(
                &model,
                &StreamOptions {
                    fast_mode: true,
                    ..Default::default()
                }
            ),
            Some("priority")
        );
    }

    #[test]
    fn gemini_three_uses_discrete_thinking_levels() {
        let mut model = Model {
            id: "gemini-3-flash-preview".into(),
            name: String::new(),
            api: "google-generative-ai".into(),
            provider: "google".into(),
            base_url: "https://example.invalid".into(),
            reasoning: true,
            input: vec!["text".into()],
            cost: Default::default(),
            prompt_cache: None,
            context_window: 100,
            max_tokens: 10,
            compat: None,
            thinking_level_map: BTreeMap::new(),
            headers: BTreeMap::new(),
            sampling_params: Default::default(),
        };
        model
            .thinking_level_map
            .insert("low".into(), Some("medium".into()));
        let body = build_request(
            &model,
            &Context::default(),
            &StreamOptions {
                reasoning: crate::ThinkingLevel::Low,
                ..Default::default()
            },
        );
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MEDIUM"
        );
        assert!(
            body["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none()
        );
    }
}
