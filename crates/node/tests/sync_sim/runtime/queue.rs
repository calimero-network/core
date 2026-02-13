//! Priority event queue with deterministic ordering.
//!
//! Events ordered by (time, seq) for deterministic processing.
//! See spec §5.1 - Event Queue Ordering.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::clock::SimTime;

/// Unique event sequence number for tie-breaking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventSeq(u64);

impl EventSeq {
    /// Create a new sequence number.
    #[must_use]
    pub const fn new(seq: u64) -> Self {
        Self(seq)
    }

    /// Get the raw sequence number.
    #[must_use]
    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

impl PartialOrd for EventSeq {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EventSeq {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

/// Wrapper for events in the queue with ordering metadata.
#[derive(Debug)]
struct QueuedEvent<E> {
    /// Scheduled execution time.
    time: SimTime,
    /// Sequence number for tie-breaking (earlier enqueue = earlier process).
    seq: EventSeq,
    /// The actual event.
    event: E,
}

impl<E> PartialEq for QueuedEvent<E> {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time && self.seq == other.seq
    }
}

impl<E> Eq for QueuedEvent<E> {}

impl<E> PartialOrd for QueuedEvent<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<E> Ord for QueuedEvent<E> {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap, so we reverse the comparison
        // to get min-heap behavior (earliest time first)
        match other.time.cmp(&self.time) {
            Ordering::Equal => other.seq.cmp(&self.seq),
            ordering => ordering,
        }
    }
}

/// Priority queue for simulation events.
///
/// Events are processed in order of (time, seq) where:
/// - `time`: Scheduled execution time
/// - `seq`: Tie-breaker for events at the same time (FIFO within same time)
#[derive(Debug)]
pub struct EventQueue<E> {
    /// Priority queue (min-heap by time, then by seq).
    heap: BinaryHeap<QueuedEvent<E>>,
    /// Counter for assigning sequence numbers.
    next_seq: u64,
}

impl<E> EventQueue<E> {
    /// Create a new empty event queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 0,
        }
    }

    /// Check if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Get the number of pending events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Schedule an event at a specific time.
    ///
    /// Returns the assigned sequence number.
    pub fn schedule(&mut self, time: SimTime, event: E) -> EventSeq {
        let seq = EventSeq::new(self.next_seq);
        self.next_seq += 1;

        self.heap.push(QueuedEvent { time, seq, event });
        seq
    }

    /// Peek at the next event without removing it.
    #[must_use]
    pub fn peek(&self) -> Option<(SimTime, &E)> {
        self.heap.peek().map(|qe| (qe.time, &qe.event))
    }

    /// Peek at the next event's time without removing it.
    #[must_use]
    pub fn peek_time(&self) -> Option<SimTime> {
        self.heap.peek().map(|qe| qe.time)
    }

    /// Pop the next event (earliest time, lowest seq).
    pub fn pop(&mut self) -> Option<(SimTime, EventSeq, E)> {
        self.heap.pop().map(|qe| (qe.time, qe.seq, qe.event))
    }

    /// Pop the next event if its time is <= the given time.
    pub fn pop_if_ready(&mut self, now: SimTime) -> Option<(SimTime, EventSeq, E)> {
        if let Some(qe) = self.heap.peek() {
            if qe.time <= now {
                return self.pop();
            }
        }
        None
    }

    /// Pop all events at or before the given time.
    pub fn pop_all_ready(&mut self, now: SimTime) -> Vec<(SimTime, EventSeq, E)> {
        let mut events = Vec::new();
        while let Some(result) = self.pop_if_ready(now) {
            events.push(result);
        }
        events
    }

    /// Cancel an event by predicate.
    ///
    /// Note: This is O(n) and rebuilds the heap. Use sparingly.
    pub fn cancel_where<F>(&mut self, predicate: F) -> usize
    where
        F: Fn(&E) -> bool,
    {
        let original_len = self.heap.len();
        let events: Vec<_> = self.heap.drain().collect();

        for qe in events {
            if !predicate(&qe.event) {
                self.heap.push(qe);
            }
        }

        original_len - self.heap.len()
    }

    /// Clear all events.
    pub fn clear(&mut self) {
        self.heap.clear();
    }

    /// Count events matching a predicate.
    pub fn count_where<F>(&self, predicate: F) -> usize
    where
        F: Fn(&E) -> bool,
    {
        self.heap.iter().filter(|qe| predicate(&qe.event)).count()
    }
}

