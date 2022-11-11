use crate::app::Hash;
use tokio::sync::broadcast::{error::SendError, Receiver, Sender};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    PendingIdentityInserted { group_id: usize, commitment: Hash },
}

pub struct EventBus {
    sender: Sender<Vec<Event>>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(capacity);
        Self { sender }
    }

    pub fn publish(&self, event: Event) -> Result<(), SendError<Event>> {
        self.sender
            .send(vec![event])
            .map_err(|error| SendError(error.0[0].clone()))?;
        Ok(())
    }

    pub fn publish_batch(&self, events: Vec<Event>) -> Result<(), SendError<Vec<Event>>> {
        self.sender.send(events)?;
        Ok(())
    }

    pub fn subscribe(&self) -> Receiver<Vec<Event>> {
        self.sender.subscribe()
    }
}
