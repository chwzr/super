// Handles the Interactive RenderSpec suspension protocol.
//
// When a tool returns `RenderSpec::Interactive`, the executor:
// 1. Emits the spec on the session bus (so TUI / web render it).
// 2. Suspends the turn and waits for a `UserInteractionResponse` event
//    whose payload matches the tool's `response_schema`.
// 3. Merges the response into the tool's input (or resolves the
//    permission decision) and resumes execution.

use serde_json::Value;
use shared::RenderSpec;

/// The response payload from the renderer after the user completes
/// an interactive widget.
#[derive(Debug, Clone)]
pub struct UserInteractionResponse {
    /// Matches the tool_use_id that triggered the interaction.
    pub tool_use_id: String,
    /// The JSON response from the user (answers, approval decision, etc.).
    pub payload: Value,
}

/// Result of the interaction phase:
/// - `Resolved`: the input has been updated with user data; proceed with call()
/// - `Denied`: user rejected the interaction; emit rejection render
/// - `Aborted`: session ended or timeout
#[derive(Debug)]
pub enum InteractionOutcome {
    Resolved {
        /// The tool input, updated with user-supplied answers/approval.
        updated_input: Value,
    },
    Denied,
    Aborted,
}

/// Given a tool's render output and the session bus, suspend the turn
/// until the user responds to the Interactive widget.
///
/// Returns the resolved outcome so the executor can proceed.
pub async fn await_interaction(
    tool_use_id: String,
    spec: &RenderSpec,
    bus: &std::sync::Arc<crate::conversation::session_bus::SessionBus>,
    parent_tool_use_id: Option<String>,
) -> InteractionOutcome {
    // Extract the response_schema from the Interactive variant
    let response_schema = match spec {
        RenderSpec::Interactive {
            response_schema, ..
        } => response_schema.clone(),
        _ => return InteractionOutcome::Aborted,
    };

    // Emit the interaction request onto the session bus
    // The TUI / web picks this up and renders the widget
    bus.emit(crate::sdk::protocol::BusMessage::InteractionRequested {
        tool_use_id: tool_use_id.clone(),
        spec: spec.clone(),
        response_schema,
        parent_tool_use_id,
        uuid: uuid::Uuid::new_v4(),
        session_id: bus.session_id().to_string(),
    });

    // Wait for the response
    let mut rx = bus.subscribe();
    loop {
        let event = rx.recv().await;
        match event {
            Ok(crate::sdk::protocol::BusMessage::InteractionResponse {
                tool_use_id: resp_id,
                payload,
                ..
            }) if resp_id == tool_use_id => {
                return InteractionOutcome::Resolved {
                    updated_input: payload,
                };
            }
            Ok(crate::sdk::protocol::BusMessage::InteractionDenied {
                tool_use_id: resp_id,
                ..
            }) if resp_id == tool_use_id => {
                return InteractionOutcome::Denied;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                // Consumer fell behind — log and continue waiting
                eprintln!("interactive executor lagged by {} messages", n);
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return InteractionOutcome::Aborted;
            }
            _ => continue,
        }
    }
}
