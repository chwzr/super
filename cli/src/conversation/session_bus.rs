use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::sdk::protocol::{BusMessage, SystemSubtype};

#[derive(Clone)]
pub struct SessionBus {
    sender: Arc<broadcast::Sender<BusMessage>>,
    pub session_id: String,
}

impl SessionBus {
    pub fn new(session_id: String) -> Self {
        let (sender, _) = broadcast::channel(256);
        Self { sender: Arc::new(sender), session_id }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusMessage> {
        self.sender.subscribe()
    }

    pub fn emit(&self, msg: BusMessage) {
        // It's fine if there are no subscribers — drop silently.
        let _ = self.sender.send(msg);
    }

    /// Convenience for emitting a system notice.
    pub fn emit_system(&self, subtype: SystemSubtype, message: impl Into<String>) {
        self.emit(BusMessage::SystemEvent {
            subtype,
            message: message.into(),
            uuid: Uuid::new_v4(),
            session_id: self.session_id.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::AnthropicUsage;

    #[tokio::test]
    async fn subscribe_receives_emitted_messages() {
        let bus = SessionBus::new("s1".into());
        let mut rx = bus.subscribe();
        bus.emit(BusMessage::Result {
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.0,
            duration_ms: 0,
            num_turns: 1,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        });
        let msg = rx.recv().await.unwrap();
        assert!(matches!(msg, BusMessage::Result { .. }));
    }

    #[tokio::test]
    async fn emit_with_no_subscribers_does_not_panic() {
        let bus = SessionBus::new("s1".into());
        bus.emit_system(SystemSubtype::Notice, "no one listening");
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive() {
        let bus = SessionBus::new("s1".into());
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        bus.emit_system(SystemSubtype::Notice, "hello");
        assert!(matches!(rx1.recv().await.unwrap(), BusMessage::SystemEvent { .. }));
        assert!(matches!(rx2.recv().await.unwrap(), BusMessage::SystemEvent { .. }));
    }
}
