use crate::error::{Error, Result};
use crate::key::{InternalKey, ValueKind};
use crate::memtable::Entry;
use crate::sstable::block::{Block, IndexBlock};
use crate::sstable::bloom::BloomFilter;
use crate::sstable::builder::{FOOTER_SIZE, SST_MAGIC};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
pub struct BlockCache {
    inner: Mutex<HashMap<(u64, u64), Arc<Block>>>,
    capacity: usize,
}

impl BlockCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            capacity,
        }
    }

    pub fn get(&self, file_num: u64, offset: u64) -> Option<Arc<Block>> {
        self.inner.lock().get(&(file_num, offset)).cloned()
    }

    pub fn insert(&self, file_num: u64, offset: u64, block: Arc<Block>) {
        let mut g = self.inner.lock();
        if g.len() >= self.capacity {
            if let Some(k) = g.keys().next().cloned() {
                g.remove(&k);
            }
        }
        g.insert((file_num, offset), block);
    }
}

#[derive(Debug)]
pub struct SsTable {
    pub file_num: u64,
    pub path: PathBuf,
    pub smallest: Vec<u8>,
    pub largest: Vec<u8>,
    pub file_size: u64,
    index: IndexBlock,
    bloom: BloomFilter,
    cache: Arc<BlockCache>,
}

impl SsTable {
    pub fn open(
        path: impl AsRef<Path>,
        file_num: u64,
        smallest: Vec<u8>,
        largest: Vec<u8>,
        cache: Arc<BlockCache>,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut f = File::open(&path)?;
        let file_size = f.metadata()?.len();
        if file_size < FOOTER_SIZE as u64 {
            return Err(Error::Corruption("sst too small".into()));
        }
        f.seek(SeekFrom::End(-(FOOTER_SIZE as i64)))?;
        let mut footer = vec![0u8; FOOTER_SIZE];
        f.read_exact(&mut footer)?;
        let magic = u32::from_le_bytes(footer[0..4].try_into().unwrap());
        if magic != SST_MAGIC {
            return Err(Error::Corruption("sst bad magic".into()));
        }
        let index_off = u64::from_le_bytes(footer[12..20].try_into().unwrap());
        let index_len = u64::from_le_bytes(footer[20..28].try_into().unwrap()) as usize;
        let bloom_off = u64::from_le_bytes(footer[28..36].try_into().unwrap());
        let bloom_len = u64::from_le_bytes(footer[36..44].try_into().unwrap()) as usize;

        let mut index_buf = vec![0u8; index_len];
        f.seek(SeekFrom::Start(index_off))?;
        f.read_exact(&mut index_buf)?;
        let index = IndexBlock::decode(&index_buf)?;

        let mut bloom_buf = vec![0u8; bloom_len];
        f.seek(SeekFrom::Start(bloom_off))?;
        f.read_exact(&mut bloom_buf)?;
        let bloom = BloomFilter::decode(&bloom_buf)?;

        Ok(Self {
            file_num,
            path,
            smallest,
            largest,
            file_size,
            index,
            bloom,
            cache,
        })
    }

    pub fn may_contain(&self, user_key: &[u8]) -> bool {
        if user_key < self.smallest.as_slice() || user_key > self.largest.as_slice() {
            return false;
        }
        self.bloom.may_contain(user_key)
    }

    pub fn get(&self, user_key: &[u8], max_seq: u64) -> Result<Option<Entry>> {
        if !self.may_contain(user_key) {
            return Ok(None);
        }
        if let Some((off, len)) = self.index.find_block(user_key) {
            let block = self.read_block(off, len)?;
            if let Some((ik, v)) = block.get(user_key, max_seq) {
                return Ok(Some(Entry {
                    kind: ik.kind,
                    value: v,
                }));
            }
        }
        for &(_, off, len) in self.index.entries() {
            let block = self.read_block(off, len)?;
            if let Some((ik, v)) = block.get(user_key, max_seq) {
                return Ok(Some(Entry {
                    kind: ik.kind,
                    value: v,
                }));
            }
        }
        Ok(None)
    }

    fn read_block(&self, offset: u64, len: u32) -> Result<Arc<Block>> {
        if let Some(b) = self.cache.get(self.file_num, offset) {
            return Ok(b);
        }
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf)?;
        let block = Arc::new(Block::from_raw(buf));
        self.cache.insert(self.file_num, offset, block.clone());
        Ok(block)
    }

    pub fn iter_all(&self) -> Result<Vec<(InternalKey, Vec<u8>)>> {
        let mut out = Vec::new();
        for &(_, off, len) in self.index.entries() {
            let block = self.read_block(off, len)?;
            for (ik, v) in block.iter_entries() {
                out.push((ik, v));
            }
        }
        Ok(out)
    }
}

pub fn is_visible(entry: &Entry) -> Option<Vec<u8>> {
    match entry.kind {
        ValueKind::Put => Some(entry.value.clone()),
        ValueKind::Delete => None,
    }
}
