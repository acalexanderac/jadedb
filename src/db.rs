use crate::compact::{pick_compaction, run_compaction};
use crate::error::{Error, Result};
use crate::iter::MergeIterator;
use crate::key::{InternalKey, ValueKind};
use crate::manifest::{FileMeta, Manifest, ManifestEdit};
use crate::memtable::MemTable;
use crate::options::Options;
use crate::snapshot::{Snapshot, SnapshotTracker};
use crate::sstable::reader::is_visible;
use crate::sstable::{sync_dir, BlockCache, SsTable, SsTableBuilder};
use crate::version::Version;
use crate::wal::Wal;
use parking_lot::{Condvar, Mutex};
use std::fs;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

struct Inner {
    opts: Options,
    dir: PathBuf,
    mem: MemTable,
    imms: Vec<Arc<MemTable>>,
    version: Arc<Version>,
    wal: Wal,
    next_seq: u64,
    next_file_num: u64,
    manifest: Manifest,
    cache: Arc<BlockCache>,
    snapshots: Arc<Mutex<SnapshotTracker>>,
    closed: bool,
    /// Background work metrics for stalls / portfolio benches.
    pub flush_count: u64,
    pub compact_count: u64,
    pub stall_count: u64,
}

pub struct Db {
    inner: Arc<Mutex<Inner>>,
    bg_cv: Arc<(Mutex<()>, Condvar)>,
    stop: Arc<AtomicBool>,
    bg: Mutex<Option<JoinHandle<()>>>,
    write_amp_bytes_written: Arc<AtomicU64>,
    write_amp_user_bytes: Arc<AtomicU64>,
}

