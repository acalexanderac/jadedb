use crate::key::{InternalKey, ValueKind};
use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: ValueKind,
    pub value: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct MemTable {
    map: BTreeMap<InternalKey, Vec<u8>>,
    approx_bytes: AtomicUsize,
}

impl MemTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&mut self, key: InternalKey, value: Vec<u8>) {
        let bytes = key.user_key.len() + value.len() + 16;
        self.approx_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.map.insert(key, value);
    }

    pub fn get(&self, user_key: &[u8], max_seq: u64) -> Option<Entry> {
        let start = InternalKey {
            user_key: user_key.to_vec(),
            seq: u64::MAX,
            kind: ValueKind::Put,
        };
        for (k, v) in self.map.range(start..) {
            if k.user_key.as_slice() != user_key {
                break;
            }
            if k.seq <= max_seq {
                return Some(Entry {
                    kind: k.kind,
                    value: v.clone(),
                });
            }
        }
        None
    }

    pub fn approx_bytes(&self) -> usize {
        self.approx_bytes.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&InternalKey, &Vec<u8>)> {
        self.map.iter()
    }

    pub fn range_from<'a>(
        &'a self,
        start: Bound<&[u8]>,
        max_seq: u64,
    ) -> MemTableIter<'a> {
        let skip_key = match start {
            Bound::Included(k) | Bound::Excluded(k) => Some(k.to_vec()),
            Bound::Unbounded => None,
        };
        let exclude = matches!(start, Bound::Excluded(_));
        MemTableIter {
            inner: self.map.iter().peekable(),
            skip_key,
            exclude,
            max_seq,
            last_user: None,
        }
    }
}

pub struct MemTableIter<'a> {
    inner: std::iter::Peekable<std::collections::btree_map::Iter<'a, InternalKey, Vec<u8>>>,
    skip_key: Option<Vec<u8>>,
    exclude: bool,
    max_seq: u64,
    last_user: Option<Vec<u8>>,
}

impl<'a> Iterator for MemTableIter<'a> {
    type Item = (InternalKey, Vec<u8>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((k, v)) = self.inner.next() {
            if let Some(ref skip) = self.skip_key {
                match k.user_key.as_slice().cmp(skip.as_slice()) {
                    std::cmp::Ordering::Less => continue,
                    std::cmp::Ordering::Equal if self.exclude => continue,
                    _ => {}
                }
            }
            if k.seq > self.max_seq {
                continue;
            }
            if self.last_user.as_ref() == Some(&k.user_key) {
                continue;
            }
            self.last_user = Some(k.user_key.clone());
            return Some((k.clone(), v.clone()));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_delete() {
        let mut m = MemTable::new();
        m.put(InternalKey::put(b"a".to_vec(), 1), b"1".to_vec());
        m.put(InternalKey::put(b"a".to_vec(), 2), b"2".to_vec());
        let e = m.get(b"a", u64::MAX).unwrap();
        assert_eq!(e.value, b"2");
        m.put(InternalKey::delete(b"a".to_vec(), 3), Vec::new());
        let e = m.get(b"a", u64::MAX).unwrap();
        assert_eq!(e.kind, ValueKind::Delete);
        assert!(m.get(b"a", 2).unwrap().value == b"2");
    }
}
