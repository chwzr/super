use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::conversation::message_queue::MessageQueue;
use crate::tools::cron_create::CronJob;

pub struct CronRuntime {
    _jobs: Arc<Mutex<HashMap<String, CronJob>>>,
    _queue: Arc<MessageQueue>,
    _wake: watch::Receiver<bool>,
}

impl CronRuntime {
    pub fn new(
        jobs: Arc<Mutex<HashMap<String, CronJob>>>,
        queue: Arc<MessageQueue>,
        wake: watch::Receiver<bool>,
    ) -> Self {
        Self {
            _jobs: jobs,
            _queue: queue,
            _wake: wake,
        }
    }

    pub async fn run(self) {
        // Placeholder — will be implemented in a later task.
        // For now, just block on the wake channel closing (which never happens
        // in normal operation) so the spawned task doesn't exit immediately.
        let mut rx = self._wake;
        loop {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }
}