impl<E> Default for EventQueue<E> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum TestEvent {
        A(u32),
        B(u32),
    }

    #[test]
    fn test_queue_basic() {
        let mut queue = EventQueue::new();

        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);

        queue.schedule(SimTime::from_millis(100), TestEvent::A(1));
        queue.schedule(SimTime::from_millis(50), TestEvent::A(2));
        queue.schedule(SimTime::from_millis(150), TestEvent::A(3));

        assert!(!queue.is_empty());
        assert_eq!(queue.len(), 3);
    }

    #[test]
    fn test_queue_ordering_by_time() {
        let mut queue = EventQueue::new();

        queue.schedule(SimTime::from_millis(100), TestEvent::A(1));
        queue.schedule(SimTime::from_millis(50), TestEvent::A(2));
        queue.schedule(SimTime::from_millis(150), TestEvent::A(3));

        let (t1, _, e1) = queue.pop().unwrap();
        let (t2, _, e2) = queue.pop().unwrap();
        let (t3, _, e3) = queue.pop().unwrap();

        assert_eq!(t1, SimTime::from_millis(50));
        assert_eq!(t2, SimTime::from_millis(100));
        assert_eq!(t3, SimTime::from_millis(150));

        assert_eq!(e1, TestEvent::A(2));
        assert_eq!(e2, TestEvent::A(1));
        assert_eq!(e3, TestEvent::A(3));
    }

    #[test]
    fn test_queue_ordering_by_seq_same_time() {
        let mut queue = EventQueue::new();

        // All at same time - should be FIFO
        let t = SimTime::from_millis(100);
        queue.schedule(t, TestEvent::A(1));
        queue.schedule(t, TestEvent::A(2));
        queue.schedule(t, TestEvent::A(3));

        let (_, _, e1) = queue.pop().unwrap();
        let (_, _, e2) = queue.pop().unwrap();
        let (_, _, e3) = queue.pop().unwrap();

        // FIFO order
        assert_eq!(e1, TestEvent::A(1));
        assert_eq!(e2, TestEvent::A(2));
        assert_eq!(e3, TestEvent::A(3));
    }

    #[test]
    fn test_peek() {
        let mut queue = EventQueue::new();

        assert!(queue.peek().is_none());
        assert!(queue.peek_time().is_none());

        queue.schedule(SimTime::from_millis(100), TestEvent::A(1));
        queue.schedule(SimTime::from_millis(50), TestEvent::A(2));

        // Peek should show earliest
        let (t, e) = queue.peek().unwrap();
        assert_eq!(t, SimTime::from_millis(50));
        assert_eq!(e, &TestEvent::A(2));

        // Peek doesn't remove
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_pop_if_ready() {
        let mut queue = EventQueue::new();

        queue.schedule(SimTime::from_millis(100), TestEvent::A(1));
        queue.schedule(SimTime::from_millis(200), TestEvent::A(2));

        // At time 50, nothing ready
        assert!(queue.pop_if_ready(SimTime::from_millis(50)).is_none());

        // At time 100, first event ready
        let (t, _, e) = queue.pop_if_ready(SimTime::from_millis(100)).unwrap();
        assert_eq!(t, SimTime::from_millis(100));
        assert_eq!(e, TestEvent::A(1));

        // At time 100, second event not ready yet
        assert!(queue.pop_if_ready(SimTime::from_millis(100)).is_none());

        // At time 200, second event ready
        let (t, _, e) = queue.pop_if_ready(SimTime::from_millis(200)).unwrap();
        assert_eq!(t, SimTime::from_millis(200));
        assert_eq!(e, TestEvent::A(2));
    }

    #[test]
    fn test_pop_all_ready() {
        let mut queue = EventQueue::new();

        queue.schedule(SimTime::from_millis(50), TestEvent::A(1));
        queue.schedule(SimTime::from_millis(100), TestEvent::A(2));
        queue.schedule(SimTime::from_millis(100), TestEvent::A(3));
        queue.schedule(SimTime::from_millis(200), TestEvent::A(4));

        let ready = queue.pop_all_ready(SimTime::from_millis(100));
        assert_eq!(ready.len(), 3);

        // Should be in order
        assert_eq!(ready[0].2, TestEvent::A(1));
        assert_eq!(ready[1].2, TestEvent::A(2));
        assert_eq!(ready[2].2, TestEvent::A(3));

        // One event left
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_cancel_where() {
        let mut queue = EventQueue::new();

        queue.schedule(SimTime::from_millis(100), TestEvent::A(1));
        queue.schedule(SimTime::from_millis(100), TestEvent::B(2));
        queue.schedule(SimTime::from_millis(100), TestEvent::A(3));

        // Cancel all A events
        let cancelled = queue.cancel_where(|e| matches!(e, TestEvent::A(_)));
        assert_eq!(cancelled, 2);
        assert_eq!(queue.len(), 1);

        let (_, _, e) = queue.pop().unwrap();
        assert_eq!(e, TestEvent::B(2));
    }

    #[test]
    fn test_count_where() {
        let mut queue = EventQueue::new();

        queue.schedule(SimTime::from_millis(100), TestEvent::A(1));
        queue.schedule(SimTime::from_millis(100), TestEvent::B(2));
        queue.schedule(SimTime::from_millis(100), TestEvent::A(3));

        let count_a = queue.count_where(|e| matches!(e, TestEvent::A(_)));
        let count_b = queue.count_where(|e| matches!(e, TestEvent::B(_)));

        assert_eq!(count_a, 2);
        assert_eq!(count_b, 1);
    }
}
