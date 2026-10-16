//! Human decisions a session is waiting for, answerable from any surface.
//!
//! The terminal shows a workflow approval, a project trust question or a login
//! prompt as a picker. A remote controller sees the same question as an
//! `approval_request` event and may answer it with `answer_approval`. Whoever
//! answers first wins; the other side is told through `approval_resolved`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// What kind of decision is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Workflow,
    Trust,
    Login,
    Other,
}

/// One selectable answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalChoice {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ApprovalChoice {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The question as shown to every surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub kind: ApprovalKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub choices: Vec<ApprovalChoice>,
}

/// Who answered and what they picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalAnswer {
    pub choice: String,
    /// `local` for the terminal, otherwise the remote device name.
    pub by: String,
}

struct Pending {
    request: ApprovalRequest,
    reply: Option<oneshot::Sender<ApprovalAnswer>>,
}

/// The registry of open approvals on one session.
#[derive(Default)]
pub struct Approvals {
    next_id: Mutex<u64>,
    pending: Mutex<BTreeMap<String, Pending>>,
}

impl Approvals {
    /// Register a question. The receiver resolves when any surface answers.
    pub fn open(&self, request: ApprovalRequest) -> (String, oneshot::Receiver<ApprovalAnswer>) {
        let id = {
            let mut next = self.next_id.lock().unwrap();
            *next += 1;
            format!("ap{}", *next)
        };
        let (reply, answer) = oneshot::channel();
        self.pending.lock().unwrap().insert(
            id.clone(),
            Pending {
                request,
                reply: Some(reply),
            },
        );
        (id, answer)
    }

    /// Answer a question. Fails when the id is unknown, already answered, or
    /// the choice is not one of the offered ones.
    pub fn answer(&self, id: &str, choice: &str, by: &str) -> Result<ApprovalRequest, String> {
        let mut pending = self.pending.lock().unwrap();
        let entry = pending
            .get_mut(id)
            .ok_or_else(|| format!("no open approval '{id}'"))?;
        if !entry.request.choices.iter().any(|c| c.id == choice) {
            return Err(format!("'{choice}' is not a choice of approval '{id}'"));
        }
        let Some(reply) = entry.reply.take() else {
            return Err(format!("approval '{id}' was already answered"));
        };
        let request = entry.request.clone();
        pending.remove(id);
        // A dropped receiver means the asker gave up; the answer is still valid.
        let _ = reply.send(ApprovalAnswer {
            choice: choice.to_string(),
            by: by.to_string(),
        });
        Ok(request)
    }

    /// Forget a question without answering it, for example when the asker
    /// went away.
    pub fn close(&self, id: &str) -> bool {
        self.pending.lock().unwrap().remove(id).is_some()
    }

    /// Every open question, oldest first.
    pub fn list(&self) -> Vec<(String, ApprovalRequest)> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .map(|(id, pending)| (id.clone(), pending.request.clone()))
            .collect()
    }
}

/// The `approval_request` event.
pub fn approval_request_event(id: &str, request: &ApprovalRequest) -> Value {
    json!({
        "type": "approval_request",
        "approvalId": id,
        "kind": request.kind,
        "title": request.title,
        "detail": request.detail,
        "choices": request.choices,
    })
}

/// The `approval_resolved` event.
pub fn approval_resolved_event(id: &str, choice: &str, by: &str) -> Value {
    json!({"type": "approval_resolved", "approvalId": id, "choice": choice, "by": by})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ApprovalRequest {
        ApprovalRequest {
            kind: ApprovalKind::Workflow,
            title: "Run workflow audit?".into(),
            detail: None,
            choices: vec![
                ApprovalChoice::new("approve", "Run it"),
                ApprovalChoice::new("cancel", "Cancel"),
            ],
        }
    }

    #[tokio::test]
    async fn the_first_answer_wins_and_the_asker_learns_who_answered() {
        let approvals = Approvals::default();
        let (id, answer) = approvals.open(request());
        assert_eq!(approvals.list().len(), 1);
        approvals.answer(&id, "approve", "iPhone").expect("answer");
        let error = approvals
            .answer(&id, "cancel", "local")
            .expect_err("second answer must fail");
        assert!(error.contains("no open approval"), "{error}");
        let answer = answer.await.expect("asker receives the answer");
        assert_eq!(answer.choice, "approve");
        assert_eq!(answer.by, "iPhone");
        assert!(approvals.list().is_empty());
    }

    #[test]
    fn unknown_choices_are_rejected_without_consuming_the_question() {
        let approvals = Approvals::default();
        let (id, _answer) = approvals.open(request());
        let error = approvals
            .answer(&id, "maybe", "local")
            .expect_err("unknown choice");
        assert!(error.contains("not a choice"), "{error}");
        assert_eq!(approvals.list().len(), 1);
    }

    #[test]
    fn events_carry_the_ids_a_client_needs_to_answer() {
        let event = approval_request_event("ap1", &request());
        assert_eq!(event["approvalId"], "ap1");
        assert_eq!(event["kind"], "workflow");
        assert_eq!(event["choices"][0]["id"], "approve");
        let resolved = approval_resolved_event("ap1", "approve", "local");
        assert_eq!(resolved["type"], "approval_resolved");
    }
}
