use crate::error::Result;
use crate::key::InternalKey;
use crate::options::FsyncMode;
use crate::sstable::block::{BlockBuilder, IndexBlock};
use crate::sstable::bloom::BloomFilter;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

pub const SST_MAGIC: u32 = 0x5353_544A; // "SSTJ"
pub const FOOTER_SIZE: usize = 4 + 8 + 8 + 8 + 8 + 8; // magic + data_end + index_off + index_len + bloom_off + bloom_len

pub struct SsTableBuilder {
    block_size: usize,
    bits_per_key: usize,
    data: Vec<u8>,
    block: BlockBuilder,
    index: IndexBlock,
    last_key: Vec<u8>,
    user_keys: Vec<Vec<u8>>,
    smallest: Option<InternalKey>,
    largest: Option<InternalKey>,
}

impl SsTableBuilder {
    pub fn new(block_size: usize, bits_per_key: usize) -> Self {
        Self {
            block_size,
            bits_per_key,
            data: Vec::new(),
            block: BlockBuilder::new(),
            index: IndexBlock::new(),
            last_key: Vec::new(),
            user_keys: Vec::new(),
            smallest: None,
            largest: None,
        }
    }

    pub fn add(&mut self, key: &InternalKey, value: &[u8]) {
        let enc = key.encode();
        if self.smallest.is_none() {
            self.smallest = Some(key.clone());
        }
        self.largest = Some(key.clone());
        self.user_keys.push(key.user_key.clone());
        self.block.add(&enc, value);
        self.last_key = enc;
        if self.block.estimated_size() >= self.block_size {
            self.flush_block();
        }
    }

    fn flush_block(&mut self) {
        if self.block.is_empty() {
            return;
        }
        let block = std::mem::take(&mut self.block).finish();
        let offset = self.data.len() as u64;
        let len = block.raw().len() as u32;
        self.data.extend_from_slice(block.raw());
        self.index
            .push(std::mem::take(&mut self.last_key), offset, len);
        self.block = BlockBuilder::new();
    }

    pub fn finish(mut self, path: impl AsRef<Path>, fsync: FsyncMode) -> Result<SsTableMeta> {
        self.flush_block();
        let bloom = BloomFilter::build(&self.user_keys, self.bits_per_key);
        let bloom_bytes = bloom.encode();
        let index_bytes = self.index.encode();

        let data_end = self.data.len() as u64;
        let mut file_buf = self.data;
        let index_off = file_buf.len() as u64;
        file_buf.extend_from_slice(&index_bytes);
        let bloom_off = file_buf.len() as u64;
        file_buf.extend_from_slice(&bloom_bytes);

        let mut footer = Vec::with_capacity(FOOTER_SIZE);
        footer.extend_from_slice(&SST_MAGIC.to_le_bytes());
        footer.extend_from_slice(&data_end.to_le_bytes());
        footer.extend_from_slice(&index_off.to_le_bytes());
        footer.extend_from_slice(&(index_bytes.len() as u64).to_le_bytes());
        footer.extend_from_slice(&bloom_off.to_le_bytes());
        footer.extend_from_slice(&(bloom_bytes.len() as u64).to_le_bytes());
        file_buf.extend_from_slice(&footer);

        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path.as_ref())?;
        f.write_all(&file_buf)?;
        if fsync != FsyncMode::Never {
            f.sync_all()?;
        }

        Ok(SsTableMeta {
            smallest: self.smallest.expect("empty sst"),
            largest: self.largest.expect("empty sst"),
            file_size: file_buf.len() as u64,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SsTableMeta {
    pub smallest: InternalKey,
    pub largest: InternalKey,
    pub file_size: u64,
}

pub fn sync_dir(dir: impl AsRef<Path>) -> Result<()> {
    #[cfg(unix)]
    {
        use std::fs::File;
        let d = File::open(dir.as_ref())?;
        d.sync_all()?;
    }
    #[cfg(windows)]
    {
        let _ = dir;
    }
    Ok(())
}
