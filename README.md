# jadedb

Educational LSM-tree storage engine in Rust (portfolio / interview project).

**Aprende el motor de punta a punta:** [docs/LSM_GUIDE.md](docs/LSM_GUIDE.md)  
(terminología, write/read path, tombstones, flush, crash recovery, manifest, compaction, snapshots, mapa a código, checklist de entrevista).

Grafo interactivo del código: [graphify-out/graph.html](graphify-out/graph.html)

## Design point

Jade optimizes for **correct durability + explainable trade-offs**, not raw RocksDB parity.

| Amplification | Choice |
|---|---|
| Write | Leveled compaction (higher WA, lower read/space amp) |
| Read | Bloom filters + block index + L0→Ln probe order |
| Space | Tombstones retained until compaction past watermark |

**Durability invariant:** write SST → fsync SST → append MANIFEST → only then discard WAL.

## Stages implemented

1. Memtable (`BTreeMap`) + in-memory API  
2. WAL + crash recovery  
3. SSTable (blocks, index, bloom) + flush  
4. Merge iterator + `scan`  
5. Block cache  
6. Manifest + leveled layout  
7. Background flush/compaction + write stalls  
8. Fsync modes: `Always` / `Batch` / `Never`  
9. Snapshots (MVCC read-by-seq)

## Quick start

```bash
cargo test
cargo run --bin jade -- bench batch
cargo bench
```

```rust
use jadedb::{Db, Options};

let db = Db::open("./data", Options::default())?;
db.put(b"hello", b"world")?;
assert_eq!(db.get(b"hello")?, Some(b"world".to_vec()));
```

## Fsync modes (measure these)

```bash
cargo run --bin jade -- bench always
cargo run --bin jade -- bench batch
cargo run --bin jade -- bench never
```

Sample on this machine (10k puts, test options):

| Mode | ops/sec | write_amp |
|------|---------|-----------|
| Never | ~117k | 1.78 |
| Batch | ~111k | 1.78 |
| Always | ~2.6k | 1.78 |

Compare ops/sec, write amplification, and stall count from `Db::stats()`.

## Layout

```
src/
  db.rs         # put/get/delete/scan, flush, compact
  memtable.rs   # BTreeMap memtable
  wal.rs        # write-ahead log
  sstable/      # block, bloom, builder, reader, cache
  manifest.rs   # version edits
  version.rs    # levels
  compact.rs    # leveled compaction
  iter.rs       # merge iterator
  snapshot.rs   # MVCC snapshots
docs/
  LSM_GUIDE.md  # full study guide (terminology → interview checklist)
```

## Reading

- **[Jade LSM Guide](docs/LSM_GUIDE.md)** (start here)
- [Mini-LSM](https://skyzh.github.io/mini-lsm/)
- [LevelDB impl.md](https://github.com/google/leveldb/blob/main/doc/impl.md)
- [graphify report](graphify-out/GRAPH_REPORT.md)
