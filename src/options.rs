/// Durability policy for WAL and critical metadata writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FsyncMode {
    /// fsync after every write (slow, strongest durability).
    Always,
    /// fsync periodically / on freeze (group commit style).
    #[default]
    Batch,
    /// never fsync (fast, unsafe across power loss).
    Never,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub memtable_bytes: usize,
    pub block_size: usize,
    pub fsync: FsyncMode,
    pub l0_compaction_trigger: usize,
    pub level_ratio: usize,
    pub base_level_bytes: usize,
    pub target_file_size: usize,
    pub bloom_bits_per_key: usize,
    pub block_cache_capacity: usize,
    pub max_levels: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            memtable_bytes: 4 * 1024 * 1024,
            block_size: 4 * 1024,
            fsync: FsyncMode::Batch,
            l0_compaction_trigger: 4,
            level_ratio: 10,
            base_level_bytes: 10 * 1024 * 1024,
            target_file_size: 2 * 1024 * 1024,
            bloom_bits_per_key: 10,
            block_cache_capacity: 64,
            max_levels: 7,
        }
    }
}

impl Options {
    pub fn for_tests() -> Self {
        Self {
            memtable_bytes: 64 * 1024,
            block_size: 1024,
            fsync: FsyncMode::Always,
            l0_compaction_trigger: 2,
            level_ratio: 10,
            base_level_bytes: 256 * 1024,
            target_file_size: 32 * 1024,
            bloom_bits_per_key: 10,
            block_cache_capacity: 16,
            max_levels: 7,
        }
    }
}