impl Db {
    pub fn open(path: impl AsRef<Path>, opts: Options) -> Result<Self> {
        let dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let cache = Arc::new(BlockCache::new(opts.block_cache_capacity));
        let recovered = Manifest::recover(&dir)?;
        let version = Arc::new(Version::from_metas(
            &dir,
            &recovered.files,
            opts.max_levels,
            cache.clone(),
        )?);

        let next_file_num = recovered.next_file_num.max(1);
        let mut next_seq = recovered.last_seq;

        // Recover WAL if present
        let wal_path = dir.join("CURRENT.log");
        let wal_entries = Wal::recover(&wal_path)?;
        let mut mem = MemTable::new();
        for (k, v) in wal_entries {
            next_seq = next_seq.max(k.seq);
            mem.put(k, v);
        }
        next_seq = next_seq.saturating_add(0);

        let wal = if wal_path.exists() {
            Wal::open_append(&wal_path, opts.fsync)?
        } else {
            Wal::create(&wal_path, opts.fsync)?
        };

        let manifest = if dir.join("MANIFEST").exists() {
            Manifest::open(&dir, opts.fsync)?
        } else {
            let mut m = Manifest::create(&dir, opts.fsync)?;
            m.log(&[
                ManifestEdit::SetNextFileNum(next_file_num),
                ManifestEdit::SetLastSeq(next_seq),
            ])?;
            m
        };

        let inner = Arc::new(Mutex::new(Inner {
            opts,
            dir,
            mem,
            imms: Vec::new(),
            version,
            wal,
            next_seq,
            next_file_num,
            manifest,
            cache,
            snapshots: Arc::new(Mutex::new(SnapshotTracker::new())),
            closed: false,
            flush_count: 0,
            compact_count: 0,
            stall_count: 0,
        }));

        let stop = Arc::new(AtomicBool::new(false));
        let bg_cv = Arc::new((Mutex::new(()), Condvar::new()));
        let db = Self {
            inner: inner.clone(),
            bg_cv: bg_cv.clone(),
            stop: stop.clone(),
            bg: Mutex::new(None),
            write_amp_bytes_written: Arc::new(AtomicU64::new(0)),
            write_amp_user_bytes: Arc::new(AtomicU64::new(0)),
        };

        let bg_inner = inner.clone();
        let bg_cv2 = bg_cv.clone();
        let stop2 = stop.clone();
        let wa = db.write_amp_bytes_written.clone();
        let handle = thread::Builder::new()
            .name("jade-bg".into())
            .spawn(move || bg_loop(bg_inner, bg_cv2, stop2, wa))
            .map_err(|e| Error::Io(std::io::Error::other(e)))?;
        *db.bg.lock() = Some(handle);

        Ok(db)
    }

    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        self.write(key, value, ValueKind::Put)
    }

    pub fn delete(&self, key: &[u8]) -> Result<()> {
        self.write(key, &[], ValueKind::Delete)
    }

    fn write(&self, key: &[u8], value: &[u8], kind: ValueKind) -> Result<()> {
        self.write_amp_user_bytes
            .fetch_add((key.len() + value.len()) as u64, Ordering::Relaxed);
        loop {
            let mut g = self.inner.lock();
            if g.closed {
                return Err(Error::Closed);
            }
            // Write stall: too many L0 / imms
            if g.imms.len() >= 2 || g.version.l0_num_files() >= g.opts.l0_compaction_trigger * 2 {
                g.stall_count += 1;
                drop(g);
                self.bg_cv.1.notify_one();
                thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }

            g.next_seq += 1;
            let seq = g.next_seq;
            let ik = InternalKey {
                user_key: key.to_vec(),
                seq,
                kind,
            };
            g.wal.append(&ik, value)?;
            g.mem.put(ik, value.to_vec());

            let should_freeze = g.mem.approx_bytes() >= g.opts.memtable_bytes;
            if should_freeze {
                g.wal.sync()?;
                let frozen = std::mem::take(&mut g.mem);
                g.imms.push(Arc::new(frozen));
                // rotate WAL file number conceptually — keep same CURRENT.log until flush
                self.bg_cv.1.notify_one();
            }
            return Ok(());
        }
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.get_with_seq(key, u64::MAX)
    }

    pub fn get_with_seq(&self, key: &[u8], max_seq: u64) -> Result<Option<Vec<u8>>> {
        let g = self.inner.lock();
        if g.closed {
            return Err(Error::Closed);
        }
        if let Some(e) = g.mem.get(key, max_seq) {
            return Ok(is_visible(&e));
        }
        for imm in g.imms.iter().rev() {
            if let Some(e) = imm.get(key, max_seq) {
                return Ok(is_visible(&e));
            }
        }
        let tables = g.version.get_sstables_for_key(key);
        drop(g);
        for sst in tables {
            if let Some(e) = sst.get(key, max_seq)? {
                return Ok(is_visible(&e));
            }
        }
        Ok(None)
    }

    pub fn scan(&self, start: Bound<&[u8]>) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.scan_with_seq(start, u64::MAX)
    }

    pub fn scan_with_seq(
        &self,
        start: Bound<&[u8]>,
        max_seq: u64,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let g = self.inner.lock();
        let mut mems: Vec<&MemTable> = Vec::new();
        mems.push(&g.mem);
        for imm in g.imms.iter().rev() {
            mems.push(imm.as_ref());
        }
        let tables = g.version.all_tables();
        // Build streams manually for start bound filtering
        let mut streams = Vec::new();
        for m in &mems {
            let items: Vec<_> = m
                .range_from(start, max_seq)
                .map(|(k, v)| (k, v))
                .collect();
            streams.push(items);
        }
        for sst in &tables {
            let mut items = sst.iter_all()?;
            items.retain(|(k, _)| {
                if k.seq > max_seq {
                    return false;
                }
                match start {
                    Bound::Unbounded => true,
                    Bound::Included(s) => k.user_key.as_slice() >= s,
                    Bound::Excluded(s) => k.user_key.as_slice() > s,
                }
            });
            streams.push(items);
        }
        drop(g);
        let merge = MergeIterator::from_sorted_streams(streams, max_seq, true);
        Ok(merge.filter_map(|(k, v)| v.map(|val| (k, val))).collect())
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let g = self.inner.lock();
        if g.closed {
            return Err(Error::Closed);
        }
        Ok(SnapshotTracker::create_snapshot(&g.snapshots, g.next_seq))
    }

    pub fn get_snapshot(&self, snap: &Snapshot, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.get_with_seq(key, snap.seq)
    }

    pub fn sync(&self) -> Result<()> {
        let mut g = self.inner.lock();
        g.wal.sync()
    }

    pub fn flush(&self) -> Result<()> {
        {
            let mut g = self.inner.lock();
            if !g.mem.is_empty() {
                g.wal.sync()?;
                let frozen = std::mem::take(&mut g.mem);
                g.imms.push(Arc::new(frozen));
            }
            flush_imms(&mut g, &self.write_amp_bytes_written)?;
        }
        self.bg_cv.1.notify_one();
        Ok(())
    }

    pub fn compact(&self) -> Result<()> {
        self.flush()?;
        for _ in 0..64 {
            let did = {
                let mut g = self.inner.lock();
                maybe_compact(&mut g, &self.write_amp_bytes_written)?
            };
            if !did {
                break;
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> DbStats {
        let g = self.inner.lock();
        let user = self.write_amp_user_bytes.load(Ordering::Relaxed);
        let written = self.write_amp_bytes_written.load(Ordering::Relaxed);
        DbStats {
            flush_count: g.flush_count,
            compact_count: g.compact_count,
            stall_count: g.stall_count,
            l0_files: g.version.l0_num_files(),
            user_bytes: user,
            disk_bytes_written: written,
            write_amplification: if user == 0 {
                0.0
            } else {
                written as f64 / user as f64
            },
        }
    }

    pub fn close(&self) -> Result<()> {
        self.stop.store(true, Ordering::SeqCst);
        self.bg_cv.1.notify_one();
        if let Some(h) = self.bg.lock().take() {
            let _ = h.join();
        }
        let mut g = self.inner.lock();
        g.wal.sync()?;
        g.closed = true;
        Ok(())
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[derive(Debug, Clone)]
pub struct DbStats {
    pub flush_count: u64,
    pub compact_count: u64,
    pub stall_count: u64,
    pub l0_files: usize,
    pub user_bytes: u64,
    pub disk_bytes_written: u64,
    pub write_amplification: f64,
}

fn bg_loop(
    inner: Arc<Mutex<Inner>>,
    cv: Arc<(Mutex<()>, Condvar)>,
    stop: Arc<AtomicBool>,
    write_amp: Arc<AtomicU64>,
) {
    while !stop.load(Ordering::SeqCst) {
        {
            let mut g = inner.lock();
            let _ = flush_imms(&mut g, &write_amp);
            let _ = maybe_compact(&mut g, &write_amp);
        }
        let (lock, cvar) = &*cv;
        let mut guard = lock.lock();
        let _ = cvar.wait_for(&mut guard, std::time::Duration::from_millis(50));
    }
}

fn flush_imms(g: &mut Inner, write_amp: &Arc<AtomicU64>) -> Result<()> {
    while let Some(imm) = g.imms.first().cloned() {
        if imm.is_empty() {
            g.imms.remove(0);
            continue;
        }
        let file_num = g.next_file_num;
        g.next_file_num += 1;
        let path = g.dir.join(format!("{file_num:06}.sst"));
        let mut builder = SsTableBuilder::new(g.opts.block_size, g.opts.bloom_bits_per_key);
        for (k, v) in imm.iter() {
            builder.add(k, v);
        }
        let meta = builder.finish(&path, g.opts.fsync)?;
        sync_dir(&g.dir)?;
        write_amp.fetch_add(meta.file_size, Ordering::Relaxed);

        let fm = FileMeta {
            file_num,
            level: 0,
            smallest: meta.smallest.user_key.clone(),
            largest: meta.largest.user_key.clone(),
            file_size: meta.file_size,
        };
        g.manifest.log(&[
            ManifestEdit::AddFile(fm.clone()),
            ManifestEdit::SetNextFileNum(g.next_file_num),
            ManifestEdit::SetLastSeq(g.next_seq),
        ])?;

        let sst = SsTable::open(
            &path,
            file_num,
            fm.smallest.clone(),
            fm.largest.clone(),
            g.cache.clone(),
        )?;
        let mut new_version = (*g.version).clone();
        new_version.levels[0].push(Arc::new(sst));
        g.version = Arc::new(new_version);
        g.imms.remove(0);
        g.flush_count += 1;

        // After successful flush+manifest, recreate WAL (discard recovered entries)
        if g.imms.is_empty() && g.mem.is_empty() {
            let wal_path = g.dir.join("CURRENT.log");
            let _ = fs::remove_file(&wal_path);
            g.wal = Wal::create(&wal_path, g.opts.fsync)?;
        }
    }
    Ok(())
}

fn maybe_compact(g: &mut Inner, write_amp: &Arc<AtomicU64>) -> Result<bool> {
    let Some(job) = pick_compaction(&g.version, &g.opts) else {
        return Ok(false);
    };
    let watermark = g.snapshots.lock().watermark();
    let mut next = g.next_file_num;
    let out = run_compaction(&g.dir, &job, &g.opts, &mut next, watermark)?;
    for m in &out.new_metas {
        write_amp.fetch_add(m.file_size, Ordering::Relaxed);
    }
    sync_dir(&g.dir)?;
    let mut edits = out.edits;
    edits.push(ManifestEdit::SetNextFileNum(next));
    g.manifest.log(&edits)?;
    g.next_file_num = next;

    // Rebuild version from recovered-style apply: reload from manifest state would be ideal;
    // apply edits in memory.
    let mut files = collect_file_metas(&g.version);
    for e in &edits {
        match e {
            ManifestEdit::AddFile(m) => files.push(m.clone()),
            ManifestEdit::DeleteFile { level, file_num } => {
                files.retain(|f| !(f.level == *level && f.file_num == *file_num));
            }
            _ => {}
        }
    }
    g.version = Arc::new(Version::from_metas(
        &g.dir,
        &files,
        g.opts.max_levels,
        g.cache.clone(),
    )?);
    g.compact_count += 1;

    // Remove obsolete SST files
    for e in &edits {
        if let ManifestEdit::DeleteFile { file_num, .. } = e {
            let p = g.dir.join(format!("{file_num:06}.sst"));
            let _ = fs::remove_file(p);
        }
    }
    Ok(true)
}

fn collect_file_metas(version: &Version) -> Vec<FileMeta> {
    let mut out = Vec::new();
    for (level, tables) in version.levels.iter().enumerate() {
        for t in tables {
            out.push(FileMeta {
                file_num: t.file_num,
                level: level as u32,
                smallest: t.smallest.clone(),
                largest: t.largest.clone(),
                file_size: t.file_size,
            });
        }
    }
    out
}
