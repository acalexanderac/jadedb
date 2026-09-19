use parking_lot::Mutex;
use std::sync::Arc;

/// Active read snapshot: reads only see seq <= seq.
#[derive(Debug)]
pub struct Snapshot {
    pub seq: u64,
    tracker: Arc<Mutex<SnapshotTracker>>,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        self.tracker.lock().release(self.seq);
    }
}

#[derive(Debug, Default)]
pub struct SnapshotTracker {
    active: Vec<u64>,
}

impl SnapshotTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_snapshot(tracker: &Arc<Mutex<Self>>, seq: u64) -> Snapshot {
        tracker.lock().active.push(seq);
        Snapshot {
            seq,
            tracker: tracker.clone(),
        }
    }

    fn release(&mut self, seq: u64) {
        if let Some(i) = self.active.iter().position(|&s| s == seq) {
            self.active.swap_remove(i);
        }
    }

    /// Oldest active snapshot seq, or u64::MAX if none.
    pub fn watermark(&self) -> u64 {
        self.active.iter().copied().min().unwrap_or(u64::MAX)
    }
}
