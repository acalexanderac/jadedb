use crate::key::{InternalKey, ValueKind};
use crate::memtable::MemTable;
use crate::sstable::SsTable;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

#[derive(Clone)]
struct HeapItem {
    key: InternalKey,
    value: Vec<u8>,
    source: usize,
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.source == other.source
    }
}
impl Eq for HeapItem {}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is max-heap; reverse so smallest InternalKey comes first
        match other.key.cmp(&self.key) {
            Ordering::Equal => other.source.cmp(&self.source),
            o => o,
        }
    }
}

/// Merges multiple sorted iterators; prefers newer sources (lower source index) for same user key.
pub struct MergeIterator {
    heap: BinaryHeap<HeapItem>,
    streams: Vec<std::vec::IntoIter<(InternalKey, Vec<u8>)>>,
    last_user: Option<Vec<u8>>,
    max_seq: u64,
    skip_tombstones: bool,
}

impl MergeIterator {
    pub fn from_sorted_streams(
        streams: Vec<Vec<(InternalKey, Vec<u8>)>>,
        max_seq: u64,
        skip_tombstones: bool,
    ) -> Self {
        let mut heap = BinaryHeap::new();
        let mut iters = Vec::new();
        for (i, stream) in streams.into_iter().enumerate() {
            let mut it = stream.into_iter();
            if let Some((k, v)) = it.next() {
                if k.seq <= max_seq {
                    heap.push(HeapItem {
                        key: k,
                        value: v,
                        source: i,
                    });
                } else {
                    // advance until visible
                    while let Some((k, v)) = it.next() {
                        if k.seq <= max_seq {
                            heap.push(HeapItem {
                                key: k,
                                value: v,
                                source: i,
                            });
                            break;
                        }
                    }
                }
            }
            iters.push(it);
        }
        Self {
            heap,
            streams: iters,
            last_user: None,
            max_seq,
            skip_tombstones,
        }
    }

    pub fn from_mem_and_sst(
        mems: &[&MemTable],
        ssts: &[Arc<SsTable>],
        max_seq: u64,
        skip_tombstones: bool,
    ) -> crate::error::Result<Self> {
        let mut streams = Vec::new();
        for m in mems {
            let v: Vec<_> = m
                .iter()
                .filter(|(k, _)| k.seq <= max_seq)
                .map(|(k, val)| (k.clone(), val.clone()))
                .collect();
            streams.push(v);
        }
        for sst in ssts {
            streams.push(sst.iter_all()?);
        }
        Ok(Self::from_sorted_streams(streams, max_seq, skip_tombstones))
    }
}

impl Iterator for MergeIterator {
    type Item = (Vec<u8>, Option<Vec<u8>>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(item) = self.heap.pop() {
            // refill stream
            let src = item.source;
            while let Some((k, v)) = self.streams[src].next() {
                if k.seq <= self.max_seq {
                    self.heap.push(HeapItem {
                        key: k,
                        value: v,
                        source: src,
                    });
                    break;
                }
            }

            if self.last_user.as_ref() == Some(&item.key.user_key) {
                continue;
            }
            self.last_user = Some(item.key.user_key.clone());
            match item.key.kind {
                ValueKind::Delete if self.skip_tombstones => continue,
                ValueKind::Delete => return Some((item.key.user_key, None)),
                ValueKind::Put => return Some((item.key.user_key, Some(item.value))),
            }
        }
        None
    }
}
