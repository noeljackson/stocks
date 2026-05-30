//! SSE fan-out hub. Each client gets a bounded broadcast::Receiver; the hub
//! holds a single Sender. Capacity matters: slow clients drop messages rather
//! than backpressure the publishers.

use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 64;

pub struct Hub {
    tx: broadcast::Sender<String>,
}

impl Hub {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// Sends `msg` to every current subscriber. Silently no-ops if no
    /// subscribers; drops the message for slow ones (Lagged on their end).
    pub fn broadcast(&self, msg: String) {
        let _ = self.tx.send(msg);
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}
