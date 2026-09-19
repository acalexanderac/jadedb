use crate::manifest::FileMeta;
use crate::sstable::reader::{BlockCache, SsTable};
use crate::error::Result;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub struct Version {
    pub levels: Vec<Vec<Arc<SsTable>>>,
}

impl Version {
    pub fn empty(max_levels: usize) -> Self {
        Self {
            levels: (0..max_levels).map(|_| Vec::new()).collect(),
        }
    }

    pub fn from_metas(
        dir: &Path,
        files: &[FileMeta],
        max_levels: usize,
        cache: Arc<BlockCache>,
    ) -> Result<Self> {
        let mut v = Self::empty(max_levels);
        for m in files {
            let path = dir.join(format!("{:06}.sst", m.file_num));
            if !path.exists() {
                continue; // orphaned manifest entry or mid-crash; skip missing
            }
            let sst = SsTable::open(
                &path,
                m.file_num,
                m.smallest.clone(),
                m.largest.clone(),
                cache.clone(),
            )?;
            let level = m.level as usize;
            if level < v.levels.len() {
                v.levels[level].push(Arc::new(sst));
            }
        }
        for level in v.levels.iter_mut().skip(1) {
            level.sort_by(|a, b| a.smallest.cmp(&b.smallest));
        }
        // L0: newest last in manifest order — keep file_num ascending (older first)
        if let Some(l0) = v.levels.first_mut() {
            l0.sort_by_key(|s| s.file_num);
        }
        Ok(v)
    }

    pub fn l0_num_files(&self) -> usize {
        self.levels.first().map(|l| l.len()).unwrap_or(0)
    }

    pub fn level_bytes(&self, level: usize) -> u64 {
        self.levels
            .get(level)
            .map(|files| files.iter().map(|f| f.file_size).sum())
            .unwrap_or(0)
    }

    pub fn get_sstables_for_key(&self, user_key: &[u8]) -> Vec<Arc<SsTable>> {
        let mut out = Vec::new();
        if let Some(l0) = self.levels.first() {
            for sst in l0.iter().rev() {
                if sst.may_contain(user_key) {
                    out.push(sst.clone());
                }
            }
        }
        for level in self.levels.iter().skip(1) {
            if let Some(sst) = find_in_level(level, user_key) {
                out.push(sst);
            }
        }
        out
    }

    pub fn all_tables(&self) -> Vec<Arc<SsTable>> {
        self.levels.iter().flatten().cloned().collect()
    }
}

fn find_in_level(level: &[Arc<SsTable>], user_key: &[u8]) -> Option<Arc<SsTable>> {
    level
        .iter()
        .find(|sst| user_key >= sst.smallest.as_slice() && user_key <= sst.largest.as_slice())
        .cloned()
}
