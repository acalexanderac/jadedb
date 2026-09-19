//! JadeDB — educational LSM-tree storage engine.
//!
//! Write path: WAL → memtable → ack. Flush: SST → fsync → manifest → drop WAL.
//! Read path: mem → imms → L0 (newest first) → leveled SSTs with bloom + block cache.

mod compact;
mod db;
mod error;
mod iter;
mod key;
mod manifest;
mod memtable;
mod options;
mod snapshot;
mod sstable;
mod version;
mod wal;

pub use db::{Db, DbStats};
pub use error::{Error, Result};
pub use options::{FsyncMode, Options};
pub use snapshot::Snapshot;
