use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use libp2p::gossipsub::TopicHash;
use tracing::debug;

// 30 s covers the worst-case cold-start path: a fresh per-context
// topic + gossipsub heartbeat-driven subscription propagation (~10 s
// in a 2-node mesh) + receiver-side `MATERIALIZATION_WINDOW` (10 s)
// + slack. Observed in run 25931680038: a queued ContextRegistered
// expired by 28 ms before the `Subscribed` event arrived. Memory
// envelope at the per-topic cap (32 × ~16 KB ≈ 0.5 MB) tolerates the
// longer dwell window.
pub const OUTBOX_TTL: Duration = Duration::from_secs(30);
pub const OUTBOX_MAX_PER_TOPIC: usize = 32;

#[derive(Debug)]
pub struct OutboxEntry {
    pub data: Vec<u8>,
    pub expires_at: Instant,
}

#[derive(Debug, Default)]
pub struct PublishOutbox {
    queues: HashMap<TopicHash, VecDeque<OutboxEntry>>,
}

impl PublishOutbox {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, topic: TopicHash, data: Vec<u8>) {
        let queue = self.queues.entry(topic.clone()).or_default();
        if queue.len() >= OUTBOX_MAX_PER_TOPIC {
            let _dropped = queue.pop_front();
            debug!(
                %topic,
                cap = OUTBOX_MAX_PER_TOPIC,
                "publish outbox at cap; dropping oldest entry"
            );
        }
        queue.push_back(OutboxEntry {
            data,
            expires_at: Instant::now() + OUTBOX_TTL,
        });
    }

    pub fn take_drainable(&mut self, topic: &TopicHash) -> Vec<OutboxEntry> {
        let Some(queue) = self.queues.remove(topic) else {
            return Vec::new();
        };
        let now = Instant::now();
        let mut kept = Vec::with_capacity(queue.len());
        for entry in queue {
            if entry.expires_at > now {
                kept.push(entry);
            } else {
                debug!(%topic, "publish outbox entry expired before drain");
            }
        }
        kept
    }

    pub fn requeue(&mut self, topic: TopicHash, entries: Vec<OutboxEntry>) {
        if entries.is_empty() {
            return;
        }
        let queue = self.queues.entry(topic).or_default();
        for entry in entries {
            queue.push_back(entry);
        }
    }

    #[cfg(test)]
    fn topic_len(&self, topic: &TopicHash) -> usize {
        self.queues.get(topic).map_or(0, VecDeque::len)
    }
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;

    use super::*;

    fn topic(s: &str) -> TopicHash {
        TopicHash::from_raw(s)
    }

    #[test]
    fn enqueue_then_drain_returns_entries_in_fifo_order() {
        let mut outbox = PublishOutbox::new();
        let t = topic("t");
        outbox.enqueue(t.clone(), b"first".to_vec());
        outbox.enqueue(t.clone(), b"second".to_vec());
        assert_eq!(outbox.topic_len(&t), 2);

        let drained = outbox.take_drainable(&t);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].data, b"first");
        assert_eq!(drained[1].data, b"second");
        assert_eq!(outbox.topic_len(&t), 0);
    }

    #[test]
    fn drain_drops_ttl_expired_entries() {
        let mut outbox = PublishOutbox::new();
        let t = topic("t");
        outbox.enqueue(t.clone(), b"stale".to_vec());

        if let Some(queue) = outbox.queues.get_mut(&t) {
            for entry in queue.iter_mut() {
                entry.expires_at = Instant::now() - Duration::from_secs(1);
            }
        }

        outbox.enqueue(t.clone(), b"fresh".to_vec());
        let drained = outbox.take_drainable(&t);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].data, b"fresh");
    }

    #[test]
    fn cap_evicts_oldest_entry() {
        let mut outbox = PublishOutbox::new();
        let t = topic("t");
        for i in 0..OUTBOX_MAX_PER_TOPIC {
            outbox.enqueue(t.clone(), vec![i as u8]);
        }
        outbox.enqueue(t.clone(), vec![0xff]);
        let drained = outbox.take_drainable(&t);
        assert_eq!(drained.len(), OUTBOX_MAX_PER_TOPIC);
        assert_eq!(drained[0].data, vec![1u8]);
        assert_eq!(drained.last().unwrap().data, vec![0xff]);
    }

    #[test]
    fn requeue_preserves_order_for_next_drain() {
        let mut outbox = PublishOutbox::new();
        let t = topic("t");
        outbox.enqueue(t.clone(), b"a".to_vec());
        outbox.enqueue(t.clone(), b"b".to_vec());
        let drained = outbox.take_drainable(&t);
        outbox.requeue(t.clone(), drained);

        let drained2 = outbox.take_drainable(&t);
        assert_eq!(drained2[0].data, b"a");
        assert_eq!(drained2[1].data, b"b");
    }

    #[test]
    fn take_drainable_on_unknown_topic_is_empty() {
        let mut outbox = PublishOutbox::new();
        assert!(outbox.take_drainable(&topic("unknown")).is_empty());
    }

    #[test]
    #[ignore = "wall-clock; opt-in"]
    fn entry_expires_after_ttl() {
        let mut outbox = PublishOutbox::new();
        let t = topic("t");
        outbox.enqueue(t.clone(), b"x".to_vec());
        sleep(OUTBOX_TTL + Duration::from_millis(50));
        assert!(outbox.take_drainable(&t).is_empty());
    }
}
