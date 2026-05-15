use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use crate::tui::scroll_area::Message;

#[derive(Clone, Default)]
pub struct TaskRecord {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum TaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
    Deleted,
}

#[derive(Clone)]
pub struct AsyncAgentHandle {
    pub agent_id: String,
    pub parent_tool_use_id: String,
    pub abort: tokio::sync::watch::Sender<bool>,
    pub description: String,
    pub started_at: std::time::Instant,
}

#[derive(Clone)]
pub struct AppState {
    pub messages: Vec<Message>,
    pub permission_mode: PermissionMode,
    pub provider: String,
    pub model_class: String,
    pub thinking_enabled: bool,
    pub effort_level: Option<String>,
    pub is_streaming: bool,
    pub should_compact: bool,
    pub tasks: HashMap<String, TaskRecord>,
    pub history: Vec<crate::conversation::anthropic::HistoryEntry>,
    pub async_agents: HashMap<String, AsyncAgentHandle>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            permission_mode: PermissionMode::default(),
            provider: "anthropic".to_string(),
            model_class: "sonnet".to_string(),
            thinking_enabled: false,
            effort_level: None,
            is_streaming: false,
            should_compact: false,
            tasks: HashMap::new(),
            history: Vec::new(),
            async_agents: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum PermissionMode {
    #[default]
    Default,
    AcceptEdits,
    Bypass,
    Plan,
    DontAsk,
    Auto,
}

pub struct Store {
    state: Arc<RwLock<AppState>>,
    subscribers: Arc<RwLock<Vec<Box<dyn Fn(&AppState) + Send + Sync>>>>,
}

impl Clone for Store {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            subscribers: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Store {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(AppState::default())),
            subscribers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn get_state(&self) -> AppState {
        self.state.read().unwrap().clone()
    }

    pub fn set_state(&self, updater: impl FnOnce(&mut AppState)) {
        {
            let mut state = self.state.write().unwrap();
            updater(&mut state);
        }
        let new_state = self.get_state();
        self.notify(&new_state);
    }

    pub fn subscribe<F>(&self, listener: F)
    where
        F: Fn(&AppState) + Send + Sync + 'static,
    {
        self.subscribers.write().unwrap().push(Box::new(listener));
    }

    fn notify(&self, state: &AppState) {
        for sub in self.subscribers.read().unwrap().iter() {
            sub(state);
        }
    }

    pub fn set_provider(&self, provider: &str) {
        let p = provider.to_string();
        self.set_state(|s| s.provider = p);
    }

    pub fn set_model_class(&self, class: &str) {
        let c = class.to_string();
        self.set_state(|s| s.model_class = c);
    }

    pub fn set_effort(&self, effort: String) {
        self.set_state(|s| s.effort_level = Some(effort));
    }

    pub fn register_async_agent(&self, handle: AsyncAgentHandle) {
        let id = handle.agent_id.clone();
        self.set_state(|s| {
            s.async_agents.insert(id.clone(), handle.clone());
        });
        let _ = id;
    }

    pub fn complete_async_agent(&self, agent_id: &str) {
        self.set_state(|s| {
            s.async_agents.remove(agent_id);
        });
    }

    /// Returns true if an agent was found and signalled.
    pub fn abort_async_agent(&self, agent_id: &str) -> bool {
        // Read out the handle outside set_state so we can call its send()
        let handle = self.state.read().unwrap().async_agents.get(agent_id).cloned();
        if let Some(h) = handle {
            let _ = h.abort.send(true);
            self.set_state(|s| { s.async_agents.remove(agent_id); });
            true
        } else {
            false
        }
    }

    pub fn list_async_agents(&self) -> Vec<AsyncAgentHandle> {
        self.state.read().unwrap().async_agents.values().cloned().collect()
    }

    /// Abort every currently-registered async agent. Called by the TUI's exit
    /// path. Returns the number of agents that were signalled.
    pub fn shutdown_async_agents(&self) -> usize {
        let ids: Vec<String> = self.list_async_agents()
            .into_iter()
            .map(|h| h.agent_id)
            .collect();
        let mut count = 0;
        for id in &ids {
            if self.abort_async_agent(id) {
                count += 1;
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_anthropic_sonnet() {
        let store = Store::new();
        let s = store.get_state();
        assert_eq!(s.provider, "anthropic");
        assert_eq!(s.model_class, "sonnet");
    }

    #[test]
    fn set_provider_updates_state() {
        let store = Store::new();
        store.set_provider("z-ai");
        assert_eq!(store.get_state().provider, "z-ai");
    }

    #[test]
    fn set_model_class_updates_state() {
        let store = Store::new();
        store.set_model_class("haiku");
        assert_eq!(store.get_state().model_class, "haiku");
    }

    #[test]
    fn register_and_complete_async_agent() {
        let store = Store::new();
        let (tx, _rx) = tokio::sync::watch::channel(false);
        store.register_async_agent(AsyncAgentHandle {
            agent_id: "a1".into(),
            parent_tool_use_id: "tu_1".into(),
            abort: tx,
            description: "test".into(),
            started_at: std::time::Instant::now(),
        });
        let list = store.list_async_agents();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].agent_id, "a1");
        store.complete_async_agent("a1");
        assert!(store.list_async_agents().is_empty());
    }

    #[test]
    fn abort_async_agent_signals_watch() {
        let store = Store::new();
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        store.register_async_agent(AsyncAgentHandle {
            agent_id: "a2".into(),
            parent_tool_use_id: "tu_2".into(),
            abort: tx,
            description: "test".into(),
            started_at: std::time::Instant::now(),
        });
        assert!(store.abort_async_agent("a2"));
        // Watch should have fired
        assert!(*rx.borrow_and_update());
        assert!(store.list_async_agents().is_empty(), "abort also removes the handle");
    }

    #[test]
    fn shutdown_aborts_all_registered_agents() {
        let store = Store::new();
        let (tx1, mut rx1) = tokio::sync::watch::channel(false);
        let (tx2, mut rx2) = tokio::sync::watch::channel(false);
        store.register_async_agent(AsyncAgentHandle {
            agent_id: "a1".into(),
            parent_tool_use_id: "tu_1".into(),
            abort: tx1,
            description: "x".into(),
            started_at: std::time::Instant::now(),
        });
        store.register_async_agent(AsyncAgentHandle {
            agent_id: "a2".into(),
            parent_tool_use_id: "tu_2".into(),
            abort: tx2,
            description: "y".into(),
            started_at: std::time::Instant::now(),
        });
        assert_eq!(store.shutdown_async_agents(), 2);
        assert!(*rx1.borrow_and_update());
        assert!(*rx2.borrow_and_update());
        assert!(store.list_async_agents().is_empty());
    }
}
