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

#[derive(Clone, Default)]
pub struct AppState {
    pub messages: Vec<Message>,
    pub permission_mode: PermissionMode,
    pub model: String,
    pub thinking_enabled: bool,
    pub is_streaming: bool,
    pub should_compact: bool,
    pub tasks: HashMap<String, TaskRecord>,
}

#[derive(Clone, Default, PartialEq)]
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
}
