# Graph Report - jadedb  (2026-09-19)

## Corpus Check
- cluster-only mode — file stats not available

## Summary
- 321 nodes · 691 edges · 14 communities (13 shown, 1 thin omitted)
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 5 edges (avg confidence: 0.85)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `0cbe5724`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- Community 0
- Community 1
- Community 2
- Community 3
- Community 4
- Community 5
- Community 6
- Community 7
- Community 8
- Community 9
- Community 10
- Community 11
- Community 12
- Community 13

## God Nodes (most connected - your core abstractions)
1. `InternalKey` - 29 edges
2. `Db` - 26 edges
3. `SsTable` - 22 edges
4. `MemTable` - 17 edges
5. `Inner` - 16 edges
6. `Version` - 15 edges
7. `Block` - 13 edges
8. `BlockCache` - 12 edges
9. `Wal` - 12 edges
10. `Manifest` - 12 edges

## Surprising Connections (you probably didn't know these)
- `flush_imms()` --calls--> `sync_dir()`  [INFERRED]
  src/db.rs → src/sstable/builder.rs
- `maybe_compact()` --calls--> `pick_compaction()`  [INFERRED]
  src/db.rs → src/compact.rs
- `maybe_compact()` --calls--> `run_compaction()`  [INFERRED]
  src/db.rs → src/compact.rs
- `maybe_compact()` --calls--> `sync_dir()`  [INFERRED]
  src/db.rs → src/sstable/builder.rs
- `Inner` --references--> `Manifest`  [EXTRACTED]
  src/db.rs → src/manifest.rs

## Import Cycles
- None detected.

## Communities (14 total, 1 thin omitted)

### Community 0 - "Community 0"
Cohesion: 0.11
Nodes (27): AtomicBool, AtomicU64, Condvar, is_visible, JoinHandle, mergeiterator, parking_lot, bg_loop() (+19 more)

### Community 1 - "Community 1"
Cohesion: 0.10
Nodes (25): openoptions, compare_encoded(), encode_roundtrip(), InternalKey, newer_seq_sorts_first_same_key(), Option, Ord, Ordering (+17 more)

### Community 2 - "Community 2"
Cohesion: 0.11
Nodes (17): atomic, AtomicUsize, BTreeMap, Iter, Peekable, Entry, MemTable, MemTableIter (+9 more)

### Community 3 - "Community 3"
Cohesion: 0.14
Nodes (21): Default, CompactionJob, CompactionOutput, HeapItem, key_range(), overlapping(), pick_compaction(), Arc (+13 more)

### Community 4 - "Community 4"
Cohesion: 0.12
Nodes (10): BlockBuilder, BlockEntryIter, BlockEntryIter<'a>, IndexBlock, Item, Iterator, Option, Result (+2 more)

### Community 5 - "Community 5"
Cohesion: 0.18
Nodes (15): file, HashMap, Block, BlockCache, is_visible(), Arc, AsRef, Mutex (+7 more)

### Community 6 - "Community 6"
Cohesion: 0.09
Nodes (13): write_bench(), black_box, bound, Criterion, env, instant, jadedb, pathbuf (+5 more)

### Community 7 - "Community 7"
Cohesion: 0.15
Nodes (16): BinaryHeap, IntoIter, HeapItem, MergeIterator, Arc, Eq, Item, Iterator (+8 more)

### Community 8 - "Community 8"
Cohesion: 0.17
Nodes (15): fs, io, Error, String, decode_record(), MAGIC, AsRef, File (+7 more)

### Community 9 - "Community 9"
Cohesion: 0.23
Nodes (15): apply_edit(), decode_edit(), encode_edit(), FileMeta, Manifest, ManifestEdit, RecoveredState, AsRef (+7 more)

### Community 10 - "Community 10"
Cohesion: 0.21
Nodes (7): Arc, Drop, Mutex, Self, Vec, Snapshot, SnapshotTracker

### Community 11 - "Community 11"
Cohesion: 0.25
Nodes (8): find_in_level(), Arc, Option, Path, Result, Self, Vec, Version

### Community 12 - "Community 12"
Cohesion: 0.33
Nodes (6): BloomFilter, hash_pair(), no_false_negative(), Result, Self, Vec

## Knowledge Gaps
- **4 isolated node(s):** `FOOTER_SIZE`, `SST_MAGIC`, `jadedb`, `MAGIC`
  These have ≤1 connection - possible missing edges or undocumented components. (Counts symbols only; 78 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **1 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `InternalKey` connect `Community 1` to `Community 2`, `Community 3`, `Community 4`, `Community 5`, `Community 7`, `Community 8`?**
  _High betweenness centrality (0.239) - this node is a cross-community bridge._
- **Why does `SsTable` connect `Community 5` to `Community 3`, `Community 4`, `Community 7`, `Community 11`, `Community 12`?**
  _High betweenness centrality (0.121) - this node is a cross-community bridge._
- **Why does `Inner` connect `Community 0` to `Community 2`, `Community 3`, `Community 5`, `Community 8`, `Community 9`, `Community 10`, `Community 11`?**
  _High betweenness centrality (0.117) - this node is a cross-community bridge._
- **What connects `FOOTER_SIZE`, `SST_MAGIC`, `jadedb` to the rest of the system?**
  _4 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Community 0` be split into smaller, more focused modules?**
  _Cohesion score 0.113107822410148 - nodes in this community are weakly interconnected._
- **Should `Community 1` be split into smaller, more focused modules?**
  _Cohesion score 0.09581646423751687 - nodes in this community are weakly interconnected._
- **Should `Community 2` be split into smaller, more focused modules?**
  _Cohesion score 0.11396011396011396 - nodes in this community are weakly interconnected._