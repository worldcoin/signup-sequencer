use crate::app::Hash;
use tokio::sync::broadcast::{error::SendError, Receiver, Sender};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    PendingIdentityInserted { group_id: usize, commitment: Hash },
}

pub struct EventBus {
    sender: Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(capacity);
        Self { sender }
    }

    pub async fn publish(&self, event: Event) -> Result<(), SendError<Event>> {
        self.sender.send(event)?;
        Ok(())
    }

    pub fn subscribe(&self) -> Receiver<Event> {
        self.sender.subscribe()
    }
}
