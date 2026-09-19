use jadedb::{Db, Options};
use std::ops::Bound;
use tempfile::tempdir;

#[test]
fn memtable_put_get_delete_via_db() {
    let dir = tempdir().unwrap();
    let db = Db::open(dir.path(), Options::for_tests()).unwrap();
    db.put(b"a", b"1").unwrap();
    db.put(b"b", b"2").unwrap();
    assert_eq!(db.get(b"a").unwrap().as_deref(), Some(b"1".as_ref()));
    db.delete(b"a").unwrap();
    assert_eq!(db.get(b"a").unwrap(), None);
    assert_eq!(db.get(b"b").unwrap().as_deref(), Some(b"2".as_ref()));
}

#[test]
fn wal_recovery_after_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = Db::open(dir.path(), Options::for_tests()).unwrap();
        db.put(b"x", b"alive").unwrap();
        db.sync().unwrap();
        // do not flush — data only in WAL + memtable
    }
    let db = Db::open(dir.path(), Options::for_tests()).unwrap();
    assert_eq!(db.get(b"x").unwrap().as_deref(), Some(b"alive".as_ref()));
}

#[test]
fn flush_then_get_from_sstable() {
    let dir = tempdir().unwrap();
    let db = Db::open(dir.path(), Options::for_tests()).unwrap();
    for i in 0..100 {
        db.put(format!("k{i:04}").as_bytes(), b"v").unwrap();
    }
    db.flush().unwrap();
    assert_eq!(db.get(b"k0000").unwrap().as_deref(), Some(b"v".as_ref()));
    assert_eq!(db.get(b"missing").unwrap(), None);
}

#[test]
fn scan_range() {
    let dir = tempdir().unwrap();
    let db = Db::open(dir.path(), Options::for_tests()).unwrap();
    for k in [b"a" as &[u8], b"b", b"c", b"d"] {
        db.put(k, k).unwrap();
    }
    db.flush().unwrap();
    let rows = db.scan(Bound::Included(b"b")).unwrap();
    let keys: Vec<_> = rows.iter().map(|(k, _)| k.as_slice()).collect();
    assert_eq!(keys, vec![b"b".as_ref(), b"c", b"d"]);
}

#[test]
fn tombstone_hides_older_value_across_sst() {
    let dir = tempdir().unwrap();
    let db = Db::open(dir.path(), Options::for_tests()).unwrap();
    db.put(b"t", b"old").unwrap();
    db.flush().unwrap();
    db.delete(b"t").unwrap();
    db.flush().unwrap();
    assert_eq!(db.get(b"t").unwrap(), None);
}

#[test]
fn compaction_runs() {
    let dir = tempdir().unwrap();
    let mut opts = Options::for_tests();
    opts.l0_compaction_trigger = 2;
    opts.memtable_bytes = 8 * 1024;
    let db = Db::open(dir.path(), opts).unwrap();
    for i in 0..500 {
        db.put(format!("key-{i:05}").as_bytes(), b"vvvvvvvv").unwrap();
    }
    db.compact().unwrap();
    let stats = db.stats();
    assert!(stats.flush_count >= 1);
    assert_eq!(db.get(b"key-00000").unwrap().as_deref(), Some(b"vvvvvvvv".as_ref()));
}

#[test]
fn snapshot_mvcc_read() {
    let dir = tempdir().unwrap();
    let db = Db::open(dir.path(), Options::for_tests()).unwrap();
    db.put(b"s", b"v1").unwrap();
    let snap = db.snapshot().unwrap();
    db.put(b"s", b"v2").unwrap();
    assert_eq!(db.get(b"s").unwrap().as_deref(), Some(b"v2".as_ref()));
    assert_eq!(
        db.get_snapshot(&snap, b"s").unwrap().as_deref(),
        Some(b"v1".as_ref())
    );
}

#[test]
fn orphan_sst_ignored_without_manifest() {
    let dir = tempdir().unwrap();
    {
        let db = Db::open(dir.path(), Options::for_tests()).unwrap();
        db.put(b"k", b"v").unwrap();
        db.flush().unwrap();
    }
    // Simulate crash mid-flush: write junk sst not in manifest
    std::fs::write(dir.path().join("999999.sst"), b"garbage").unwrap();
    let db = Db::open(dir.path(), Options::for_tests()).unwrap();
    assert_eq!(db.get(b"k").unwrap().as_deref(), Some(b"v".as_ref()));
}
