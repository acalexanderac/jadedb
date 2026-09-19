use crate::error::Result;
use crate::key::{InternalKey, ValueKind};
use crate::manifest::{FileMeta, ManifestEdit};
use crate::options::Options;
use crate::sstable::{SsTable, SsTableBuilder};
use crate::version::Version;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
struct HeapItem {
    key: InternalKey,
    value: Vec<u8>,
    idx: usize,
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.idx == other.idx
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
        match other.key.cmp(&self.key) {
            Ordering::Equal => other.idx.cmp(&self.idx),
            o => o,
        }
    }
}

pub struct CompactionJob {
    pub level: usize,
    pub inputs_upper: Vec<Arc<SsTable>>,
    pub inputs_lower: Vec<Arc<SsTable>>,
}

pub fn pick_compaction(version: &Version, opts: &Options) -> Option<CompactionJob> {
    if version.l0_num_files() >= opts.l0_compaction_trigger {
        let upper = version.levels[0].clone();
        let (min_k, max_k) = key_range(&upper)?;
        let lower = overlapping(&version.levels[1], &min_k, &max_k);
        return Some(CompactionJob {
            level: 0,
            inputs_upper: upper,
            inputs_lower: lower,
        });
    }
    for level in 1..opts.max_levels.saturating_sub(1) {
        let limit = opts.base_level_bytes as u64
            * (opts.level_ratio as u64).pow(level as u32 - 1);
        if version.level_bytes(level) > limit {
            let upper = version.levels[level].first()?.clone();
            let lower = overlapping(
                &version.levels[level + 1],
                &upper.smallest,
                &upper.largest,
            );
            return Some(CompactionJob {
                level,
                inputs_upper: vec![upper],
                inputs_lower: lower,
            });
        }
    }
    None
}

fn key_range(tables: &[Arc<SsTable>]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut min_k = tables.first()?.smallest.clone();
    let mut max_k = tables.first()?.largest.clone();
    for t in tables.iter().skip(1) {
        if t.smallest < min_k {
            min_k = t.smallest.clone();
        }
        if t.largest > max_k {
            max_k = t.largest.clone();
        }
    }
    Some((min_k, max_k))
}

fn overlapping(level: &[Arc<SsTable>], smallest: &[u8], largest: &[u8]) -> Vec<Arc<SsTable>> {
    level
        .iter()
        .filter(|t| t.largest.as_slice() >= smallest && t.smallest.as_slice() <= largest)
        .cloned()
        .collect()
}

pub struct CompactionOutput {
    pub edits: Vec<ManifestEdit>,
    pub new_metas: Vec<FileMeta>,
}

pub fn run_compaction(
    dir: &Path,
    job: &CompactionJob,
    opts: &Options,
    next_file_num: &mut u64,
    watermark: u64,
) -> Result<CompactionOutput> {
    let mut streams = Vec::new();
    // newer first: upper then lower (upper is newer level)
    for t in job.inputs_upper.iter().rev().chain(job.inputs_lower.iter()) {
        streams.push(t.iter_all()?);
    }

    let mut heap = BinaryHeap::new();
    let mut iters: Vec<_> = streams.into_iter().map(|s| s.into_iter()).collect();
    for (i, it) in iters.iter_mut().enumerate() {
        if let Some((k, v)) = it.next() {
            heap.push(HeapItem {
                key: k,
                value: v,
                idx: i,
            });
        }
    }

    let output_level = (job.level + 1) as u32;
    let mut builder: Option<SsTableBuilder> = None;
    let mut new_metas = Vec::new();
    let mut edits = Vec::new();
    let mut last_user: Option<Vec<u8>> = None;
    let mut current_size = 0usize;

    let flush_builder = |builder: &mut Option<SsTableBuilder>,
                         next_file_num: &mut u64,
                         new_metas: &mut Vec<FileMeta>,
                         edits: &mut Vec<ManifestEdit>|
     -> Result<()> {
        if let Some(b) = builder.take() {
            let file_num = *next_file_num;
            *next_file_num += 1;
            let path = dir.join(format!("{file_num:06}.sst"));
            let meta = b.finish(&path, opts.fsync)?;
            let fm = FileMeta {
                file_num,
                level: output_level,
                smallest: meta.smallest.user_key.clone(),
                largest: meta.largest.user_key.clone(),
                file_size: meta.file_size,
            };
            edits.push(ManifestEdit::AddFile(fm.clone()));
            new_metas.push(fm);
        }
        Ok(())
    };

    while let Some(item) = heap.pop() {
        let idx = item.idx;
        if let Some((k, v)) = iters[idx].next() {
            heap.push(HeapItem {
                key: k,
                value: v,
                idx,
            });
        }

        if last_user.as_ref() == Some(&item.key.user_key) {
            continue;
        }
        last_user = Some(item.key.user_key.clone());

        // Drop tombstones only when safe past watermark and no lower levels need them.
        // Simplified: drop delete if seq < watermark (no open snapshot needs it) AND
        // we're compacting into the last level OR tombstone is obsolete.
        let drop = match item.key.kind {
            ValueKind::Delete => {
                item.key.seq <= watermark && (output_level as usize + 1 >= opts.max_levels)
            }
            ValueKind::Put => false,
        };
        if drop {
            continue;
        }

        if builder.is_none() {
            builder = Some(SsTableBuilder::new(opts.block_size, opts.bloom_bits_per_key));
            current_size = 0;
        }
        let b = builder.as_mut().unwrap();
        b.add(&item.key, &item.value);
        current_size += item.key.user_key.len() + item.value.len() + 16;
        if current_size >= opts.target_file_size {
            flush_builder(&mut builder, next_file_num, &mut new_metas, &mut edits)?;
        }
    }
    flush_builder(&mut builder, next_file_num, &mut new_metas, &mut edits)?;

    for t in job.inputs_upper.iter().chain(job.inputs_lower.iter()) {
        edits.push(ManifestEdit::DeleteFile {
            level: if job.inputs_upper.iter().any(|x| x.file_num == t.file_num) {
                job.level as u32
            } else {
                (job.level + 1) as u32
            },
            file_num: t.file_num,
        });
    }

    Ok(CompactionOutput { edits, new_metas })
}
